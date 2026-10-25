//! What the agent reads on the target and tells the board when it is asked:
//! the system, the monitors and the keyboard layouts. Every value is bounded
//! here, on both ends: the board refuses an answer that is not, rather than
//! carrying it to a page.

use serde::{Deserialize, Serialize};

/// The longest text a read carries.
pub const MAX_TEXT: usize = 128;
/// The most monitors a desktop is said to have.
pub const MAX_MONITORS: usize = 16;
/// The most keyboard layouts.
pub const MAX_LAYOUTS: usize = 32;
/// The farthest a monitor lies from the desktop's origin, and its largest
/// side, in pixels (the absolute mouse's range).
pub const MAX_COORDINATE: i32 = 32767;
/// The digits of a monitor's id.
pub const MONITOR_ID_DIGITS: usize = 16;

/// The target's operating system and session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct System {
    /// The product, as its settings name it: "Windows 11 Pro".
    pub product: String,
    /// The release: "24H2".
    pub release: String,
    /// The build and its revision: 26100 and 2033.
    pub build: u32,
    pub revision: u32,
    /// "x86_64", "aarch64" or "x86".
    pub architecture: String,
    /// The session is locked: the sign-in screen is in front.
    pub locked: bool,
}

/// One active monitor, in the desktop's pixels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Monitor {
    /// The monitor as the target connects it, the same while it stays on
    /// its connector: lowercase hex digits the agent makes from Windows'
    /// name for the monitor's device, by which the operator says which one
    /// is the board's. Empty from an agent that does not say it.
    #[serde(default)]
    pub id: String,
    /// The monitor's name, as its EDID gives it.
    pub name: String,
    /// The EDID's maker (three letters) and product code.
    pub maker: String,
    pub product: u16,
    /// Its place in the desktop (the primary monitor's top left is 0, 0;
    /// others may lie left of or above it) and its size.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Refresh rate in thousandths of a hertz.
    pub refresh_millihertz: u32,
    pub primary: bool,
    /// The scale, in percent.
    pub scale: u32,
    /// This is the board's capture: the monitor the operator chose, or the
    /// one whose EDID the agent recognises.
    pub board: bool,
}

/// One keyboard layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    /// The layout's handle as eight hex digits: "04090409".
    pub id: String,
    /// The language's tag: "en-US".
    pub language: String,
    /// What the target calls it: "English (United States) - US".
    pub name: String,
}

/// The layout of the window in front, and the layouts installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keyboard {
    pub current: Option<Layout>,
    pub installed: Vec<Layout>,
}

fn text(value: &str) -> bool {
    value.chars().count() <= MAX_TEXT && !value.chars().any(char::is_control)
}

fn named(value: &str) -> bool {
    !value.is_empty() && text(value)
}

/// Whether `id` is a monitor's id.
pub fn monitor_id_is_valid(id: &str) -> bool {
    id.len() == MONITOR_ID_DIGITS
        && id
            .bytes()
            .all(|digit| digit.is_ascii_digit() || (b'a'..=b'f').contains(&digit))
}

impl System {
    pub fn is_valid(&self) -> bool {
        named(&self.product)
            && text(&self.release)
            && matches!(
                self.architecture.as_str(),
                "x86_64" | "aarch64" | "x86" | "unknown"
            )
    }
}

impl Monitor {
    pub fn is_valid(&self) -> bool {
        let inside = |place: i32, size: u32| {
            (-MAX_COORDINATE..=MAX_COORDINATE).contains(&place)
                && size > 0
                && size <= MAX_COORDINATE as u32
                && i64::from(place) + i64::from(size) <= i64::from(MAX_COORDINATE) + 1
        };
        (self.id.is_empty() || monitor_id_is_valid(&self.id))
            && text(&self.name)
            && self.maker.len() <= 3
            && self.maker.chars().all(|letter| letter.is_ascii_uppercase())
            && inside(self.x, self.width)
            && inside(self.y, self.height)
            && (100..=500).contains(&self.scale)
            && self.refresh_millihertz <= 1_000_000
    }
}

impl Layout {
    pub fn is_valid(&self) -> bool {
        self.id.len() == 8
            && self.id.chars().all(|digit| digit.is_ascii_hexdigit())
            && self.language.len() <= 85
            && self
                .language
                .chars()
                .all(|letter| letter.is_ascii_alphanumeric() || letter == '-')
            && text(&self.name)
    }
}

impl Keyboard {
    pub fn is_valid(&self) -> bool {
        self.installed.len() <= MAX_LAYOUTS
            && self.installed.iter().all(Layout::is_valid)
            && self.current.as_ref().is_none_or(Layout::is_valid)
    }
}

/// Whether a desktop's monitors are a desktop: few enough, each valid, at
/// most one primary and at most one the board's, and no id twice.
pub fn monitors_are_valid(monitors: &[Monitor]) -> bool {
    let mut ids: Vec<&str> = monitors
        .iter()
        .map(|monitor| monitor.id.as_str())
        .filter(|id| !id.is_empty())
        .collect();
    let named = ids.len();
    ids.sort_unstable();
    ids.dedup();
    monitors.len() <= MAX_MONITORS
        && monitors.iter().all(Monitor::is_valid)
        && monitors.iter().filter(|monitor| monitor.primary).count() <= 1
        && monitors.iter().filter(|monitor| monitor.board).count() <= 1
        && ids.len() == named
}

