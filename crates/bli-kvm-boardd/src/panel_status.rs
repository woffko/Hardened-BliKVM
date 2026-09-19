//! Status frame for the ST7789 panel: the vendor's status screen content.
//!
//! The vendor draws a background image and overlays the host name, address,
//! temperature, uptime, CPU load and memory. This module renders the same
//! information with the embedded 8x8 font; the background image is the vendor
//! `oled_info.bmp` when the operator provides it.

use std::{
    fs,
    net::UdpSocket,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use bli_kvm_st7789_model::{DISPLAY_HEIGHT, DISPLAY_PIXEL_COUNT, DISPLAY_WIDTH, Rgb565};

use crate::panel_font::draw_text;

/// Reason a status frame could not be prepared.
#[derive(Debug, PartialEq, Eq)]
pub enum StatusError {
    /// The background image is not a supported uncompressed 24/32-bit BMP.
    UnsupportedImage,
    /// The background image is damaged.
    DamagedImage,
}

/// The dynamic values shown on the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemInfo {
    pub hostname: String,
    pub address: String,
    pub temperature: String,
    pub uptime: String,
    pub load: String,
    pub memory: String,
}

impl SystemInfo {
    /// Collect the visible values from the kernel interfaces. Missing files
    /// degrade to `n/a` instead of failing.
    pub fn collect() -> Self {
        Self {
            hostname: read_trimmed("/proc/sys/kernel/hostname").unwrap_or_else(|| "blikvm".into()),
            address: primary_address().unwrap_or_else(|| "no ip".into()),
            temperature: temperature_celsius(),
            uptime: uptime_text(),
            load: read_trimmed("/proc/loadavg")
                .and_then(|value| value.split_whitespace().next().map(str::to_string))
                .unwrap_or_else(|| "n/a".into()),
            memory: memory_text(),
        }
    }

    /// Lines in the order they are rendered.
    pub fn lines(&self) -> [String; 5] {
        [
            self.address.clone(),
            format!("{}  {}", self.temperature, self.uptime),
            format!("load {}", self.load),
            self.memory.clone(),
            self.hostname.clone(),
        ]
    }
}

fn read_trimmed(path: &str) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// The primary IPv4 address without sending any packets.
fn primary_address() -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let address = socket.local_addr().ok()?.ip();
    if address.is_unspecified() {
        None
    } else {
        Some(address.to_string())
    }
}

fn temperature_celsius() -> String {
    // The H616 exposes its thermal zone through the sunxi hwmon; fall back to
    // the generic thermal zone and then to unknown.
    for path in [
        "/sys/class/thermal/thermal_zone0/temp",
        "/sys/class/hwmon/hwmon0/temp1_input",
    ] {
        if let Some(raw) = read_trimmed(path) {
            if let Ok(millidegrees) = raw.parse::<i64>() {
                return format!("{}C", millidegrees / 1000);
            }
        }
    }
    "n/a".into()
}

fn uptime_text() -> String {
    let Some(raw) = read_trimmed("/proc/uptime") else {
        return "n/a".into();
    };
    let seconds = raw
        .split_whitespace()
        .next()
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(0.0) as u64;
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("{days}d{hours}h")
    } else if hours > 0 {
        format!("{hours}h{minutes}m")
    } else {
        format!("{minutes}m")
    }
}

fn memory_text() -> String {
    let Some(raw) = fs::read_to_string("/proc/meminfo").ok() else {
        return "mem n/a".into();
    };
    let mut total = 0_u64;
    let mut available = 0_u64;
    for line in raw.lines() {
        if let Some(value) = line.strip_prefix("MemTotal:") {
            total = parse_kib(value);
        } else if let Some(value) = line.strip_prefix("MemAvailable:") {
            available = parse_kib(value);
        }
    }
    if total == 0 {
        return "mem n/a".into();
    }
    let used_mib = total.saturating_sub(available) / 1024;
    format!("mem {used_mib}M")
}

fn parse_kib(value: &str) -> u64 {
    value
        .split_whitespace()
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
}

