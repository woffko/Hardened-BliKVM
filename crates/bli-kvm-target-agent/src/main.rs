//! The agent's program. On Windows it shows itself in the notification area
//! and runs until the user quits it; elsewhere it says it runs on Windows.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod tray;

#[cfg(windows)]
fn main() {
    std::process::exit(tray::run());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("{} runs on Windows", bli_kvm_target_agent::title());
    std::process::exit(2);
}
