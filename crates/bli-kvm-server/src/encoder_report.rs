//! The encoders' reports (`docs/design/h264-engine-choice-and-quality-monitor-v1.md`):
//! files that another process writes into its runtime directory, read
//! bounded and checked like the status word.

use std::{io::Read, path::Path, time::SystemTime};

use serde::{Deserialize, Serialize};

/// A report is never larger; a larger file is no report.
pub(crate) const MAX_REPORT_BYTES: usize = 4096;
/// The report's form that this server reads.
const SCHEMA: u32 = 1;
/// A reason is at most this many characters.
const MAX_REASON_CHARS: usize = 120;
/// A stream's side is at most this many pixels.
const MAX_SIDE: u32 = 8192;
/// A stage's mean time a frame is at most this, in microseconds.
const MAX_STAGE_MICROS: u64 = 10_000_000;

/// The stages' mean times a frame over the encoder's last second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StageMicros {
    pub decode: u64,
    pub convert: u64,
    pub encode: u64,
}

/// The gate's counts since the encoder's start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GateCounts {
    pub admitted: u64,
    pub too_soon: u64,
    pub unchanged: u64,
}

/// The video engine's counts since the encoder's start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct HardwareCounts {
    pub passes: u64,
    pub both_searches: u64,
    pub wide: u64,
    pub higher_qp: u64,
    pub left_out: u64,
    pub key_frames: u64,
    pub failures: u64,
    pub engine_micros: u64,
}

/// An encoder's whole report, as its schema has it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EncoderReport {
    pub schema: u32,
    pub codec: String,
    pub streaming: bool,
    pub engine: Option<String>,
    pub engine_wanted: Option<String>,
    pub hardware_available: Option<bool>,
    pub hardware_reason: Option<String>,
    pub width: u32,
    pub height: u32,
    pub kbps: u32,
    pub max_fps: u32,
    pub converters: u32,
    pub encoded_frames: u64,
    pub stage_micros: StageMicros,
    pub gate: GateCounts,
    pub hardware: Option<HardwareCounts>,
}

/// Why a report file was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReportRefusal {
    TooLarge,
    NotJson,
    OutOfBounds,
}

impl ReportRefusal {
    pub(crate) fn says(self) -> &'static str {
        match self {
            Self::TooLarge => "larger than a report",
            Self::NotJson => "not a report's JSON",
            Self::OutOfBounds => "out of its bounds",
        }
    }
}

impl EncoderReport {
    /// Every field within the design note's bounds, for `codec`.
    fn within_bounds(&self, codec: &str) -> bool {
        let engines: &[&str] = match codec {
            "h264" => &["hardware", "x264"],
            "vp8" => &["libvpx"],
            _ => return false,
        };
        let wanted_ok = match codec {
            "h264" => matches!(self.engine_wanted.as_deref(), Some("hardware" | "software")),
            _ => self.engine_wanted.is_none(),
        };
        let stages = [
            self.stage_micros.decode,
            self.stage_micros.convert,
            self.stage_micros.encode,
        ];
        self.schema == SCHEMA
            && self.codec == codec
            && self
                .engine
                .as_deref()
                .is_none_or(|engine| engines.contains(&engine))
            && wanted_ok
            && (codec == "h264" || self.hardware_available.is_none() && self.hardware.is_none())
            && self
                .hardware_reason
                .as_deref()
                .is_none_or(|reason| reason.chars().count() <= MAX_REASON_CHARS)
            && self.width <= MAX_SIDE
            && self.height <= MAX_SIDE
            && (64..=20_000).contains(&self.kbps)
            && (1..=60).contains(&self.max_fps)
            && (1..=4).contains(&self.converters)
            && stages.iter().all(|micros| *micros <= MAX_STAGE_MICROS)
    }
}

/// The report of `codec`'s encoder at `path` with the time the file was
/// last written: `Ok(None)` while there is none, a refusal for a file that
/// is too large, not a report's JSON, or out of its bounds.
pub(crate) fn read_encoder_report(
    path: &Path,
    codec: &str,
) -> Result<Option<(EncoderReport, SystemTime)>, ReportRefusal> {
    let Ok(modified) = std::fs::metadata(path).and_then(|metadata| metadata.modified()) else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    let Ok(file) = std::fs::File::open(path) else {
        return Ok(None);
    };
    file.take(MAX_REPORT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ReportRefusal::NotJson)?;
    if bytes.len() > MAX_REPORT_BYTES {
        return Err(ReportRefusal::TooLarge);
    }
    let report: EncoderReport =
        serde_json::from_slice(&bytes).map_err(|_| ReportRefusal::NotJson)?;
    if !report.within_bounds(codec) {
        return Err(ReportRefusal::OutOfBounds);
    }
    Ok(Some((report, modified)))
}

