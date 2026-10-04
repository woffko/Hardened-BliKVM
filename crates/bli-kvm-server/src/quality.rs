//! `GET /api/vm/quality` (`docs/design/h264-engine-choice-and-quality-monitor-v1.md`):
//! what the board knows of the picture's way, for the page's quality
//! monitor: the capture, the encoders' reports with rates made from the
//! last two of each, the encoder units' share of the processors, the SoC's
//! temperature and the slot's version. It is built at most once a second;
//! a request within that second gets the same answer. The slot's version is
//! read apart from it, in the background (`known_version`).

use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::Path,
    sync::Arc,
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
/// A unit's processor time is compared only with a reading no older than
/// this: the monitor reads once a second while it is open, and a reading
/// from before it was closed says nothing of the second since.
const USAGE_FOR: Duration = Duration::from_secs(10);
/// The slot's version is asked for at most this often.
const VERSION_FOR: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(crate) struct QualityCache {
    memory: tokio::sync::Mutex<Memory>,
    version: std::sync::Mutex<Version>,
}

/// The selected slot's version, as the update runtime's status said it.
#[derive(Default)]
struct Version {
    known: Option<String>,
    /// When it was last asked for.
    asked: Option<Instant>,
}

impl Version {
    /// Whether to ask for it at `now`: the first time, and a minute after
    /// it was asked for last, whatever the answer was.
    fn due(&mut self, now: Instant) -> bool {
        if self
            .asked
            .is_some_and(|at| now.saturating_duration_since(at) < VERSION_FOR)
        {
            return false;
        }
        self.asked = Some(now);
        true
    }
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
    let version = known_version(state);
    json!({
        "capture": capture,
        "h264": h264,
        "vp8": vp8,
        "units": units,
        "temperatureMillidegrees": temperature,
        "version": version,
    })
}

