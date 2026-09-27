//! Reviewed WiFi control through NetworkManager, owned by the board owner.
//!
//! The board has a real `wlan0` and an active NetworkManager. The owner reads
//! the device, the scan list and the saved profiles with `nmcli`, and
//! connects by writing a mode-0600 NetworkManager keyfile, loading it and
//! bringing that profile up, so a password never reaches a command line, the
//! environment or a log; a profile that fails to come up is removed again.
//! Every request runs outside the owner's shared state (see `server.rs`), so
//! a slow association never holds HID or media requests.

use std::path::{Path, PathBuf};
use std::process::Command;

use bli_kvm_control_protocol::{
    ControlRejectionCode, ControlResponse, WifiNetwork, WifiProfileView,
};
use bli_kvm_core::{
    MAX_WIFI_NETWORKS, MAX_WIFI_PROFILES, PlatformControlRequestV2, WifiProfileAction,
    valid_wifi_password, valid_wifi_profile_id, valid_wifi_ssid,
};

pub const NM_CONNECTIONS_DIR: &str = "/etc/NetworkManager/system-connections";
const NMCLI: &str = "/usr/bin/nmcli";
/// The board's WiFi device; profiles are bound to it.
const DEVICE: &str = "wlan0";
/// Profiles this owner creates; saved profiles of other origins are listed
/// and can be used or forgotten as well.
const PROFILE_PREFIX: &str = "hardened-blikvm-wifi-";
/// How long NetworkManager may take to associate and obtain an address.
const ACTIVATION_SECONDS: &str = "30";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiStatus {
    pub available: bool,
    pub up: bool,
    pub interface: Option<String>,
    pub ssid: Option<String>,
    pub state: String,
    pub radio: bool,
    pub signal: Option<u8>,
    pub security: Option<String>,
    pub address: Option<String>,
}

impl Default for WifiStatus {
    fn default() -> Self {
        Self {
            available: false,
            up: false,
            interface: None,
            ssid: None,
            state: "unavailable".into(),
            radio: false,
            signal: None,
            security: None,
            address: None,
        }
    }
}

/// Runs the reviewed `nmcli` commands and writes the keyfile. Injected so a
/// test asserts the exact profile bytes and the failure path.
pub trait WifiRunner: Send {
    fn nmcli(&mut self, args: &[&str]) -> Result<String, String>;
    fn write_keyfile(&mut self, name: &str, contents: &str) -> Result<PathBuf, String>;
    fn remove_keyfile(&mut self, path: &Path) -> Result<(), String>;
    /// A fresh profile UUID from the kernel.
    fn new_uuid(&mut self) -> Result<String, String> {
        let uuid = std::fs::read_to_string("/proc/sys/kernel/random/uuid")
            .map_err(|error| error.to_string())?;
        Ok(uuid.trim().to_string())
    }
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
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn write_keyfile(&mut self, name: &str, contents: &str) -> Result<PathBuf, String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let metadata = std::fs::symlink_metadata(&self.directory)
            .map_err(|error| format!("the NetworkManager directory is unavailable: {error}"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("the NetworkManager directory must be a real directory".into());
        }
        let path = self.directory.join(format!("{name}.nmconnection"));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .map_err(|error| format!("the WiFi profile could not be created: {error}"))?;
        file.write_all(contents.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("the WiFi profile could not be written: {error}"))?;
        Ok(path)
    }

    fn remove_keyfile(&mut self, path: &Path) -> Result<(), String> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}

/// The WiFi state behind its own lock, served on a blocking worker.
pub type SharedWifi = std::sync::Arc<std::sync::Mutex<WifiState>>;

pub struct WifiState {
    runner: Box<dyn WifiRunner>,
    capability_enabled: bool,
}