/// What the H.264 encoder says of the board's video engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EngineAvailability {
    /// The engine can be had: the device answered, or encodes.
    pub available: bool,
    /// Why it cannot, in the encoder's words.
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EngineFields {
    schema: u32,
    codec: String,
    hardware_available: Option<bool>,
    hardware_reason: Option<String>,
}

/// At most [`MAX_REPORT_BYTES`] of the file at `path`; `None` for a file
/// that is missing or larger.
fn read_bounded(path: &Path) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_REPORT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= MAX_REPORT_BYTES).then_some(bytes)
}

/// Whether the board's video engine can encode H.264, from the H.264
/// encoder's report at `path`. A unit that names no device has no engine to
/// have. `None` while there is no report, or none this server understands.
#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
pub(crate) fn read_engine_availability(path: &Path) -> Option<EngineAvailability> {
    let report: EngineFields = serde_json::from_slice(&read_bounded(path)?).ok()?;
    if report.schema != SCHEMA || report.codec != "h264" {
        return None;
    }
    let reason = match report.hardware_reason {
        Some(reason) if reason.chars().count() > MAX_REASON_CHARS => return None,
        reason => reason,
    };
    let available = report.hardware_available == Some(true);
    Some(EngineAvailability {
        available,
        reason: reason.filter(|_| !available),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A whole report as the encoder writes it, as JSON to change.
    pub(crate) fn whole_report(codec: &str) -> serde_json::Value {
        let h264 = codec == "h264";
        serde_json::json!({
            "schema": 1,
            "codec": codec,
            "streaming": true,
            "engine": if h264 { "hardware" } else { "libvpx" },
            "engineWanted": if h264 { Some("hardware") } else { None },
            "hardwareAvailable": if h264 { Some(true) } else { None },
            "hardwareReason": null,
            "width": 1920,
            "height": 1080,
            "kbps": 8000,
            "maxFps": 30,
            "converters": 3,
            "encodedFrames": 1000,
            "stageMicros": { "decode": 28100, "convert": 9600, "encode": 17600 },
            "gate": { "admitted": 1000, "tooSoon": 500, "unchanged": 20 },
            "hardware": if h264 {
                serde_json::json!({
                    "passes": 1300, "bothSearches": 200, "wide": 150, "higherQp": 9,
                    "leftOut": 3, "keyFrames": 40, "failures": 0, "engineMicros": 9490000
                })
            } else {
                serde_json::Value::Null
            }
        })
    }

    /// A report is taken whole and within every bound, or refused: too
    /// large, damaged, of another form, another codec, or out of bounds.
    #[test]
    fn a_whole_report_is_taken_only_within_its_bounds() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.json");
        assert_eq!(read_encoder_report(&path, "h264"), Ok(None));
        for codec in ["h264", "vp8"] {
            std::fs::write(&path, whole_report(codec).to_string()).unwrap();
            let (report, _) = read_encoder_report(&path, codec).unwrap().unwrap();
            assert_eq!(report.codec, codec);
            assert_eq!(report.gate.too_soon, 500);
            assert_eq!(report.hardware.is_some(), codec == "h264");
            // Another codec's report is not this one's.
            let other = if codec == "h264" { "vp8" } else { "h264" };
            assert_eq!(
                read_encoder_report(&path, other),
                Err(ReportRefusal::OutOfBounds)
            );
        }
        let changed = |pointer: &str, value: serde_json::Value| {
            let mut report = whole_report("h264");
            *report.pointer_mut(pointer).unwrap() = value;
            report.to_string()
        };
        for (written, refusal) in [
            (changed("/schema", 2.into()), ReportRefusal::OutOfBounds),
            (changed("/engine", "vp8".into()), ReportRefusal::OutOfBounds),
            (
                changed("/engineWanted", "fast".into()),
                ReportRefusal::OutOfBounds,
            ),
            (
                changed("/engineWanted", serde_json::Value::Null),
                ReportRefusal::OutOfBounds,
            ),
            (
                changed("/hardwareReason", "x".repeat(121).into()),
                ReportRefusal::OutOfBounds,
            ),
            (changed("/width", 8193.into()), ReportRefusal::OutOfBounds),
            (changed("/kbps", 63.into()), ReportRefusal::OutOfBounds),
            (changed("/kbps", 20001.into()), ReportRefusal::OutOfBounds),
            (changed("/maxFps", 0.into()), ReportRefusal::OutOfBounds),
            (changed("/maxFps", 61.into()), ReportRefusal::OutOfBounds),
            (changed("/converters", 5.into()), ReportRefusal::OutOfBounds),
            (
                changed("/stageMicros/decode", 10_000_001.into()),
                ReportRefusal::OutOfBounds,
            ),
            (
                changed("/encodedFrames", (-1).into()),
                ReportRefusal::NotJson,
            ),
            (
                changed("/gate/admitted", "many".into()),
                ReportRefusal::NotJson,
            ),
            (changed("/streaming", "yes".into()), ReportRefusal::NotJson),
            (
                whole_report("h264").to_string()[..60].to_string(),
                ReportRefusal::NotJson,
            ),
            (String::from("not json"), ReportRefusal::NotJson),
        ] {
            std::fs::write(&path, &written).unwrap();
            assert_eq!(
                read_encoder_report(&path, "h264"),
                Err(refusal),
                "{written}"
            );
        }
        // A field the schema does not have, and a VP8 report with the
        // engine's counts.
        let mut report = whole_report("h264");
        report["extra"] = 1.into();
        std::fs::write(&path, report.to_string()).unwrap();
        assert_eq!(
            read_encoder_report(&path, "h264"),
            Err(ReportRefusal::NotJson)
        );
        let mut report = whole_report("vp8");
        report["hardware"] = whole_report("h264")["hardware"].clone();
        std::fs::write(&path, report.to_string()).unwrap();
        assert_eq!(
            read_encoder_report(&path, "vp8"),
            Err(ReportRefusal::OutOfBounds)
        );
        // Larger than a report, though whole JSON.
        let mut large = whole_report("h264").to_string().into_bytes();
        large.resize(MAX_REPORT_BYTES + 1, b' ');
        std::fs::write(&path, &large).unwrap();
        assert_eq!(
            read_encoder_report(&path, "h264"),
            Err(ReportRefusal::TooLarge)
        );
        large.truncate(MAX_REPORT_BYTES);
        std::fs::write(&path, &large).unwrap();
        assert!(read_encoder_report(&path, "h264").unwrap().is_some());
        assert_eq!(ReportRefusal::TooLarge.says(), "larger than a report");
    }

    #[test]
    fn the_engines_availability_comes_from_a_checked_report() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.json");
        assert_eq!(read_engine_availability(&path), None);
        let report = |available: &str, reason: &str| {
            format!(
                r#"{{"schema":1,"codec":"h264","streaming":false,"engine":null,"engineWanted":"hardware","hardwareAvailable":{available},"hardwareReason":{reason}}}"#
            )
        };
        for (written, read) in [
            (
                report("true", "null"),
                Some(EngineAvailability {
                    available: true,
                    reason: None,
                }),
            ),
            (
                report("false", r#""/dev/x: busy""#),
                Some(EngineAvailability {
                    available: false,
                    reason: Some("/dev/x: busy".into()),
                }),
            ),
            // No device named: no engine to have.
            (
                report("null", "null"),
                Some(EngineAvailability {
                    available: false,
                    reason: None,
                }),
            ),
            // Another codec's, another form's, damaged, out of bounds.
            (report("true", "null").replace("h264", "vp8"), None),
            (
                report("true", "null").replace(r#""schema":1"#, r#""schema":2"#),
                None,
            ),
            (report("true", "null")[..40].to_string(), None),
            (report("1", "null"), None),
            (report("false", &format!("\"{}\"", "x".repeat(121))), None),
            (String::new(), None),
        ] {
            std::fs::write(&path, &written).unwrap();
            assert_eq!(read_engine_availability(&path), read, "{written}");
        }
        // A reason of 120 characters is the most there is.
        std::fs::write(
            &path,
            report("false", &format!("\"{}\"", "\u{e9}".repeat(120))),
        )
        .unwrap();
        assert_eq!(
            read_engine_availability(&path)
                .unwrap()
                .reason
                .unwrap()
                .chars()
                .count(),
            120
        );
        // A file larger than a report is not read to its end.
        let mut large = report("true", "null").into_bytes();
        large.resize(MAX_REPORT_BYTES + 1, b' ');
        std::fs::write(&path, &large).unwrap();
        assert_eq!(read_engine_availability(&path), None);
        large.truncate(MAX_REPORT_BYTES);
        std::fs::write(&path, &large).unwrap();
        assert!(read_engine_availability(&path).unwrap().available);
    }
}
