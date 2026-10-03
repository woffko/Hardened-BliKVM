//! `GET /api/vm/quality` (`docs/design/h264-engine-choice-and-quality-monitor-v1.md`):
//! what the board knows of the picture's way, for the page's quality
//! monitor: the capture, the encoders' reports with rates made from the
//! last two of each, the encoder units' share of the processors, the SoC's
//! temperature and the slot's version. It is built at most once a second;
//! a request within that second gets the same answer.

use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::Path,
    time::{Duration, Instant, SystemTime},
};

use axum::{Extension, Json, extract::State};
use serde_json::{Map, Value, json};

use crate::{
    AppState,
    auth::middleware::CurrentSession,
    encoder_report::{EncoderReport, read_encoder_report},
    error::{ApiResponse, AppResult},
};

/// An answer is built at most this often.
const CACHE: Duration = Duration::from_secs(1);
/// A report older than this no longer says how the encoder goes.
const STALE: Duration = Duration::from_secs(3);
/// The encoder units whose processor time is read, by codec.
const UNITS: [(&str, &str); 2] = [
    ("h264", "hardened-blikvm-video-encoder-h264.service"),
    ("vp8", "hardened-blikvm-video-encoder.service"),
];
/// A control group's `cpu.stat` is read no further.
const MAX_CPU_STAT_BYTES: u64 = 4096;

#[derive(Default)]
pub(crate) struct QualityCache {
    memory: tokio::sync::Mutex<Memory>,
}

#[derive(Default)]
struct Memory {
    answer: Option<(Instant, Value)>,
    /// The last two reports of each codec that were written at different
    /// times, for the rates.
    reports: HashMap<&'static str, (Option<Seen>, Seen)>,
    /// Each unit's processor time when it was last read.
    usage: HashMap<&'static str, (Instant, u64)>,
    /// Codecs whose refused report the log has already been told of.
    told: HashSet<&'static str>,
    /// How often an answer was built, for the tests of its cost.
    #[cfg(test)]
    built: u64,
}

#[derive(Clone)]
struct Seen {
    modified: SystemTime,
    report: EncoderReport,
}

pub(crate) async fn quality(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let mut memory = state.quality.memory.lock().await;
    if let Some((at, answer)) = &memory.answer
        && at.elapsed() < CACHE
    {
        return Ok(Json(ApiResponse::ok(answer.clone())));
    }
    let answer = build(&state, &session.token, &mut memory).await;
    memory.answer = Some((Instant::now(), answer.clone()));
    Ok(Json(ApiResponse::ok(answer)))
}

async fn build(state: &AppState, session_token: &str, memory: &mut Memory) -> Value {
    #[cfg(test)]
    {
        memory.built += 1;
    }
    let capture = capture(state, session_token).await;
    let h264 = encoder(state.options.h264_encoder_report.as_deref(), "h264", memory);
    let vp8 = encoder(state.options.vp8_encoder_report.as_deref(), "vp8", memory);
    let units = units(&state.options.unit_cgroups, memory);
    let temperature = bli_kvm_platform_bli::read_h616_temperature_millidegrees().ok();
    let version = match state.update_status.as_ref() {
        Some(client) => client
            .status()
            .await
            .ok()
            .map(|status| status.selected.version),
        None => None,
    };
    json!({
        "capture": capture,
        "h264": h264,
        "vp8": vp8,
        "units": units,
        "temperatureMillidegrees": temperature,
        "version": version,
    })
}

fn round(value: f64, places: i32) -> f64 {
    let scale = 10_f64.powi(places);
    (value * scale).round() / scale
}

