//! What Windows' raw values are called, and how the agent says what it read:
//! the parts of the reads that are not Windows' calls, tested everywhere.

use bli_kvm_agent_protocol::{Keyboard, Monitor, System};

/// The first build of Windows 11, whose registry still says "Windows 10".
const WINDOWS_11_BUILD: u32 = 22000;

/// The capture devices the agent recognises by their EDID's maker and
/// product, as the target reads them: the board's monitor among the
/// target's. Filled from what the operator's target reads (gate 2 of the
/// plan); empty, no monitor is the board's.
pub const BOARD_CAPTURE_EDIDS: &[(&str, u16)] = &[];

/// The product as Windows' settings name it, from the registry's product
/// name and the build.
pub fn product_name(registry: &str, build: u32) -> String {
    let registry = registry.trim();
    if build >= WINDOWS_11_BUILD && registry.starts_with("Windows 10") {
        format!("Windows 11{}", &registry["Windows 10".len()..])
    } else if registry.is_empty() {
        "Windows".into()
    } else {
        registry.into()
    }
}

/// The architecture's name for `GetNativeSystemInfo`'s processor
/// architecture.
pub fn architecture_name(code: u16) -> &'static str {
    match code {
        9 => "x86_64",
        12 => "aarch64",
        0 => "x86",
        _ => "unknown",
    }
}

/// A keyboard layout's handle as eight hex digits.
pub fn layout_id(handle: usize) -> String {
    format!("{:08X}", handle as u32)
}

/// The registry's name for a layout's handle (under
/// `SYSTEM\CurrentControlSet\Control\Keyboard Layouts`), when the handle
/// names it directly: its high word is the layout unless it is a variant
/// (high nibble F), which only the registry's layout ids say.
pub fn layout_registry_key(handle: usize) -> Option<String> {
    let high = ((handle as u32) >> 16) as u16;
    (high & 0xf000 != 0xf000).then(|| format!("{:08X}", u32::from(high)))
}

/// The language's identifier in a layout's handle.
pub fn layout_language(handle: usize) -> u16 {
    handle as u32 as u16
}

/// The board's monitor among the target's, by the capture's EDID; the
/// first that matches.
pub fn mark_board_monitor(monitors: &mut [Monitor], known: &[(&str, u16)]) {
    if let Some(board) = monitors.iter_mut().find(|monitor| {
        known
            .iter()
            .any(|(maker, product)| monitor.maker == *maker && monitor.product == *product)
    }) {
        board.board = true;
    }
}

/// The value the agent's sign-in start has in the user's Run key.
pub const AUTOSTART_VALUE: &str = "HardenedBliKVMAgent";
/// The folder, under the user's local application data, that keeps the
/// program for the sign-in start, and the program's name there.
pub const AUTOSTART_FOLDER: &str = "HardenedBliKVM";
pub const AUTOSTART_PROGRAM: &str = "hardened-blikvm-agent.exe";

/// The Run key's command for the program at `path`: the path in quotes,
/// which a path cannot contain.
pub fn run_command(path: &str) -> String {
    format!("\"{}\"", path.replace('"', ""))
}

/// The words of the icon's menu and status, in Russian where the user's
/// Windows speaks it and in English otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Words {
    pub show: &'static str,
    pub autostart: &'static str,
    pub stop: &'static str,
    pub start: &'static str,
    pub exit: &'static str,
    pub stopped: &'static str,
    pub not_connected: &'static str,
    pub connected: &'static str,
    pub channel_off: &'static str,
}

const ENGLISH: Words = Words {
    show: "Show what the agent reads",
    autostart: "Start at sign-in",
    stop: "Stop",
    start: "Start",
    exit: "Exit",
    stopped: "Stopped",
    not_connected: "Not connected",
    connected: "Connected to",
    channel_off: "the board's channel is off",
};

const RUSSIAN: Words = Words {
    show: "Показать, что читает агент",
    autostart: "Запускать при входе",
    stop: "Остановить",
    start: "Запустить",
    exit: "Выйти",
    stopped: "Остановлен",
    not_connected: "Не подключён",
    connected: "Подключён к",
    channel_off: "канал платы выключен",
};

/// The words for the user's interface language (a Windows language id).
pub fn words(language: u16) -> Words {
    // The primary language is the id's low ten bits; 0x19 is Russian.
    if language & 0x3ff == 0x19 {
        RUSSIAN
    } else {
        ENGLISH
    }
}

