//! The encoder's report for the web server: what encodes, what the board
//! chose and whether the engine can be had, the stream's size and bitrate
//! (`docs/design/h264-engine-choice-and-quality-monitor-v1.md`). The unit
//! names the file; it is replaced whole and readable by another user, like
//! the status word.

use std::{io, path::Path};

use serde::Serialize;

/// The report's form, which the server checks.
pub const SCHEMA: u32 = 1;
/// A report is never larger.
pub const MAX_BYTES: usize = 4096;

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
}

/// What the report calls the encoder that `Engine::encoder_name` names.
pub fn engine_name(encoder: &'static str) -> &'static str {
    match encoder {
        "vp8" => "libvpx",
        other => other,
    }
}

impl Report {
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
                "converters": 3
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