/// Splits one terse `nmcli` line, where `:` and `\` inside a value are
/// escaped with a backslash.
fn terse_fields(line: &str) -> Vec<String> {
    let mut fields = vec![String::new()];
    let mut escaped = false;
    for character in line.chars() {
        if escaped {
            fields.last_mut().expect("one field").push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == ':' {
            fields.push(String::new());
        } else {
            fields.last_mut().expect("one field").push(character);
        }
    }
    fields
}

/// The security label of an `nmcli` SECURITY value: `open`, `wpa2` (WPA1 and
/// WPA2 personal), `wpa3`, `wpa2_wpa3`, `wep`, `enterprise`, or `other` (OWE
/// and anything unknown), which cannot be joined here.
fn security_label(security: &str) -> &'static str {
    let security = security.trim();
    let has = |label: &str| security.split_whitespace().any(|token| token == label);
    if security.is_empty() || security == "--" {
        "open"
    } else if has("802.1X") {
        "enterprise"
    } else if has("WPA3") && (has("WPA2") || has("WPA1")) {
        "wpa2_wpa3"
    } else if has("WPA3") {
        "wpa3"
    } else if has("WPA2") || has("WPA1") {
        "wpa2"
    } else if has("WEP") {
        "wep"
    } else {
        "other"
    }
}

impl WifiState {
    pub fn open(runner: Box<dyn WifiRunner>, capability_enabled: bool) -> Self {
        Self {
            runner,
            capability_enabled,
        }
    }

    pub fn dispatch(&mut self, request: &PlatformControlRequestV2) -> ControlResponse {
        match request {
            PlatformControlRequestV2::WifiState {} => match self.status() {
                Ok(status) => ControlResponse::WifiState {
                    available: status.available,
                    up: status.up,
                    interface: status.interface,
                    ssid: status.ssid,
                    state: status.state,
                    radio: status.radio,
                    signal: status.signal,
                    security: status.security,
                    address: status.address,
                },
                Err(reason) => {
                    eprintln!("wifi: state read failed: {reason}");
                    reject(ControlRejectionCode::BackendRejected)
                }
            },
            PlatformControlRequestV2::WifiScan { rescan } => match self.scan(*rescan) {
                Ok(networks) => ControlResponse::WifiNetworks { networks },
                Err(reason) => {
                    eprintln!("wifi: scan failed: {reason}");
                    reject(ControlRejectionCode::BackendRejected)
                }
            },
            PlatformControlRequestV2::WifiSaved {} => match self.saved() {
                Ok(profiles) => ControlResponse::WifiProfiles { profiles },
                Err(reason) => {
                    eprintln!("wifi: saved networks read failed: {reason}");
                    reject(ControlRejectionCode::BackendRejected)
                }
            },
            PlatformControlRequestV2::WifiConnect {
                ssid,
                password,
                hidden,
            } => self.connect(ssid, password, *hidden),
            PlatformControlRequestV2::WifiDisconnect {} => self.disconnect(),
            PlatformControlRequestV2::WifiProfile { id, action } => self.profile(id, *action),
            PlatformControlRequestV2::WifiRadio { enabled } => self.radio(*enabled),
            _ => reject(ControlRejectionCode::CapabilityDisabled),
        }
    }

    pub fn status(&mut self) -> Result<WifiStatus, String> {
        let listing = self
            .runner
            .nmcli(&["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device"])?;
        let mut status = WifiStatus::default();
        for line in listing.lines() {
            let fields = terse_fields(line);
            if fields.len() < 4 || fields[0] != DEVICE || fields[1] != "wifi" {
                continue;
            }
            status.available = true;
            status.interface = Some(fields[0].clone());
            status.state = fields[2].clone();
            status.up = fields[2] == "connected";
            break;
        }
        if !status.available {
            return Ok(status);
        }
        status.radio = self
            .runner
            .nmcli(&["radio", "wifi"])
            .map(|state| state.trim() == "enabled")
            .unwrap_or(false);
        if status.up {
            // The SSID comes from the scan entry in use: a profile name is
            // not the network's name.
            if let Some(network) = self.scan(false)?.into_iter().find(|network| network.in_use) {
                status.signal = Some(network.signal);
                status.security = Some(network.security);
                status.ssid = Some(network.ssid);
            }
            status.address = self
                .runner
                .nmcli(&["-g", "IP4.ADDRESS", "device", "show", DEVICE])
                .ok()
                .and_then(|addresses| {
                    // Several addresses come as one line joined by " | ".
                    addresses
                        .lines()
                        .next()
                        .and_then(|line| line.split(" | ").next())
                        .map(|address| address.trim().to_string())
                })
                .filter(|address| !address.is_empty() && address.len() <= 43);
        }
        Ok(status)
    }

