//! Reviewed video-stream settings contract.
//!
//! The reference interface sets the desired frame rate and the JPEG quality of
//! the video engine and expects the change to be applied immediately. Both are
//! start-up options of the retained `ustreamer`, so the privileged owner renders
//! the accepted values into the environment file its unit reads and restarts it.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Highest desired frame rate the interface offers.
pub const VIDEO_FPS_MAX: u16 = 60;
/// Lowest desired frame rate that still streams.
pub const VIDEO_FPS_MIN: u16 = 1;
/// JPEG quality bounds of the video engine (`1` worst, `100` lossless).
pub const VIDEO_QUALITY_MIN: u8 = 1;
pub const VIDEO_QUALITY_MAX: u8 = 100;
/// Desired frame rate and quality the unit starts with when nothing is set.
pub const VIDEO_FPS_DEFAULT: u16 = 40;
pub const VIDEO_QUALITY_DEFAULT: u8 = 80;

/// Environment keys of the reviewed environment file.
pub const VIDEO_FPS_ENV: &str = "BLI_KVM_USTREAMER_FPS";
pub const VIDEO_RESOLUTION_ENV: &str = "BLI_KVM_USTREAMER_RESOLUTION";
/// The reviewed capture modes, taken from the vendor list and from the modes
/// the interface offers, keyed by the height it sends.
pub const VIDEO_RESOLUTIONS: [(&str, u16); 4] = [
    ("1920x1080", 1080),
    ("1280x720", 720),
    ("800x600", 600),
    ("640x480", 480),
];
pub const VIDEO_RESOLUTION_DEFAULT: &str = "1280x720";
pub const VIDEO_QUALITY_ENV: &str = "BLI_KVM_USTREAMER_QUALITY";

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum VideoSettingsError {
    #[error("the desired frame rate is out of range")]
    FpsOutOfRange,
    #[error("the JPEG quality is out of range")]
    QualityOutOfRange,
    #[error("the settings request changes nothing")]
    Empty,
    #[error("the capture resolution is not reviewed")]
    ResolutionUnsupported,
}

/// Whether the mode is one of the reviewed capture modes.
pub fn reviewed_resolution(resolution: &str) -> bool {
    VIDEO_RESOLUTIONS
        .iter()
        .any(|(mode, _)| *mode == resolution)
}

/// Maps the height the interface sends onto a reviewed mode.
pub fn resolution_for_height(height: u16) -> Option<&'static str> {
    VIDEO_RESOLUTIONS
        .iter()
        .find(|(_, reviewed_height)| *reviewed_height == height)
        .map(|(mode, _)| *mode)
}

/// One bounded change request: either field may be absent, and at least one
/// must be present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VideoSettingsChange {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
}

impl VideoSettingsChange {
    pub fn new(fps: Option<u16>, quality: Option<u8>) -> Self {
        Self {
            fps,
            quality,
            resolution: None,
        }
    }

    pub fn with_resolution(mut self, resolution: impl Into<String>) -> Self {
        self.resolution = Some(resolution.into());
        self
    }

    pub fn validate(&self) -> Result<(), VideoSettingsError> {
        if self.fps.is_none() && self.quality.is_none() && self.resolution.is_none() {
            return Err(VideoSettingsError::Empty);
        }
        if let Some(fps) = self.fps {
            if !(VIDEO_FPS_MIN..=VIDEO_FPS_MAX).contains(&fps) {
                return Err(VideoSettingsError::FpsOutOfRange);
            }
        }
        if let Some(quality) = self.quality {
            if !(VIDEO_QUALITY_MIN..=VIDEO_QUALITY_MAX).contains(&quality) {
                return Err(VideoSettingsError::QualityOutOfRange);
            }
        }
        if let Some(resolution) = self.resolution.as_deref() {
            if !reviewed_resolution(resolution) {
                return Err(VideoSettingsError::ResolutionUnsupported);
            }
        }
        Ok(())
    }
}

/// The complete desired video state, as it is written to the environment file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoSettings {
    pub fps: u16,
    pub quality: u8,
    pub resolution: String,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            fps: VIDEO_FPS_DEFAULT,
            quality: VIDEO_QUALITY_DEFAULT,
            resolution: VIDEO_RESOLUTION_DEFAULT.to_string(),
        }
    }
}

impl VideoSettings {
    pub fn new(fps: u16, quality: u8) -> Result<Self, VideoSettingsError> {
        VideoSettingsChange::new(Some(fps), Some(quality)).validate()?;
        Ok(Self {
            fps,
            quality,
            resolution: VIDEO_RESOLUTION_DEFAULT.to_string(),
        })
    }

