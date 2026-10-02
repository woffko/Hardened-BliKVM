//! The lab tool's commands. Those that need the board's engine come with
//! the lab module; these work anywhere.

use std::{
    fs::File,
    io::{BufWriter, Read, Write},
    process::ExitCode,
};

use bli_kvm_ve_lab::{
    device::Device,
    engine::{Engine, FakeEngine, Step},
    headers::{Stream, codec_string},
    lab,
    picture::{Picture, Sequence, parse_size, render},
    psnr,
    regs::{self, Hypothesis, Op},
    session::Session,
};
use serde_json::{Value, json};

const USAGE: &str = "usage:
  bli-kvm-ve-lab generate SEQUENCE WIDTHxHEIGHT FRAMES OUTPUT
      NV12 frames of the visible size, one after the other
  bli-kvm-ve-lab standin MAP SEQUENCE WIDTHxHEIGHT FRAMES KEY_EVERY OUTPUT
      an Annex B stream from the stand-in through the plan of MAP (older, v3s, h6)
  bli-kvm-ve-lab psnr SEQUENCE WIDTHxHEIGHT FRAMES DECODED
      luma PSNR of decoded frames (planar or NV12, visible size) against the sequence
on the board, with the lab module loaded (each writes a receipt to OUTPUT):
  bli-kvm-ve-lab probe OUTPUT
      the engine's version and the registers of the blocks the open maps name; reads only
  bli-kvm-ve-lab buffers OUTPUT
      buffers of several sizes: their addresses, and that they keep what is written
  bli-kvm-ve-lab masks MAP OUTPUT
      with the encoder selected as MAP says: which bits of its registers can be set; no trigger
  bli-kvm-ve-lab bits MAP WIDTHxHEIGHT TIMES OUTPUT
      SPS and PPS, TIMES times, through the engine's bit writer into the stream buffer; no encode
  bli-kvm-ve-lab encode MAP SEQUENCE WIDTHxHEIGHT FRAMES KEY_EVERY QP STREAM OUTPUT
      frames of SEQUENCE encoded by the engine into STREAM (Annex B)
sequences: scroll, drag, motion, pointer";

/// The commit the tool was built from, when the build says.
const COMMIT: &str = match option_env!("BLI_KVM_VE_LAB_COMMIT") {
    Some(commit) => commit,
    None => "unknown",
};
/// Where the board's memory begins and ends, for a plausibility check of the
/// addresses the lab module gives: 1 GiB from 0x4000_0000.
const MEMORY: std::ops::Range<u64> = 0x4000_0000..0x8000_0000;

fn hex(value: u32) -> String {
    format!("{value:#010x}")
}

fn receipt(output: &str, experiment: &str, body: Value) -> Result<(), String> {
    let mut document = json!({
        "schemaVersion": 1,
        "experiment": experiment,
        "toolCommit": COMMIT,
    });
    if let (Some(document), Some(body)) = (document.as_object_mut(), body.as_object()) {
        document.extend(body.clone());
    }
    let text = serde_json::to_string_pretty(&document).map_err(|error| error.to_string())?;
    std::fs::write(output, text + "\n").map_err(|error| format!("{output}: {error}"))
}

fn open_device() -> Result<Device, String> {
    Device::open().map_err(|error| format!("{}: {error}", bli_kvm_ve_lab::device::DEVICE))
}

fn steps(trace: &[Step]) -> Vec<Value> {
    trace
        .iter()
        .map(|step| match step.op {
            Op::Write { reg, value } => json!(["write", hex(reg), hex(value)]),
            Op::Update { reg, clear, set } => {
                json!(["update", hex(reg), hex(clear), hex(set), step.read.map(hex)])
            }
            Op::Until { reg, mask } => json!(["until", hex(reg), hex(mask), step.read.map(hex)]),
            Op::Wait => json!(["wait", step.read.map(hex)]),
        })
        .collect()
}

fn probe(arguments: &[String]) -> Result<(), String> {
    let [output] = arguments else {
        return Err(USAGE.into());
    };
    let mut device = open_device()?;
    let mut blocks = serde_json::Map::new();
    for (name, from, to) in lab::BLOCKS {
        let to = to.min(device.window() as u32);
        let registers: serde_json::Map<String, Value> = lab::dump(&mut device, from, to)
            .into_iter()
            .map(|(reg, value)| (format!("{reg:#05x}"), json!(hex(value))))
            .collect();
        blocks.insert(name.into(), Value::Object(registers));
    }
    let version = device.version();
    receipt(
        output,
        "probe",
        json!({
            "versionRegister": hex(version),
            "engineVersion": format!("{:#06x}", version >> 16),
            "windowBytes": device.window(),
            "blocks": blocks,
        }),
    )?;
    println!("engine version {:#06x}; receipt {output}", version >> 16);
    Ok(())
}