/// What the agent read, as lines for its user.
pub fn describe(system: &System, monitors: &[Monitor], keyboard: &Keyboard) -> String {
    let mut lines = vec![format!(
        "{} {} (build {}.{}), {}{}",
        system.product,
        system.release,
        system.build,
        system.revision,
        system.architecture,
        if system.locked { ", locked" } else { "" }
    )];
    lines.push(format!("Monitors: {}", monitors.len()));
    for monitor in monitors {
        lines.push(format!(
            "  {} [{} {:04X}]: {}x{} at {},{}, {}.{:03} Hz, {} %{}{}",
            if monitor.name.is_empty() {
                "(no name)"
            } else {
                &monitor.name
            },
            monitor.maker,
            monitor.product,
            monitor.width,
            monitor.height,
            monitor.x,
            monitor.y,
            monitor.refresh_millihertz / 1000,
            monitor.refresh_millihertz % 1000,
            monitor.scale,
            if monitor.primary { ", primary" } else { "" },
            if monitor.board { ", the board's" } else { "" }
        ));
    }
    match &keyboard.current {
        Some(layout) => lines.push(format!(
            "Keyboard layout in front: {} ({}, {})",
            layout.name, layout.language, layout.id
        )),
        None => lines.push("Keyboard layout in front: unknown".into()),
    }
    lines.push(format!(
        "Keyboard layouts installed: {}",
        keyboard.installed.len()
    ));
    for layout in &keyboard.installed {
        lines.push(format!(
            "  {} ({}, {})",
            layout.name, layout.language, layout.id
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use bli_kvm_agent_protocol::Layout;

    fn monitor(maker: &str, product: u16, x: i32) -> Monitor {
        Monitor {
            name: "DELL U2719D".into(),
            maker: maker.into(),
            product,
            x,
            y: 0,
            width: 2560,
            height: 1440,
            refresh_millihertz: 59_951,
            primary: x == 0,
            scale: 100,
            board: false,
        }
    }

    #[test]
    fn windows_11_is_named_by_its_build() {
        assert_eq!(product_name("Windows 10 Pro", 26100), "Windows 11 Pro");
        assert_eq!(product_name("Windows 10 Pro", 19045), "Windows 10 Pro");
        assert_eq!(
            product_name("Windows Server 2022 Standard", 20348),
            "Windows Server 2022 Standard"
        );
        assert_eq!(product_name(" ", 26100), "Windows");
        assert_eq!(architecture_name(9), "x86_64");
        assert_eq!(architecture_name(12), "aarch64");
        assert_eq!(architecture_name(5), "unknown");
    }

    #[test]
    fn layouts_are_named_by_their_handles() {
        assert_eq!(layout_id(0x0409_0409), "04090409");
        assert_eq!(layout_id(0xf002_0409_usize), "F0020409");
        assert_eq!(
            layout_registry_key(0x0419_0419).as_deref(),
            Some("00000419")
        );
        // A variant (Dvorak) is only in the registry's layout ids.
        assert_eq!(layout_registry_key(0xf002_0409_usize), None);
        assert_eq!(layout_language(0x0809_0409), 0x0409);
    }

    #[test]
    fn the_boards_monitor_is_the_one_with_the_captures_edid() {
        let mut monitors = vec![monitor("DEL", 1, 0), monitor("MSI", 0x2131, 2560)];
        mark_board_monitor(&mut monitors, &[("MSI", 0x2131)]);
        assert_eq!(
            monitors.iter().map(|m| m.board).collect::<Vec<_>>(),
            [false, true]
        );
        let mut none = vec![monitor("DEL", 1, 0)];
        mark_board_monitor(&mut none, BOARD_CAPTURE_EDIDS);
        assert!(!none[0].board);
    }

    #[test]
    fn the_run_keys_command_quotes_the_path() {
        assert_eq!(
            run_command(
                r"C:\Users\Op Erator\AppData\Local\HardenedBliKVM\hardened-blikvm-agent.exe"
            ),
            r#""C:\Users\Op Erator\AppData\Local\HardenedBliKVM\hardened-blikvm-agent.exe""#
        );
        assert_eq!(run_command(r#"C:\a"b.exe"#), r#""C:\ab.exe""#);
    }

    #[test]
    fn the_menu_speaks_russian_on_a_russian_windows() {
        // ru-RU and ru-UA are Russian; en-US and uk-UA are not.
        assert_eq!(words(0x0419).exit, "Выйти");
        assert_eq!(words(0x0819).stop, "Остановить");
        assert_eq!(words(0x0409).exit, "Exit");
        assert_eq!(words(0x0422).exit, "Exit");
    }

    #[test]
    fn what_was_read_is_said_line_by_line() {
        let system = System {
            product: "Windows 11 Pro".into(),
            release: "24H2".into(),
            build: 26100,
            revision: 2033,
            architecture: "x86_64".into(),
            locked: false,
        };
        let mut monitors = vec![monitor("DEL", 0xd0a5, 0), monitor("DEL", 0xd0a5, 2560)];
        monitors[1].board = true;
        let layout = Layout {
            id: "04190419".into(),
            language: "ru-RU".into(),
            name: "Russian (Russia) - Russian".into(),
        };
        let keyboard = Keyboard {
            current: Some(layout.clone()),
            installed: vec![layout],
        };
        let text = describe(&system, &monitors, &keyboard);
        assert_eq!(
            text,
            "Windows 11 Pro 24H2 (build 26100.2033), x86_64\n\
             Monitors: 2\n  \
             DELL U2719D [DEL D0A5]: 2560x1440 at 0,0, 59.951 Hz, 100 %, primary\n  \
             DELL U2719D [DEL D0A5]: 2560x1440 at 2560,0, 59.951 Hz, 100 %, the board's\n\
             Keyboard layout in front: Russian (Russia) - Russian (ru-RU, 04190419)\n\
             Keyboard layouts installed: 1\n  \
             Russian (Russia) - Russian (ru-RU, 04190419)"
        );
    }
}