    /// The networks in range, strongest first, one entry per name.
    pub fn scan(&mut self, rescan: bool) -> Result<Vec<WifiNetwork>, String> {
        if rescan {
            // Asks for a new scan and returns at once; NetworkManager refuses
            // one right after another, which is harmless.
            let _ = self
                .runner
                .nmcli(&["device", "wifi", "rescan", "ifname", DEVICE]);
        }
        let listing = self.runner.nmcli(&[
            "-t",
            "-f",
            "IN-USE,SSID,SIGNAL,SECURITY,CHAN",
            "device",
            "wifi",
            "list",
            "--rescan",
            "no",
            "ifname",
            DEVICE,
        ])?;
        let mut networks: Vec<WifiNetwork> = Vec::new();
        for line in listing.lines() {
            let fields = terse_fields(line);
            // A hidden network announces no name; it is joined as "other".
            if fields.len() < 5 || !valid_wifi_ssid(&fields[1]) {
                continue;
            }
            let network = WifiNetwork {
                ssid: fields[1].clone(),
                signal: fields[2].trim().parse::<u8>().unwrap_or(0).min(100),
                security: security_label(&fields[3]).to_string(),
                channel: fields[4].trim().parse::<u16>().unwrap_or(0),
                in_use: fields[0].trim() == "*",
            };
            match networks.iter_mut().find(|known| known.ssid == network.ssid) {
                Some(known) => {
                    known.in_use |= network.in_use;
                    if network.signal > known.signal {
                        known.signal = network.signal;
                        known.channel = network.channel;
                    }
                }
                None => networks.push(network),
            }
        }
        networks.sort_by(|a, b| b.in_use.cmp(&a.in_use).then(b.signal.cmp(&a.signal)));
        networks.truncate(MAX_WIFI_NETWORKS);
        Ok(networks)
    }

    /// The saved WiFi profiles.
    pub fn saved(&mut self) -> Result<Vec<WifiProfileView>, String> {
        let listing = self.runner.nmcli(&[
            "-t",
            "-f",
            "UUID,TYPE,AUTOCONNECT,ACTIVE",
            "connection",
            "show",
        ])?;
        let mut profiles = Vec::new();
        for line in listing.lines() {
            let fields = terse_fields(line);
            if fields.len() < 4
                || fields[1] != "802-11-wireless"
                || !valid_wifi_profile_id(&fields[0])
            {
                continue;
            }
            // A name the protocol cannot carry is listed without it.
            let ssid = self
                .runner
                .nmcli(&[
                    "-g",
                    "802-11-wireless.ssid",
                    "connection",
                    "show",
                    "uuid",
                    &fields[0],
                ])
                .map(|ssid| terse_fields(ssid.trim_end_matches('\n')).join(":"))
                .ok()
                .filter(|ssid| valid_wifi_ssid(ssid))
                .unwrap_or_default();
            profiles.push(WifiProfileView {
                id: fields[0].clone(),
                ssid,
                autoconnect: fields[2] == "yes",
                active: fields[3] == "yes",
            });
            if profiles.len() == MAX_WIFI_PROFILES {
                break;
            }
        }
        Ok(profiles)
    }

