//! The encoder's report for the web server: what encodes, what the board
//! chose and whether the engine can be had, the stream's size and bitrate,
//! and the numbers of the statistics line: frames, the stages' times, the
//! gate's counts and the engine's (`docs/design/h264-engine-choice-and-quality-monitor-v1.md`).
//! The unit names the file; it is replaced whole and readable by another
//! user, like the status word.

use std::{io, path::Path};

use serde::Serialize;

/// The report's form, which the server checks.
pub const SCHEMA: u32 = 1;
/// A report is never larger.
pub const MAX_BYTES: usize = 4096;
/// A stream's side is at most this many pixels in the report.
pub const MAX_SIDE: u32 = 8192;
/// A stage's mean time a frame is at most this, in microseconds.
pub const MAX_STAGE_MICROS: u64 = 10_000_000;

/// The stages' mean times a frame over the last second, in microseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct StageMicros {
    pub decode: u64,
    pub convert: u64,
    pub encode: u64,
}

/// What the gate did with the capture's frames since the encoder's start:
/// let through, kept back as too soon, skipped as unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GateCounts {
    pub admitted: u64,
    pub too_soon: u64,
    pub unchanged: u64,
}

/// What the video engine did since the encoder's start, over every stream
/// it encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareCounts {
    pub passes: u64,
    pub both_searches: u64,
    pub wide: u64,
    pub higher_qp: u64,
    pub left_out: u64,
    pub key_frames: u64,
    pub failures: u64,
    /// The engine's time summed over its passes.
    pub engine_micros: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub schema: u32,
    /// `h264` or `vp8`.
    pub codec: &'static str,
    /// A viewer is connected.
    pub streaming: bool,
    /// `hardware`, `x264` or `libvpx`; `None` before the first stream.
    pub engine: Option<&'static str>,
    /// `hardware` or `software`; `None` for VP8.
    pub engine_wanted: Option<&'static str>,
    /// `None` where the unit names no device.
    pub hardware_available: Option<bool>,
    pub hardware_reason: Option<String>,
    /// The stream's, 0 before the first.
    pub width: u32,
    pub height: u32,
    pub kbps: u32,
    pub max_fps: u32,
    pub converters: u32,
    /// Frames handed to the encoder since its start.
    pub encoded_frames: u64,
    pub stage_micros: StageMicros,
    pub gate: GateCounts,
    /// `None` where the engine has not encoded since the start.
    pub hardware: Option<HardwareCounts>,
}

