//! Reviewed WiFi control through NetworkManager, owned by the board owner.
//!
//! The board has a real `wlan0` and an active NetworkManager. The owner reads
//! the device state with `nmcli` and connects by writing a mode-0600
//! NetworkManager keyfile and bringing that profile up, so the password never
//! reaches a command line, the environment or a log; a profile that fails to
//! come up is removed again.

use std::path::{Path, PathBuf};
use std::process::Command;

use bli_kvm_control_protocol::{ControlRejectionCode, ControlResponse};

pub const NM_CONNECTIONS_DIR: &str = "/etc/NetworkManager/system-connections";
const MAX_SSID_BYTES: usize = 32;
const MAX_PASSWORD_BYTES: usize = 64;
const NMCLI: &str = "/usr/bin/nmcli";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiStatus {
    pub available: bool,
    pub up: bool,
    pub interface: Option<String>,
    pub ssid: Option<String>,
    pub state: String,
}

impl Default for WifiStatus {
    fn default() -> Self {
        Self {
            available: false,
            up: false,
            interface: None,
            ssid: None,
            state: "unavailable".into(),
        }
    }
}

/// Runs the reviewed `nmcli` commands and writes the keyfile. Injected so a
/// test asserts the exact profile bytes and the failure path.
pub trait WifiRunner: Send {
    fn nmcli(&mut self, args: &[&str]) -> Result<String, String>;
    fn write_keyfile(&mut self, name: &str, contents: &str) -> Result<PathBuf, String>;
    fn remove_keyfile(&mut self, path: &Path) -> Result<(), String>;
}

/// The device runner: bounded `nmcli` calls and a mode-0600 keyfile.
pub struct NmcliRunner {
    pub directory: PathBuf,
}

impl NmcliRunner {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }
}

impl WifiRunner for NmcliRunner {
    fn nmcli(&mut self, args: &[&str]) -> Result<String, String> {
        let output = Command::new(NMCLI)
            .args(args)
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn write_keyfile(&mut self, name: &str, contents: &str) -> Result<PathBuf, String> {
        let metadata = std::fs::symlink_metadata(&self.directory)
            .map_err(|error| format!("the NetworkManager directory is unavailable: {error}"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("the NetworkManager directory must be a real directory".into());
        }
        let path = self.directory.join(format!("{name}.nmconnection"));
        std::fs::write(&path, contents)
            .map_err(|error| format!("the WiFi profile could not be written: {error}"))?;
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600))
            .map_err(|error| format!("the WiFi profile mode could not be set: {error}"))?;
        Ok(path)
    }

    fn remove_keyfile(&mut self, path: &Path) -> Result<(), String> {
        std::fs::remove_file(path).map_err(|error| error.to_string())
    }
}

pub struct WifiState {
    runner: Box<dyn WifiRunner>,
    capability_enabled: bool,
    /// The profile the owner created for the active connection, if any.
    active_profile: Option<(String, PathBuf)>,
}

impl WifiState {
    pub fn open(runner: Box<dyn WifiRunner>, capability_enabled: bool) -> Self {
        Self {
            runner,
            capability_enabled,
            active_profile: None,
        }
    }

    pub fn status(&mut self) -> Result<WifiStatus, String> {
        let listing = self
            .runner
            .nmcli(&["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device"])?;
        let mut status = WifiStatus::default();
        for line in listing.lines() {
            let fields: Vec<&str> = line.split(':').collect();
            if fields.len() < 4 || fields[1] != "wifi" {
                continue;
            }
            status.available = true;
            status.interface = Some(fields[0].to_string());
            status.state = fields[2].to_string();
            status.up = fields[2] == "connected";
            if status.up && fields[3] != "--" {
                status.ssid = Some(fields[3].to_string());
            }
            break;
        }
        Ok(status)
    }