    pub fn connect(&mut self, ssid: &str, password: &str, hidden: bool) -> ControlResponse {
        if !self.capability_enabled {
            return reject(ControlRejectionCode::CapabilityDisabled);
        }
        if !valid_wifi_ssid(ssid) || !valid_wifi_password(password) {
            return reject(ControlRejectionCode::InvalidState);
        }
        // The key management follows what the network announces; a hidden
        // one is WPA2-personal with a password and open without.
        let security = if hidden {
            None
        } else {
            match self.scan(false) {
                Ok(networks) => match networks.into_iter().find(|network| network.ssid == ssid) {
                    Some(network) => Some(network.security),
                    // Out of range: only a hidden network is joined unseen.
                    None => return reject(ControlRejectionCode::InvalidState),
                },
                Err(reason) => {
                    eprintln!("wifi: scan before connecting failed: {reason}");
                    return reject(ControlRejectionCode::BackendRejected);
                }
            }
        };
        let key_management = match (security.as_deref(), password.is_empty()) {
            (Some("open") | None, true) => None,
            (Some("wpa3"), false) => Some("sae"),
            (Some("wpa2" | "wpa2_wpa3") | None, false) => Some("wpa-psk"),
            // Enterprise, WEP and other security are not offered; an open
            // network takes no password and a secured one needs it.
            _ => return reject(ControlRejectionCode::InvalidState),
        };
        let earlier: Vec<String> = match self.saved() {
            Ok(profiles) => profiles
                .into_iter()
                .filter(|profile| profile.ssid == ssid)
                .map(|profile| profile.id)
                .collect(),
            Err(reason) => {
                eprintln!("wifi: saved networks read failed: {reason}");
                return reject(ControlRejectionCode::BackendRejected);
            }
        };
        let uuid = match self.runner.new_uuid() {
            Ok(uuid) if valid_wifi_profile_id(&uuid) => uuid.to_ascii_lowercase(),
            _ => return reject(ControlRejectionCode::BackendRejected),
        };
        let name = format!("{PROFILE_PREFIX}{}", &uuid[..8]);
        let contents = keyfile(&name, &uuid, ssid, password, key_management, hidden);
        let path = match self.runner.write_keyfile(&name, &contents) {
            Ok(path) => path,
            Err(reason) => {
                eprintln!("wifi: profile write failed: {reason}");
                return reject(ControlRejectionCode::BackendRejected);
            }
        };
        let path_text = path.to_string_lossy().into_owned();
        let activated = self
            .runner
            .nmcli(&["connection", "load", &path_text])
            .and_then(|_| {
                self.runner.nmcli(&[
                    "--wait",
                    ACTIVATION_SECONDS,
                    "connection",
                    "up",
                    "uuid",
                    &uuid,
                ])
            });
        if let Err(reason) = activated {
            eprintln!("wifi: profile activation failed: {reason}");
            // Reversible: a profile that never came up is removed again.
            let _ = self.runner.nmcli(&["connection", "delete", "uuid", &uuid]);
            if let Err(cleanup) = self.runner.remove_keyfile(&path) {
                eprintln!("wifi: profile cleanup failed: {cleanup}");
            }
            return reject(ControlRejectionCode::BackendRejected);
        }
        // One profile per network: the earlier ones go once the new one is up,
        // so a wrong password never costs the working profile.
        for id in earlier {
            if let Err(reason) = self.runner.nmcli(&["connection", "delete", "uuid", &id]) {
                eprintln!("wifi: earlier profile removal failed: {reason}");
            }
        }
        ControlResponse::Applied
    }

    pub fn disconnect(&mut self) -> ControlResponse {
        if !self.capability_enabled {
            return reject(ControlRejectionCode::CapabilityDisabled);
        }
        match self.runner.nmcli(&["device", "disconnect", DEVICE]) {
            Ok(_) => ControlResponse::Applied,
            Err(reason) => {
                eprintln!("wifi: disconnect failed: {reason}");
                reject(ControlRejectionCode::BackendRejected)
            }
        }
    }

    pub fn profile(&mut self, id: &str, action: WifiProfileAction) -> ControlResponse {
        if !self.capability_enabled {
            return reject(ControlRejectionCode::CapabilityDisabled);
        }
        // Only a saved WiFi profile can be named here.
        match self.saved() {
            Ok(profiles) if profiles.iter().any(|profile| profile.id == id) => {}
            Ok(_) => return reject(ControlRejectionCode::InvalidState),
            Err(reason) => {
                eprintln!("wifi: saved networks read failed: {reason}");
                return reject(ControlRejectionCode::BackendRejected);
            }
        }
        let result = match action {
            WifiProfileAction::Connect => {
                self.runner
                    .nmcli(&["--wait", ACTIVATION_SECONDS, "connection", "up", "uuid", id])
            }
            WifiProfileAction::Forget => self.runner.nmcli(&["connection", "delete", "uuid", id]),
            WifiProfileAction::AutoconnectOn | WifiProfileAction::AutoconnectOff => {
                let value = if action == WifiProfileAction::AutoconnectOn {
                    "yes"
                } else {
                    "no"
                };
                self.runner.nmcli(&[
                    "connection",
                    "modify",
                    "uuid",
                    id,
                    "connection.autoconnect",
                    value,
                ])
            }
        };
        match result {
            Ok(_) => ControlResponse::Applied,
            Err(reason) => {
                eprintln!("wifi: saved network action failed: {reason}");
                reject(ControlRejectionCode::BackendRejected)
            }
        }
    }