fn buffers(arguments: &[String]) -> Result<(), String> {
    let [output] = arguments else {
        return Err(USAGE.into());
    };
    let mut device = open_device()?;
    let sizes = [4096, 65_536, 1 << 20, 4 << 20, 4 << 20, 16 << 20];
    let checks = lab::check_buffers(&mut device, &sizes).map_err(|error| error.to_string())?;
    let rows: Vec<Value> = checks
        .iter()
        .map(|check| {
            let end = check.address as u64 + check.bytes as u64;
            json!({
                "bytes": check.bytes,
                "address": hex(check.address),
                "inMemory": MEMORY.contains(&(check.address as u64)) && end <= MEMORY.end,
                "zeroed": check.zeroed,
                "holds": check.holds,
            })
        })
        .collect();
    let good = rows
        .iter()
        .all(|row| row["inMemory"] == true && row["zeroed"] == true && row["holds"] == true);
    receipt(output, "buffers", json!({"buffers": rows, "passed": good}))?;
    println!("{} buffers, passed: {good}; receipt {output}", checks.len());
    if good {
        Ok(())
    } else {
        Err("a buffer failed its check".into())
    }
}

fn masks(arguments: &[String]) -> Result<(), String> {
    let [map, output] = arguments else {
        return Err(USAGE.into());
    };
    let hypothesis = Hypothesis::named(map).ok_or_else(|| format!("no map is called {map}"))?;
    let mut device = open_device()?;
    let mut trace = Vec::new();
    let before = (device.read(regs::MODE), device.read(regs::RESET));
    let rows = lab::probe_masks(&mut device, &hypothesis, &mut trace)
        .map_err(|fault| fault.to_string())?;
    let after = (device.read(regs::MODE), device.read(regs::RESET));
    let table: Vec<Value> = rows
        .iter()
        .map(|row| {
            json!({
                "register": format!("{:#05x}", row.reg),
                "name": regs::name(row.reg),
                "tried": hex(row.tried),
                "before": hex(row.before),
                "ones": hex(row.ones),
                "zeros": hex(row.zeros),
                "after": hex(row.after),
            })
        })
        .collect();
    receipt(
        output,
        "masks",
        json!({
            "map": hypothesis.name,
            "modeBefore": hex(before.0),
            "resetBefore": hex(before.1),
            "modeAfter": hex(after.0),
            "resetAfter": hex(after.1),
            "selection": steps(&trace),
            "registers": table,
            "unknownInterrupt": device.unknown_interrupt,
        }),
    )?;
    let holding = rows.iter().filter(|row| row.ones != 0).count();
    println!(
        "{} of {} registers hold bits; receipt {output}",
        holding,
        rows.len()
    );
    Ok(())
}

fn count(value: &str, what: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .ok()
        .filter(|value| (1..=100_000).contains(value))
        .ok_or_else(|| format!("{what} is a number from 1 to 100000, not {value}"))
}