/// What the report calls the encoder that `Engine::encoder_name` names.
pub fn engine_name(encoder: &'static str) -> &'static str {
    match encoder {
        "vp8" => "libvpx",
        other => other,
    }
}

impl Report {
    /// Whether `other` says the same as this but for its numbers, which
    /// change with every frame.
    pub fn same_but_numbers(&self, other: &Self) -> bool {
        let without = |report: &Self| Self {
            encoded_frames: 0,
            stage_micros: StageMicros::default(),
            gate: GateCounts::default(),
            hardware: None,
            ..report.clone()
        };
        without(self) == without(other)
    }

    /// The report as JSON, or `None` where it would be larger than
    /// [`MAX_BYTES`].
    pub fn to_json(&self) -> Option<Vec<u8>> {
        let mut bytes = serde_json::to_vec(self).ok()?;
        bytes.push(b'\n');
        (bytes.len() <= MAX_BYTES).then_some(bytes)
    }

    /// Replaces the file at `path` with the report, mode 0644: the web
    /// server reads it, which is another user, and the unit's mask would
    /// keep the file to the encoder.
    pub fn write(&self, path: &Path) -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let bytes = self
            .to_json()
            .ok_or_else(|| io::Error::other("the report is too large"))?;
        let temporary = path.with_extension("new");
        std::fs::write(&temporary, bytes)?;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o644))?;
        std::fs::rename(&temporary, path)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn report() -> Report {
        Report {
            schema: SCHEMA,
            codec: "h264",
            streaming: true,
            engine: Some("x264"),
            engine_wanted: Some("hardware"),
            hardware_available: Some(false),
            hardware_reason: Some("/dev/hardened-blikvm-ve-enc: Device or resource busy".into()),
            width: 1920,
            height: 1080,
            kbps: 8000,
            max_fps: 30,
            converters: 3,
            encoded_frames: 123_456,
            stage_micros: StageMicros {
                decode: 28_100,
                convert: 9_600,
                encode: 17_600,
            },
            gate: GateCounts {
                admitted: 123_500,
                too_soon: 61_000,
                unchanged: 5_000,
            },
            hardware: Some(HardwareCounts {
                passes: 150_000,
                both_searches: 30_000,
                wide: 20_000,
                higher_qp: 900,
                left_out: 300,
                key_frames: 4_100,
                failures: 0,
                engine_micros: 1_100_000_000,
            }),
        }
    }

    #[test]
    fn the_report_is_json_with_the_design_notes_names() {
        let value: serde_json::Value =
            serde_json::from_slice(&report().to_json().unwrap()).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "schema": 1,
                "codec": "h264",
                "streaming": true,
                "engine": "x264",
                "engineWanted": "hardware",
                "hardwareAvailable": false,
                "hardwareReason": "/dev/hardened-blikvm-ve-enc: Device or resource busy",
                "width": 1920,
                "height": 1080,
                "kbps": 8000,
                "maxFps": 30,
                "converters": 3,
                "encodedFrames": 123456,
                "stageMicros": { "decode": 28100, "convert": 9600, "encode": 17600 },
                "gate": { "admitted": 123500, "tooSoon": 61000, "unchanged": 5000 },
                "hardware": {
                    "passes": 150000, "bothSearches": 30000, "wide": 20000,
                    "higherQp": 900, "leftOut": 300, "keyFrames": 4100,
                    "failures": 0, "engineMicros": 1100000000u64
                }
            })
        );
        let vp8 = Report {
            codec: "vp8",
            engine: Some(engine_name("vp8")),
            engine_wanted: None,
            hardware_available: None,
            hardware_reason: None,
            ..report()
        };
        let value: serde_json::Value = serde_json::from_slice(&vp8.to_json().unwrap()).unwrap();
        assert_eq!(value["engine"], "libvpx");
        assert!(value["engineWanted"].is_null() && value["hardwareAvailable"].is_null());
        assert_eq!(engine_name("hardware"), "hardware");
        // The numbers change with every frame; the rest says when to write.
        let later = Report {
            encoded_frames: 200_000,
            gate: GateCounts::default(),
            hardware: None,
            ..report()
        };
        assert!(report().same_but_numbers(&later));
        assert!(!report().same_but_numbers(&Report {
            engine: Some("hardware"),
            ..later
        }));
    }

    /// The largest report there can be stays far below the bound.
    #[test]
    fn the_largest_report_is_far_below_the_bound() {
        let most = HardwareCounts {
            passes: u64::MAX,
            both_searches: u64::MAX,
            wide: u64::MAX,
            higher_qp: u64::MAX,
            left_out: u64::MAX,
            key_frames: u64::MAX,
            failures: u64::MAX,
            engine_micros: u64::MAX,
        };
        let largest = Report {
            hardware_reason: Some("\u{1F4A5}".repeat(120)),
            width: MAX_SIDE,
            height: MAX_SIDE,
            encoded_frames: u64::MAX,
            stage_micros: StageMicros {
                decode: MAX_STAGE_MICROS,
                convert: MAX_STAGE_MICROS,
                encode: MAX_STAGE_MICROS,
            },
            gate: GateCounts {
                admitted: u64::MAX,
                too_soon: u64::MAX,
                unchanged: u64::MAX,
            },
            hardware: Some(most),
            ..report()
        };
        assert!(largest.to_json().unwrap().len() < MAX_BYTES / 2);
    }

    #[test]
    fn the_report_replaces_the_file_whole_and_is_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("report.json");
        std::fs::write(&path, "old").unwrap();
        report().write(&path).unwrap();
        let written = std::fs::read(&path).unwrap();
        assert_eq!(written, report().to_json().unwrap());
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        // A reason as long as the choice lets it be keeps it far below the
        // bound; a report above it is not written.
        let long = Report {
            // 120 characters of four bytes, escaped or not.
            hardware_reason: Some("\u{1F4A5}".repeat(120)),
            ..report()
        };
        assert!(long.to_json().unwrap().len() < MAX_BYTES / 4);
        let huge = Report {
            hardware_reason: Some("x".repeat(MAX_BYTES)),
            ..report()
        };
        assert!(huge.to_json().is_none());
        assert!(huge.write(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), written);
    }
}
