//! The product's x264, as the encoder unit drives it, over pictures from a
//! file: the reference a hardware encoder is measured against
//! (`docs/design/h616-ve-h264-encoder.md`, "Measures"). It has no other use.
//!
//! usage: h264_reference WIDTH HEIGHT FPS KBPS KEY_EVERY PRESET THREADS < NV12 > ANNEX_B
//!
//! NV12 pictures of WIDTH by HEIGHT are read from standard input until it
//! ends; every access unit goes to standard output. A key frame is made for
//! the first picture and every KEY_EVERY pictures. One line of numbers goes
//! to standard error at the end.

use std::{
    env,
    io::{self, BufWriter, Read, Write},
    process::ExitCode,
    time::{Duration, Instant},
};

use bli_kvm_video_encoder::{
    h264::{H264Config, H264Encoder, H264Preset},
    vp8::I420,
};

fn number(arguments: &[String], index: usize, name: &str) -> Result<u32, String> {
    arguments
        .get(index)
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| format!("{name} is a number"))
}

fn run() -> Result<(), String> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    if arguments.len() != 7 {
        return Err(
            "usage: h264_reference WIDTH HEIGHT FPS KBPS KEY_EVERY PRESET THREADS < NV12 > ANNEX_B"
                .into(),
        );
    }
    let width = number(&arguments, 0, "WIDTH")?;
    let height = number(&arguments, 1, "HEIGHT")?;
    let fps = number(&arguments, 2, "FPS")?;
    let kbps = number(&arguments, 3, "KBPS")?;
    let key_every = number(&arguments, 4, "KEY_EVERY")?.max(1);
    let preset = H264Preset::parse(&arguments[5]).ok_or("PRESET is superfast or ultrafast")?;
    let threads = number(&arguments, 6, "THREADS")?;
    if width % 2 != 0 || height % 2 != 0 || width == 0 || height == 0 {
        return Err("WIDTH and HEIGHT are even".into());
    }
    let mut encoder = H264Encoder::new(H264Config {
        width,
        height,
        fps,
        kbps,
        threads,
        // The key frames are asked for one by one below.
        keyframe_interval: 600,
        preset,
    })
    .map_err(|error| format!("x264: {error:?}"))?;

    let (width, height) = (width as usize, height as usize);
    let mut luma = vec![0u8; width * height];
    let mut chroma = vec![0u8; width * height / 2];
    let mut u = vec![0u8; width * height / 4];
    let mut v = vec![0u8; width * height / 4];
    let mut input = io::stdin().lock();
    let mut out = BufWriter::new(io::stdout().lock());
    let (mut frames, mut bytes, mut longest) = (0u64, 0u64, Duration::ZERO);
    let mut spent = Duration::ZERO;
    loop {
        match input.read_exact(&mut luma) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(format!("input: {error}")),
        }
        input
            .read_exact(&mut chroma)
            .map_err(|error| format!("input: half a picture: {error}"))?;
        for (index, pair) in chroma.chunks_exact(2).enumerate() {
            u[index] = pair[0];
            v[index] = pair[1];
        }
        let began = Instant::now();
        let units = encoder
            .encode(
                I420 {
                    y: &luma,
                    u: &u,
                    v: &v,
                    y_stride: width,
                    uv_stride: width / 2,
                },
                (frames * 1_000_000 / fps as u64) as i64,
                frames % key_every as u64 == 0,
            )
            .map_err(|error| format!("x264: frame {frames}: {error:?}"))?;
        let took = began.elapsed();
        spent += took;
        longest = longest.max(took);
        for unit in &units {
            out.write_all(&unit.data)
                .map_err(|error| format!("output: {error}"))?;
            bytes += unit.data.len() as u64;
        }
        frames += 1;
    }
    out.flush().map_err(|error| format!("output: {error}"))?;
    // One line of numbers for whoever reads the run.
    let per_frame = |total: u64| total.checked_div(frames).unwrap_or(0);
    eprintln!(
        "{{\"frames\":{frames},\"bytes\":{bytes},\"kbpsMean\":{},\"encodeMicrosMean\":{},\"encodeMicrosMax\":{}}}",
        per_frame(bytes * 8 * fps as u64) / 1000,
        per_frame(spent.as_micros() as u64),
        longest.as_micros()
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(2)
        }
    }
}