/// What `GET /api/vm/screen` reads of the capture: the engine's state, and
/// the JPEG quality the owner runs it with.
#[cfg(feature = "restricted-control-surface")]
async fn capture(state: &AppState, session_token: &str) -> Value {
    let engine = match state.options.engine_socket.clone() {
        Some(socket) => {
            tokio::task::spawn_blocking(move || crate::video_engine::read_state(&socket))
                .await
                .ok()
                .flatten()
        }
        None => None,
    };
    let quality = match state.control.as_ref() {
        Some(control) => match control
            .execute_v2(
                crate::video_settings::operation_id(session_token),
                bli_kvm_core::PlatformControlRequestV2::VideoSettingsState {},
            )
            .await
        {
            Ok(bli_kvm_control_protocol::ControlResponse::VideoState { quality, .. }) => {
                Some(quality)
            }
            _ => None,
        },
        None => None,
    };
    json!({
        "width": engine.as_ref().and_then(|engine| engine.width),
        "height": engine.as_ref().and_then(|engine| engine.height),
        "fps": engine.as_ref().and_then(|engine| engine.captured_fps).map(|fps| round(fps, 1)),
        "queuedFps": engine.as_ref().and_then(|engine| engine.queued_fps).map(|fps| round(fps, 1)),
        "quality": quality,
    })
}

/// Without the owner's surface the server reads nothing of the capture.
#[cfg(not(feature = "restricted-control-surface"))]
async fn capture(_state: &AppState, _session_token: &str) -> Value {
    json!({ "width": null, "height": null, "fps": null, "queuedFps": null, "quality": null })
}

/// The report of `codec`'s encoder with its rates, or null where there is
/// none, it is older than three seconds, or it was refused.
fn encoder(path: Option<&Path>, codec: &'static str, memory: &mut Memory) -> Value {
    let Some(path) = path else {
        return Value::Null;
    };
    let (report, modified) = match read_encoder_report(path, codec) {
        Ok(Some(read)) => read,
        Ok(None) => return Value::Null,
        Err(refusal) => {
            if memory.told.insert(codec) {
                eprintln!(
                    "quality: the {codec} encoder's report is {}; it is not shown",
                    refusal.says()
                );
            }
            return Value::Null;
        }
    };
    memory.told.remove(codec);
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default();
    if age > STALE {
        return Value::Null;
    }
    let seen = Seen { modified, report };
    // A report written later moves the last one back.
    let pair = match memory.reports.remove(codec) {
        Some((previous, last)) if last.modified == seen.modified => (previous, last),
        Some((_, last)) if last.modified < seen.modified => (Some(last), seen),
        _ => (None, seen),
    };
    let value = with_rates(pair.0.as_ref(), &pair.1);
    memory.reports.insert(codec, pair);
    value
}

/// The report with its rates: frames encoded, and the gate's counts, a
/// second; for the engine its passes a second and its time a pass. Null
/// until there are two reports, and where a count went back (the encoder
/// started again).
fn with_rates(previous: Option<&Seen>, last: &Seen) -> Value {
    let mut value = serde_json::to_value(&last.report).unwrap_or(Value::Null);
    let Value::Object(fields) = &mut value else {
        return Value::Null;
    };
    let seconds = previous
        .and_then(|previous| last.modified.duration_since(previous.modified).ok())
        .map(|span| span.as_secs_f64())
        .filter(|seconds| *seconds > 0.0);
    let rate = |now: u64, before: u64| -> Option<f64> {
        let seconds = seconds?;
        Some(round(now.checked_sub(before)? as f64 / seconds, 2))
    };
    let before = previous.map(|previous| &previous.report);
    fields.insert(
        "fps".into(),
        json!(before.and_then(|before| rate(last.report.encoded_frames, before.encoded_frames))),
    );
    let gate = before.and_then(|before| {
        let (now, then) = (last.report.gate, before.gate);
        Some(json!({
            "admitted": rate(now.admitted, then.admitted)?,
            "tooSoon": rate(now.too_soon, then.too_soon)?,
            "unchanged": rate(now.unchanged, then.unchanged)?,
        }))
    });
    fields.insert("gatePerSecond".into(), gate.unwrap_or(Value::Null));
    if let (Some(Value::Object(hardware)), Some(now)) =
        (fields.get_mut("hardware"), last.report.hardware)
    {
        let then = before.and_then(|before| before.hardware);
        let passes = then.and_then(|then| now.passes.checked_sub(then.passes));
        let micros = then.and_then(|then| now.engine_micros.checked_sub(then.engine_micros));
        hardware.insert(
            "passesPerSecond".into(),
            json!(then.and_then(|then| rate(now.passes, then.passes))),
        );
        hardware.insert(
            "engineMicrosPerPass".into(),
            json!(match (passes, micros) {
                (Some(passes), Some(micros)) if passes > 0 => Some(micros / passes),
                _ => None,
            }),
        );
    }
    value
}