    pub fn connect(&mut self, ssid: &str, password: &str) -> ControlResponse {
        if !self.capability_enabled {
            return reject(ControlRejectionCode::CapabilityDisabled);
        }
        if !valid_ssid(ssid) || !valid_password(password) {
            return reject(ControlRejectionCode::InvalidState);
        }
        let name = profile_name(ssid);
        let contents = keyfile(ssid, password);
        let path = match self.runner.write_keyfile(&name, &contents) {
            Ok(path) => path,
            Err(reason) => {
                eprintln!("wifi: profile write failed: {reason}");
                return reject(ControlRejectionCode::BackendRejected);
            }
        };
        if let Err(reason) = self.runner.nmcli(&["con", "up", &name]) {
            eprintln!("wifi: profile activation failed: {reason}");
            // Reversible: a profile that never came up is removed again.
            if let Err(cleanup) = self.runner.remove_keyfile(&path) {
                eprintln!("wifi: profile cleanup failed: {cleanup}");
            }
            return reject(ControlRejectionCode::BackendRejected);
        }
        self.active_profile = Some((name, path));
        ControlResponse::Applied
    }

    pub fn disconnect(&mut self) -> ControlResponse {
        if !self.capability_enabled {
            return reject(ControlRejectionCode::CapabilityDisabled);
        }
        let Some((name, _path)) = self.active_profile.clone() else {
            return reject(ControlRejectionCode::InvalidState);
        };
        match self.runner.nmcli(&["con", "down", &name]) {
            Ok(_) => ControlResponse::Applied,
            Err(reason) => {
                eprintln!("wifi: disconnect failed: {reason}");
                reject(ControlRejectionCode::BackendRejected)
            }
        }
    }
}

fn valid_ssid(ssid: &str) -> bool {
    !ssid.is_empty() && ssid.len() <= MAX_SSID_BYTES && !ssid.chars().any(char::is_control)
}

fn valid_password(password: &str) -> bool {
    (8..=MAX_PASSWORD_BYTES).contains(&password.len()) && !password.chars().any(char::is_control)
}

fn profile_name(ssid: &str) -> String {
    let mut name: String = ssid
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect();
    if name.is_empty() {
        name.push_str("hardened-blikvm-wifi");
    }
    name.truncate(48);
    format!("hardened-blikvm-{name}")
}

fn keyfile(ssid: &str, password: &str) -> String {
    format!(
        "[connection]\nid={ssid}\ntype=wifi\ninterface-name=wlan0\n\n[wifi]\nmode=infrastructure\nssid={ssid}\n\n[wifi-security]\nkey-mgmt=wpa-psk\npsk={password}\n\n[ipv4]\nmethod=auto\n\n[ipv6]\nmethod=auto\n"
    )
}

