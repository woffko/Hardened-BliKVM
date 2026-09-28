//! VP8 WebRTC feed for Janus: reads the uStreamer MJPEG stream, encodes
//! changed frames with libvpx and sends RTP to a loopback UDP port. With a
//! demand socket it works only while a WebRTC viewer is connected.

use std::{
    env,
    io::{self, Write},
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use bli_kvm_video_encoder::{
    FrameDecision, FrameGate, GatePolicy, LatestFrame, Vp8Packetizer,
    demand::{Demand, group_id},
    mjpeg::MjpegReader,
    vp8::{I420, Vp8Config, Vp8Encoder},
    yuv::{I420Frame, JpegToI420},
};

const STREAM_REQUEST: &[u8] =
    b"GET /stream HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nUser-Agent: Hardened-BliKVM-vp8/0.1\r\n\r\n";
const PAYLOAD_TYPE: u8 = 96;
const MTU: usize = 1200;

struct Settings {
    upstream: PathBuf,
    rtp: SocketAddr,
    max_fps: u32,
    kbps: u32,
    threads: u32,
    halve: bool,
    demand: Option<PathBuf>,
    demand_group: String,
}

/// After the last viewer leaves, the capture stream is kept this long.
const IDLE_GRACE: Duration = Duration::from_secs(2);

fn number(name: &str, default: u32, range: std::ops::RangeInclusive<u32>) -> Result<u32, String> {
    match env::var(name) {
        Err(_) => Ok(default),
        Ok(value) => value
            .parse::<u32>()
            .ok()
            .filter(|parsed| range.contains(parsed) && parsed.to_string() == value)
            .ok_or_else(|| format!("{name} is invalid")),
    }
}

fn settings() -> Result<Settings, String> {
    let upstream = PathBuf::from(
        env::var_os("BLI_KVM_USTREAMER_SOCKET")
            .unwrap_or_else(|| "/run/hardened-blikvm-ustreamer/ustreamer.sock".into()),
    );
    if !upstream.is_absolute() {
        return Err("BLI_KVM_USTREAMER_SOCKET must be absolute".into());
    }
    // RTP never leaves the host: Janus reads it from loopback.
    let port = number("BLI_KVM_VP8_RTP_PORT", 5004, 1024..=65535)?;
    Ok(Settings {
        upstream,
        rtp: SocketAddr::from((Ipv4Addr::LOCALHOST, port as u16)),
        max_fps: number("BLI_KVM_VP8_MAX_FPS", 30, 1..=60)?,
        kbps: number("BLI_KVM_VP8_KBPS", 2500, 64..=20000)?,
        threads: number("BLI_KVM_VP8_THREADS", 3, 1..=4)?,
        halve: number("BLI_KVM_VP8_HALVE", 0, 0..=1)? == 1,
        demand: match env::var_os("BLI_KVM_VP8_DEMAND_SOCKET").map(PathBuf::from) {
            Some(path) if !path.is_absolute() => {
                return Err("BLI_KVM_VP8_DEMAND_SOCKET must be absolute".into());
            }
            path => path,
        },
        demand_group: env::var("BLI_KVM_VP8_DEMAND_GROUP")
            .unwrap_or_else(|_| "hardened-blikvm".into()),
    })
}

struct Captured {
    jpeg: Vec<u8>,
    at: Instant,
}

/// Read frames until the stream fails; the caller reconnects.
fn read_stream(
    settings: &Settings,
    gate: &mut FrameGate,
    slot: &LatestFrame<Captured>,
    demand: &Demand,
    stop: &AtomicBool,
) -> io::Result<()> {
    let mut stream = UnixStream::connect(&settings.upstream)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.write_all(STREAM_REQUEST)?;
    let mut reader = MjpegReader::new(stream)?;
    let mut idle_since: Option<Instant> = None;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        // Nobody watches: leave the capture stream until someone does.
        if demand.active() {
            idle_since = None;
        } else if idle_since.get_or_insert_with(Instant::now).elapsed() >= IDLE_GRACE {
            return Ok(());
        }
        let mut jpeg = Vec::new();
        reader.next_frame(&mut jpeg)?;
        let now = Instant::now();
        if gate.admit(&jpeg, now) == FrameDecision::Encode {
            slot.publish(Captured { jpeg, at: now });
        }
    }
}