/// A control group's processor time, in microseconds.
fn usage_micros(path: &Path) -> Option<u64> {
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_CPU_STAT_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("usage_usec "))?
        .trim()
        .parse()
        .ok()
}

/// Each encoder unit's share of the processors since the last read, in
/// percent of one core; null at the first read and where it cannot be read.
fn units(root: &Path, memory: &mut Memory) -> Value {
    let mut shares = Map::new();
    for (codec, unit) in UNITS {
        let now = Instant::now();
        let usage = usage_micros(&root.join(unit).join("cpu.stat"));
        let share = match (usage, memory.usage.get(codec)) {
            (Some(usage), Some((at, before))) if usage >= *before => {
                let seconds = now.duration_since(*at).as_secs_f64();
                (seconds > 0.0).then(|| round((usage - before) as f64 / 1e6 / seconds * 100.0, 1))
            }
            _ => None,
        };
        match usage {
            Some(usage) => {
                memory.usage.insert(codec, (now, usage));
            }
            None => {
                memory.usage.remove(codec);
            }
        }
        shares.insert(format!("{codec}CpuPercent"), json!(share));
    }
    Value::Object(shares)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder_report::tests::whole_report;

    fn seen(report: serde_json::Value, modified: SystemTime) -> Seen {
        Seen {
            modified,
            report: serde_json::from_value(report).unwrap(),
        }
    }

    /// Rates come from the last two reports by the times they were written,
    /// and are null where there is one, or a count went back.
    #[test]
    fn rates_come_from_two_reports() {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let before = seen(whole_report("h264"), at);
        let mut later = whole_report("h264");
        later["encodedFrames"] = 1060.into();
        later["gate"] = serde_json::json!({ "admitted": 1060, "tooSoon": 620, "unchanged": 20 });
        later["hardware"]["passes"] = 1376.into();
        later["hardware"]["engineMicros"] = (9_490_000 + 76 * 7300).into();
        let later = seen(later, at + Duration::from_secs(2));
        let value = with_rates(Some(&before), &later);
        assert_eq!(value["fps"], 30.0);
        assert_eq!(
            value["gatePerSecond"],
            serde_json::json!({ "admitted": 30.0, "tooSoon": 60.0, "unchanged": 0.0 })
        );
        assert_eq!(value["hardware"]["passesPerSecond"], 38.0);
        assert_eq!(value["hardware"]["engineMicrosPerPass"], 7300);
        assert_eq!(value["encodedFrames"], 1060);
        // One report: no rates yet.
        let alone = with_rates(None, &later);
        assert!(alone["fps"].is_null() && alone["gatePerSecond"].is_null());
        assert!(alone["hardware"]["passesPerSecond"].is_null());
        // The encoder started again: its counts went back.
        let again = with_rates(
            Some(&later),
            &seen(whole_report("h264"), at + Duration::from_secs(3)),
        );
        assert!(again["fps"].is_null() && again["gatePerSecond"].is_null());
        assert!(again["hardware"]["engineMicrosPerPass"].is_null());
        // VP8 has no engine's counts.
        let vp8 = with_rates(None, &seen(whole_report("vp8"), at));
        assert!(vp8["hardware"].is_null());
    }

    /// The reader keeps the last two reports written at different times,
    /// shows none that is older than three seconds, and tells the log of a
    /// refused report once.
    #[test]
    fn reports_are_paired_by_their_times_and_stale_ones_are_not_shown() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.json");
        let mut memory = Memory::default();
        assert!(encoder(None, "h264", &mut memory).is_null());
        assert!(encoder(Some(&path), "h264", &mut memory).is_null());
        // Every report's time is set from one moment, so that two of them
        // are apart by what the test says however slowly it runs.
        let base = SystemTime::now();
        let write = |report: &serde_json::Value, age: Duration| {
            std::fs::write(&path, report.to_string()).unwrap();
            let file = std::fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(base - age).unwrap();
        };
        let mut report = whole_report("h264");
        write(&report, Duration::from_millis(1500));
        let first = encoder(Some(&path), "h264", &mut memory);
        assert_eq!(first["encodedFrames"], 1000);
        assert!(first["fps"].is_null());
        // Read again before the next report: the same, still no rates.
        assert_eq!(encoder(Some(&path), "h264", &mut memory), first);
        report["encodedFrames"] = 1030.into();
        write(&report, Duration::from_millis(500));
        let second = encoder(Some(&path), "h264", &mut memory);
        assert!(
            (second["fps"].as_f64().unwrap() - 30.0).abs() < 0.1,
            "{second}"
        );
        // The pair stays until a report written later comes.
        assert_eq!(encoder(Some(&path), "h264", &mut memory), second);
        // Older than three seconds: not shown.
        write(&report, Duration::from_secs(4));
        assert!(encoder(Some(&path), "h264", &mut memory).is_null());
        // Refused, told once until a report is taken again.
        std::fs::write(&path, "{").unwrap();
        assert!(encoder(Some(&path), "h264", &mut memory).is_null());
        assert!(memory.told.contains("h264"));
        write(&report, Duration::ZERO);
        assert!(!encoder(Some(&path), "h264", &mut memory).is_null());
        assert!(!memory.told.contains("h264"));
    }

    /// A unit's share of the processors over the span between two reads.
    #[test]
    fn a_units_share_comes_from_its_control_group() {
        let directory = tempfile::tempdir().unwrap();
        let unit = directory
            .path()
            .join("hardened-blikvm-video-encoder-h264.service");
        std::fs::create_dir(&unit).unwrap();
        let stat = |usage: u64| {
            std::fs::write(
                unit.join("cpu.stat"),
                format!("usage_usec {usage}\nuser_usec 1\nsystem_usec 2\n"),
            )
            .unwrap();
        };
        stat(1_000_000);
        let mut memory = Memory::default();
        let began = Instant::now();
        let first = units(directory.path(), &mut memory);
        let first_done = Instant::now();
        assert_eq!(
            first,
            serde_json::json!({ "h264CpuPercent": null, "vp8CpuPercent": null })
        );
        std::thread::sleep(Duration::from_millis(200));
        stat(1_300_000);
        let second_began = Instant::now();
        let second = units(directory.path(), &mut memory);
        let ended = Instant::now();
        let share = second["h264CpuPercent"].as_f64().unwrap();
        // 0.3 s of a core over the span between the two reads, which lies
        // between the sleep and the whole of both reads.
        let most = 30.0 / second_began.duration_since(first_done).as_secs_f64();
        let least = 30.0 / ended.duration_since(began).as_secs_f64();
        assert!(
            share >= least - 0.1 && share <= most + 0.1,
            "{share} not within {least}..{most}"
        );
        assert!(second["vp8CpuPercent"].is_null());
        // A count that went back (the unit started again) gives no share.
        stat(10);
        assert!(units(directory.path(), &mut memory)["h264CpuPercent"].is_null());
        assert_eq!(usage_micros(&unit.join("missing")), None);
    }
}
