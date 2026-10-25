//! The agent's program. On Windows it shows itself in the notification area
//! and runs until the user quits it; elsewhere it says it runs on Windows.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod actions;
#[cfg(windows)]
mod autostart;
#[cfg(windows)]
mod channel;
#[cfg(windows)]
mod hid;
#[cfg(windows)]
mod reads;
#[cfg(windows)]
mod tray;

#[cfg(windows)]
fn main() {
    // DLLs loaded later come from the system folder only, never from the
    // folder the agent runs from (security review of 2026-10-18, HIGH-08).
    // SAFETY: a process-wide setting with a documented flag.
    unsafe {
        windows_sys::Win32::System::LibraryLoader::SetDefaultDllDirectories(
            windows_sys::Win32::System::LibraryLoader::LOAD_LIBRARY_SEARCH_SYSTEM32,
        );
    }
    reads::aware_of_each_monitors_scale();
    std::process::exit(tray::run());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("{} runs on Windows", bli_kvm_target_agent::title());
    std::process::exit(2);
}
