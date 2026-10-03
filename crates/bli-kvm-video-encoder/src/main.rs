//! WebRTC video feed for Janus: reads the uStreamer MJPEG stream, encodes
//! changed frames with libvpx (VP8) or as H.264 and sends RTP to a loopback
//! UDP port. H.264 comes from the board's video engine where the unit names
//! its device, the engine encodes and the board has not chosen x264, and
//! from x264 otherwise. With a demand socket it works only while a viewer is
//! connected. The H.264 unit also hands every encoded frame to the frame
//! socket, for the QUIC gateway. A report says what encodes, for the web
//! server.
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
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use bli_kvm_ve_encoder::{
    driver::Driver,
    encoder::{Counters, HardwareEncoder},
};
use bli_kvm_video_encoder::{
    FrameDecision, FrameGate, GatePolicy, LatestFrame,
    choice::{Choice, ProbeHardware},
    demand::{Demand, group_id},
    engine::{Codec, Engine, EngineSettings, Hardware, OpenHardware, add_counters},
    frames::{FrameHub, MonotonicBase, Numbering, user_id},
    h264::H264Preset,
    mjpeg::MjpegReader,
    report::{
        GateCounts, HardwareCounts, MAX_SIDE, MAX_STAGE_MICROS, Report, SCHEMA, StageMicros,
        engine_name,
    },
    yuv::{I420Frame, JpegBackend, JpegToI420, scale_two_thirds},
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
    /// The frame socket, and the only user it serves.
    frames: Option<PathBuf>,
    frame_user: Option<String>,
    /// The video engine's encoder, where the unit has one: its device.
    hardware: Option<PathBuf>,
    /// Where the encoder says which engine encodes, for the web server.
    status: Option<PathBuf>,
    /// Where it reports what encodes and what the board chose.
    report: Option<PathBuf>,
    /// Threads that decode and convert capture frames.
    converters: usize,
}

/// A frame let through for a viewer that waits for a key frame is not
/// followed by another for this long: the encoder takes the request when
/// that frame reaches it.
const KEY_ADMISSION_SPACING: Duration = Duration::from_millis(250);

/// After the last viewer leaves, the capture stream is kept this long.
const IDLE_GRACE: Duration = Duration::from_secs(2);
/// A key frame at least this often, whatever the frame rate: frames come only
/// when the picture changes, Janus cannot pass a viewer's key frame request
/// on to an RTP source, and a viewer that lost the last key frame (or joined
/// before its media was up) sees nothing until the next.
const KEY_FRAME_EVERY: Duration = Duration::from_secs(2);
/// Decoding and converting a 1080p frame takes about 45 ms on the H616,
/// more than encoding it with x264 ultrafast (29 ms): two threads take turns
/// so preparation keeps up with the encoder (plan J2). The video engine
/// encodes in 7 ms and leaves the processors to more of them; the unit says
/// how many.
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
        frames: match env::var_os("BLI_KVM_ENCODER_FRAME_SOCKET").map(PathBuf::from) {
            Some(path) if !path.is_absolute() => {
                return Err("BLI_KVM_ENCODER_FRAME_SOCKET must be absolute".into());
            }
            // The gateway's decoder is the browser's: H.264 only.
            Some(_) if codec == Codec::Vp8 => {
                return Err("BLI_KVM_ENCODER_FRAME_SOCKET needs an H.264 codec".into());
            }
            path => path,
        },
        frame_user: env::var("BLI_KVM_ENCODER_FRAME_USER").ok(),
        hardware: match env::var_os("BLI_KVM_ENCODER_HARDWARE").map(PathBuf::from) {
            Some(path) if !path.is_absolute() => {
                return Err("BLI_KVM_ENCODER_HARDWARE must be absolute".into());
            }
            // The engine encodes H.264 and nothing else.
            Some(_) if codec == Codec::Vp8 => {
                return Err("BLI_KVM_ENCODER_HARDWARE needs an H.264 codec".into());
            }
            path => path,
        },
        status: match env::var_os("BLI_KVM_ENCODER_STATUS").map(PathBuf::from) {
            Some(path) if !path.is_absolute() => {
                return Err("BLI_KVM_ENCODER_STATUS must be absolute".into());
            }
            path => path,
        },
        report: match env::var_os("BLI_KVM_ENCODER_REPORT").map(PathBuf::from) {
            Some(path) if !path.is_absolute() => {
                return Err("BLI_KVM_ENCODER_REPORT must be absolute".into());
            }
            path => path,
        },
        converters: number("BLI_KVM_ENCODER_CONVERTERS", CONVERTERS as u32, 1..=4)? as usize,
    })
}