/// The slot's version as it was read last, null before the first read has
/// answered. The update runtime's status takes about half a second, and the
/// version changes with an update only: an answer never waits for it, and a
/// read is begun in the background when the one before is a minute old.
fn known_version(state: &AppState) -> Option<String> {
    let client = state.update_status.as_ref()?;
    let mut version = state.quality.version.lock().expect("quality version lock");
    if version.due(Instant::now()) {
        let (client, cache) = (Arc::clone(client), Arc::clone(&state.quality));
        tokio::spawn(async move {
            if let Ok(status) = client.status().await {
                cache.version.lock().expect("quality version lock").known =
                    Some(status.selected.version);
            }
        });
    }
    version.known.clone()
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
    // A report that is absent, refused or stale ends the pair: the next one
    // is paired with nothing, and no rate is made across the pause.
    let (report, modified) = match read_encoder_report(path, codec) {
        Ok(Some(read)) => read,
        Ok(None) => {
            memory.reports.remove(codec);
            return Value::Null;
        }
        Err(refusal) => {
            memory.reports.remove(codec);
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
        memory.reports.remove(codec);
        return Value::Null;
    }
    let pair = pair_reports(memory.reports.remove(codec), Seen { modified, report });
    let value = with_rates(pair.0.as_ref(), &pair.1);
    memory.reports.insert(codec, pair);
    value
}

/// What a codec's stored pair becomes with `seen`, the report read now: the
/// same report read again keeps its pair; a report written later moves the
/// last one back where they were written no more than [`STALE`] apart (a
/// longer pause makes no rate); anything else starts again.
fn pair_reports(stored: Option<(Option<Seen>, Seen)>, seen: Seen) -> (Option<Seen>, Seen) {
    match stored {
        Some((previous, last)) if last.modified == seen.modified => (previous, last),
        Some((_, last))
            if seen
                .modified
                .duration_since(last.modified)
                .is_ok_and(|apart| apart <= STALE) =>
        {
            (Some(last), seen)
        }
        _ => (None, seen),
    }
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
    units_at(root, memory, Instant::now())
}

/// [`units`] at `now`, which a test chooses.
fn units_at(root: &Path, memory: &mut Memory, now: Instant) -> Value {
    let mut shares = Map::new();
    for (codec, unit) in UNITS {
        let usage = usage_micros(&root.join(unit).join("cpu.stat"));
        let share = match (usage, memory.usage.get(codec)) {
            (Some(usage), Some((at, before)))
                if usage >= *before && now.saturating_duration_since(*at) <= USAGE_FOR =>
            {
                let seconds = now.saturating_duration_since(*at).as_secs_f64();
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

    /// The slot's version is asked for at once, and then a minute after it
    /// was asked for last.
    #[test]
    fn the_version_is_asked_for_once_a_minute() {
        let mut version = Version::default();
        let start = Instant::now();
        assert!(version.due(start));
        assert!(!version.due(start));
        assert!(!version.due(start + Duration::from_secs(59)));
        assert!(version.due(start + VERSION_FOR));
        assert!(!version.due(start + VERSION_FOR + Duration::from_secs(1)));
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
        // Every report's time is set, and not taken from the clock: a time
        // ahead of now is a report written just now however slowly the test
        // runs, and two of them are apart by what the test says.
        let ahead = SystemTime::now() + Duration::from_secs(60);
        let write = |report: &serde_json::Value, at: SystemTime| {
            std::fs::write(&path, report.to_string()).unwrap();
            let file = std::fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(at).unwrap();
        };
        let mut report = whole_report("h264");
        write(&report, ahead);
        let first = encoder(Some(&path), "h264", &mut memory);
        assert_eq!(first["encodedFrames"], 1000);
        assert!(first["fps"].is_null());
        // Read again before the next report: the same, still no rates.
        assert_eq!(encoder(Some(&path), "h264", &mut memory), first);
        report["encodedFrames"] = 1030.into();
        write(&report, ahead + Duration::from_secs(1));
        let second = encoder(Some(&path), "h264", &mut memory);
        assert!(
            (second["fps"].as_f64().unwrap() - 30.0).abs() < 0.1,
            "{second}"
        );
        // The pair stays until a report written later comes.
        assert_eq!(encoder(Some(&path), "h264", &mut memory), second);
        // Older than three seconds: not shown.
        write(&report, SystemTime::now() - Duration::from_secs(4));
        assert!(encoder(Some(&path), "h264", &mut memory).is_null());
        // Refused, told once until a report is taken again.
        std::fs::write(&path, "{").unwrap();
        assert!(encoder(Some(&path), "h264", &mut memory).is_null());
        assert!(memory.told.contains("h264"));
        write(&report, ahead + Duration::from_secs(2));
        assert!(!encoder(Some(&path), "h264", &mut memory).is_null());
        assert!(!memory.told.contains("h264"));
    }

    /// A pause makes no rate: after the encoder's report went stale, the next
    /// one is paired with nothing, and not with the last one before the pause.
    #[test]
    fn a_pause_makes_no_rate() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.json");
        let mut memory = Memory::default();
        // Every report's time is set, as in the test above.
        let ahead = SystemTime::now() + Duration::from_secs(60);
        let write = |report: &serde_json::Value, at: SystemTime| {
            std::fs::write(&path, report.to_string()).unwrap();
            let file = std::fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(at).unwrap();
        };
        let mut report = whole_report("h264");
        write(&report, ahead);
        assert!(encoder(Some(&path), "h264", &mut memory)["fps"].is_null());
        report["encodedFrames"] = 1030.into();
        write(&report, ahead + Duration::from_secs(1));
        let rated = encoder(Some(&path), "h264", &mut memory);
        assert!(rated["fps"].as_f64().unwrap() > 0.0, "{rated}");
        // The report goes stale; a new one comes: no rate yet.
        write(&report, SystemTime::now() - Duration::from_secs(4));
        assert!(encoder(Some(&path), "h264", &mut memory).is_null());
        report["encodedFrames"] = 1060.into();
        write(&report, ahead + Duration::from_secs(2));
        let after = encoder(Some(&path), "h264", &mut memory);
        assert_eq!(after["encodedFrames"], 1060);
        assert!(after["fps"].is_null(), "a rate over the pause: {after}");
        assert!(after["gatePerSecond"].is_null(), "{after}");
        assert!(after["hardware"]["passesPerSecond"].is_null(), "{after}");
        // The next report, a second later, has its rates again.
        report["encodedFrames"] = 1090.into();
        write(&report, ahead + Duration::from_secs(3));
        let next = encoder(Some(&path), "h264", &mut memory);
        assert!((next["fps"].as_f64().unwrap() - 30.0).abs() < 0.1, "{next}");
        // An absent report and a refused one end the pair as well.
        std::fs::remove_file(&path).unwrap();
        assert!(encoder(Some(&path), "h264", &mut memory).is_null());
        assert!(!memory.reports.contains_key("h264"));
        write(&report, ahead + Duration::from_secs(4));
        std::fs::write(&path, "{").unwrap();
        assert!(encoder(Some(&path), "h264", &mut memory).is_null());
        assert!(!memory.reports.contains_key("h264"));
    }

    /// Two reports are paired for rates only when they were written no more
    /// than three seconds apart.
    #[test]
    fn two_reports_are_paired_only_within_three_seconds() {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let first = seen(whole_report("h264"), at);
        let alone = pair_reports(None, first.clone());
        assert!(alone.0.is_none());
        // The same report read again keeps its pair.
        let kept = pair_reports(Some(alone), first.clone());
        assert!(kept.0.is_none() && kept.1.modified == at);
        let written =
            |seconds: f64| seen(whole_report("h264"), at + Duration::from_secs_f64(seconds));
        let near = pair_reports(Some((None, first.clone())), written(2.5));
        assert_eq!(near.0.map(|last| last.modified), Some(at));
        let edge = pair_reports(Some((None, first.clone())), written(3.0));
        assert!(edge.0.is_some(), "three seconds apart still pairs");
        let far = pair_reports(Some((None, first.clone())), written(10.0));
        assert!(far.0.is_none(), "a pause of ten seconds pairs");
        assert_eq!(far.1.modified, at + Duration::from_secs(10));
        // A report older than the one before it (the clock went back) starts again.
        let older = pair_reports(Some((None, written(5.0))), first);
        assert!(older.0.is_none());
    }

    /// A unit's processor time is compared only with a reading no older than
    /// ten seconds.
    #[test]
    fn a_unit_is_not_compared_with_a_reading_older_than_ten_seconds() {
        let directory = tempfile::tempdir().unwrap();
        let unit = directory
            .path()
            .join("hardened-blikvm-video-encoder-h264.service");
        std::fs::create_dir(&unit).unwrap();
        let stat = |usage: u64| {
            std::fs::write(unit.join("cpu.stat"), format!("usage_usec {usage}\n")).unwrap()
        };
        let mut memory = Memory::default();
        let start = Instant::now();
        let share = |memory: &mut Memory, seconds: u64| {
            units_at(
                directory.path(),
                memory,
                start + Duration::from_secs(seconds),
            )["h264CpuPercent"]
                .clone()
        };
        stat(1_000_000);
        assert!(share(&mut memory, 0).is_null(), "the first read");
        stat(1_300_000);
        assert_eq!(share(&mut memory, 1).as_f64(), Some(30.0));
        // The monitor was closed meanwhile: the reading is eleven seconds old.
        stat(5_000_000);
        assert!(share(&mut memory, 12).is_null(), "a share over the pause");
        stat(5_300_000);
        assert_eq!(share(&mut memory, 13).as_f64(), Some(30.0));
        // Ten seconds is still a span that is measured.
        stat(6_000_000);
        assert_eq!(share(&mut memory, 23).as_f64(), Some(7.0));
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
