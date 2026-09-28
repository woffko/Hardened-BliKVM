//! WebRTC video feed for Janus: reads the uStreamer MJPEG stream, encodes
//! changed frames with libvpx (VP8) or x264 (H.264) and sends RTP to a
//! loopback UDP port. With a demand socket it works only while a WebRTC
//! viewer is connected.
//!
//! `bli-kvm-video-encoder bench [FRAMES]` instead reads that many frames from
//! uStreamer into memory and measures every codec and size on them (plan
//! J4); the frames never leave the process.

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
    FrameDecision, FrameGate, GatePolicy, LatestFrame,
    demand::{Demand, group_id},
    engine::{Codec, Engine, EngineSettings},
    h264::H264Preset,
    mjpeg::MjpegReader,
    yuv::{I420Frame, JpegToI420, scale_two_thirds},
};

const STREAM_REQUEST: &[u8] =
    b"GET /stream HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nUser-Agent: Hardened-BliKVM-webrtc/0.1\r\n\r\n";

/// The size encoded, from the capture's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scale {
    Full,
    /// 1920x1080 to 1280x720.
    TwoThirds,
    /// 1920x1080 to 960x540.
    Half,
}

impl Scale {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "full" => Some(Self::Full),
            "two-thirds" => Some(Self::TwoThirds),
            "half" => Some(Self::Half),
            _ => None,
        }
    }
}

struct Settings {
    upstream: PathBuf,
    rtp: SocketAddr,
    max_fps: u32,
    engine: EngineSettings,
    scale: Scale,
    demand: Option<PathBuf>,
    demand_group: String,
}