/// The three letters of an EDID's maker, from the two bytes it is stored in
/// (bytes 8 and 9 of the EDID, big-endian, five bits a letter).
pub fn edid_maker(bytes: [u8; 2]) -> String {
    let value = u16::from_be_bytes(bytes);
    [10, 5, 0]
        .iter()
        .map(|shift| ((value >> shift) & 0x1f) as u8)
        .filter(|letter| (1..=26).contains(letter))
        .map(|letter| char::from(b'A' + letter - 1))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn monitor(x: i32, primary: bool) -> Monitor {
        Monitor {
            id: format!("{:016x}", x.unsigned_abs() + 1),
            name: "DELL U2719D".into(),
            maker: "DEL".into(),
            product: 0xd0a5,
            x,
            y: 0,
            width: 2560,
            height: 1440,
            refresh_millihertz: 59_951,
            primary,
            scale: 100,
            board: false,
        }
    }

    #[test]
    fn the_edids_maker_is_three_letters() {
        // "DEL" and "SAM" as monitors store them.
        assert_eq!(edid_maker([0x10, 0xac]), "DEL");
        assert_eq!(edid_maker([0x4c, 0x2d]), "SAM");
        assert_eq!(edid_maker([0, 0]), "");
    }

    #[test]
    fn a_desktop_is_valid_within_its_bounds() {
        assert!(monitors_are_valid(&[
            monitor(0, true),
            monitor(2560, false)
        ]));
        assert!(monitors_are_valid(&[
            monitor(-2560, false),
            monitor(0, true)
        ]));
        assert!(!monitors_are_valid(&[
            monitor(0, true),
            monitor(2560, true)
        ]));
        let mut far = monitor(31_000, false);
        assert!(!far.is_valid());
        far.x = 30_207;
        assert!(far.is_valid());
        let mut odd = monitor(0, true);
        odd.name = "bell\u{7}".into();
        assert!(!odd.is_valid());
        odd.name = "x".into();
        odd.maker = "dEL".into();
        assert!(!odd.is_valid());
        let mut boards = vec![monitor(0, true), monitor(2560, false)];
        boards[0].board = true;
        boards[1].board = true;
        assert!(!monitors_are_valid(&boards));
        let many: Vec<Monitor> = (0..=MAX_MONITORS as i32)
            .map(|place| monitor(place * 10, false))
            .collect();
        assert!(!monitors_are_valid(&many));
        assert!(monitors_are_valid(&many[..MAX_MONITORS]));
    }

    #[test]
    fn a_monitors_id_is_sixteen_hex_digits_once_in_a_desktop() {
        let mut one = monitor(0, true);
        assert!(one.is_valid());
        one.id = String::new();
        assert!(one.is_valid());
        one.id = "0123456789ABCDEF".into();
        assert!(!one.is_valid());
        one.id = "0123456789abcde".into();
        assert!(!one.is_valid());
        let mut twins = vec![monitor(0, true), monitor(2560, false)];
        twins[1].id = twins[0].id.clone();
        assert!(!monitors_are_valid(&twins));
        twins[0].id = String::new();
        twins[1].id = String::new();
        assert!(monitors_are_valid(&twins));
        // From an agent that does not say the id.
        let unsaid: Monitor = serde_json::from_str(
            r#"{"name":"BenQ LCD","maker":"BNQ","product":32804,"x":0,"y":0,"width":2560,"height":1440,"refresh_millihertz":59951,"primary":true,"scale":100,"board":false}"#,
        )
        .unwrap();
        assert_eq!(unsaid.id, "");
        assert!(unsaid.is_valid());
    }

    #[test]
    fn layouts_and_systems_are_bounded() {
        let layout = Layout {
            id: "04190419".into(),
            language: "ru-RU".into(),
            name: "Russian (Russia) - Russian".into(),
        };
        assert!(layout.is_valid());
        let keyboard = Keyboard {
            current: Some(layout.clone()),
            installed: vec![layout.clone(); MAX_LAYOUTS],
        };
        assert!(keyboard.is_valid());
        let mut odd = layout.clone();
        odd.id = "0419".into();
        assert!(!odd.is_valid());
        odd.id = "0419041g".into();
        assert!(!odd.is_valid());
        let mut odd = layout;
        odd.language = "ru RU".into();
        assert!(!odd.is_valid());

        let mut system = System {
            product: "Windows 11 Pro".into(),
            release: "24H2".into(),
            build: 26100,
            revision: 2033,
            architecture: "x86_64".into(),
            locked: false,
        };
        assert!(system.is_valid());
        system.architecture = "sparc".into();
        assert!(!system.is_valid());
        system.architecture = "x86_64".into();
        system.product = String::new();
        assert!(!system.is_valid());
    }
}