    pub fn radio(&mut self, enabled: bool) -> ControlResponse {
        if !self.capability_enabled {
            return reject(ControlRejectionCode::CapabilityDisabled);
        }
        match self
            .runner
            .nmcli(&["radio", "wifi", if enabled { "on" } else { "off" }])
        {
            Ok(_) => ControlResponse::Applied,
            Err(reason) => {
                eprintln!("wifi: radio switch failed: {reason}");
                reject(ControlRejectionCode::BackendRejected)
            }
        }
    }
}

/// A GKeyFile value: backslashes and leading or trailing spaces escaped.
fn keyfile_value(value: &str) -> String {
    let mut escaped = value.replace('\\', "\\\\");
    if escaped.starts_with(' ') {
        escaped.replace_range(0..1, "\\s");
    }
    if escaped.ends_with(' ') {
        escaped.pop();
        escaped.push_str("\\s");
    }
    escaped
}

fn keyfile(
    name: &str,
    uuid: &str,
    ssid: &str,
    password: &str,
    key_management: Option<&str>,
    hidden: bool,
) -> String {
    // The SSID as its byte list, which the keyfile format reads for any name.
    let ssid_bytes: String = ssid.bytes().map(|byte| format!("{byte};")).collect();
    let mut contents = format!(
        "[connection]\nid={name}\nuuid={uuid}\ntype=wifi\ninterface-name={DEVICE}\nautoconnect=true\n\n[wifi]\nmode=infrastructure\nssid={ssid_bytes}\n"
    );
    if hidden {
        contents.push_str("hidden=true\n");
    }
    if let Some(key_management) = key_management {
        contents.push_str(&format!(
            "\n[wifi-security]\nkey-mgmt={key_management}\npsk={}\n",
            keyfile_value(password)
        ));
    }
    contents.push_str("\n[ipv4]\nmethod=auto\n\n[ipv6]\nmethod=auto\n");
    contents
}