    /// Reads the state back from the reviewed environment-file text, falling
    /// back to the defaults for absent or rejected keys.
    pub fn from_env_file(text: &str) -> Self {
        let mut settings = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if let Some(value) = line.strip_prefix(&format!("{VIDEO_FPS_ENV}=")) {
                if let Ok(fps) = value.trim().parse::<u16>() {
                    if (VIDEO_FPS_MIN..=VIDEO_FPS_MAX).contains(&fps) {
                        settings.fps = fps;
                    }
                }
            } else if let Some(value) = line.strip_prefix(&format!("{VIDEO_RESOLUTION_ENV}=")) {
                let value = value.trim();
                if reviewed_resolution(value) {
                    settings.resolution = value.to_string();
                }
            } else if let Some(value) = line.strip_prefix(&format!("{VIDEO_QUALITY_ENV}=")) {
                if let Ok(quality) = value.trim().parse::<u8>() {
                    if (VIDEO_QUALITY_MIN..=VIDEO_QUALITY_MAX).contains(&quality) {
                        settings.quality = quality;
                    }
                }
            }
        }
        settings
    }

    /// Applies one validated change on top of the current state.
    pub fn apply(&self, change: &VideoSettingsChange) -> Result<Self, VideoSettingsError> {
        change.validate()?;
        Ok(Self {
            fps: change.fps.unwrap_or(self.fps),
            quality: change.quality.unwrap_or(self.quality),
            resolution: change
                .resolution
                .clone()
                .unwrap_or_else(|| self.resolution.clone()),
        })
    }

    /// Renders the exact environment file the video-engine unit reads.
    pub fn to_env_file(&self) -> String {
        format!(
            "# Written by the Hardened BliKVM board owner from the reviewed \
             video-settings request.\n{VIDEO_FPS_ENV}={}\n{VIDEO_QUALITY_ENV}={}\n\
             {VIDEO_RESOLUTION_ENV}={}\n",
            self.fps, self.quality, self.resolution
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_are_bounded_and_never_empty() {
        assert_eq!(
            VideoSettingsChange::new(None, None).validate(),
            Err(VideoSettingsError::Empty)
        );
        assert_eq!(
            VideoSettingsChange::new(Some(0), None).validate(),
            Err(VideoSettingsError::FpsOutOfRange)
        );
        assert_eq!(
            VideoSettingsChange::new(Some(61), None).validate(),
            Err(VideoSettingsError::FpsOutOfRange)
        );
        assert_eq!(
            VideoSettingsChange::new(None, Some(0)).validate(),
            Err(VideoSettingsError::QualityOutOfRange)
        );
        assert_eq!(
            VideoSettingsChange::new(None, Some(101)).validate(),
            Err(VideoSettingsError::QualityOutOfRange)
        );
        assert_eq!(
            VideoSettingsChange::new(Some(60), Some(100)).validate(),
            Ok(())
        );
        assert_eq!(VideoSettingsChange::new(Some(30), None).validate(), Ok(()));
    }

    #[test]
    fn a_change_applies_on_top_of_the_current_state() {
        let current = VideoSettings::new(40, 80).unwrap();
        let next = current
            .apply(&VideoSettingsChange::new(Some(24), None))
            .unwrap();
        assert_eq!(
            next,
            VideoSettings {
                fps: 24,
                quality: 80,
                resolution: VIDEO_RESOLUTION_DEFAULT.to_string()
            }
        );
        let next = current
            .apply(&VideoSettingsChange::new(None, Some(100)))
            .unwrap();
        assert_eq!(
            next,
            VideoSettings {
                fps: 40,
                quality: 100,
                resolution: VIDEO_RESOLUTION_DEFAULT.to_string()
            }
        );
    }

    #[test]
    fn the_reviewed_resolution_modes_are_what_the_interface_offers() {
        assert_eq!(resolution_for_height(1080), Some("1920x1080"));
        assert_eq!(resolution_for_height(720), Some("1280x720"));
        assert_eq!(resolution_for_height(600), Some("800x600"));
        assert_eq!(resolution_for_height(480), Some("640x480"));
        // "auto" and anything the vendor list does not carry stay unsupported.
        assert_eq!(resolution_for_height(0), None);
        assert_eq!(resolution_for_height(1234), None);
        assert!(reviewed_resolution("1920x1080"));
        assert!(!reviewed_resolution("4096x2160"));

        let change = VideoSettingsChange::new(None, None).with_resolution("1920x1080");
        assert_eq!(change.validate(), Ok(()));
        assert_eq!(
            VideoSettingsChange::new(None, None)
                .with_resolution("4096x2160")
                .validate(),
            Err(VideoSettingsError::ResolutionUnsupported)
        );
        let current = VideoSettings::new(60, 100).unwrap();
        let next = current.apply(&change).unwrap();
        assert_eq!(next.resolution, "1920x1080");
        assert_eq!(next.fps, 60);
        assert_eq!(next.quality, 100);
        assert!(
            next.to_env_file()
                .contains("BLI_KVM_USTREAMER_RESOLUTION=1920x1080")
        );
        assert_eq!(VideoSettings::from_env_file(&next.to_env_file()), next);
        assert_eq!(
            VideoSettings::from_env_file("BLI_KVM_USTREAMER_RESOLUTION=4096x2160\n"),
            VideoSettings::default()
        );
    }

    #[test]
    fn the_environment_file_round_trips_and_rejects_tampering() {
        let settings = VideoSettings::new(60, 50).unwrap();
        let text = settings.to_env_file();
        assert!(text.contains("BLI_KVM_USTREAMER_FPS=60"));
        assert!(text.contains("BLI_KVM_USTREAMER_QUALITY=50"));
        assert_eq!(VideoSettings::from_env_file(&text), settings);

        // An out-of-range or unknown value falls back to the reviewed default.
        assert_eq!(
            VideoSettings::from_env_file("BLI_KVM_USTREAMER_FPS=999\n"),
            VideoSettings::default()
        );
        assert_eq!(
            VideoSettings::from_env_file("OTHER=1\n"),
            VideoSettings::default()
        );
        assert_eq!(
            VideoSettings::from_env_file("BLI_KVM_USTREAMER_QUALITY=101\n"),
            VideoSettings::default()
        );
    }
}

/// The reference mouse jiggler keeps a target awake by nudging the pointer.
///
/// Only relative nudging is offered: an absolute move needs the current
/// pointer position, which this arrangement never learns from the target, so
/// the reference's absolute mode is refused instead of being faked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseJigglerMode {
    Disable,
    Relative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MouseJigglerChange {
    pub enabled: bool,
    pub mode: MouseJigglerMode,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum MouseJigglerError {
    #[error("the requested mouse-jiggler mode is not supported")]
    UnsupportedMode,
}

impl MouseJigglerChange {
    pub fn new(enabled: bool, mode: MouseJigglerMode) -> Self {
        Self { enabled, mode }
    }

    /// Normalises the reference shape: a disabled jiggler has no mode, and a
    /// jiggler can only be enabled in a mode this arrangement can drive.
    pub fn resolve(&self) -> Result<Option<MouseJigglerMode>, MouseJigglerError> {
        if !self.enabled || self.mode == MouseJigglerMode::Disable {
            return Ok(None);
        }
        match self.mode {
            MouseJigglerMode::Relative => Ok(Some(MouseJigglerMode::Relative)),
            MouseJigglerMode::Disable => Ok(None),
        }
    }

    /// The mode the interface should read back.
    pub fn reported_mode(&self) -> MouseJigglerMode {
        match self.resolve() {
            Ok(Some(MouseJigglerMode::Relative)) => MouseJigglerMode::Relative,
            _ => MouseJigglerMode::Disable,
        }
    }
}

/// The bounded nudge the owner sends on every tick: a one-pixel relative move
/// followed by a release report, so a jiggler never leaves a held movement.
pub const JIGGLE_DELTA: i8 = 1;
pub const JIGGLE_INTERVAL_SECONDS: u64 = 10;

/// The report pair for one tick.
pub fn jiggle_reports() -> [[u8; 4]; 2] {
    [[0, JIGGLE_DELTA as u8, 0, 0], [0, 0, 0, 0]]
}

#[cfg(test)]
mod jiggler_tests {
    use super::*;

    #[test]
    fn the_reference_shape_is_normalised_and_absolute_is_refused() {
        let off = MouseJigglerChange::new(false, MouseJigglerMode::Relative);
        assert_eq!(off.resolve(), Ok(None));
        assert_eq!(off.reported_mode(), MouseJigglerMode::Disable);

        let relative = MouseJigglerChange::new(true, MouseJigglerMode::Relative);
        assert_eq!(relative.resolve(), Ok(Some(MouseJigglerMode::Relative)));
        assert_eq!(relative.reported_mode(), MouseJigglerMode::Relative);

        let absolute = MouseJigglerChange::new(true, MouseJigglerMode::Disable);
        assert_eq!(absolute.resolve(), Ok(None));
    }

    #[test]
    fn the_nudge_always_releases_the_movement() {
        let reports = jiggle_reports();
        assert_ne!(reports[0], [0, 0, 0, 0]);
        assert_eq!(reports[1], [0, 0, 0, 0]);
        assert_eq!(reports[0][1], JIGGLE_DELTA as u8);
    }
}
