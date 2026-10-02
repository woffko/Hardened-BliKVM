//! The lab tool's commands. Those that need the board's engine come with
//! the lab module; these work anywhere.

use std::{
    fs::File,
    io::{BufWriter, Read, Write},
    process::ExitCode,
};

use bli_kvm_ve_lab::{
    engine::FakeEngine,
    headers::{Stream, codec_string},
    picture::{Picture, Sequence, parse_size, render},
    psnr,
    regs::Hypothesis,
    session::Session,
};
use serde_json::json;

const USAGE: &str = "usage:
  bli-kvm-ve-lab generate SEQUENCE WIDTHxHEIGHT FRAMES OUTPUT
      NV12 frames of the visible size, one after the other
  bli-kvm-ve-lab standin MAP SEQUENCE WIDTHxHEIGHT FRAMES KEY_EVERY OUTPUT
      an Annex B stream from the stand-in through the plan of MAP (older, v3s)
  bli-kvm-ve-lab psnr SEQUENCE WIDTHxHEIGHT FRAMES DECODED
      luma PSNR of decoded frames (planar or NV12, visible size) against the sequence
sequences: scroll, drag, motion, pointer";

fn count(value: &str, what: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .ok()
        .filter(|value| (1..=100_000).contains(value))
        .ok_or_else(|| format!("{what} is a number from 1 to 100000, not {value}"))
}

fn stream(width: usize, height: usize, deblocking: bool) -> Stream {
    Stream {
        width: width as u32,
        height: height as u32,
        fps: 30,
        cabac: false,
        chroma_qp_offset: 4,
        deblocking,
    }
}

/// The visible part of a picture as NV12.
fn visible(picture: &Picture, out: &mut impl Write) -> std::io::Result<()> {
    for row in picture
        .luma
        .chunks_exact(picture.coded_width)
        .take(picture.height)
    {
        out.write_all(&row[..picture.width])?;
    }
    for row in picture
        .chroma
        .chunks_exact(picture.coded_width)
        .take(picture.height / 2)
    {
        out.write_all(&row[..picture.width])?;
    }
    Ok(())
}

fn generate(arguments: &[String]) -> Result<(), String> {
    let [sequence, size, frames, output] = arguments else {
        return Err(USAGE.into());
    };
    let sequence: Sequence = sequence.parse()?;
    let (width, height) = parse_size(size)?;
    let frames = count(frames, "FRAMES")?;
    let file = File::create(output).map_err(|error| format!("{output}: {error}"))?;
    let mut out = BufWriter::new(file);
    let mut picture = Picture::new(width, height);
    for frame in 0..frames {
        render(sequence, frame, &mut picture);
        visible(&picture, &mut out).map_err(|error| format!("{output}: {error}"))?;
    }
    out.flush().map_err(|error| format!("{output}: {error}"))?;
    println!(
        "{}",
        json!({"sequence": sequence.name(), "width": width, "height": height, "frames": frames})
    );
    Ok(())
}

fn standin(arguments: &[String]) -> Result<(), String> {
    let [map, sequence, size, frames, key_every, output] = arguments else {
        return Err(USAGE.into());
    };
    let hypothesis = Hypothesis::named(map).ok_or_else(|| format!("no map is called {map}"))?;
    let sequence: Sequence = sequence.parse()?;
    let (width, height) = parse_size(size)?;
    let frames = count(frames, "FRAMES")?;
    let key_every = count(key_every, "KEY_EVERY")?;
    // The stand-in's samples are exact; a deblocking filter has nothing to do.
    let stream = stream(width, height, false);
    let mut session = Session::open(FakeEngine::new(), hypothesis, stream, &stream)
        .map_err(|error| error.to_string())?;
    let file = File::create(output).map_err(|error| format!("{output}: {error}"))?;
    let mut out = BufWriter::new(file);
    let mut picture = Picture::new(width, height);
    let mut sizes = Vec::new();
    for frame in 0..frames {
        render(sequence, frame, &mut picture);
        let encoded = session
            .encode(&picture, frame % key_every == 0, 26)
            .map_err(|error| format!("frame {frame}: {error}"))?;
        out.write_all(&encoded.bytes)
            .map_err(|error| format!("{output}: {error}"))?;
        sizes.push(json!({"key": encoded.key, "bytes": encoded.bytes.len()}));
    }
    out.flush().map_err(|error| format!("{output}: {error}"))?;
    let steps = session.trace.len();
    session.close().map_err(|error| error.to_string())?;
    println!(
        "{}",
        json!({
            "map": hypothesis.name,
            "sequence": sequence.name(),
            "width": width,
            "height": height,
            "codec": codec_string(&stream),
            "frames": sizes,
            "steps": steps,
        })
    );
    Ok(())
}

fn compare(arguments: &[String]) -> Result<(), String> {
    let [sequence, size, frames, decoded] = arguments else {
        return Err(USAGE.into());
    };
    let sequence: Sequence = sequence.parse()?;
    let (width, height) = parse_size(size)?;
    let frames = count(frames, "FRAMES")?;
    let mut file = File::open(decoded).map_err(|error| format!("{decoded}: {error}"))?;
    let mut picture = Picture::new(width, height);
    let mut frame_bytes = vec![0u8; width * height * 3 / 2];
    let mut values = Vec::new();
    for frame in 0..frames {
        file.read_exact(&mut frame_bytes)
            .map_err(|error| format!("{decoded}: frame {frame}: {error}"))?;
        render(sequence, frame, &mut picture);
        values.push(psnr::luma(
            &picture.luma,
            picture.coded_width,
            &frame_bytes,
            width,
            width,
            height,
        ));
    }
    let finite: Vec<f64> = values
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .collect();
    let mean = if finite.is_empty() {
        None
    } else {
        Some(finite.iter().sum::<f64>() / finite.len() as f64)
    };
    let lowest = finite.iter().copied().fold(f64::INFINITY, f64::min);
    println!(
        "{}",
        json!({
            "sequence": sequence.name(),
            "width": width,
            "height": height,
            "frames": frames,
            "exact": values.len() - finite.len(),
            "meanDb": mean,
            "lowestDb": if finite.is_empty() { None } else { Some(lowest) },
            "db": values
                .iter()
                .map(|value| if value.is_finite() { json!((value * 100.0).round() / 100.0) } else { json!("exact") })
                .collect::<Vec<_>>(),
        })
    );
    Ok(())
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let result = match arguments.first().map(String::as_str) {
        Some("generate") => generate(&arguments[1..]),
        Some("standin") => standin(&arguments[1..]),
        Some("psnr") => compare(&arguments[1..]),
        _ => Err(USAGE.into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(2)
        }
    }
}
