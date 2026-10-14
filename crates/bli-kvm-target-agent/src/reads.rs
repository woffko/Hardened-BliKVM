//! What the agent reads on the target's Windows, with the user's rights:
//! the system and session, the active monitors (`QueryDisplayConfig`) and the
//! keyboard layouts. Names and limits are the portable `names` module's and
//! the protocol's; Windows' calls are this module's unsafe code.

use std::mem::{size_of, zeroed};
use std::ptr::null_mut;

use bli_kvm_agent_protocol::{Keyboard, Layout, Monitor, System};
use bli_kvm_target_agent::names;
use windows_sys::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_MODE_INFO,
    DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_TARGET_DEVICE_NAME,
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS,
    QueryDisplayConfig,
};
use windows_sys::Win32::Foundation::{ERROR_SUCCESS, POINT};
use windows_sys::Win32::Globalization::{
    GetLocaleInfoEx, LCIDToLocaleName, LOCALE_SLOCALIZEDDISPLAYNAME,
};
use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint};
use windows_sys::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};
use windows_sys::Win32::System::StationsAndDesktops::{
    CloseDesktop, DESKTOP_READOBJECTS, GetUserObjectInformationW, OpenInputDesktop, UOI_NAME,
};
use windows_sys::Win32::System::SystemInformation::{GetNativeSystemInfo, SYSTEM_INFO};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI,
    SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, GetKeyboardLayoutList};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use bli_kvm_target_agent::wide;

/// Text from UTF-16 up to its first zero.
fn text(units: &[u16]) -> String {
    let length = units
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(units.len());
    String::from_utf16_lossy(&units[..length])
}

/// The process reads each monitor's own scale: called once, first.
pub fn aware_of_each_monitors_scale() {
    // SAFETY: a documented constant; failing leaves the process as it was.
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

fn registry_text(key: &str, value: &str) -> Option<String> {
    let key = wide(key, 512);
    let value = wide(value, 128);
    let mut buffer = [0_u16; 256];
    let mut bytes = (buffer.len() * 2) as u32;
    // SAFETY: zero-ended names, and the buffer's size in bytes with it.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    (status == ERROR_SUCCESS).then(|| text(&buffer))
}

fn registry_number(key: &str, value: &str) -> Option<u32> {
    let key = wide(key, 512);
    let value = wide(value, 128);
    let mut number = 0_u32;
    let mut bytes = size_of::<u32>() as u32;
    // SAFETY: zero-ended names, and the number's size with it.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&mut number as *mut u32).cast(),
            &mut bytes,
        )
    };
    (status == ERROR_SUCCESS).then_some(number)
}

/// Whether the session is locked: the desktop in front is not the user's.
fn locked() -> bool {
    // SAFETY: the desktop opened is closed; the name's buffer goes with its
    // size in bytes.
    unsafe {
        let desktop = OpenInputDesktop(0, 0, DESKTOP_READOBJECTS);
        if desktop.is_null() {
            // The sign-in desktop cannot be opened by the user.
            return true;
        }
        let mut name = [0_u16; 64];
        let mut needed = 0_u32;
        let read = GetUserObjectInformationW(
            desktop,
            UOI_NAME,
            name.as_mut_ptr().cast(),
            (name.len() * 2) as u32,
            &mut needed,
        );
        CloseDesktop(desktop);
        read == 0 || !text(&name).eq_ignore_ascii_case("Default")
    }
}

pub fn system() -> System {
    const VERSION_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let build: u32 = registry_text(VERSION_KEY, "CurrentBuildNumber")
        .and_then(|build| build.trim().parse().ok())
        .unwrap_or(0);
    // SAFETY: the structure is the call's to fill.
    let architecture = unsafe {
        let mut info: SYSTEM_INFO = zeroed();
        GetNativeSystemInfo(&mut info);
        info.Anonymous.Anonymous.wProcessorArchitecture
    };
    let clip = |value: String| -> String {
        value
            .chars()
            .filter(|letter| !letter.is_control())
            .take(bli_kvm_agent_protocol::reads::MAX_TEXT)
            .collect()
    };
    System {
        product: clip(names::product_name(
            &registry_text(VERSION_KEY, "ProductName").unwrap_or_default(),
            build,
        )),
        release: clip(registry_text(VERSION_KEY, "DisplayVersion").unwrap_or_default()),
        build,
        revision: registry_number(VERSION_KEY, "UBR").unwrap_or(0),
        architecture: names::architecture_name(architecture).into(),
        locked: locked(),
    }
}