/// After the last viewer leaves, the capture stream is kept this long.
const IDLE_GRACE: Duration = Duration::from_secs(2);
/// A key frame at least this often, whatever the frame rate: frames come only
/// when the picture changes, Janus cannot pass a viewer's key frame request
/// on to an RTP source, and a viewer that lost the last key frame (or joined
/// before its media was up) sees nothing until the next.
const KEY_FRAME_EVERY: Duration = Duration::from_secs(2);
/// Decoding and converting a 1080p frame takes about 45 ms on the H616,
/// more than encoding it with x264 ultrafast (29 ms): two threads take turns
/// so preparation keeps up with the encoder (plan J2).
const CONVERTERS: usize = 2;

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
    let codec = match env::var("BLI_KVM_ENCODER_CODEC") {
        Err(_) => Codec::Vp8,
        Ok(value) => Codec::parse(&value).ok_or("BLI_KVM_ENCODER_CODEC is invalid")?,
    };
    let scale = match env::var("BLI_KVM_ENCODER_SCALE") {
        Ok(value) => Scale::parse(&value).ok_or("BLI_KVM_ENCODER_SCALE is invalid")?,
        // The first VP8 unit's switch.
        Err(_) if number("BLI_KVM_VP8_HALVE", 0, 0..=1)? == 1 => Scale::Half,
        Err(_) => Scale::Full,
    };
    // RTP never leaves the host: Janus reads it from loopback.
    let default_port = if codec == Codec::Vp8 { 5004 } else { 5006 };
    let port = match env::var("BLI_KVM_ENCODER_RTP_PORT") {
        Ok(_) => number("BLI_KVM_ENCODER_RTP_PORT", default_port, 1024..=65535)?,
        Err(_) => number("BLI_KVM_VP8_RTP_PORT", default_port, 1024..=65535)?,
    };
    let max_fps = number("BLI_KVM_VP8_MAX_FPS", 30, 1..=60)?;
    Ok(Settings {
        upstream,
        rtp: SocketAddr::from((Ipv4Addr::LOCALHOST, port as u16)),
        max_fps,
        engine: EngineSettings {
            codec,
            fps: max_fps,
            kbps: number("BLI_KVM_VP8_KBPS", 2500, 64..=20000)?,
            threads: number("BLI_KVM_VP8_THREADS", 3, 1..=4)?,
        },
        scale,
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

fn open_capture(settings: &Settings) -> io::Result<MjpegReader<UnixStream>> {
    let mut stream = UnixStream::connect(&settings.upstream)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.write_all(STREAM_REQUEST)?;
    MjpegReader::new(stream)
}

/// Read frames until the stream fails; the caller reconnects.
fn read_stream(
    settings: &Settings,
    gate: &mut FrameGate,
    slot: &LatestFrame<Captured>,
    demand: &Demand,
    stop: &AtomicBool,
) -> io::Result<()> {
    let mut reader = open_capture(settings)?;
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

/// Decodes one capture frame to I420 at the configured size.
fn prepare(
    converter: &mut JpegToI420,
    jpeg: &[u8],
    scale: Scale,
    full: &mut I420Frame,
) -> Option<(I420Frame, Duration)> {
    converter.convert(jpeg, scale == Scale::Half, full).ok()?;
    if scale != Scale::TwoThirds {
        return Some((std::mem::take(full), converter.last_decode));
    }
    let mut scaled = I420Frame::default();
    scale_two_thirds(full, &mut scaled).ok()?;
    Some((scaled, converter.last_decode))
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
    let mut full = I420Frame::default();
    while !stop.load(Ordering::Relaxed) {
        let Some(captured) = input.take(Duration::from_millis(500)) else {
            continue;
        };
        let start = Instant::now();
        let Some((frame, decode)) =
            prepare(&mut converter, &captured.jpeg, settings.scale, &mut full)
        else {
            rejected.fetch_add(1, Ordering::Relaxed);
            continue;
        };
        output.publish(Decoded {
            frame,
            at: captured.at,
            decode,
            convert: start.elapsed().saturating_sub(decode),
        });
    }
}

fn session_ids() -> Result<(u32, u16), String> {
    let mut random = [0_u8; 6];
    getrandom(&mut random)?;
    Ok((
        u32::from_be_bytes([random[0], random[1], random[2], random[3]]),
        u16::from_be_bytes([random[4], random[5]]),
    ))
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
    let (ssrc, sequence) = session_ids()?;
    let mut engine: Option<Engine> = None;
    let start = Instant::now();
    let mut last_pts = -1_i64;
    let mut stats = Stats::default();
    let mut generation = demand.generation();
    let mut last_capture: Option<Instant> = None;
    let mut last_key: Option<Instant> = None;
    while !stop.load(Ordering::Relaxed) {
        stats.report(slot.replaced(), rejected.load(Ordering::Relaxed));
        let Some(decoded) = slot.take(Duration::from_millis(500)) else {
            continue;
        };
        // The converters take turns, so a frame may finish after a newer one.
        if last_capture.is_some_and(|last| decoded.at <= last) {
            continue;
        }
        last_capture = Some(decoded.at);
        stats.decode += decoded.decode;
        stats.convert += decoded.convert;
        let frame = &decoded.frame;
        if engine
            .as_ref()
            .is_none_or(|engine| engine.size() != (frame.width, frame.height))
        {
            engine = Some(Engine::new(
                settings.engine,
                frame.width,
                frame.height,
                ssrc,
                sequence,
            )?);
        }
        let engine = engine.as_mut().expect("engine was just created");
        let pts =
            (decoded.at.saturating_duration_since(start).as_micros() as i64).max(last_pts + 1);
        last_pts = pts;
        // A new viewer starts from a key frame instead of the next periodic
        // one, and a key frame follows at least every KEY_FRAME_EVERY.
        let force_key = demand.generation() != generation
            || last_key.is_none_or(|last| last.elapsed() >= KEY_FRAME_EVERY);
        generation = demand.generation();
        if force_key {
            last_key = Some(Instant::now());
        }
        let encode_start = Instant::now();
        let packets = engine.encode(frame, pts, force_key)?;
        stats.encode += encode_start.elapsed();
        stats.frames += 1;
        for packet in packets {
            // Loss is tolerated: the next key frame repairs the view.
            let _ = socket.send(&packet);
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
        if self.frames == 0 {
            // Idle between viewers: nothing to report.
            self.since = Some(Instant::now());
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

/// Shifts a frame's rows as if the screen scrolled by `rows`, so every
/// benchmark frame differs from the one before.
fn scrolled(frame: &I420Frame, rows: usize) -> I420Frame {
    let shift = |plane: &[u8], width: usize, height: usize, by: usize| {
        let mut out = Vec::with_capacity(plane.len());
        let by = by % height.max(1);
        out.extend_from_slice(&plane[by * width..]);
        out.extend_from_slice(&plane[..by * width]);
        out
    };
    let rows = rows & !1;
    I420Frame {
        width: frame.width,
        height: frame.height,
        y: shift(&frame.y, frame.width, frame.height, rows),
        u: shift(&frame.u, frame.width / 2, frame.height / 2, rows / 2),
        v: shift(&frame.v, frame.width / 2, frame.height / 2, rows / 2),
    }
}

fn millis(total: Duration, count: usize) -> f64 {
    (total.as_secs_f64() * 1000.0 / count.max(1) as f64 * 10.0).round() / 10.0
}

/// Reads `count` capture frames and measures each codec and size on them.
fn bench(settings: &Settings, count: usize) -> Result<(), String> {
    let mut reader = open_capture(settings).map_err(|error| error.to_string())?;
    let mut jpegs = Vec::with_capacity(count);
    while jpegs.len() < count {
        let mut jpeg = Vec::new();
        reader
            .next_frame(&mut jpeg)
            .map_err(|error| error.to_string())?;
        jpegs.push(jpeg);
    }
    drop(reader);
    let mut rows = Vec::new();
    for scale in [Scale::Full, Scale::TwoThirds] {
        let mut converter = JpegToI420::default();
        let mut full = I420Frame::default();
        let (mut decode, mut convert) = (Duration::ZERO, Duration::ZERO);
        let mut frames = Vec::with_capacity(jpegs.len());
        for jpeg in &jpegs {
            let start = Instant::now();
            let (frame, decoded) = prepare(&mut converter, jpeg, scale, &mut full)
                .ok_or("a capture frame did not decode")?;
            decode += decoded;
            convert += start.elapsed().saturating_sub(decoded);
            frames.push(frame);
        }
        let (width, height) = (frames[0].width, frames[0].height);
        let prepare_ms = millis(decode + convert, frames.len());
        let codecs = [
            Codec::Vp8,
            Codec::H264(H264Preset::Superfast),
            Codec::H264(H264Preset::Ultrafast),
        ];
        for codec in codecs {
            for threads in [3, 4] {
                let engine_settings = EngineSettings {
                    codec,
                    threads,
                    ..settings.engine
                };
                let mut engine = Engine::new(engine_settings, width, height, 1, 0)?;
                let (mut encode, mut bytes) = (Duration::ZERO, 0_usize);
                for (index, frame) in frames.iter().enumerate() {
                    let moved = scrolled(frame, index * 24);
                    let start = Instant::now();
                    let packets = engine.encode(&moved, index as i64 * 33_333, index == 0)?;
                    encode += start.elapsed();
                    bytes += packets.iter().map(Vec::len).sum::<usize>();
                }
                let encode_ms = millis(encode, frames.len());
                // Preparation (on CONVERTERS threads) and encoding overlap.
                let pipelined = 1000.0 / (prepare_ms / CONVERTERS as f64).max(encode_ms).max(0.1);
                rows.push(format!(
                    "{{\"codec\":\"{}\",\"threads\":{},\"width\":{},\"height\":{},\"decodeMs\":{},\"convertMs\":{},\"encodeMs\":{},\"pipelinedFps\":{:.1},\"kbitPerFrame\":{:.0}}}",
                    codec.name(),
                    threads,
                    width,
                    height,
                    millis(decode, frames.len()),
                    millis(convert, frames.len()),
                    encode_ms,
                    pipelined,
                    bytes as f64 * 8.0 / 1000.0 / frames.len() as f64,
                ));
            }
        }
    }
    println!(
        "{{\"frames\":{},\"results\":[{}]}}",
        jpegs.len(),
        rows.join(",")
    );
    Ok(())
}

fn main() -> ExitCode {
    let settings = match settings() {
        Ok(settings) => Arc::new(settings),
        Err(error) => {
            eprintln!("bli-kvm-video-encoder: {error}");
            return ExitCode::from(2);
        }
    };
    let arguments: Vec<String> = env::args().skip(1).collect();
    if arguments.first().map(String::as_str) == Some("bench") {
        let count = arguments
            .get(1)
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|count| (1..=600).contains(count))
            .unwrap_or(60);
        return match bench(&settings, count) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bli-kvm-video-encoder: bench: {error}");
                ExitCode::FAILURE
            }
        };
    }
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
    let converters: Vec<_> = (0..CONVERTERS)
        .map(|_| {
            let (settings, captured, decoded, rejected, stop) = (
                Arc::clone(&settings),
                Arc::clone(&captured),
                Arc::clone(&decoded),
                Arc::clone(&rejected),
                Arc::clone(&stop),
            );
            thread::spawn(move || convert_loop(&settings, &captured, &decoded, &rejected, &stop))
        })
        .collect();
    let result = encode_loop(&settings, &decoded, &rejected, &demand, &stop);
    // Any encoder failure ends the process; systemd restarts it with backoff.
    stop.store(true, Ordering::Relaxed);
    captured.close();
    decoded.close();
    drop((reader, converters));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bli-kvm-video-encoder: {error}");
            ExitCode::FAILURE
        }
    }
}
