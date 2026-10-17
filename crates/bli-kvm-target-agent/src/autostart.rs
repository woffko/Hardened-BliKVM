//! "Start at sign-in", at the user's choice in the icon's menu: the program
//! is copied to the user's own folder (the agent's drive may get another
//! letter, or be gone while the board's switch is off) and named in the
//! user's Run key, which Windows starts at every sign-in; choosing it again
//! takes the key away. No administrator rights are needed or used.
//! Windows' calls are this module's unsafe code.

use std::path::{Path, PathBuf};
use std::ptr::null_mut;
use std::time::Duration;

use bli_kvm_target_agent::{names, wide};
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// Where the program is kept for the sign-in start.
fn kept_program() -> Option<PathBuf> {
    let folder = std::env::var_os("LOCALAPPDATA")?;
    Some(
        PathBuf::from(folder)
            .join(names::AUTOSTART_FOLDER)
            .join(names::AUTOSTART_PROGRAM),
    )
}

/// Whether two paths name one file.
fn same_file(one: &Path, other: &Path) -> bool {
    match (std::fs::canonicalize(one), std::fs::canonicalize(other)) {
        (Ok(one), Ok(other)) => one == other,
        _ => false,
    }
}

/// Whether the running program is the copy the sign-in start keeps.
pub fn runs_from_kept_copy() -> bool {
    match (std::env::current_exe(), kept_program()) {
        (Ok(running), Some(kept)) => same_file(&running, &kept),
        _ => false,
    }
}

/// Whether the user's Run key names the agent.
pub fn enabled() -> bool {
    let key = wide(RUN_KEY, 128);
    let value = wide(names::AUTOSTART_VALUE, 64);
    let mut bytes = 0_u32;
    // SAFETY: zero-ended names; asking for the size only.
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            null_mut(),
            &mut bytes,
        ) == ERROR_SUCCESS
    }
}

/// Starts the agent at sign-in: the running program copied to the user's
/// folder (unless it runs from there) and named in the Run key. The copy
/// replaces an older one, which may take a moment to be let go of by an
/// agent that has just left.
pub fn enable() -> Result<(), String> {
    let running = std::env::current_exe().map_err(|error| error.to_string())?;
    let kept = kept_program().ok_or("the user's local application folder is not known")?;
    if !same_file(&running, &kept) {
        if let Some(folder) = kept.parent() {
            std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
        }
        let mut copied = std::fs::copy(&running, &kept);
        for _ in 0..10 {
            if copied.is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
            copied = std::fs::copy(&running, &kept);
        }
        copied.map_err(|error| error.to_string())?;
    }
    let command = wide(&names::run_command(&kept.to_string_lossy()), 1024);
    let key = wide(RUN_KEY, 128);
    let value = wide(names::AUTOSTART_VALUE, 64);
    // SAFETY: zero-ended names and the command's bytes with its zero.
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            REG_SZ,
            command.as_ptr().cast(),
            (command.len() * 2) as u32,
        )
    };
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("the Run key was not written ({status})"))
    }
}

/// Stops starting the agent at sign-in: the Run key's value goes; the kept
/// program stays (it may be the one running).
pub fn disable() -> Result<(), String> {
    let key = wide(RUN_KEY, 128);
    let value = wide(names::AUTOSTART_VALUE, 64);
    // SAFETY: zero-ended names.
    let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) };
    if status == ERROR_SUCCESS || !enabled() {
        Ok(())
    } else {
        Err(format!("the Run key was not changed ({status})"))
    }
}