/// The active display configuration: its paths and their modes.
pub struct DisplayConfig {
    pub paths: Vec<DISPLAYCONFIG_PATH_INFO>,
    pub modes: Vec<DISPLAYCONFIG_MODE_INFO>,
}

/// The active paths and modes, as `QueryDisplayConfig` gives them.
pub fn active_config() -> Option<DisplayConfig> {
    // SAFETY: the arrays have the sizes the first call gave, and the second
    // says how many it filled.
    unsafe {
        let (mut path_count, mut mode_count) = (0_u32, 0_u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
            != ERROR_SUCCESS
        {
            return None;
        }
        let mut paths: Vec<DISPLAYCONFIG_PATH_INFO> = vec![zeroed(); path_count as usize];
        let mut modes: Vec<DISPLAYCONFIG_MODE_INFO> = vec![zeroed(); mode_count as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            paths.as_mut_ptr(),
            &mut mode_count,
            modes.as_mut_ptr(),
            null_mut(),
        ) != ERROR_SUCCESS
        {
            return None;
        }
        paths.truncate(path_count as usize);
        modes.truncate(mode_count as usize);
        Some(DisplayConfig { paths, modes })
    }
}

/// A path's monitor as its EDID names it: its name, maker and product, when
/// Windows has them.
pub struct TargetName {
    pub name: String,
    pub maker: String,
    pub product: u16,
}

pub fn target_name(path: &DISPLAYCONFIG_PATH_INFO) -> TargetName {
    // SAFETY: the request's header is filled from the path it names, with
    // the request's own type and size.
    unsafe {
        let mut name: DISPLAYCONFIG_TARGET_DEVICE_NAME = zeroed();
        name.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
        name.header.size = size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
        name.header.adapterId = path.targetInfo.adapterId;
        name.header.id = path.targetInfo.id;
        let named = DisplayConfigGetDeviceInfo(&mut name.header) == 0;
        let edid = named && name.flags.Anonymous.value & 0b100 != 0;
        TargetName {
            name: if named {
                text(&name.monitorFriendlyDeviceName)
                    .chars()
                    .filter(|letter| !letter.is_control())
                    .take(bli_kvm_agent_protocol::reads::MAX_TEXT)
                    .collect()
            } else {
                String::new()
            },
            maker: if edid {
                bli_kvm_agent_protocol::reads::edid_maker(name.edidManufactureId.to_le_bytes())
            } else {
                String::new()
            },
            product: if edid { name.edidProductCodeId } else { 0 },
        }
    }
}

/// Whether a path's monitor is the board's capture, by its EDID.
pub fn is_board(path: &DISPLAYCONFIG_PATH_INFO) -> bool {
    let target = target_name(path);
    names::BOARD_CAPTURE_EDIDS
        .iter()
        .any(|(maker, product)| target.maker == *maker && target.product == *product)
}

pub fn monitors() -> Vec<Monitor> {
    let Some(config) = active_config() else {
        return Vec::new();
    };
    let mut monitors = Vec::new();
    for path in &config.paths {
        // SAFETY: the union is the index while the configuration is not
        // queried as virtual-mode aware; the mode is read only when it is a
        // source's.
        let source = unsafe {
            let index = path.sourceInfo.Anonymous.modeInfoIdx as usize;
            match config.modes.get(index) {
                Some(mode) if mode.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE => {
                    mode.Anonymous.sourceMode
                }
                _ => continue,
            }
        };
        let target = target_name(path);
        let rate = path.targetInfo.refreshRate;
        let refresh = if rate.Denominator == 0 {
            0
        } else {
            (u64::from(rate.Numerator) * 1000 / u64::from(rate.Denominator)) as u32
        };
        let (x, y) = (source.position.x, source.position.y);
        let centre = POINT {
            x: x + (source.width / 2) as i32,
            y: y + (source.height / 2) as i32,
        };
        let (mut dpi_x, mut dpi_y) = (96_u32, 96_u32);
        // SAFETY: a point and the out values of the monitor's DPI.
        unsafe {
            let monitor = MonitorFromPoint(centre, MONITOR_DEFAULTTONEAREST);
            if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) != 0 {
                dpi_x = 96;
            }
        }
        monitors.push(Monitor {
            name: target.name,
            maker: target.maker,
            product: target.product,
            x,
            y,
            width: source.width,
            height: source.height,
            refresh_millihertz: refresh.min(1_000_000),
            primary: x == 0 && y == 0,
            scale: (dpi_x * 100 / 96).clamp(100, 500),
            board: false,
        });
    }
    names::mark_board_monitor(&mut monitors, names::BOARD_CAPTURE_EDIDS);
    monitors.truncate(bli_kvm_agent_protocol::reads::MAX_MONITORS);
    monitors
}

