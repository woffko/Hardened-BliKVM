//! The encoders' reports (`docs/design/h264-engine-choice-and-quality-monitor-v1.md`):
//! files that another process writes into its runtime directory, read
//! bounded and checked like the status word.

use std::{io::Read, path::Path};

use serde::Deserialize;

/// A report is never larger; a larger file is no report.
pub(crate) const MAX_REPORT_BYTES: usize = 4096;
/// The report's form that this server reads.
const SCHEMA: u32 = 1;
/// A reason is at most this many characters.
const MAX_REASON_CHARS: usize = 120;

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
mod tests {
    use super::*;

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