fn reject(code: ControlRejectionCode) -> ControlResponse {
    ControlResponse::Rejected { code }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    const OLD: &str = "11111111-2222-4333-8444-555555555555";
    const NEW: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    const DEVICES: &str =
        "eth0:ethernet:connected:Wired connection 1\nwlan0:wifi:disconnected:--\n";
    const SCAN: &str = concat!(
        " :HomeNet:62:WPA2:6\n",
        " :HomeNet:81:WPA2:36\n",
        " :Sae\\:Only:70:WPA3:1\n",
        " :Transition:40:WPA2 WPA3:11\n",
        " :Cafe:55::11\n",
        " :Corp:90:WPA2 802.1X:1\n",
        " :Old:20:WEP:1\n",
        " ::99:WPA2:1\n",
    );

    #[derive(Default)]
    struct Fake {
        answers: HashMap<String, Result<String, String>>,
        calls: Vec<String>,
        written: Vec<(String, String)>,
        removed: Vec<PathBuf>,
    }

    struct Runner(Arc<Mutex<Fake>>);

    impl WifiRunner for Runner {
        fn nmcli(&mut self, args: &[&str]) -> Result<String, String> {
            let mut fake = self.0.lock().unwrap();
            let call = args.join(" ");
            fake.calls.push(call.clone());
            fake.answers
                .get(&call)
                .cloned()
                .unwrap_or(Ok(String::new()))
        }

        fn write_keyfile(&mut self, name: &str, contents: &str) -> Result<PathBuf, String> {
            let mut fake = self.0.lock().unwrap();
            fake.written.push((name.into(), contents.into()));
            Ok(PathBuf::from(format!("/nm/{name}.nmconnection")))
        }

        fn remove_keyfile(&mut self, path: &Path) -> Result<(), String> {
            self.0.lock().unwrap().removed.push(path.into());
            Ok(())
        }

        fn new_uuid(&mut self) -> Result<String, String> {
            Ok(NEW.to_ascii_uppercase())
        }
    }

    const SAVED: &str = "-t -f UUID,TYPE,AUTOCONNECT,ACTIVE connection show";
    const LIST: &str =
        "-t -f IN-USE,SSID,SIGNAL,SECURITY,CHAN device wifi list --rescan no ifname wlan0";

    fn fixture(answers: &[(&str, Result<&str, &str>)]) -> (WifiState, Arc<Mutex<Fake>>) {
        let mut fake = Fake::default();
        fake.answers.insert(
            "-t -f DEVICE,TYPE,STATE,CONNECTION device".into(),
            Ok(DEVICES.into()),
        );
        fake.answers
            .insert("radio wifi".into(), Ok("enabled\n".into()));
        fake.answers.insert(LIST.into(), Ok(SCAN.into()));
        fake.answers.insert(
            SAVED.into(),
            Ok(format!(
                "{OLD}:802-11-wireless:yes:no\n0000:ethernet:yes:yes\nnot-a-uuid:802-11-wireless:no:no\n"
            )),
        );
        fake.answers.insert(
            format!("-g 802-11-wireless.ssid connection show uuid {OLD}"),
            Ok("HomeNet\n".into()),
        );
        for (call, answer) in answers {
            fake.answers.insert(
                (*call).into(),
                answer.map(str::to_string).map_err(str::to_string),
            );
        }
        let fake = Arc::new(Mutex::new(fake));
        (WifiState::open(Box::new(Runner(fake.clone())), true), fake)
    }

    fn rejected(code: ControlRejectionCode) -> ControlResponse {
        ControlResponse::Rejected { code }
    }

    #[test]
    fn scan_lists_each_named_network_once_strongest_first() {
        let (mut state, fake) = fixture(&[]);
        let networks = state.scan(true).unwrap();
        let summary: Vec<(&str, u8, &str, u16)> = networks
            .iter()
            .map(|network| {
                (
                    network.ssid.as_str(),
                    network.signal,
                    network.security.as_str(),
                    network.channel,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("Corp", 90, "enterprise", 1),
                ("HomeNet", 81, "wpa2", 36),
                ("Sae:Only", 70, "wpa3", 1),
                ("Cafe", 55, "open", 11),
                ("Transition", 40, "wpa2_wpa3", 11),
                ("Old", 20, "wep", 1),
            ]
        );
        assert_eq!(
            fake.lock().unwrap().calls[0],
            "device wifi rescan ifname wlan0"
        );
        let crowded: String = (0..30)
            .map(|index| format!(" :Net{index}:{index}:WPA2:1\n"))
            .collect();
        let (mut state, _) = fixture(&[(LIST, Ok(crowded.as_str()))]);
        let networks = state.scan(false).unwrap();
        assert_eq!(networks.len(), MAX_WIFI_NETWORKS);
        assert_eq!(networks[0].ssid, "Net29");
    }

    #[test]
    fn status_names_the_network_in_use_with_signal_and_address() {
        let in_use = SCAN.replacen(" :HomeNet:81", "*:HomeNet:81", 1);
        let (mut state, _) = fixture(&[
            (
                "-t -f DEVICE,TYPE,STATE,CONNECTION device",
                Ok("wlan0:wifi:connected:hardened-blikvm-wifi-11111111\n"),
            ),
            (LIST, Ok(in_use.as_str())),
            (
                "-g IP4.ADDRESS device show wlan0",
                Ok("192.0.2.40/24 | 192.0.2.41/24\n"),
            ),
        ]);
        let status = state.status().unwrap();
        assert!(status.available && status.up && status.radio);
        assert_eq!(status.ssid.as_deref(), Some("HomeNet"));
        assert_eq!(status.signal, Some(81));
        assert_eq!(status.security.as_deref(), Some("wpa2"));
        assert_eq!(status.address.as_deref(), Some("192.0.2.40/24"));

        let (mut state, _) = fixture(&[("radio wifi", Ok("disabled\n"))]);
        let status = state.status().unwrap();
        assert!(status.available && !status.up && !status.radio);
        let (mut state, _) = fixture(&[(
            "-t -f DEVICE,TYPE,STATE,CONNECTION device",
            Ok("eth0:ethernet:connected:Wired\n"),
        )]);
        assert!(!state.status().unwrap().available);
    }

    #[test]
    fn connects_through_its_own_keyfile_and_then_drops_the_older_profile() {
        let (mut state, fake) = fixture(&[]);
        assert_eq!(
            state.connect("HomeNet", "pass\\word ", false),
            ControlResponse::Applied
        );
        let fake = fake.lock().unwrap();
        let name = "hardened-blikvm-wifi-aaaaaaaa";
        assert_eq!(fake.written.len(), 1);
        assert_eq!(fake.written[0].0, name);
        assert_eq!(
            fake.written[0].1,
            format!(
                "[connection]\nid={name}\nuuid={NEW}\ntype=wifi\ninterface-name=wlan0\nautoconnect=true\n\n\
                 [wifi]\nmode=infrastructure\nssid=72;111;109;101;78;101;116;\n\n\
                 [wifi-security]\nkey-mgmt=wpa-psk\npsk=pass\\\\word\\s\n\n\
                 [ipv4]\nmethod=auto\n\n[ipv6]\nmethod=auto\n"
            )
        );
        let tail: Vec<&str> = fake
            .calls
            .iter()
            .rev()
            .take(3)
            .rev()
            .map(String::as_str)
            .collect();
        assert_eq!(
            tail,
            [
                format!("connection load /nm/{name}.nmconnection").as_str(),
                format!("--wait 30 connection up uuid {NEW}").as_str(),
                format!("connection delete uuid {OLD}").as_str(),
            ]
        );
        assert!(fake.calls.iter().all(|call| !call.contains("word")));
    }

    #[test]
    fn key_management_follows_the_network_and_unsupported_ones_are_refused() {
        for (ssid, password, hidden, expected) in [
            ("Sae:Only", "eight888", false, Some("key-mgmt=sae")),
            ("Transition", "eight888", false, Some("key-mgmt=wpa-psk")),
            ("Cafe", "", false, None),
            ("Hidden", "eight888", true, Some("key-mgmt=wpa-psk\n")),
            ("Hidden", "", true, None),
        ] {
            let (mut state, fake) = fixture(&[]);
            assert_eq!(
                state.connect(ssid, password, hidden),
                ControlResponse::Applied,
                "{ssid}"
            );
            let fake = fake.lock().unwrap();
            let contents = &fake.written[0].1;
            match expected {
                Some(line) => assert!(contents.contains(line), "{contents}"),
                None => assert!(!contents.contains("[wifi-security]"), "{contents}"),
            }
            assert_eq!(contents.contains("hidden=true\n"), hidden);
        }
        for (ssid, password) in [
            ("Corp", "eight888"),
            ("Old", "eight888"),
            ("Cafe", "eight888"),
            ("HomeNet", ""),
            ("Elsewhere", "eight888"),
            ("", "eight888"),
            ("HomeNet", "short"),
        ] {
            let (mut state, fake) = fixture(&[]);
            assert_eq!(
                state.connect(ssid, password, false),
                rejected(ControlRejectionCode::InvalidState),
                "{ssid}"
            );
            assert!(fake.lock().unwrap().written.is_empty());
        }
        let fake = Arc::new(Mutex::new(Fake::default()));
        let mut disabled = WifiState::open(Box::new(Runner(fake.clone())), false);
        for request in [
            PlatformControlRequestV2::WifiConnect {
                ssid: "Cafe".into(),
                password: String::new(),
                hidden: false,
            },
            PlatformControlRequestV2::WifiDisconnect {},
            PlatformControlRequestV2::WifiRadio { enabled: false },
            PlatformControlRequestV2::WifiProfile {
                id: OLD.into(),
                action: WifiProfileAction::Forget,
            },
        ] {
            assert_eq!(
                disabled.dispatch(&request),
                rejected(ControlRejectionCode::CapabilityDisabled)
            );
        }
        assert!(fake.lock().unwrap().calls.is_empty());
    }

    #[test]
    fn a_profile_that_does_not_come_up_is_removed_and_the_old_one_kept() {
        let up = format!("--wait 30 connection up uuid {NEW}");
        let (mut state, fake) = fixture(&[(up.as_str(), Err("Secrets were required"))]);
        assert_eq!(
            state.connect("HomeNet", "wrong-password", false),
            rejected(ControlRejectionCode::BackendRejected)
        );
        let fake = fake.lock().unwrap();
        assert_eq!(
            fake.calls.last().unwrap(),
            &format!("connection delete uuid {NEW}")
        );
        assert!(
            !fake
                .calls
                .contains(&format!("connection delete uuid {OLD}"))
        );
        assert_eq!(
            fake.removed,
            [PathBuf::from(
                "/nm/hardened-blikvm-wifi-aaaaaaaa.nmconnection"
            )]
        );
    }

    #[test]
    fn saved_profiles_are_listed_and_only_they_can_be_acted_on() {
        let (mut state, fake) = fixture(&[]);
        match state.dispatch(&PlatformControlRequestV2::WifiSaved {}) {
            ControlResponse::WifiProfiles { profiles } => assert_eq!(
                profiles,
                [WifiProfileView {
                    id: OLD.into(),
                    ssid: "HomeNet".into(),
                    autoconnect: true,
                    active: false,
                }]
            ),
            other => panic!("{other:?}"),
        }
        for (action, call) in [
            (
                WifiProfileAction::Connect,
                format!("--wait 30 connection up uuid {OLD}"),
            ),
            (
                WifiProfileAction::AutoconnectOff,
                format!("connection modify uuid {OLD} connection.autoconnect no"),
            ),
            (
                WifiProfileAction::AutoconnectOn,
                format!("connection modify uuid {OLD} connection.autoconnect yes"),
            ),
            (
                WifiProfileAction::Forget,
                format!("connection delete uuid {OLD}"),
            ),
        ] {
            assert_eq!(state.profile(OLD, action), ControlResponse::Applied);
            assert_eq!(fake.lock().unwrap().calls.last().unwrap(), &call);
        }
        let before = fake.lock().unwrap().calls.len();
        assert_eq!(
            state.profile(NEW, WifiProfileAction::Forget),
            rejected(ControlRejectionCode::InvalidState)
        );
        // Only the saved list was read.
        assert_eq!(fake.lock().unwrap().calls.len(), before + 2);

        let many: String = (0..20)
            .map(|index| {
                format!(
                    "{:08x}-2222-4333-8444-555555555555:802-11-wireless:yes:no\n",
                    index
                )
            })
            .collect();
        let (mut state, _) = fixture(&[(SAVED, Ok(many.as_str()))]);
        assert_eq!(state.saved().unwrap().len(), MAX_WIFI_PROFILES);
    }

    #[test]
    fn radio_and_disconnect_act_on_the_device() {
        let (mut state, fake) = fixture(&[]);
        assert_eq!(state.radio(false), ControlResponse::Applied);
        assert_eq!(fake.lock().unwrap().calls.last().unwrap(), "radio wifi off");
        assert_eq!(state.radio(true), ControlResponse::Applied);
        assert_eq!(fake.lock().unwrap().calls.last().unwrap(), "radio wifi on");
        assert_eq!(state.disconnect(), ControlResponse::Applied);
        assert_eq!(
            fake.lock().unwrap().calls.last().unwrap(),
            "device disconnect wlan0"
        );
        let (mut failing, _) = fixture(&[("device disconnect wlan0", Err("not active"))]);
        assert_eq!(
            failing.disconnect(),
            rejected(ControlRejectionCode::BackendRejected)
        );
    }
}
