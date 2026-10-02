//! The lab tool's commands. Those that need the board's engine come with
//! the lab module; these work anywhere.

use std::{
    fs::File,
    io::{BufWriter, Read, Write},
    process::ExitCode,
};

use bli_kvm_ve_lab::{
    device::Device,
    engine::{Engine, FakeEngine, Patience, Step},
    headers::{Stream, codec_string},
    lab,
    picture::{Page, Picture, Sequence, parse_size, render, render_moved, ring_index},
    psnr,
    rate::RateControl,
    regs::{self, Adjust, Hypothesis, Op},
    session::{Encoded, Session},
};
use serde_json::{Value, json};

const USAGE: &str = "usage:
  bli-kvm-ve-lab generate SEQUENCE WIDTHxHEIGHT FRAMES OUTPUT
      NV12 frames of the visible size, one after the other
  bli-kvm-ve-lab standin MAP SEQUENCE WIDTHxHEIGHT FRAMES KEY_EVERY OUTPUT
      an Annex B stream from the stand-in through the plan of MAP (older, v3s, h6, h616)
  bli-kvm-ve-lab psnr SEQUENCE WIDTHxHEIGHT FRAMES DECODED [ring=N] [luma] [floor=DB]
      luma PSNR of at most FRAMES decoded frames (planar or NV12, visible size) against
      the sequence; with ring=N against a stream of N pictures played forth and back,
      and without the list of every frame's value; with luma the decoded frames are
      luma planes only; with floor=DB the frames below DB are counted
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
  bli-kvm-ve-lab stream MAP SEQUENCE WIDTHxHEIGHT FRAMES KEY_EVERY RATE FPS STREAM OUTPUT [OPTION...]
      a long run: RATE is qpN (every frame at QP N) or kilobits a second, held by the
      rate control with a second pass for a frame that does not fit. Options: paced
      (a frame begins every 1/FPS s), ring=N (N pictures drawn once and played forth
      and back), guards=N (the buffers' guards read every N passes), timeout=N (the
      Nth frame is not waited for: the engine is reset and the stream goes on with a
      key frame), seconds=N (the run ends after N seconds, before FRAMES frames if
      they take longer), period=N (every frame is compared, byte for byte, with the
      frame N before it: for a fixed QP, a ring and key frames that repeat with N),
      adjust=REG:CLEAR:SET (hex: fields of 0xb08, 0xb10, 0xb48 or 0xb4c that say how
      the engine searches and decides, changed in every frame's plan)
  bli-kvm-ve-lab search MAP WIDTHxHEIGHT QP FRAMES STREAM OUTPUT [grain] [adjust=REG:CLEAR:SET...] RIGHT,DOWN...
      what the engine's search for motion finds: for every vector a key frame and
      FRAMES P frames of a page of text that moves RIGHT,DOWN pixels a frame, at QP;
      with grain a page of which no part looks like another
sequences: scroll, drag, motion, pointer; and PAGE.RIGHT.DOWN, a page (text or grain)
that moves RIGHT,DOWN pixels a frame, as in text.0.-20";

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
    let [sequence, size, frames, decoded, options @ ..] = arguments else {
        return Err(USAGE.into());
    };
    let (mut ring, mut luma_only, mut floor) = (0u32, false, None);
    for option in options {
        match option.split_once('=') {
            Some(("ring", value)) => {
                ring = value
                    .parse::<u32>()
                    .ok()
                    .filter(|ring| (2..=600).contains(ring))
                    .ok_or("ring=N with N from 2 to 600")?;
            }
            Some(("floor", value)) => {
                floor = Some(
                    value
                        .parse::<f64>()
                        .ok()
                        .filter(|floor| (0.0..=100.0).contains(floor))
                        .ok_or("floor=DB with DB from 0 to 100")?,
                );
            }
            None if option == "luma" => luma_only = true,
            _ => return Err(USAGE.into()),
        }
    }
    let sequence: Sequence = sequence.parse()?;
    let (width, height) = parse_size(size)?;
    let frames: u32 = frames
        .parse()
        .ok()
        .filter(|frames| (1..=1_000_000).contains(frames))
        .ok_or("FRAMES is a number from 1 to 1000000")?;
    let mut file = File::open(decoded).map_err(|error| format!("{decoded}: {error}"))?;
    let mut picture = Picture::new(width, height);
    let pictures: Vec<Picture> = (0..ring)
        .map(|frame| {
            render(sequence, frame, &mut picture);
            picture.clone()
        })
        .collect();
    let mut frame_bytes = vec![
        0u8;
        if luma_only {
            width * height
        } else {
            width * height * 3 / 2
        }
    ];
    let mut values = Vec::new();
    for frame in 0..frames {
        // The input may end before FRAMES frames, but not within one.
        let mut filled = 0;
        while filled < frame_bytes.len() {
            match file.read(&mut frame_bytes[filled..]) {
                Ok(0) => break,
                Ok(count) => filled += count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(format!("{decoded}: frame {frame}: {error}")),
            }
        }
        if filled == 0 {
            break;
        }
        if filled < frame_bytes.len() {
            return Err(format!("{decoded}: frame {frame} is cut short"));
        }
        let source = if ring > 0 {
            &pictures[ring_index(frame, ring)]
        } else {
            render(sequence, frame, &mut picture);
            &picture
        };
        values.push(psnr::luma(
            &source.luma,
            source.coded_width,
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
    let below: Vec<usize> = match floor {
        Some(floor) => (0..values.len())
            .filter(|&frame| values[frame] < floor)
            .collect(),
        None => Vec::new(),
    };
    println!(
        "{}",
        json!({
            "sequence": sequence.name(),
            "width": width,
            "height": height,
            "frames": values.len(),
            "floorDb": floor,
            "belowFloor": floor.map(|_| below.len()),
            "firstBelowFloor": floor.map(|_| &below[..below.len().min(10)]),
            "exact": values.len() - finite.len(),
            "meanDb": mean,
            "lowestDb": if finite.is_empty() { None } else { Some(lowest) },
            "lowestFrame": if finite.is_empty() {
                None
            } else {
                values.iter().position(|value| *value == lowest)
            },
            // A long run's list would be longer than its use.
            "db": if ring > 0 {
                Value::Null
            } else {
                json!(values
                    .iter()
                    .map(|value| if value.is_finite() { json!((value * 100.0).round() / 100.0) } else { json!("exact") })
                    .collect::<Vec<_>>())
            },
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

/// `REG:CLEAR:SET` in hex: a change of fields a lab run may change.
fn parse_adjust(value: &str) -> Result<Adjust, String> {
    let number = |text: &str| u32::from_str_radix(text.trim_start_matches("0x"), 16).ok();
    let mut parts = value.split(':').map(number);
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(Some(reg)), Some(Some(clear)), Some(Some(set)), None) => Adjust::new(reg, clear, set)
            .ok_or_else(|| {
                format!(
                    "{value}: of register {reg:#x} a lab run may change the bits {:#010x}",
                    regs::adjustable(reg)
                )
            }),
        _ => Err(format!(
            "an adjustment is REG:CLEAR:SET in hex, not {value}"
        )),
    }
}

fn adjusts_json(adjusts: &[Adjust]) -> Value {
    adjusts
        .iter()
        .map(|adjust| json!({"reg": hex(adjust.reg), "clear": hex(adjust.clear), "set": hex(adjust.set)}))
        .collect()
}

/// `RIGHT,DOWN` in pixels a frame.
fn parse_vector(value: &str) -> Result<(i32, i32), String> {
    let part = |text: &str| text.parse::<i32>().ok().filter(|part| part.abs() <= 256);
    value
        .split_once(',')
        .and_then(|(right, down)| Some((part(right)?, part(down)?)))
        .ok_or_else(|| format!("a vector is RIGHT,DOWN with both up to 256, not {value}"))
}

fn search(arguments: &[String]) -> Result<(), String> {
    let [map, size, qp, frames, stream_path, output, rest @ ..] = arguments else {
        return Err(USAGE.into());
    };
    let hypothesis = Hypothesis::named(map).ok_or_else(|| format!("no map is called {map}"))?;
    let (width, height) = parse_size(size)?;
    let qp: i32 = qp
        .parse()
        .ok()
        .filter(|qp| (0..=51).contains(qp))
        .ok_or_else(|| format!("QP is a number from 0 to 51, not {qp}"))?;
    let frames: u32 = frames
        .parse()
        .ok()
        .filter(|frames| (1..=30).contains(frames))
        .ok_or("FRAMES is a number from 1 to 30")?;
    let (mut adjusts, mut vectors, mut page) = (Vec::new(), Vec::new(), Page::Text);
    for word in rest {
        match word.strip_prefix("adjust=") {
            Some(value) => adjusts.push(parse_adjust(value)?),
            None if word == "grain" => page = Page::Grain,
            None => vectors.push(parse_vector(word)?),
        }
    }
    if vectors.is_empty() {
        return Err(USAGE.into());
    }

    let stream = stream(&hypothesis, width, height, true);
    let room = room_for(&hypothesis, width, height);
    let mut session = Session::open(open_device()?, hypothesis, stream, &room)
        .map_err(|error| error.to_string())?;
    session.set_adjusts(adjusts.clone());
    let file = File::create(stream_path).map_err(|error| format!("{stream_path}: {error}"))?;
    let mut out = BufWriter::new(file);
    let mut picture = Picture::new(width, height);
    let mut rows = Vec::new();
    let mut failure = None;
    'vectors: for &(right, down) in &vectors {
        let (mut key_bytes, mut sizes, mut micros, mut statistics) =
            (0, Vec::new(), Vec::new(), Vec::new());
        for frame in 0..=frames as i32 {
            render_moved(page, right * frame, down * frame, &mut picture);
            let encoded = match session.encode(&picture, frame == 0, qp) {
                Ok(encoded) => encoded,
                Err(error) => {
                    failure = Some(format!("vector {right},{down} frame {frame}: {error}"));
                    break 'vectors;
                }
            };
            if let Err(error) = out.write_all(&encoded.bytes) {
                failure = Some(format!("{stream_path}: {error}"));
                break 'vectors;
            }
            if frame == 0 {
                key_bytes = encoded.bytes.len();
            } else {
                sizes.push(encoded.bytes.len());
                micros.push(encoded.time.as_micros() as u64);
                // What the engine counted in the frame.
                let engine = session.engine();
                statistics.push(
                    [
                        regs::AVC_MAD,
                        regs::AVC_RESIDUAL_BITS,
                        regs::AVC_HEADER_BITS,
                        regs::AVC_ME_INFO,
                    ]
                    .map(|reg| hex(engine.read(reg))),
                );
            }
            session.trace.clear();
        }
        let mean = sizes.iter().sum::<usize>() / sizes.len();
        println!("search: {right:5} {down:5}  key {key_bytes:7}  P {mean:7}");
        rows.push(json!({
            "right": right,
            "down": down,
            "keyBytes": key_bytes,
            "pBytes": sizes,
            "pBytesMean": mean,
            "encodeMicrosMean": micros.iter().sum::<u64>() / micros.len() as u64,
            "statistics": statistics,
        }));
    }
    let flushed = out
        .flush()
        .map_err(|error| format!("{stream_path}: {error}"));
    if let Some(buffer) = session.broken_guard() {
        failure.get_or_insert(format!("the engine wrote outside the {buffer} buffer"));
    }
    let body = json!({
        "map": hypothesis.name,
        "width": width,
        "height": height,
        "qp": qp,
        "pFrames": frames,
        "page": page.name(),
        "adjusts": adjusts_json(&adjusts),
        "codec": codec_string(&stream),
        "vectors": rows,
        "error": failure,
        "encoderAfter": nonzero(session.engine(), 0xb00, 0xc00),
        "topAfter": nonzero(session.engine(), 0x000, 0x100),
        "unknownInterrupt": session.engine().unknown_interrupt,
    });
    let closed = session
        .close()
        .map(|_| ())
        .map_err(|error| error.to_string());
    receipt(output, "search", body)?;
    flushed?;
    match failure {
        Some(failure) => {
            println!("search: {failure}; receipt {output}");
            Err(failure)
        }
        None => {
            println!("search: {} vectors; receipt {output}", vectors.len());
            closed
        }
    }
}

/// FNV-1a: a frame's fingerprint.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// What a long run is asked to hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rate {
    Qp(i32),
    Kbps(u32),
}

fn parse_rate(value: &str) -> Result<Rate, String> {
    let wrong = || format!("RATE is qp0 to qp51 or 64 to 20000 kbit/s, not {value}");
    match value.strip_prefix("qp") {
        Some(qp) => qp
            .parse()
            .ok()
            .filter(|qp| (0..=51).contains(qp))
            .map(Rate::Qp)
            .ok_or_else(wrong),
        None => value
            .parse()
            .ok()
            .filter(|kbps| (64..=20000).contains(kbps))
            .map(Rate::Kbps)
            .ok_or_else(wrong),
    }
}

/// Smallest, mean, 99th percentile and largest of times, in microseconds.
fn spread(mut micros: Vec<u64>) -> Value {
    if micros.is_empty() {
        return Value::Null;
    }
    micros.sort_unstable();
    let mean = micros.iter().sum::<u64>() / micros.len() as u64;
    json!({
        "min": micros[0],
        "mean": mean,
        "p99": micros[(micros.len() * 99 / 100).min(micros.len() - 1)],
        "max": micros[micros.len() - 1],
    })
}

fn stream_run(arguments: &[String]) -> Result<(), String> {
    let [
        map,
        sequence,
        size,
        frames,
        key_every,
        rate,
        fps,
        stream_path,
        output,
        options @ ..,
    ] = arguments
    else {
        return Err(USAGE.into());
    };
    let hypothesis = Hypothesis::named(map).ok_or_else(|| format!("no map is called {map}"))?;
    let sequence: Sequence = sequence.parse()?;
    let (width, height) = parse_size(size)?;
    let frames: u32 = frames
        .parse()
        .ok()
        .filter(|frames| (1..=1_000_000).contains(frames))
        .ok_or("FRAMES is a number from 1 to 1000000")?;
    let key_every: u32 = key_every
        .parse()
        .ok()
        .filter(|every| *every >= 1)
        .ok_or("KEY_EVERY is a number from 1")?;
    let rate = parse_rate(rate)?;
    let fps: u32 = fps
        .parse()
        .ok()
        .filter(|fps| (1..=120).contains(fps))
        .ok_or("FPS is a number from 1 to 120")?;
    let (mut paced, mut ring, mut guards, mut timeout_at) = (false, 0u32, 1u32, None);
    let (mut adjusts, mut seconds, mut period) = (Vec::new(), None, None);
    for option in options {
        let number = |text: &str| {
            text.parse::<u32>()
                .map_err(|_| format!("not an option: {option}"))
        };
        match option.split_once('=') {
            None if option == "paced" => paced = true,
            Some(("ring", value)) => ring = number(value)?.clamp(2, 600),
            Some(("guards", value)) => guards = number(value)?.max(1),
            Some(("timeout", value)) => timeout_at = Some(number(value)?),
            Some(("adjust", value)) => adjusts.push(parse_adjust(value)?),
            Some(("seconds", value)) => seconds = Some(number(value)?.max(1)),
            Some(("period", value)) => period = Some(number(value)?.clamp(1, 100_000) as usize),
            _ => return Err(format!("not an option: {option}")),
        }
    }

    let stream = Stream {
        fps,
        ..stream(&hypothesis, width, height, true)
    };
    let room = room_for(&hypothesis, width, height);
    let mut session = Session::open(open_device()?, hypothesis, stream, &room)
        .map_err(|error| error.to_string())?;
    session.set_guard_checks(guards);
    session.set_adjusts(adjusts.clone());
    let patience = Patience::default();
    let file = File::create(stream_path).map_err(|error| format!("{stream_path}: {error}"))?;
    let mut out = BufWriter::new(file);

    let mut picture = Picture::new(width, height);
    let pictures: Vec<Picture> = (0..ring)
        .map(|frame| {
            render(sequence, frame, &mut picture);
            picture.clone()
        })
        .collect();
    let mut control = match rate {
        Rate::Kbps(kbps) => Some(RateControl::new(kbps, fps)),
        Rate::Qp(_) => None,
    };
    let interval = std::time::Duration::from_secs_f64(1.0 / fps as f64);

    let (mut sizes, mut qps, mut encode_micros, mut frame_micros) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut keys, mut second_passes, mut late, mut resets) = (Vec::new(), Vec::new(), 0u32, 0u32);
    let mut left_out = Vec::new();
    // What every frame of the last period was, and the frames that were
    // not what the frame a period before them was.
    let (mut fingerprints, mut differing) = (Vec::new(), Vec::new());
    let mut failure = None;
    let mut first_frame = Vec::new();
    let mut force_key = false;
    let began = std::time::Instant::now();
    for frame in 0..frames {
        if seconds.is_some_and(|seconds| began.elapsed().as_secs() >= seconds as u64) {
            break;
        }
        if paced {
            let due = began + interval * frame;
            match due.checked_duration_since(std::time::Instant::now()) {
                Some(wait) => std::thread::sleep(wait),
                None => late += 1,
            }
        }
        let frame_began = std::time::Instant::now();
        let source = if ring > 0 {
            &pictures[ring_index(frame, ring)]
        } else {
            render(sequence, frame, &mut picture);
            &picture
        };
        let key = frame % key_every == 0 || std::mem::take(&mut force_key);
        if timeout_at == Some(frame) {
            // A wait that cannot be met: the frame is given up, the engine
            // reset, and the stream goes on with a key frame.
            session.set_patience(Patience {
                encode: std::time::Duration::ZERO,
                ..patience
            });
        }
        let attempt = (|| -> Result<(Encoded, u32), String> {
            let Some(control) = control.as_mut() else {
                let Rate::Qp(qp) = rate else { unreachable!() };
                let encoded = session
                    .pass(source, key, qp, true)
                    .map_err(|error| error.to_string())?;
                return Ok((encoded, 1));
            };
            control.tick();
            let qp = control.next_qp(key || frame == 0);
            let first = session
                .pass(source, key, qp, true)
                .map_err(|error| error.to_string())?;
            let (last, passes) = match control.again(first.bytes.len() as u64 * 8, qp) {
                None => (first, 1),
                Some(higher) => (
                    session
                        .pass(source, key, higher, false)
                        .map_err(|error| error.to_string())?,
                    2,
                ),
            };
            // A frame that fits at no QP is left out, unless the stream
            // needs it to begin with.
            if last.key || control.fits(last.bytes.len() as u64 * 8) {
                Ok((last, passes))
            } else {
                let unchanged = session.unchanged();
                control.left_out(unchanged.bytes.len() as u64 * 8, last.qp);
                Ok((unchanged, 0))
            }
        })();
        if timeout_at == Some(frame) {
            session.set_patience(patience);
            match attempt {
                Err(error) if error.contains("did not end") => {
                    session
                        .engine()
                        .reset()
                        .map_err(|error| format!("reset after the timeout: {error}"))?;
                    resets += 1;
                    force_key = true;
                    session.trace.clear();
                    continue;
                }
                other => {
                    failure = Some(format!(
                        "frame {frame}: the timeout did not come: {}",
                        other
                            .err()
                            .unwrap_or_else(|| "the frame was encoded".into())
                    ));
                    break;
                }
            }
        }
        let (encoded, passes) = match attempt {
            Ok(done) => done,
            Err(error) => {
                failure = Some(format!("frame {frame}: {error}"));
                break;
            }
        };
        // No pass: the frame was left out, and the control knows.
        if passes > 0 {
            session.accept(&encoded);
            if let Some(control) = control.as_mut() {
                control.sent(encoded.bytes.len() as u64 * 8, encoded.qp, encoded.key);
            }
        } else {
            left_out.push(frame);
        }
        if let Err(error) = out.write_all(&encoded.bytes) {
            failure = Some(format!("{stream_path}: {error}"));
            break;
        }
        if let Some(period) = period {
            let fingerprint = fnv1a(&encoded.bytes);
            let place = sizes.len() % period;
            if sizes.len() < period {
                fingerprints.push(fingerprint);
            } else if std::mem::replace(&mut fingerprints[place], fingerprint) != fingerprint {
                differing.push(frame);
            }
        }
        if encoded.key {
            keys.push(frame);
        }
        if passes == 2 {
            second_passes.push(frame);
        }
        sizes.push(encoded.bytes.len() as u64);
        qps.push(encoded.qp);
        encode_micros.push(encoded.time.as_micros() as u64);
        frame_micros.push(frame_began.elapsed().as_micros() as u64);
        if frame == 0 {
            first_frame = steps(&session.trace);
        }
        // A long run does not keep every step.
        session.trace.clear();
    }
    let elapsed = began.elapsed();
    let flushed = out
        .flush()
        .map_err(|error| format!("{stream_path}: {error}"));
    if let Some(buffer) = session.broken_guard() {
        failure.get_or_insert(format!("the engine wrote outside the {buffer} buffer"));
    }

    // The rate in every window of ten seconds.
    let window = (fps * 10) as usize;
    let windows: Vec<u64> = sizes
        .windows(window)
        .map(|frames| frames.iter().sum::<u64>() * 8 / 10_000)
        .collect();
    let done = sizes.len();
    let total_bytes: u64 = sizes.iter().sum();
    let body = json!({
        "map": hypothesis.name,
        "sequence": sequence.name(),
        "width": width,
        "height": height,
        "fps": fps,
        "rate": match rate {
            Rate::Qp(qp) => json!({"qp": qp}),
            Rate::Kbps(kbps) => json!({"kbps": kbps}),
        },
        "keyEvery": key_every,
        "paced": paced,
        "secondsAsked": seconds,
        "ring": ring,
        "guardsEvery": guards,
        "adjusts": adjusts_json(&adjusts),
        "codec": codec_string(&stream),
        "framesAsked": frames,
        "framesDone": done,
        "bytes": total_bytes,
        "kbpsMean": if done == 0 { 0 } else { total_bytes * 8 * fps as u64 / done as u64 / 1000 },
        "kbpsTenSecondWindows": if windows.is_empty() {
            Value::Null
        } else {
            json!({"lowest": windows.iter().min(), "highest": windows.iter().max(), "count": windows.len()})
        },
        "encodeMicros": spread(encode_micros.clone()),
        "frameMicros": spread(frame_micros.clone()),
        "seconds": elapsed.as_secs_f64(),
        "framesPerSecond": done as f64 / elapsed.as_secs_f64().max(1e-9),
        "lateFrames": late,
        "resetsAfterTimeout": resets,
        "keyFrames": keys,
        "secondPasses": second_passes,
        "framesLeftOut": left_out.len(),
        "period": period,
        "framesUnlikeAPeriodBefore": period.map(|_| differing.len()),
        "firstFramesUnlikeAPeriodBefore": period.map(|_| &differing[..differing.len().min(10)]),
        "overflows": control.map(|control| control.overflows),
        "frameBytes": sizes,
        "frameQp": qps,
        "frameEncodeMicros": encode_micros,
        "error": failure,
        "firstFrameSteps": first_frame,
        "topAfter": nonzero(session.engine(), 0x000, 0x100),
        "unknownInterrupt": session.engine().unknown_interrupt,
    });
    let closed = session
        .close()
        .map(|_| ())
        .map_err(|error| error.to_string());
    receipt(output, "stream", body)?;
    flushed?;
    match failure {
        Some(failure) => {
            println!("stream: {failure}; receipt {output}");
            Err(failure)
        }
        None => {
            println!(
                "stream: {done} frames, {total_bytes} bytes, {:.1} frames a second; receipt {output}",
                done as f64 / elapsed.as_secs_f64().max(1e-9)
            );
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
        Some("stream") => stream_run(&arguments[1..]),
        Some("search") => search(&arguments[1..]),
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