/// Load a bottom-up or top-down uncompressed 24/32-bit BMP as RGB565.
pub fn load_background(path: &Path) -> Result<Vec<Rgb565>, StatusError> {
    let data = fs::read(path).map_err(|_| StatusError::DamagedImage)?;
    if data.len() < 54 || &data[0..2] != b"BM" {
        return Err(StatusError::UnsupportedImage);
    }
    let offset = u32::from_le_bytes([data[10], data[11], data[12], data[13]]) as usize;
    let width = i32::from_le_bytes([data[18], data[19], data[20], data[21]]);
    let height = i32::from_le_bytes([data[22], data[23], data[24], data[25]]);
    let bits = u16::from_le_bytes([data[28], data[29]]);
    let compression = u32::from_le_bytes([data[30], data[31], data[32], data[33]]);
    if compression != 0 || !(bits == 24 || bits == 32) {
        return Err(StatusError::UnsupportedImage);
    }
    let (width, height) = (width.unsigned_abs(), height.unsigned_abs());
    if width == 0 || height == 0 || width > 1024 || height > 1024 {
        return Err(StatusError::UnsupportedImage);
    }
    let bytes_per_pixel = usize::from(bits / 8);
    let row_bytes = (u64::from(width) * bits as u64).div_ceil(32) as usize * 4;
    let top_down = i32::from_le_bytes([data[22], data[23], data[24], data[25]]) < 0;
    let mut frame = vec![Rgb565::BLACK; DISPLAY_PIXEL_COUNT as usize];
    for row in 0..DISPLAY_HEIGHT.min(height as u16) {
        let source_row = if top_down {
            row
        } else {
            height as u16 - 1 - row
        };
        let row_start = offset + source_row as usize * row_bytes;
        for column in 0..DISPLAY_WIDTH.min(width as u16) {
            let index = row_start + column as usize * bytes_per_pixel;
            let (red, green, blue) = (data[index + 2], data[index + 1], data[index]);
            let destination = row as usize * usize::from(DISPLAY_WIDTH) + column as usize;
            frame[destination] = Rgb565::from_rgb888(red, green, blue);
        }
    }
    Ok(frame)
}

/// Render the status screen: the background image, then the text overlay.
pub fn render(background: Option<&[Rgb565]>, info: &SystemInfo) -> Vec<Rgb565> {
    let mut frame = match background {
        Some(image) if image.len() == DISPLAY_PIXEL_COUNT as usize => image.to_vec(),
        _ => vec![Rgb565::BLACK; DISPLAY_PIXEL_COUNT as usize],
    };
    draw_text(
        &mut frame,
        DISPLAY_WIDTH,
        DISPLAY_HEIGHT,
        8,
        8,
        "BLIKVM V4",
        Rgb565::WHITE,
    );
    let mut y = 40;
    for line in info.lines() {
        draw_text(
            &mut frame,
            DISPLAY_WIDTH,
            DISPLAY_HEIGHT,
            8,
            y,
            &line,
            Rgb565::WHITE,
        );
        y += 24;
    }
    frame
}

/// Milliseconds since the Unix epoch, clamped to 0.
pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Whether `deadline` has passed relative to `now`.
pub fn due(now: u64, last: u64, interval: Duration) -> bool {
    now.saturating_sub(last) >= interval.as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_bmp() -> Vec<u8> {
        let mut data = vec![0_u8; 54 + 4];
        data[0..2].copy_from_slice(b"BM");
        data[10..14].copy_from_slice(&54_u32.to_le_bytes());
        data[14..18].copy_from_slice(&40_u32.to_le_bytes());
        data[18..22].copy_from_slice(&1_i32.to_le_bytes());
        data[22..26].copy_from_slice(&1_i32.to_le_bytes());
        data[26..28].copy_from_slice(&1_u16.to_le_bytes());
        data[28..30].copy_from_slice(&24_u16.to_le_bytes());
        // One pixel, stored BGR.
        data[54] = 30;
        data[55] = 20;
        data[56] = 10;
        data
    }

    #[test]
    fn a_tiny_bmp_converts_to_rgb565() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tiny.bmp");
        fs::write(&path, tiny_bmp()).unwrap();
        let frame = load_background(&path).unwrap();
        assert_eq!(frame.len(), DISPLAY_PIXEL_COUNT as usize);
        assert_eq!(frame[0], Rgb565::from_rgb888(10, 20, 30));
        assert_eq!(frame[1], Rgb565::BLACK);
    }

    #[test]
    fn a_non_bmp_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bad.bmp");
        fs::write(&path, b"not a bitmap").unwrap();
        assert_eq!(load_background(&path), Err(StatusError::UnsupportedImage));
    }

    #[test]
    fn render_produces_a_full_frame_and_uses_the_background() {
        let info = SystemInfo {
            hostname: "blikvm".into(),
            address: "192.0.2.9".into(),
            temperature: "42C".into(),
            uptime: "1h2m".into(),
            load: "0.10".into(),
            memory: "mem 100M".into(),
        };
        let background = vec![Rgb565::GREEN; DISPLAY_PIXEL_COUNT as usize];
        let frame = render(Some(&background), &info);
        assert_eq!(frame.len(), DISPLAY_PIXEL_COUNT as usize);
        assert!(frame.contains(&Rgb565::WHITE));
        assert!(frame.contains(&Rgb565::GREEN));
        let empty = render(None, &info);
        assert!(empty.contains(&Rgb565::WHITE));
    }

    #[test]
    fn the_interval_helper_respects_the_deadline() {
        assert!(!due(1_000, 0, Duration::from_millis(5_000)));
        assert!(due(5_000, 0, Duration::from_millis(5_000)));
        assert!(due(9_000, 4_000, Duration::from_millis(5_000)));
    }

    #[test]
    fn memory_and_uptime_parsers_are_bounded() {
        assert_eq!(parse_kib("  2048 kB"), 2048);
        assert_eq!(parse_kib("garbage"), 0);
    }
}
