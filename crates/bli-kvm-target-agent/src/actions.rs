//! What the agent does on the target's Windows when the board asks, each at
//! the operator's action from the page and under the operator's HID lease:
//! the keyboard layout of the window in front, the monitors' mode (undone by
//! itself after 15 s unless kept, as Windows' own "Keep changes" is), the
//! clipboard's text both ways, locking the session, and keeping the
//! operator's choice of the board's monitor. Windows' calls are this
//! module's unsafe code.

use std::ptr::{null, null_mut};
use std::time::Duration;

use bli_kvm_agent_protocol::{DISPLAYS_REVERT_SECONDS, DisplayMode};
use bli_kvm_target_agent::revert::Revert;
use bli_kvm_target_agent::{names, wide};
use windows_sys::Win32::Devices::Display::{
    SDC_ALLOW_CHANGES, SDC_APPLY, SDC_SAVE_TO_DATABASE, SDC_TOPOLOGY_CLONE, SDC_TOPOLOGY_EXTEND,
    SDC_USE_SUPPLIED_DISPLAY_CONFIG, SetDisplayConfig,
};
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, GlobalFree, HWND};
use windows_sys::Win32::Graphics::Gdi::{
    DISPLAYCONFIG_PATH_ACTIVE, DISPLAYCONFIG_PATH_MODE_IDX_INVALID,
};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW,
};
use windows_sys::Win32::System::Shutdown::LockWorkStation;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayoutList;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, PostMessageW, WM_INPUTLANGCHANGEREQUEST,
};

use crate::reads::{self, DisplayConfig};

/// The clipboard's format of UTF-16 text.
const UNICODE_TEXT: u32 = 13;

pub fn set_layout(id: &str) -> Result<(), String> {
    // SAFETY: the list's buffer has the size the first call gave; the
    // window in front is asked, not touched.
    unsafe {
        let count = GetKeyboardLayoutList(0, null_mut());
        let mut handles = vec![null_mut(); count.max(0) as usize];
        let filled = GetKeyboardLayoutList(count, handles.as_mut_ptr());
        handles.truncate(filled.max(0) as usize);
        let Some(handle) = handles
            .into_iter()
            .find(|handle| names::layout_id(*handle as usize).eq_ignore_ascii_case(id))
        else {
            return Err("the layout is not installed".into());
        };
        let window = GetForegroundWindow();
        if window.is_null() {
            return Err("no window is in front".into());
        }
        if PostMessageW(window, WM_INPUTLANGCHANGEREQUEST, 0, handle as isize) == 0 {
            return Err("the window in front was not asked".into());
        }
        Ok(())
    }
}

/// Keeps the operator's choice of the board's monitor for the user, or
/// forgets it; the reads mark that monitor from then on.
pub fn set_board_monitor(id: Option<&str>) -> Result<(), String> {
    let key = wide(names::SETTINGS_KEY, 128);
    let value = wide(names::BOARD_MONITOR_VALUE, 64);
    let status = match id {
        Some(id) => {
            if !bli_kvm_agent_protocol::reads::monitor_id_is_valid(id) {
                return Err("the monitor's id is out of its bounds".into());
            }
            let data = wide(id, 32);
            // SAFETY: zero-ended names and the id's bytes with its zero.
            unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    key.as_ptr(),
                    value.as_ptr(),
                    REG_SZ,
                    data.as_ptr().cast(),
                    (data.len() * 2) as u32,
                )
            }
        }
        // SAFETY: zero-ended names.
        None => {
            match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) } {
                ERROR_FILE_NOT_FOUND => ERROR_SUCCESS,
                status => status,
            }
        }
    };
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("the choice was not kept ({status})"))
    }
}

pub fn lock() -> Result<(), String> {
    // SAFETY: no arguments.
    if unsafe { LockWorkStation() } == 0 {
        Err("the session was not locked".into())
    } else {
        Ok(())
    }
}

