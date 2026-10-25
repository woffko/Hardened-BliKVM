//! The board display's sleep: how long the display stays lit without
//! activity, as Hardened NanoKVM offers it.
//!
//! The board owner writes the choice; the board daemon reads it again on
//! every sample and turns the display's backlight off once the time has
//! passed. A missing, oversized or malformed file, or a time that is not one
//! of the choices, means never.

use std::{io::Read, path::Path};

/// Owner-written state file (root, 0600) in the owner's state directory,
/// which the board daemon, also root, reads.
pub const DISPLAY_SLEEP_PATH: &str = "/var/lib/hardened-blikvm-gadget/display-sleep.env";
/// The times offered, in seconds; 0 is never.
pub const DISPLAY_SLEEP_CHOICES: [u32; 9] = [0, 15, 30, 60, 180, 300, 600, 1800, 3600];
const MAX_FILE_BYTES: u64 = 256;
const KEY: &str = "sleep_seconds=";

/// Whether `seconds` is one of the times offered.
pub fn display_sleep_is_choice(seconds: u32) -> bool {
    DISPLAY_SLEEP_CHOICES.contains(&seconds)
}

/// The time the file's text says, if it says one of the choices.
pub fn parse_display_sleep(text: &str) -> Option<u32> {
    let mut found = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value = line.strip_prefix(KEY)?;
        if found.is_some() {
            return None;
        }
        found = Some(value.trim().parse::<u32>().ok()?);
    }
    found.filter(|seconds| display_sleep_is_choice(*seconds))
}

/// The file's text for a time.
pub fn render_display_sleep(seconds: u32) -> String {
    format!("{KEY}{seconds}\n")
}

/// Reads the time from the file; anything but a small file that says one of
/// the choices means never.
pub fn load_display_sleep(path: &Path) -> u32 {
    let Ok(file) = std::fs::File::open(path) else {
        return 0;
    };
    let mut text = String::new();
    if file
        .take(MAX_FILE_BYTES + 1)
        .read_to_string(&mut text)
        .is_err()
        || text.len() as u64 > MAX_FILE_BYTES
    {
        return 0;
    }
    parse_display_sleep(&text).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_choices_are_read() {
        for seconds in DISPLAY_SLEEP_CHOICES {
            assert_eq!(
                parse_display_sleep(&render_display_sleep(seconds)),
                Some(seconds)
            );
        }
        for text in [
            "",
            "sleep_seconds=20\n",
            "sleep_seconds=-15\n",
            "sleep_seconds=15\nsleep_seconds=30\n",
            "sleep=15\n",
            "sleep_seconds=15\nother=1\n",
            "sleep_seconds=fifteen\n",
        ] {
            assert_eq!(parse_display_sleep(text), None, "{text:?}");
        }
        assert_eq!(parse_display_sleep("  sleep_seconds= 60 \n\n"), Some(60));
    }

    #[test]
    fn a_missing_or_oversized_file_is_never() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("display-sleep.env");
        assert_eq!(load_display_sleep(&path), 0);
        std::fs::write(&path, render_display_sleep(300)).unwrap();
        assert_eq!(load_display_sleep(&path), 300);
        std::fs::write(
            &path,
            format!("{}{}", render_display_sleep(300), " ".repeat(300)),
        )
        .unwrap();
        assert_eq!(load_display_sleep(&path), 0);
    }
}