fn stream(hypothesis: &Hypothesis, width: usize, height: usize, deblocking: bool) -> Stream {
    Stream {
        width: width as u32,
        height: height as u32,
        fps: 30,
        cabac: false,
        chroma_qp_offset: hypothesis.chroma_qp_offset(),
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
    let stream = stream(&hypothesis, width, height, false);
    let engine = FakeEngine::for_hypothesis(&hypothesis);
    let mut session =
        Session::open(engine, hypothesis, stream, &stream).map_err(|error| error.to_string())?;
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

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The registers of a range that are not zero.
fn nonzero(engine: &mut dyn Engine, from: u32, to: u32) -> Value {
    Value::Object(
        lab::dump(engine, from, to)
            .into_iter()
            .filter(|(_, value)| *value != 0)
            .map(|(reg, value)| (format!("{reg:#05x}"), json!(hex(value))))
            .collect(),
    )
}

/// Buffers are made for the largest picture of the lab, so that a small
/// one has room many times its size.
fn room_for(hypothesis: &Hypothesis, width: usize, height: usize) -> Stream {
    stream(hypothesis, width.max(1920), height.max(1080), true)
}

fn bits(arguments: &[String]) -> Result<(), String> {
    let [map, size, times, output] = arguments else {
        return Err(USAGE.into());
    };
    let hypothesis = Hypothesis::named(map).ok_or_else(|| format!("no map is called {map}"))?;
    let (width, height) = parse_size(size)?;
    let times = count(times, "TIMES")? as usize;
    let stream = stream(&hypothesis, width, height, true);
    let room = room_for(&hypothesis, width, height);
    let mut session = Session::open(open_device()?, hypothesis, stream, &room)
        .map_err(|error| error.to_string())?;
    let result = session.put_parameter_sets(times);
    let encoder = nonzero(session.engine(), 0xb00, 0xc00);
    let top = nonzero(session.engine(), 0x000, 0x100);
    let mut body = json!({
        "map": hypothesis.name,
        "width": width,
        "height": height,
        "times": times,
        // The first 200 steps; the rest repeats them.
        "steps": steps(&session.trace[..session.trace.len().min(200)]),
        "encoderAfter": encoder,
        "topAfter": top,
        "unknownInterrupt": session.engine().unknown_interrupt,
    });
    let outcome = match &result {
        Ok(report) => {
            body["expected"] = json!(hex_bytes(&report.expected));
            body["found"] = json!(hex_bytes(&report.found));
            body["bitsReported"] = json!(report.bits);
            body["bytesInBuffer"] = json!(
                report
                    .found
                    .iter()
                    .zip(&report.expected)
                    .take_while(|(found, expected)| found == expected)
                    .count()
            );
            body["status"] = json!(hex(report.status));
            body["matches"] = json!(report.matches());
            if report.matches() {
                Ok(())
            } else {
                Err(format!(
                    "the stream buffer does not hold the parameter sets ({} bits reported, {} expected)",
                    report.bits,
                    report.expected.len() * 8
                ))
            }
        }
        Err(error) => {
            body["error"] = json!(error.to_string());
            Err(error.to_string())
        }
    };
    let closed = session
        .close()
        .map(|_| ())
        .map_err(|error| error.to_string());
    receipt(output, "bits", body)?;
    println!(
        "bits: {}; receipt {output}",
        if outcome.is_ok() {
            "the parameter sets are in the buffer"
        } else {
            "failed"
        }
    );
    outcome.and(closed)
}

fn encode(arguments: &[String]) -> Result<(), String> {
    let [
        map,
        sequence,
        size,
        frames,
        key_every,
        qp,
        stream_path,
        output,
    ] = arguments
    else {
        return Err(USAGE.into());
    };
    let hypothesis = Hypothesis::named(map).ok_or_else(|| format!("no map is called {map}"))?;
    let sequence: Sequence = sequence.parse()?;
    let (width, height) = parse_size(size)?;
    let frames = count(frames, "FRAMES")?;
    let key_every = count(key_every, "KEY_EVERY")?;
    let qp: i32 = qp
        .parse()
        .ok()
        .filter(|qp| (10..=51).contains(qp))
        .ok_or_else(|| format!("QP is a number from 10 to 51, not {qp}"))?;
    let stream = stream(&hypothesis, width, height, true);
    let room = room_for(&hypothesis, width, height);
    let mut session = Session::open(open_device()?, hypothesis, stream, &room)
        .map_err(|error| error.to_string())?;
    let file = File::create(stream_path).map_err(|error| format!("{stream_path}: {error}"))?;
    let mut out = BufWriter::new(file);
    let mut picture = Picture::new(width, height);
    let mut rows = Vec::new();
    let mut failure = None;
    let mut first_frame_steps = 0;
    for frame in 0..frames {
        render(sequence, frame, &mut picture);
        match session.encode(&picture, frame % key_every == 0, qp) {
            Ok(encoded) => {
                out.write_all(&encoded.bytes)
                    .map_err(|error| format!("{stream_path}: {error}"))?;
                rows.push(json!({
                    "key": encoded.key,
                    "bytes": encoded.bytes.len(),
                    "status": hex(encoded.status),
                    "micros": encoded.time.as_micros() as u64,
                }));
            }
            Err(error) => {
                failure = Some(format!("frame {frame}: {error}"));
            }
        }
        if frame == 0 {
            first_frame_steps = session.trace.len();
        }
        if failure.is_some() {
            break;
        }
    }
    out.flush()
        .map_err(|error| format!("{stream_path}: {error}"))?;
    let input_stage = nonzero(session.engine(), 0xa00, 0xb00);
    let encoder = nonzero(session.engine(), 0xb00, 0xc00);
    let top = nonzero(session.engine(), 0x000, 0x100);
    // The first frame's steps in full; of a failed later frame, its own.
    let shown = if failure.is_some() && session.trace.len() > first_frame_steps {
        let last = regs::select(&hypothesis).len();
        let from = session.trace.len().saturating_sub(4096).max(last);
        steps(&session.trace[from..])
    } else {
        steps(&session.trace[..first_frame_steps.min(session.trace.len())])
    };
    let body = json!({
        "map": hypothesis.name,
        "sequence": sequence.name(),
        "width": width,
        "height": height,
        "qp": qp,
        "keyEvery": key_every,
        "codec": codec_string(&stream),
        "frames": rows,
        "error": failure,
        "steps": shown,
        "inputStageAfter": input_stage,
        "encoderAfter": encoder,
        "topAfter": top,
        "unknownInterrupt": session.engine().unknown_interrupt,
    });
    let closed = session
        .close()
        .map(|_| ())
        .map_err(|error| error.to_string());
    receipt(output, "encode", body)?;
    match failure {
        Some(failure) => {
            println!("encode: {failure}; receipt {output}");
            Err(failure)
        }
        None => {
            println!("encode: {frames} frames in {stream_path}; receipt {output}");
            closed
        }
    }
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let result = match arguments.first().map(String::as_str) {
        Some("generate") => generate(&arguments[1..]),
        Some("standin") => standin(&arguments[1..]),
        Some("psnr") => compare(&arguments[1..]),
        Some("probe") => probe(&arguments[1..]),
        Some("buffers") => buffers(&arguments[1..]),
        Some("masks") => masks(&arguments[1..]),
        Some("bits") => bits(&arguments[1..]),
        Some("encode") => encode(&arguments[1..]),
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