/// Opens the video engine's encoder for a stream.
fn open_hardware(path: PathBuf) -> Box<OpenHardware> {
    Box::new(move |config| {
        let driver = Driver::open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        HardwareEncoder::new(driver, config)
            .map(|encoder| Box::new(encoder) as Box<dyn Hardware>)
            .map_err(|error| error.to_string())
    })
}

/// Opens the video engine's device, asks what it is and closes it again.
fn probe_hardware(path: PathBuf) -> Box<ProbeHardware> {
    Box::new(move || {
        Driver::open(&path)
            .map(drop)
            .map_err(|error| format!("{}: {error}", path.display()))
    })
}

/// Says which engine encodes: one word in a file, replaced whole. The web
/// server reads it, which is another user: the unit's mask would keep the
/// file to the encoder.
fn write_status(path: &std::path::Path, name: &str) {
    use std::os::unix::fs::PermissionsExt;
    let temporary = path.with_extension("new");
    let written = std::fs::write(&temporary, format!("{name}\n"))
        .and_then(|()| std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o644)))
        .and_then(|()| std::fs::rename(&temporary, path));
    if let Err(error) = written {
        eprintln!("bli-kvm-video-encoder: {}: {error}", path.display());
    }
}

/// Writes the status word where what encodes has changed.
fn note_encoder(settings: &Settings, encoding: &mut &'static str, name: &'static str) {
    if name != *encoding {
        *encoding = name;
        if let Some(path) = &settings.status {
            write_status(path, name);
        }
    }
}

/// What the gate did with the capture's frames, counted by the reader for
/// the report.
#[derive(Default)]
struct GateTally {
    admitted: AtomicU64,
    too_soon: AtomicU64,
    unchanged: AtomicU64,
}