fn reject(code: ControlRejectionCode) -> ControlResponse {
    ControlResponse::Rejected { code }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct RecordingRunner {
        listing: String,
        fail_up: bool,
        calls: Arc<Mutex<Vec<String>>>,
        written: Arc<Mutex<Vec<(String, String)>>>,
        removed: Arc<Mutex<Vec<PathBuf>>>,
    }

    impl WifiRunner for RecordingRunner {
        fn nmcli(&mut self, args: &[&str]) -> Result<String, String> {
            self.calls.lock().unwrap().push(args.join(" "));
            if self.fail_up && args.first() == Some(&"con") {
                return Err("activation failure".into());
            }
            Ok(self.listing.clone())
        }

        fn write_keyfile(&mut self, name: &str, contents: &str) -> Result<PathBuf, String> {
            self.written
                .lock()
                .unwrap()
                .push((name.to_string(), contents.to_string()));
            Ok(PathBuf::from(format!("/tmp/{name}.nmconnection")))
        }

        fn remove_keyfile(&mut self, path: &Path) -> Result<(), String> {
            self.removed.lock().unwrap().push(path.to_path_buf());
            Ok(())
        }
    }

    type Calls = Arc<Mutex<Vec<String>>>;
    type Written = Arc<Mutex<Vec<(String, String)>>>;
    type Removed = Arc<Mutex<Vec<PathBuf>>>;

    fn fixture(listing: &str, fail_up: bool) -> (WifiState, Calls, Written, Removed) {
        let runner = RecordingRunner {
            listing: listing.into(),
            fail_up,
            ..Default::default()
        };
        let calls = runner.calls.clone();
        let written = runner.written.clone();
        let removed = runner.removed.clone();
        (
            WifiState::open(Box::new(runner), true),
            calls,
            written,
            removed,
        )
    }

    #[test]
    fn reads_the_wifi_device_state() {
        let (mut state, _, _, _) = fixture(
            "eth0:ethernet:connected:Wired connection 1\nwlan0:wifi:connected:HomeNet\nlo:loopback:unmanaged:--\n",
            false,
        );
        let status = state.status().unwrap();
        assert!(status.available);
        assert!(status.up);
        assert_eq!(status.interface.as_deref(), Some("wlan0"));
        assert_eq!(status.ssid.as_deref(), Some("HomeNet"));

        let (mut state, _, _, _) = fixture("eth0:ethernet:connected:Wired\n", false);
        let status = state.status().unwrap();
        assert!(!status.available);
    }

    #[test]
    fn connects_through_a_private_keyfile_and_fails_closed() {
        let (mut state, calls, written, _removed) = fixture("wlan0:wifi:disconnected:--\n", false);
        assert_eq!(
            state.connect("HomeNet", "secret-password"),
            ControlResponse::Applied
        );
        let written = written.lock().unwrap();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].0, "hardened-blikvm-HomeNet");
        assert!(written[0].1.contains("psk=secret-password"));
        assert!(written[0].1.contains("ssid=HomeNet"));
        drop(written);
        assert!(
            calls
                .lock()
                .unwrap()
                .contains(&"con up hardened-blikvm-HomeNet".to_string())
        );

        // A profile that cannot come up is removed again, and the password is
        // never part of the command line.
        let (mut failing, calls, _, removed) = fixture("wlan0:wifi:disconnected:--\n", true);
        assert_eq!(
            failing.connect("HomeNet", "secret-password"),
            ControlResponse::Rejected {
                code: ControlRejectionCode::BackendRejected
            }
        );
        assert_eq!(removed.lock().unwrap().len(), 1);
        assert!(
            calls
                .lock()
                .unwrap()
                .iter()
                .all(|call| !call.contains("secret-password"))
        );
    }

    #[test]
    fn refuses_bad_input_and_disabled_capability() {
        let (mut state, _, _, _) = fixture("wlan0:wifi:disconnected:--\n", false);
        assert_eq!(
            state.connect("", "secret-password"),
            ControlResponse::Rejected {
                code: ControlRejectionCode::InvalidState
            }
        );
        assert_eq!(
            state.connect("HomeNet", "short"),
            ControlResponse::Rejected {
                code: ControlRejectionCode::InvalidState
            }
        );
        let runner = RecordingRunner::default();
        let mut disabled = WifiState::open(Box::new(runner), false);
        assert_eq!(
            disabled.connect("HomeNet", "secret-password"),
            ControlResponse::Rejected {
                code: ControlRejectionCode::CapabilityDisabled
            }
        );
    }

    #[test]
    fn disconnect_needs_a_profile_the_owner_created() {
        let (mut state, calls, _, _) = fixture("wlan0:wifi:connected:HomeNet\n", false);
        assert_eq!(
            state.disconnect(),
            ControlResponse::Rejected {
                code: ControlRejectionCode::InvalidState
            }
        );
        assert_eq!(
            state.connect("HomeNet", "secret-password"),
            ControlResponse::Applied
        );
        assert_eq!(state.disconnect(), ControlResponse::Applied);
        assert!(
            calls
                .lock()
                .unwrap()
                .contains(&"con down hardened-blikvm-HomeNet".to_string())
        );
    }
}