struct Decoded {
    frame: I420Frame,
    at: Instant,
    decode: Duration,
    convert: Duration,
}

/// Decode and convert on its own thread so JPEG work overlaps encoding.
fn convert_loop(
    settings: &Settings,
    input: &LatestFrame<Captured>,
    output: &LatestFrame<Decoded>,
    rejected: &AtomicU32,
    stop: &AtomicBool,
) {
    let mut converter = JpegToI420::default();
    while !stop.load(Ordering::Relaxed) {
        let Some(captured) = input.take(Duration::from_millis(500)) else {
            continue;
        };
        let start = Instant::now();
        let mut frame = I420Frame::default();
        if converter
            .convert(&captured.jpeg, settings.halve, &mut frame)
            .is_err()
        {
            rejected.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let decode = converter.last_decode;
        output.publish(Decoded {
            frame,
            at: captured.at,
            decode,
            convert: start.elapsed().saturating_sub(decode),
        });
    }
}

fn encode_loop(
    settings: &Settings,
    slot: &LatestFrame<Decoded>,
    rejected: &AtomicU32,
    demand: &Demand,
    stop: &AtomicBool,
) -> Result<(), String> {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|error| error.to_string())?;
    socket
        .connect(settings.rtp)
        .map_err(|error| error.to_string())?;
    let mut random = [0_u8; 6];
    getrandom(&mut random)?;
    let mut packetizer = Vp8Packetizer::new(
        PAYLOAD_TYPE,
        u32::from_be_bytes([random[0], random[1], random[2], random[3]]),
        u16::from_be_bytes([random[4], random[5]]),
        MTU,
    )
    .map_err(|error| error.to_string())?;
    let mut encoder: Option<Vp8Encoder> = None;
    let start = Instant::now();
    let mut last_pts = -1_i64;
    let mut stats = Stats::default();
    let mut generation = demand.generation();
    while !stop.load(Ordering::Relaxed) {
        stats.report(slot.replaced(), rejected.load(Ordering::Relaxed));
        let Some(decoded) = slot.take(Duration::from_millis(500)) else {
            continue;
        };
        stats.decode += decoded.decode;
        stats.convert += decoded.convert;
        let frame = &decoded.frame;
        let (width, height) = (frame.width as u32, frame.height as u32);
        if encoder.as_ref().is_none_or(|encoder| {
            (encoder.config().width, encoder.config().height) != (width, height)
        }) {
            encoder = Some(
                Vp8Encoder::new(Vp8Config {
                    width,
                    height,
                    fps: settings.max_fps,
                    kbps: settings.kbps,
                    threads: settings.threads,
                    // Janus RTP input cannot request key frames; send one every
                    // two seconds so joining viewers converge quickly.
                    keyframe_interval: settings.max_fps * 2,
                })
                .map_err(|error| error.to_string())?,
            );
        }
        let encoder = encoder.as_mut().expect("encoder was just created");
        let pts =
            (decoded.at.saturating_duration_since(start).as_micros() as i64).max(last_pts + 1);
        last_pts = pts;
        // A new viewer starts from a key frame instead of the next periodic one.
        let force_key = demand.generation() != generation;
        generation = demand.generation();
        let encode_start = Instant::now();
        let encoded = encoder
            .encode(
                I420 {
                    y: &frame.y,
                    u: &frame.u,
                    v: &frame.v,
                    y_stride: frame.width,
                    uv_stride: frame.width / 2,
                },
                pts,
                1_000_000 / i64::from(settings.max_fps),
                force_key,
            )
            .map_err(|error| error.to_string())?;
        stats.encode += encode_start.elapsed();
        stats.frames += 1;
        let timestamp = Vp8Packetizer::timestamp(pts as u64);
        for output in encoded {
            if let Ok(packets) = packetizer.packetize(&output.data, timestamp) {
                for packet in packets {
                    // Loss is tolerated: the next key frame repairs the view.
                    let _ = socket.send(&packet);
                }
            }
        }
    }
    Ok(())
}