/// Opens the clipboard, trying a few times while another program has it.
fn open_clipboard(owner: HWND) -> Result<(), String> {
    for _ in 0..10 {
        // SAFETY: the owner is the icon's window, or none for a read.
        if unsafe { OpenClipboard(owner) } != 0 {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    Err("another program holds the clipboard".into())
}

/// The clipboard's text (`names::clipboard_text`), read no further than its
/// block: another program may have put text there without a zero.
pub fn read_clipboard() -> Result<String, String> {
    open_clipboard(null_mut())?;
    // SAFETY: the clipboard is open; the text's memory is locked while it
    // is read, within the size Windows gives for the block, and the
    // clipboard is closed on every way out.
    let text = unsafe {
        let handle = GetClipboardData(UNICODE_TEXT);
        let mut text = String::new();
        if !handle.is_null() {
            let pointer = GlobalLock(handle) as *const u16;
            if !pointer.is_null() {
                let units = GlobalSize(handle) / 2;
                text = names::clipboard_text(std::slice::from_raw_parts(pointer, units));
                GlobalUnlock(handle);
            }
        }
        CloseClipboard();
        text
    };
    Ok(text)
}

/// Puts `text` on the clipboard, owned by the icon's window.
pub fn write_clipboard(owner: HWND, text: &str) -> Result<(), String> {
    if !bli_kvm_agent_protocol::clipboard_text_is_valid(text) {
        return Err("the text is out of its bounds".into());
    }
    let mut units: Vec<u16> = text.encode_utf16().collect();
    units.push(0);
    open_clipboard(owner)?;
    // SAFETY: the clipboard is open; the memory is allocated for the units
    // and their zero, filled while locked, and given to the clipboard, which
    // owns it from then on (it is freed here only when it was not taken).
    unsafe {
        let placed = EmptyClipboard() != 0 && {
            let memory = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2);
            if memory.is_null() {
                false
            } else {
                let pointer = GlobalLock(memory) as *mut u16;
                if pointer.is_null() {
                    GlobalFree(memory);
                    false
                } else {
                    std::ptr::copy_nonoverlapping(units.as_ptr(), pointer, units.len());
                    GlobalUnlock(memory);
                    if SetClipboardData(UNICODE_TEXT, memory).is_null() {
                        GlobalFree(memory);
                        false
                    } else {
                        true
                    }
                }
            }
        };
        CloseClipboard();
        if placed {
            Ok(())
        } else {
            Err("the clipboard did not take the text".into())
        }
    }
}

/// The monitors' changes that wait to be kept or put back.
static DISPLAYS: Revert<DisplayConfig> = Revert::new();

fn apply(config: &DisplayConfig, flags: u32) -> Result<(), String> {
    // SAFETY: the arrays are a configuration Windows gave, changed only in
    // the paths' flags and indexes and the sources' places.
    let status = unsafe {
        SetDisplayConfig(
            config.paths.len() as u32,
            config.paths.as_ptr(),
            config.modes.len() as u32,
            config.modes.as_ptr(),
            flags | SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(format!(
            "Windows refused the monitors' configuration ({status})"
        ))
    }
}

fn topology(flag: u32) -> Result<(), String> {
    // SAFETY: a topology alone, with no arrays.
    let status = unsafe { SetDisplayConfig(0, null(), 0, null(), SDC_APPLY | flag) };
    if status == 0 {
        Ok(())
    } else {
        Err(format!("Windows refused the monitors' mode ({status})"))
    }
}

/// The active configuration with only the paths `keep` takes left active.
fn only(mut config: DisplayConfig, keep: impl Fn(bool) -> bool) -> Result<DisplayConfig, String> {
    let boards: Vec<bool> = config.paths.iter().map(reads::is_board).collect();
    if !boards.contains(&true) {
        return Err("the board's monitor is not known among the target's".into());
    }
    if !boards.iter().any(|board| keep(*board)) {
        return Err("no monitor would be left".into());
    }
    for (path, board) in config.paths.iter_mut().zip(boards) {
        if !keep(board) {
            path.flags &= !DISPLAYCONFIG_PATH_ACTIVE;
            path.sourceInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
            path.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
        }
    }
    // The desktop begins at its primary monitor: the monitors left are moved
    // so that the top left one is at the origin, as Windows' own "show only
    // on" does. Windows may refuse a desktop with no monitor there.
    let mut sources: Vec<usize> = config
        .paths
        .iter()
        .filter(|path| path.flags & DISPLAYCONFIG_PATH_ACTIVE != 0)
        // SAFETY: the union is the index of a source mode in this
        // configuration.
        .map(|path| unsafe { path.sourceInfo.Anonymous.modeInfoIdx } as usize)
        .filter(|index| {
            config.modes.get(*index).is_some_and(|mode| {
                mode.infoType
                    == windows_sys::Win32::Devices::Display::DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE
            })
        })
        .collect();
    // Duplicated monitors share one source, moved once.
    sources.sort_unstable();
    sources.dedup();
    let origin = names::least_corner(sources.iter().map(|index| {
        // SAFETY: each index is a source mode's, as filtered above.
        let place = unsafe { config.modes[*index].Anonymous.sourceMode.position };
        (place.x, place.y)
    }));
    if let Some((x, y)) = origin {
        for index in sources {
            // SAFETY: as above.
            unsafe {
                config.modes[index].Anonymous.sourceMode.position.x -= x;
                config.modes[index].Anonymous.sourceMode.position.y -= y;
            }
        }
    }
    Ok(config)
}

/// The active configuration moved so that the board's monitor is at the
/// desktop's origin, the primary one.
fn board_primary(mut config: DisplayConfig) -> Result<DisplayConfig, String> {
    let Some(path) = config.paths.iter().find(|path| reads::is_board(path)) else {
        return Err("the board's monitor is not known among the target's".into());
    };
    // SAFETY: the union is the index of a source mode in this configuration.
    let index = unsafe { path.sourceInfo.Anonymous.modeInfoIdx } as usize;
    // SAFETY: the mode at a source index is a source mode.
    let origin = unsafe {
        config
            .modes
            .get(index)
            .ok_or("the board's monitor has no source")?
            .Anonymous
            .sourceMode
            .position
    };
    for mode in &mut config.modes {
        if mode.infoType
            == windows_sys::Win32::Devices::Display::DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE
        {
            // SAFETY: the mode is a source's.
            unsafe {
                mode.Anonymous.sourceMode.position.x -= origin.x;
                mode.Anonymous.sourceMode.position.y -= origin.y;
            }
        }
    }
    Ok(config)
}

/// Leaves active only the monitors `keep` takes: the board's or the
/// others. When none of them is active (only the board's shows and the
/// others are wanted, or the reverse), every monitor is first made active
/// as Windows extends them, so that the one wanted can be kept; if keeping
/// it then fails, `before` is put back at once.
fn only_after_extending(before: &DisplayConfig, keep: impl Fn(bool) -> bool) -> Result<(), String> {
    let read = || reads::active_config().ok_or("the monitors' configuration was not read");
    let mut current = read()?;
    let extended = !current.paths.iter().any(|path| keep(reads::is_board(path)));
    if extended {
        topology(SDC_TOPOLOGY_EXTEND)?;
        current = read()?;
    }
    let kept = only(current, keep).and_then(|config| apply(&config, 0));
    if kept.is_err() && extended {
        let _ = apply(before, 0);
    }
    kept
}

/// Changes the monitors' mode and puts the configuration before it back
/// after `DISPLAYS_REVERT_SECONDS` unless `keep_displays` comes first. A
/// second change before the first is kept puts back the configuration
/// before the first (`Revert`, whose lock the change and its timer hold).
pub fn set_displays(mode: DisplayMode) -> Result<u32, String> {
    let change = DISPLAYS.change(|| {
        let before = reads::active_config().ok_or("the monitors' configuration was not read")?;
        match mode {
            DisplayMode::Extend => topology(SDC_TOPOLOGY_EXTEND)?,
            DisplayMode::Duplicate => topology(SDC_TOPOLOGY_CLONE)?,
            DisplayMode::BoardOnly => only_after_extending(&before, |board| board)?,
            DisplayMode::OthersOnly => only_after_extending(&before, |board| !board)?,
            DisplayMode::BoardPrimary => {
                let current =
                    reads::active_config().ok_or("the monitors' configuration was not read")?;
                apply(&board_primary(current)?, 0)?;
            }
        }
        Ok(before)
    })?;
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(u64::from(DISPLAYS_REVERT_SECONDS)));
        DISPLAYS.due(change, |config| {
            let _ = apply(&config, SDC_SAVE_TO_DATABASE);
        });
    });
    Ok(DISPLAYS_REVERT_SECONDS)
}

/// Keeps the monitors' change: nothing is put back, and the configuration
/// is saved as Windows' own. With no change waiting it fails, and a change
/// not saved is still put back (`Revert::keep`).
pub fn keep_displays() -> Result<(), String> {
    DISPLAYS.keep(|| {
        let current = reads::active_config().ok_or("the monitors' configuration was not read")?;
        apply(&current, SDC_SAVE_TO_DATABASE)
    })
}