/// A layout's language tag, what the target calls it, and the layout's
/// own name from the registry when the handle names it.
fn layout(handle: usize) -> Layout {
    let language = names::layout_language(handle);
    let mut tag = [0_u16; 86];
    // SAFETY: the buffer's length in characters goes with it.
    let tag_length =
        unsafe { LCIDToLocaleName(u32::from(language), tag.as_mut_ptr(), tag.len() as i32, 0) };
    let tag_text = if tag_length > 0 {
        text(&tag)
    } else {
        String::new()
    };
    let mut display = [0_u16; 128];
    // SAFETY: a zero-ended locale name, and the buffer's length with it.
    let display_length = if tag_length > 0 {
        unsafe {
            GetLocaleInfoEx(
                tag.as_ptr(),
                LOCALE_SLOCALIZEDDISPLAYNAME,
                display.as_mut_ptr(),
                display.len() as i32,
            )
        }
    } else {
        0
    };
    let language_name = if display_length > 0 {
        text(&display)
    } else {
        tag_text.clone()
    };
    let layout_name = names::layout_registry_key(handle).and_then(|key| {
        registry_text(
            &format!(r"SYSTEM\CurrentControlSet\Control\Keyboard Layouts\{key}"),
            "Layout Text",
        )
    });
    let name: String = match layout_name {
        Some(layout) if !layout.is_empty() => format!("{language_name} - {layout}"),
        _ => language_name,
    };
    Layout {
        id: names::layout_id(handle),
        language: tag_text
            .chars()
            .filter(|letter| letter.is_ascii_alphanumeric() || *letter == '-')
            .collect(),
        name: name
            .chars()
            .filter(|letter| !letter.is_control())
            .take(bli_kvm_agent_protocol::reads::MAX_TEXT)
            .collect(),
    }
}

pub fn keyboard() -> Keyboard {
    // SAFETY: the window in front may be none (its thread is then 0, the
    // caller's own layout); the list's buffer has the size the first call
    // gave.
    unsafe {
        let window = GetForegroundWindow();
        let thread = if window.is_null() {
            0
        } else {
            GetWindowThreadProcessId(window, null_mut())
        };
        let current = GetKeyboardLayout(thread);
        let count = GetKeyboardLayoutList(0, null_mut());
        let mut handles = vec![null_mut(); count.max(0) as usize];
        let filled = GetKeyboardLayoutList(count, handles.as_mut_ptr());
        handles.truncate(filled.max(0) as usize);
        handles.truncate(bli_kvm_agent_protocol::reads::MAX_LAYOUTS);
        Keyboard {
            current: (!current.is_null()).then(|| layout(current as usize)),
            installed: handles
                .into_iter()
                .map(|handle| layout(handle as usize))
                .collect(),
        }
    }
}
