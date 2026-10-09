//! A frame socket without a board: a moving test picture, encoded by the
//! encoder's own x264 engine and served on a frame socket like the encoder
//! unit's. The transport's host tests decode what it makes; it has no other
//! use.
//!
//! usage: h264_frame_source SOCKET [WIDTH HEIGHT FPS KBPS KEY_EVERY_MS]
//!
//! The picture: a diagonal gradient that moves with every frame, and in its
//! top-left corner sixteen squares of 16 by 16 pixels that spell the frame's
//! number in binary, least significant bit first, white for one. A decoder's
//! output can be told apart frame by frame from those squares. A key frame
//! is made for the first frame, on request, and every KEY_EVERY_MS
//! milliseconds (0: never by itself). A line `kbps N` on the standard input
//! changes the bitrate from the next frame on, as the encoder unit's demand
//! socket does.

use std::{
    env,
    io::BufRead,
    path::PathBuf,
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use bli_kvm_video_encoder::{
    engine::{Codec, Engine, EngineSettings},
    frames::{FrameHub, Numbering, monotonic_micros},
    h264::H264Preset,
    yuv::I420Frame,
};

const BITS: usize = 16;
const SQUARE: usize = 16;

fn picture(width: usize, height: usize, number: u32) -> I420Frame {
    let mut y = vec![0u8; width * height];
    let shift = number as usize * 3;
    for (row, line) in y.chunks_exact_mut(width).enumerate() {
        for (column, value) in line.iter_mut().enumerate() {
            // Mid greys, so the squares stand out at either end.
            *value = 64 + (((column + row + shift) / 2) % 128) as u8;
        }
    }
    for bit in 0..BITS {
        let value = if number >> bit & 1 == 1 { 235 } else { 16 };
        for row in 0..SQUARE.min(height) {
            let start = row * width + bit * SQUARE;
            if start + SQUARE <= y.len() && (bit + 1) * SQUARE <= width {
                y[start..start + SQUARE].fill(value);
            }
        }
    }
    I420Frame {
        width,
        height,
        y,
        u: vec![128; (width / 2) * (height / 2)],
        v: vec![128; (width / 2) * (height / 2)],
    }
}

fn argument<T: std::str::FromStr>(index: usize, default: T) -> Result<T, String> {
    match env::args().nth(index) {
        Some(value) => value
            .parse()
            .map_err(|_| format!("argument {index} is not a number: {value}")),
        None => Ok(default),
    }
}

fn run() -> Result<(), String> {
    let socket = PathBuf::from(
        env::args()
            .nth(1)
            .ok_or("usage: h264_frame_source SOCKET [WIDTH HEIGHT FPS KBPS KEY_EVERY_MS]")?,
    );
    let width: usize = argument(2, 1280)?;
    let height: usize = argument(3, 720)?;
    let fps: u32 = argument(4, 30)?;
    let kbps: u32 = argument(5, 2500)?;
    let key_every: u64 = argument(6, 2000)?;
    if width % 2 != 0 || height % 2 != 0 || width < BITS * SQUARE || fps == 0 || fps > 60 {
        return Err("the size must be even and at least 256 wide, the rate 1 to 60".into());
    }
    let hub = FrameHub::listen(&socket, None).map_err(|error| error.to_string())?;
    let mut engine = Engine::new(
        EngineSettings {
            codec: Codec::H264(H264Preset::Ultrafast),
            fps,
            kbps,
            threads: 2,
        },
        width,
        height,
        1,
        0,
    )?;
    let mut numbering = Numbering::new(std::process::id().max(1));
    let asked = Arc::new(AtomicU32::new(kbps));
    let reading = Arc::clone(&asked);
    thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { return };
            if let Some(kbps) = line
                .strip_prefix("kbps ")
                .and_then(|value| value.trim().parse::<u32>().ok())
            {
                reading.store(kbps, Ordering::Relaxed);
            }
        }
    });
    let mut kbps = kbps;
    println!("listening");
    let start = Instant::now();
    let interval = Duration::from_micros(1_000_000 / u64::from(fps));
    let mut last_key: Option<Instant> = None;
    for number in 1u32.. {
        let due = start + interval * number;
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            thread::sleep(wait);
        }
        // Nobody listens: the picture moves on, nothing is encoded.
        if hub.clients() == 0 {
            continue;
        }
        let wanted = asked.load(Ordering::Relaxed);
        if wanted != kbps {
            engine.set_bitrate(wanted)?;
            kbps = wanted;
        }
        let now = Instant::now();
        let force_key = last_key.is_none()
            || hub.take_key_request(now)
            || (key_every > 0
                && last_key.is_some_and(|last| last.elapsed().as_millis() as u64 >= key_every));
        if force_key {
            last_key = Some(now);
        }
        let captured = monotonic_micros();
        let frame = picture(width, height, number);
        let (_, encoded) =
            engine.encode_frames(&frame, start.elapsed().as_micros() as i64, force_key)?;
        if encoded.iter().any(|output| output.key_frame) {
            last_key = Some(now);
        }
        for output in &encoded {
            if let Some(header) =
                numbering.next(output.key_frame, width, height, output.data.len(), captured)
            {
                hub.publish(&header, &output.data);
            }
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("h264_frame_source: {error}");
            ExitCode::FAILURE
        }
    }
}