impl GateTally {
    fn count(&self, decision: FrameDecision) {
        let counter = match decision {
            FrameDecision::Encode => &self.admitted,
            FrameDecision::TooSoon => &self.too_soon,
            FrameDecision::Unchanged => &self.unchanged,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    fn counts(&self) -> GateCounts {
        GateCounts {
            admitted: self.admitted.load(Ordering::Relaxed),
            too_soon: self.too_soon.load(Ordering::Relaxed),
            unchanged: self.unchanged.load(Ordering::Relaxed),
        }
    }
}

/// The stages' times over the frames since the report was last written.
#[derive(Default)]
struct StageWindow {
    frames: u32,
    decode: Duration,
    convert: Duration,
    encode: Duration,
}

impl StageWindow {
    fn means(&self) -> StageMicros {
        let mean = |total: Duration| {
            (total.as_micros() / u128::from(self.frames.max(1))).min(u128::from(MAX_STAGE_MICROS))
                as u64
        };
        StageMicros {
            decode: mean(self.decode),
            convert: mean(self.convert),
            encode: mean(self.encode),
        }
    }
}

/// What the report counts.
struct ReportNumbers<'a> {
    encoded_frames: u64,
    window: &'a StageWindow,
    gate: &'a GateTally,
    /// What the engine did since the start, where it has encoded.
    hardware: Option<Counters>,
}

/// What the report says now.
fn report_of(
    settings: &Settings,
    choice: &Choice,
    engine: Option<&Engine>,
    streaming: bool,
    kbps: u32,
    numbers: &ReportNumbers,
) -> Report {
    let h264 = settings.engine.codec != Codec::Vp8;
    let (width, height) = engine.map_or((0, 0), Engine::size);
    Report {
        schema: SCHEMA,
        codec: if h264 { "h264" } else { "vp8" },
        streaming,
        engine: engine.map(|engine| engine_name(engine.encoder_name())),
        engine_wanted: h264.then(|| choice.wanted().name()),
        hardware_available: choice.available(),
        hardware_reason: choice.reason().map(str::to_string),
        width: (width as u32).min(MAX_SIDE),
        height: (height as u32).min(MAX_SIDE),
        kbps,
        max_fps: settings.max_fps,
        converters: settings.converters as u32,
        encoded_frames: numbers.encoded_frames,
        stage_micros: numbers.window.means(),
        gate: numbers.gate.counts(),
        hardware: numbers.hardware.map(|counters| HardwareCounts {
            passes: counters.passes,
            both_searches: counters.both_searches,
            wide: counters.wide,
            higher_qp: counters.higher_qp,
            left_out: counters.left_out,
            key_frames: counters.key_frames,
            failures: counters.failures,
            engine_micros: counters.engine_micros,
        }),
    }
}

/// The report is written at the start, whenever what it says but its
/// numbers changes, and once a second while a viewer watches.
const REPORT_EVERY: Duration = Duration::from_secs(1);

/// Replaces the report where it is due; true where it was written.
fn write_report(settings: &Settings, last: &mut Option<(Report, Instant)>, report: Report) -> bool {
    let Some(path) = &settings.report else {
        return false;
    };
    let due = match last {
        None => true,
        Some((written, at)) => {
            !written.same_but_numbers(&report)
                || (report.streaming && at.elapsed() >= REPORT_EVERY && *written != report)
        }
    };
    if !due {
        return false;
    }
    if let Err(error) = report.write(path) {
        eprintln!("bli-kvm-video-encoder: {}: {error}", path.display());
    }
    *last = Some((report, Instant::now()));
    true
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
    tally: &GateTally,
    slot: &LatestFrame<Captured>,
    demand: &Demand,
    frames: Option<&FrameHub>,
    stop: &AtomicBool,
) -> io::Result<()> {
    let mut reader = open_capture(settings)?;
    let mut idle_since: Option<Instant> = None;
    let mut generation = demand.generation();
    let mut let_through: Option<Instant> = None;
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
        // A viewer that has just come, and a client of the frame socket that
        // asks for a key frame, are answered from the next captured frame,
        // changed or not: on a still screen they would otherwise wait for
        // the periodic one.
        let waiting =
            demand.generation() != generation || frames.is_some_and(|hub| hub.key_due(now));
        if waiting
            && let_through
                .is_none_or(|at| now.saturating_duration_since(at) >= KEY_ADMISSION_SPACING)
        {
            generation = demand.generation();
            let_through = Some(now);
            gate.reset();
        }
        let decision = gate.admit(&jpeg, now);
        tally.count(decision);
        if decision == FrameDecision::Encode {
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

/// A stream id for the frame socket: random, never zero.
fn stream_id() -> Result<u32, String> {
    let mut random = [0_u8; 4];
    getrandom(&mut random)?;
    Ok(u32::from_be_bytes(random).max(1))
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
    tally: &GateTally,
    demand: &Demand,
    frames: Option<&FrameHub>,
    stop: &AtomicBool,
) -> Result<(), String> {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|error| error.to_string())?;
    socket
        .connect(settings.rtp)
        .map_err(|error| error.to_string())?;
    let (ssrc, sequence) = session_ids()?;
    let mut engine: Option<Engine> = None;
    // The board's choice of the engine, and whether the engine can be had.
    let mut choice = Choice::new(
        settings.hardware.clone().map(open_hardware),
        settings.hardware.clone().map(probe_hardware),
    );
    let mut encoding = "";
    let mut reported: Option<(Report, Instant)> = None;
    // The report's numbers: frames since the start, the stages' times since
    // it was last written, and what the engine did in the streams before
    // this one.
    let mut encoded_frames = 0_u64;
    let mut window = StageWindow::default();
    let mut hardware_before = Counters::default();
    let mut hardware_ever = false;
    let start = Instant::now();
    let mut last_pts = -1_i64;
    let mut stats = Stats::default();
    let mut generation = demand.generation();
    let mut last_capture: Option<Instant> = None;
    let mut last_key: Option<Instant> = None;
    // The unit's bitrate until the server asks for another.
    let mut kbps = demand.kbps().unwrap_or(settings.engine.kbps);
    // The frame socket's records: a stream is new with every encoder.
    let clock = MonotonicBase::now();
    let mut numbering = Numbering::new(stream_id()?);
    while !stop.load(Ordering::Relaxed) {
        stats.report(slot.replaced(), rejected.load(Ordering::Relaxed));
        // The board's choice reaches the stream between two frames: a
        // switch does not wait for the picture to change. The choice also
        // asks for a key frame, which the next frame is.
        let lines = demand.engine_lines();
        choice.take(lines, demand.engine(), engine.as_ref());
        if let Some(engine) = engine.as_mut() {
            choice.apply(engine)?;
            note_encoder(settings, &mut encoding, engine.encoder_name());
        }
        hardware_ever |= engine
            .as_ref()
            .is_some_and(|engine| engine.encoder_name() == "hardware");
        let numbers = ReportNumbers {
            encoded_frames,
            window: &window,
            gate: tally,
            hardware: hardware_ever.then(|| {
                add_counters(
                    hardware_before,
                    engine
                        .as_ref()
                        .map(Engine::hardware_totals)
                        .unwrap_or_default(),
                )
            }),
        };
        let report = report_of(
            settings,
            &choice,
            engine.as_ref(),
            demand.active(),
            kbps,
            &numbers,
        );
        if write_report(settings, &mut reported, report) {
            window = StageWindow::default();
        }
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
        window.decode += decoded.decode;
        window.convert += decoded.convert;
        let frame = &decoded.frame;
        if engine
            .as_ref()
            .is_none_or(|engine| engine.size() != (frame.width, frame.height))
        {
            // A new size builds a new encoder but the same stream: the SSRC
            // stays and the sequence numbers go on where the last one ended.
            // The old encoder goes first: two at once would have twice the
            // threads, more than the unit may run.
            if let Some(old) = &engine {
                hardware_before = add_counters(hardware_before, old.hardware_totals());
            }
            let next = engine.take().map_or(sequence, |old| old.next_sequence());
            numbering = Numbering::new(stream_id()?);
            let open = choice.opener();
            let tried = open.is_some();
            let opened = Engine::with_hardware(
                EngineSettings {
                    kbps,
                    ..settings.engine
                },
                frame.width,
                frame.height,
                ssrc,
                next,
                open,
            )?;
            choice.opened(&opened, tried);
            engine = Some(opened);
        }
        let engine = engine.as_mut().expect("engine was just created");
        // The quality the operator picked reaches the running encoder.
        if let Some(requested) = demand.kbps()
            && requested != kbps
        {
            match engine.set_bitrate(requested) {
                Ok(()) => {
                    eprintln!("bli-kvm-video-encoder: bitrate {requested} kbit/s");
                    kbps = requested;
                }
                Err(error) => eprintln!("bli-kvm-video-encoder: bitrate {requested}: {error}"),
            }
        }
        let pts =
            (decoded.at.saturating_duration_since(start).as_micros() as i64).max(last_pts + 1);
        last_pts = pts;
        // A new viewer starts from a key frame instead of the next periodic
        // one, a key frame follows at least every KEY_FRAME_EVERY, and a
        // client of the frame socket may ask for one.
        let force_key = demand.generation() != generation
            || last_key.is_none_or(|last| last.elapsed() >= KEY_FRAME_EVERY)
            || frames.is_some_and(|hub| hub.take_key_request(Instant::now()));
        generation = demand.generation();
        if force_key {
            last_key = Some(Instant::now());
        }
        let encode_start = Instant::now();
        let was_hardware = engine.encoder_name() == "hardware";
        let (packets, encoded) = engine.encode_frames(frame, pts, force_key)?;
        stats.encode += encode_start.elapsed();
        stats.frames += 1;
        window.encode += encode_start.elapsed();
        window.frames += 1;
        encoded_frames += 1;
        if was_hardware && engine.encoder_name() != "hardware" {
            choice.given_up(engine);
        }
        note_encoder(settings, &mut encoding, engine.encoder_name());
        stats.engine = encoding;
        // Counted since the start, so that a switch does not lose them.
        stats.hardware = engine
            .hardware_counters()
            .map(|_| add_counters(hardware_before, engine.hardware_totals()));
        // The encoder makes key frames of its own at its interval: one of
        // those counts like one that was asked for, or two would follow each
        // other whenever the two clocks meet.
        if encoded.iter().any(|output| output.key_frame) {
            last_key = Some(Instant::now());
        }
        for packet in packets {
            // Loss is tolerated: the next key frame repairs the view.
            let _ = socket.send(&packet);
        }
        if let Some(hub) = frames {
            for output in &encoded {
                if let Some(header) = numbering.next(
                    output.key_frame,
                    frame.width,
                    frame.height,
                    output.data.len(),
                    clock.micros_at(decoded.at),
                ) {
                    hub.publish(&header, &output.data);
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
    /// Which engine encodes, and what the video engine's encoder has done
    /// since it began; `reported` is what it had done at the last report.
    engine: &'static str,
    hardware: Option<Counters>,
    reported: Counters,
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
        // What the video engine's encoder did in these ten seconds.
        let engine = match self.hardware {
            Some(now) => {
                let before = self.reported;
                format!(
                    " engine=hardware passes={} both_searches={} wide={} higher_qp={} left_out={} key={} failures={} engine_us={}",
                    now.passes.saturating_sub(before.passes),
                    now.both_searches.saturating_sub(before.both_searches),
                    now.wide.saturating_sub(before.wide),
                    now.higher_qp.saturating_sub(before.higher_qp),
                    now.left_out.saturating_sub(before.left_out),
                    now.key_frames.saturating_sub(before.key_frames),
                    now.failures.saturating_sub(before.failures),
                    now.engine_micros.saturating_sub(before.engine_micros)
                        / now.passes.saturating_sub(before.passes).max(1),
                )
            }
            None => format!(" engine={}", self.engine),
        };
        eprintln!(
            "bli-kvm-video-encoder: fps={:.1} rejected={} replaced_total={} per_frame_us decode={} convert={} encode={}{engine}",
            f64::from(self.frames) / elapsed.as_secs_f64(),
            rejected,
            replaced,
            per_frame(self.decode),
            per_frame(self.convert),
            per_frame(self.encode),
        );
        *self = Self {
            since: Some(Instant::now()),
            engine: self.engine,
            // The engine's counts go on since the start, also while x264
            // encodes.
            reported: self.hardware.unwrap_or(self.reported),
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
    let mut decoder_rows = Vec::new();
    for scale in [Scale::Full, Scale::TwoThirds] {
        // Every decoder built in prepares the same frames; the encoders get
        // the preferred decoder's (the first).
        let mut decoders: Vec<(String, JpegToI420)> = Vec::new();
        for backend in JpegBackend::available() {
            decoders.push((
                backend.name().to_string(),
                JpegToI420::with_backend(backend, false),
            ));
            #[cfg(feature = "turbojpeg")]
            if backend == JpegBackend::Turbo {
                decoders.push((
                    format!("{} fast IDCT", backend.name()),
                    JpegToI420::with_backend(backend, true),
                ));
            }
        }
        let mut prepared: Option<(Vec<I420Frame>, Duration, Duration)> = None;
        for (name, mut converter) in decoders {
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
            decoder_rows.push(format!(
                "{{\"decoder\":\"{}\",\"width\":{},\"height\":{},\"decodeMs\":{},\"convertMs\":{}}}",
                name,
                frames[0].width,
                frames[0].height,
                millis(decode, frames.len()),
                millis(convert, frames.len()),
            ));
            if prepared.is_none() {
                prepared = Some((frames, decode, convert));
            }
        }
        let (frames, decode, convert) = prepared.ok_or("no JPEG decoder is built in")?;
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
                let pipelined = 1000.0
                    / (prepare_ms / settings.converters as f64)
                        .max(encode_ms)
                        .max(0.1);
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
        "{{\"frames\":{},\"decoders\":[{}],\"results\":[{}]}}",
        jpegs.len(),
        decoder_rows.join(","),
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
    // The frame socket serves the gateway's user and nobody else. A user
    // that does not exist leaves it closed, and the encoder as it is.
    let frames = match &settings.frames {
        Some(path) => {
            let admitted = match &settings.frame_user {
                Some(name) => {
                    let found = std::fs::read_to_string("/etc/passwd")
                        .ok()
                        .and_then(|users| user_id(&users, name));
                    if found.is_none() {
                        eprintln!(
                            "bli-kvm-video-encoder: frame socket: user {name} does not exist; no frames are served"
                        );
                    }
                    found.map(Some)
                }
                None => Some(None),
            };
            match admitted.map(|admitted| FrameHub::listen(path, admitted)) {
                Some(Ok(hub)) => Some(hub),
                Some(Err(error)) => {
                    eprintln!("bli-kvm-video-encoder: frame socket: {error}");
                    return ExitCode::from(2);
                }
                None => None,
            }
        }
        None => None,
    };
    let stop = Arc::new(AtomicBool::new(false));
    let captured = Arc::new(LatestFrame::new());
    let decoded = Arc::new(LatestFrame::new());
    let rejected = Arc::new(AtomicU32::new(0));
    let tally = Arc::new(GateTally::default());
    let reader = {
        let (settings, captured, demand, frames, stop, tally) = (
            Arc::clone(&settings),
            Arc::clone(&captured),
            Arc::clone(&demand),
            frames.clone(),
            Arc::clone(&stop),
            Arc::clone(&tally),
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
                if let Err(error) = read_stream(
                    &settings,
                    &mut gate,
                    &tally,
                    &captured,
                    &demand,
                    frames.as_deref(),
                    &stop,
                ) {
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
    let converters: Vec<_> = (0..settings.converters)
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
    let result = encode_loop(
        &settings,
        &decoded,
        &rejected,
        &tally,
        &demand,
        frames.as_deref(),
        &stop,
    );
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