/// Per-stage timing, reported to the journal every ten seconds.
#[derive(Default)]
struct Stats {
    since: Option<Instant>,
    frames: u32,
    decode: Duration,
    convert: Duration,
    encode: Duration,
}

impl Stats {
    fn report(&mut self, replaced: u64, rejected: u32) {
        let since = *self.since.get_or_insert_with(Instant::now);
        let elapsed = since.elapsed();
        if elapsed < Duration::from_secs(10) {
            return;
        }
        let per_frame = |total: Duration| total.as_micros() / u128::from(self.frames.max(1));
        eprintln!(
            "bli-kvm-video-encoder: fps={:.1} rejected={} replaced_total={} per_frame_us decode={} convert={} encode={}",
            f64::from(self.frames) / elapsed.as_secs_f64(),
            rejected,
            replaced,
            per_frame(self.decode),
            per_frame(self.convert),
            per_frame(self.encode),
        );
        *self = Self {
            since: Some(Instant::now()),
            ..Self::default()
        };
    }
}

fn getrandom(buffer: &mut [u8]) -> Result<(), String> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(buffer))
        .map_err(|error| error.to_string())
}

fn main() -> ExitCode {
    let settings = match settings() {
        Ok(settings) => Arc::new(settings),
        Err(error) => {
            eprintln!("bli-kvm-video-encoder: {error}");
            return ExitCode::from(2);
        }
    };
    let demand = match &settings.demand {
        Some(path) => {
            let group = std::fs::read_to_string("/etc/group")
                .ok()
                .and_then(|groups| group_id(&groups, &settings.demand_group));
            match Demand::listen(path, group) {
                Ok(demand) => demand,
                Err(error) => {
                    eprintln!("bli-kvm-video-encoder: demand socket: {error}");
                    return ExitCode::from(2);
                }
            }
        }
        None => Demand::always(),
    };
    let stop = Arc::new(AtomicBool::new(false));
    let captured = Arc::new(LatestFrame::new());
    let decoded = Arc::new(LatestFrame::new());
    let rejected = Arc::new(AtomicU32::new(0));
    let reader = {
        let (settings, captured, demand, stop) = (
            Arc::clone(&settings),
            Arc::clone(&captured),
            Arc::clone(&demand),
            Arc::clone(&stop),
        );
        thread::spawn(move || {
            let policy = GatePolicy::new(settings.max_fps, Duration::from_secs(2))
                .expect("validated frame rate");
            let mut gate = FrameGate::new(policy);
            let mut backoff = Duration::from_millis(200);
            while !stop.load(Ordering::Relaxed) {
                demand.wait_for_viewers(&stop);
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let started = Instant::now();
                if let Err(error) = read_stream(&settings, &mut gate, &captured, &demand, &stop) {
                    eprintln!("bli-kvm-video-encoder: capture stream ended: {error}");
                }
                gate.reset();
                if !demand.active() {
                    // Left for want of viewers, not because the stream failed.
                    backoff = Duration::from_millis(200);
                    continue;
                }
                if started.elapsed() > Duration::from_secs(10) {
                    backoff = Duration::from_millis(200);
                }
                thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(5));
            }
        })
    };
    let converter = {
        let (settings, captured, decoded, rejected, stop) = (
            Arc::clone(&settings),
            Arc::clone(&captured),
            Arc::clone(&decoded),
            Arc::clone(&rejected),
            Arc::clone(&stop),
        );
        thread::spawn(move || convert_loop(&settings, &captured, &decoded, &rejected, &stop))
    };
    let result = encode_loop(&settings, &decoded, &rejected, &demand, &stop);
    // Any encoder failure ends the process; systemd restarts it with backoff.
    stop.store(true, Ordering::Relaxed);
    captured.close();
    decoded.close();
    drop((reader, converter));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bli-kvm-video-encoder: {error}");
            ExitCode::FAILURE
        }
    }
}
