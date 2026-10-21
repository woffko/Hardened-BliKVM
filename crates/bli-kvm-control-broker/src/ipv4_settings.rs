//! IPv4 addressing and DNS through NetworkManager, owned by the board owner.
//!
//! The reference's network page shows the interface, address, mask, router
//! and resolvers and switches between DHCP and manual settings. Changes go to
//! the active NetworkManager profile and are reapplied to the device. A
//! change of the addressing itself is reverted after two minutes unless the
//! operator confirms it from the new address, so a mistake cannot strand the
//! board; a change of the resolvers alone applies at once.
//!
//! These requests run on their own lock and a blocking worker (see
//! `server.rs`): reading NetworkManager takes a second or more on the board,
//! and HID reports must not wait behind it. The DNS, IPv6 and About pages
//! load together, so one reading serves them for two seconds.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bli_kvm_control_protocol::{ControlRejectionCode, ControlResponse, NetworkInterfaceInfo};
use bli_kvm_core::{Ipv4Change, Ipv6Change, PlatformControlRequestV2};

use crate::network_settings::WifiRunner;
use crate::revert_journal::RevertJournal;

pub const REVERT_AFTER: Duration = Duration::from_secs(120);
/// One reading of NetworkManager serves the pages that ask together.
const INFO_REUSE: Duration = Duration::from_secs(2);

/// The network settings behind their own lock, served on a blocking worker.
pub type SharedIpv4 = Arc<Mutex<Ipv4Settings>>;

type Runner = Arc<Mutex<Box<dyn WifiRunner>>>;

/// The profile settings a revert restores.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Snapshot {
    method: String,
    addresses: String,
    gateway: String,
    dns: String,
    ignore_auto_dns: String,
}

/// An interface's unconfirmed addressing change: the state its revert
/// restores is the one before the first unconfirmed change, whatever
/// changes come after it inside the window (code audit of 2026-10-20,
/// AUD-08).
struct Pending {
    generation: u64,
    deadline: Instant,
    uuid: String,
    before: Snapshot,
}

/// Pending reverts by device.
type PendingReverts = Arc<Mutex<std::collections::BTreeMap<String, Pending>>>;

/// What an unconfirmed addressing change must restore, kept across a restart.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct RevertRecord {
    device: String,
    uuid: String,
    before: Snapshot,
}

/// The journal's records, one for each interface with an unconfirmed
/// change; a single one is written as older owners wrote theirs, so they
/// still read it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum RevertRecords {
    One(RevertRecord),
    Many(Vec<RevertRecord>),
}

pub struct Ipv4Settings {
    runner: Runner,
    pending: PendingReverts,
    generation: u64,
    revert_after: Duration,
    journal: Option<RevertJournal>,
    /// The last network information and when it was read.
    cache: Option<(Instant, ControlResponse)>,
}

fn nmcli(runner: &Runner, args: &[&str]) -> Result<String, String> {
    runner
        .lock()
        .map_err(|_| "runner poisoned".to_string())?
        .nmcli(args)
}

/// `nmcli -t` escapes `:` and `\\` inside a field.
fn split_terse(line: &str) -> Vec<String> {
    let mut fields = vec![String::new()];
    let mut chars = line.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                if let Some(next) = chars.next() {
                    fields.last_mut().expect("one field").push(next);
                }
            }
            ':' => fields.push(String::new()),
            _ => fields.last_mut().expect("one field").push(ch),
        }
    }
    fields
}

/// Values of a `-g` field: several are joined by ` | ` (device) or `,`
/// (profile), and `:` and `\\` inside a value are escaped.
fn values(field: &str, separator: &str) -> Vec<String> {
    field
        .split(separator)
        .map(|value| unescape(value.trim()))
        .filter(|value| !value.is_empty())
        .collect()
}

fn valid_uuid(uuid: &str) -> bool {
    uuid.len() == 36 && uuid.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(ch);
        }
    }
    out
}

impl Ipv4Settings {
    pub fn open(runner: Box<dyn WifiRunner>) -> Self {
        Self {
            runner: Arc::new(Mutex::new(runner)),
            pending: Arc::new(Mutex::new(std::collections::BTreeMap::new())),
            generation: 0,
            revert_after: REVERT_AFTER,
            journal: None,
            cache: None,
        }
    }

    /// Keeps an unconfirmed addressing change in `journal`.
    pub fn with_journal(mut self, journal: RevertJournal) -> Self {
        self.journal = Some(journal);
        self
    }

    /// A journal record at start means the owner stopped before the change
    /// was confirmed, so nothing can confirm it now: it is reverted at once,
    /// retrying while NetworkManager is not up. Returns whether one was found.
    pub fn reconcile(&self) -> bool {
        let Some(journal) = self.journal.clone() else {
            return false;
        };
        let Some(records) = journal.read::<RevertRecords>() else {
            return false;
        };
        let mut records = match records {
            RevertRecords::One(record) => vec![record],
            RevertRecords::Many(records) => records,
        };
        let runner = Arc::clone(&self.runner);
        std::thread::spawn(move || {
            for attempt in 0..12 {
                if attempt > 0 {
                    std::thread::sleep(Duration::from_secs(5));
                }
                records.retain(|record| !Self::revert_record(&runner, record));
                if let Err(error) = write_records(&journal, records.clone()) {
                    eprintln!("network: the revert journal could not be written: {error}");
                }
                if records.is_empty() {
                    return;
                }
            }
            eprintln!("network: the unconfirmed IPv4 change could not be reverted");
        });
        true
    }

    fn revert_record(runner: &Runner, record: &RevertRecord) -> bool {
        match Self::apply(
            runner,
            &record.device,
            &record.uuid,
            &Self::restore_settings(&record.before),
        ) {
            Ok(()) => {
                eprintln!(
                    "network: unconfirmed IPv4 change on {} reverted after a restart",
                    record.device
                );
                true
            }
            Err(reason) => {
                eprintln!("network: reverting {} failed: {reason}", record.device);
                false
            }
        }
    }

    /// Answers the network information, IPv4, IPv6 and confirmation requests.
    pub fn dispatch(&mut self, request: &PlatformControlRequestV2) -> ControlResponse {
        match request {
            PlatformControlRequestV2::NetworkInfo {} => self.info(),
            PlatformControlRequestV2::Ipv4Set { change } => self.set(change),
            PlatformControlRequestV2::Ipv6Set { change } => self.set_ipv6(change),
            PlatformControlRequestV2::Ipv4Confirm {} => self.confirm(),
            _ => reject(ControlRejectionCode::CapabilityDisabled),
        }
    }

    #[cfg(test)]
    fn with_revert_after(mut self, revert_after: Duration) -> Self {
        self.revert_after = revert_after;
        self
    }

    /// Connected wired and wireless devices with their active profile.
    fn devices(&self) -> Result<Vec<(String, String, String)>, String> {
        let output = nmcli(
            &self.runner,
            &["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device"],
        )?;
        Ok(output
            .lines()
            .map(split_terse)
            .filter(|fields| {
                fields.len() == 4
                    && matches!(fields[1].as_str(), "ethernet" | "wifi")
                    && fields[2] == "connected"
                    && !fields[3].is_empty()
            })
            .map(|fields| (fields[0].clone(), fields[1].clone(), fields[3].clone()))
            .collect())
    }

    fn profile_uuid(&self, connection: &str) -> Result<String, String> {
        let uuid = nmcli(
            &self.runner,
            &["-g", "connection.uuid", "connection", "show", connection],
        )?;
        let uuid = uuid.trim().to_string();
        if valid_uuid(&uuid) {
            Ok(uuid)
        } else {
            Err("the active profile has no valid UUID".into())
        }
    }

    fn snapshot(&self, uuid: &str) -> Result<Snapshot, String> {
        let output = nmcli(
            &self.runner,
            &[
                "-g",
                "ipv4.method,ipv4.addresses,ipv4.gateway,ipv4.dns,ipv4.ignore-auto-dns",
                "connection",
                "show",
                uuid,
            ],
        )?;
        let mut lines = output.lines().map(str::trim).map(str::to_string);
        let mut next = || lines.next().unwrap_or_default();
        Ok(Snapshot {
            method: next(),
            addresses: next(),
            gateway: next(),
            dns: next(),
            ignore_auto_dns: next(),
        })
    }

    fn interface(
        &self,
        name: &str,
        kind: &str,
        connection: &str,
    ) -> Result<NetworkInterfaceInfo, String> {
        let output = nmcli(
            &self.runner,
            &[
                "-g",
                "IP4.ADDRESS,IP4.GATEWAY,IP4.DNS,IP4.DOMAIN,DHCP4.OPTION,IP6.ADDRESS,IP6.GATEWAY,GENERAL.CON-UUID",
                "device",
                "show",
                name,
            ],
        )?;
        let lines: Vec<&str> = output.lines().collect();
        let field = |index: usize| lines.get(index).copied().unwrap_or_default();
        let (address, prefix) = values(field(0), " | ")
            .first()
            .and_then(|cidr| {
                let (address, prefix) = cidr.split_once('/')?;
                Some((address.to_string(), prefix.parse::<u8>().ok()?))
            })
            .unwrap_or_default();
        let dhcp_dns = values(field(4), " | ")
            .into_iter()
            .find_map(|option| {
                option
                    .strip_prefix("domain_name_servers = ")
                    .map(|servers| values(servers, " "))
            })
            .unwrap_or_default();
        // The active profile's UUID comes with the device; a NetworkManager
        // that does not report it is asked by the profile's name.
        let uuid = match unescape(field(7).trim()) {
            uuid if valid_uuid(&uuid) => uuid,
            _ => self.profile_uuid(connection)?,
        };
        // IPv4 and IPv6 settings of the profile in one reading.
        let settings = nmcli(
            &self.runner,
            &[
                "-g",
                "ipv4.method,ipv4.addresses,ipv4.gateway,ipv4.dns,ipv4.ignore-auto-dns,ipv6.method,ipv6.addresses,ipv6.gateway",
                "connection",
                "show",
                &uuid,
            ],
        )?;
        let mut settings = settings.lines().map(str::trim);
        let mut next = || settings.next().unwrap_or_default().to_string();
        let profile = Snapshot {
            method: next(),
            addresses: next(),
            gateway: next(),
            dns: next(),
            ignore_auto_dns: next(),
        };
        let ipv6_method = next();
        let ipv6_addresses = next();
        let ipv6_gateway = next();
        let ipv6_mode = match ipv6_method.as_str() {
            "disabled" | "ignore" => "disabled",
            "dhcp" => "dhcpv6",
            "manual" => "manual",
            _ => "slaac",
        };
        let ipv6_manual_address = values(&ipv6_addresses, ",")
            .into_iter()
            .next()
            .unwrap_or_default();
        let ipv6_manual_gateway = unescape(&ipv6_gateway);
        Ok(NetworkInterfaceInfo {
            name: name.to_string(),
            kind: kind.to_string(),
            method: profile.method,
            address,
            prefix,
            gateway: unescape(field(1).trim()),
            dns: values(field(2), " | "),
            dhcp_dns,
            manual_dns: profile.ignore_auto_dns == "yes",
            search: values(field(3), " | "),
            ipv6_mode: ipv6_mode.into(),
            ipv6_addresses: values(field(5), " | "),
            ipv6_gateway: unescape(field(6).trim()),
            ipv6_manual_address,
            ipv6_manual_gateway,
        })
    }

    /// The soonest pending revert.
    fn revert_in_seconds(&self) -> u32 {
        let pending = self.pending.lock().expect("pending poisoned");
        pending
            .values()
            .map(|pending| {
                pending
                    .deadline
                    .saturating_duration_since(Instant::now())
                    .as_secs()
                    .max(1) as u32
            })
            .min()
            .unwrap_or(0)
    }

    pub fn info(&mut self) -> ControlResponse {
        if let Some((at, response)) = &self.cache
            && at.elapsed() < INFO_REUSE
        {
            return response.clone();
        }
        let response = self.read_info();
        if matches!(response, ControlResponse::NetworkInfo { .. }) {
            self.cache = Some((Instant::now(), response.clone()));
        }
        response
    }

    fn read_info(&mut self) -> ControlResponse {
        let devices = match self.devices() {
            Ok(devices) => devices,
            Err(reason) => {
                eprintln!("network: device list failed: {reason}");
                return reject(ControlRejectionCode::BackendRejected);
            }
        };
        let mut interfaces = Vec::new();
        for (name, kind, connection) in devices.into_iter().take(4) {
            match self.interface(&name, &kind, &connection) {
                Ok(info) => interfaces.push(info),
                Err(reason) => eprintln!("network: {name} could not be read: {reason}"),
            }
        }
        ControlResponse::NetworkInfo {
            interfaces,
            revert_in_seconds: self.revert_in_seconds(),
        }
    }

    fn apply(
        runner: &Runner,
        device: &str,
        uuid: &str,
        settings: &[(&str, String)],
    ) -> Result<(), String> {
        let mut args = vec!["connection", "modify", uuid];
        for (key, value) in settings {
            args.push(key);
            args.push(value.as_str());
        }
        nmcli(runner, &args)?;
        if nmcli(runner, &["device", "reapply", device]).is_err() {
            nmcli(runner, &["connection", "up", uuid])?;
        }
        Ok(())
    }

    fn restore_settings(snapshot: &Snapshot) -> Vec<(&'static str, String)> {
        vec![
            ("ipv4.method", snapshot.method.clone()),
            ("ipv4.addresses", snapshot.addresses.clone()),
            ("ipv4.gateway", snapshot.gateway.clone()),
            ("ipv4.dns", snapshot.dns.clone()),
            ("ipv4.ignore-auto-dns", snapshot.ignore_auto_dns.clone()),
        ]
    }

    pub fn set(&mut self, change: &Ipv4Change) -> ControlResponse {
        // Whatever happens, the next reading is fresh.
        self.cache = None;
        if change.validate().is_err() {
            return reject(ControlRejectionCode::InvalidState);
        }
        let devices = match self.devices() {
            Ok(devices) => devices,
            Err(reason) => {
                eprintln!("network: device list failed: {reason}");
                return reject(ControlRejectionCode::BackendRejected);
            }
        };
        let Some((device, _, connection)) = devices
            .into_iter()
            .find(|(name, _, _)| *name == change.interface)
        else {
            return reject(ControlRejectionCode::InvalidState);
        };
        let (uuid, current) = match self
            .profile_uuid(&connection)
            .and_then(|uuid| self.snapshot(&uuid).map(|snapshot| (uuid, snapshot)))
        {
            Ok(found) => found,
            Err(reason) => {
                eprintln!("network: profile read failed: {reason}");
                return reject(ControlRejectionCode::BackendRejected);
            }
        };
        // Inside a window the revert keeps its first state.
        let first = self
            .pending
            .lock()
            .expect("pending poisoned")
            .get(&device)
            .filter(|pending| pending.uuid == uuid)
            .map(|pending| pending.before.clone());
        let in_window = first.is_some();
        let before = first.unwrap_or_else(|| current.clone());
        let mut settings: Vec<(&str, String)> = Vec::new();
        if change.dhcp {
            settings.extend([
                ("ipv4.method", "auto".to_string()),
                ("ipv4.addresses", String::new()),
                ("ipv4.gateway", String::new()),
                ("ipv4.dns", String::new()),
                ("ipv4.ignore-auto-dns", "no".to_string()),
            ]);
        } else {
            if let (Some(address), Some(prefix), Some(gateway)) =
                (&change.address, change.prefix, &change.gateway)
            {
                settings.extend([
                    ("ipv4.method", "manual".to_string()),
                    ("ipv4.addresses", format!("{address}/{prefix}")),
                    ("ipv4.gateway", gateway.clone()),
                ]);
            }
            settings.extend([
                ("ipv4.dns", change.dns.join(",")),
                ("ipv4.ignore-auto-dns", "yes".to_string()),
            ]);
        }
        // Written before the change: a restart from here on still reverts it.
        if let Err(error) = self.write_journal(Some(RevertRecord {
            device: device.clone(),
            uuid: uuid.clone(),
            before: before.clone(),
        })) {
            eprintln!("network: the revert journal could not be written: {error}");
            return reject(ControlRejectionCode::BackendRejected);
        }
        if let Err(reason) = Self::apply(&self.runner, &device, &uuid, &settings) {
            eprintln!("network: applying the IPv4 change failed: {reason}");
            // The state before this change comes back; a pending revert
            // stays as it was.
            let _ = Self::apply(
                &self.runner,
                &device,
                &uuid,
                &Self::restore_settings(&current),
            );
            if let Err(error) = self.write_journal(None) {
                eprintln!("network: the revert journal could not be written: {error}");
            }
            return reject(ControlRejectionCode::BackendRejected);
        }
        let after = self.snapshot(&uuid).unwrap_or_else(|_| current.clone());
        let addressing_changed = after.method != before.method
            || after.addresses != before.addresses
            || after.gateway != before.gateway;
        eprintln!(
            "network: {device} set to {}{}",
            if change.dhcp { "DHCP" } else { "manual" },
            if addressing_changed {
                " (reverts unless confirmed)"
            } else {
                ""
            }
        );
        if addressing_changed || in_window {
            // A change inside a window, the resolvers' alone included, keeps
            // the window's revert and starts its time again.
            self.arm_revert(device, uuid, before);
        } else if let Err(error) = self.write_journal(None) {
            // A change of the resolvers alone needs no confirmation.
            eprintln!("network: the revert journal could not be written: {error}");
        }
        self.info()
    }

    /// Writes the pending reverts, with `adding` among them, before a change.
    fn write_journal(&self, adding: Option<RevertRecord>) -> std::io::Result<()> {
        let Some(journal) = &self.journal else {
            return Ok(());
        };
        let pending = self.pending.lock().expect("pending poisoned");
        let mut records = pending_records(&pending);
        if let Some(adding) = adding {
            records.retain(|record| record.device != adding.device);
            records.push(adding);
        }
        write_records(journal, records)
    }

    fn arm_revert(&mut self, device: String, uuid: String, before: Snapshot) {
        self.generation += 1;
        let generation = self.generation;
        self.pending.lock().expect("pending poisoned").insert(
            device.clone(),
            Pending {
                generation,
                deadline: Instant::now() + self.revert_after,
                uuid,
                before,
            },
        );
        let pending = Arc::clone(&self.pending);
        let runner = Arc::clone(&self.runner);
        let journal = self.journal.clone();
        let wait = self.revert_after;
        std::thread::spawn(move || {
            std::thread::sleep(wait);
            let mut current = pending.lock().expect("pending poisoned");
            if current.get(&device).map(|pending| pending.generation) != Some(generation) {
                return;
            }
            let Some(due) = current.remove(&device) else {
                return;
            };
            drop(current);
            match Self::apply(
                &runner,
                &device,
                &due.uuid,
                &Self::restore_settings(&due.before),
            ) {
                Ok(()) => {
                    if let Some(journal) = &journal {
                        let current = pending.lock().expect("pending poisoned");
                        if let Err(error) = write_records(journal, pending_records(&current)) {
                            eprintln!("network: the revert journal could not be written: {error}");
                        }
                    }
                    eprintln!("network: unconfirmed IPv4 change on {device} reverted");
                }
                // The journal keeps its record: the next owner tries again.
                Err(reason) => eprintln!("network: reverting {device} failed: {reason}"),
            }
        });
    }

    /// IPv6 does not carry the management address, so it applies at once.
    pub fn set_ipv6(&mut self, change: &Ipv6Change) -> ControlResponse {
        self.cache = None;
        if change.validate().is_err() {
            return reject(ControlRejectionCode::InvalidState);
        }
        let devices = match self.devices() {
            Ok(devices) => devices,
            Err(reason) => {
                eprintln!("network: device list failed: {reason}");
                return reject(ControlRejectionCode::BackendRejected);
            }
        };
        let Some((device, _, connection)) = devices
            .into_iter()
            .find(|(name, _, _)| *name == change.interface)
        else {
            return reject(ControlRejectionCode::InvalidState);
        };
        let uuid = match self.profile_uuid(&connection) {
            Ok(uuid) => uuid,
            Err(reason) => {
                eprintln!("network: profile read failed: {reason}");
                return reject(ControlRejectionCode::BackendRejected);
            }
        };
        let method = match change.mode.as_str() {
            "disabled" => "disabled",
            "dhcpv6" => "dhcp",
            "manual" => "manual",
            _ => "auto",
        };
        let (addresses, gateway) = match (&change.address, change.prefix) {
            (Some(address), Some(prefix)) if method == "manual" => (
                format!("{address}/{prefix}"),
                change.gateway.clone().unwrap_or_default(),
            ),
            _ => (String::new(), String::new()),
        };
        // A manual address must be set before the method that requires it.
        let settings: Vec<(&str, String)> = if method == "manual" {
            vec![
                ("ipv6.addresses", addresses),
                ("ipv6.gateway", gateway),
                ("ipv6.method", method.to_string()),
            ]
        } else {
            vec![
                ("ipv6.method", method.to_string()),
                ("ipv6.addresses", addresses),
                ("ipv6.gateway", gateway),
            ]
        };
        if let Err(reason) = Self::apply(&self.runner, &device, &uuid, &settings) {
            eprintln!("network: applying the IPv6 change failed: {reason}");
            return reject(ControlRejectionCode::BackendRejected);
        }
        eprintln!("network: {device} IPv6 set to {}", change.mode);
        self.info()
    }

    pub fn confirm(&mut self) -> ControlResponse {
        self.cache = None;
        let confirmed = std::mem::take(&mut *self.pending.lock().expect("pending poisoned"));
        if !confirmed.is_empty() {
            if let Some(journal) = &self.journal {
                journal.clear();
            }
            eprintln!("network: IPv4 change confirmed");
            ControlResponse::Applied
        } else {
            reject(ControlRejectionCode::InvalidState)
        }
    }
}

fn reject(code: ControlRejectionCode) -> ControlResponse {
    ControlResponse::Rejected { code }
}

fn pending_records(pending: &std::collections::BTreeMap<String, Pending>) -> Vec<RevertRecord> {
    pending
        .iter()
        .map(|(device, pending)| RevertRecord {
            device: device.clone(),
            uuid: pending.uuid.clone(),
            before: pending.before.clone(),
        })
        .collect()
}

/// One record is written alone, as older owners read it; none clears the
/// journal.
fn write_records(journal: &RevertJournal, mut records: Vec<RevertRecord>) -> std::io::Result<()> {
    match records.len() {
        0 => {
            journal.clear();
            Ok(())
        }
        1 => journal.write(&RevertRecords::One(records.remove(0))),
        _ => journal.write(&RevertRecords::Many(records)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIRED_UUID: &str = "8fe436fc-dbd5-3398-9d6d-669fa69d8a73";
    const WIRELESS_UUID: &str = "1b2c3d4e-0000-4000-8000-000000000077";

    /// A NetworkManager stand-in for one wired profile, and a wireless one
    /// when `wlan` is set.
    struct FakeNm {
        profile: Arc<Mutex<Snapshot>>,
        wlan: Option<Arc<Mutex<Snapshot>>>,
        calls: Arc<Mutex<Vec<String>>>,
        ipv6: Arc<Mutex<std::collections::BTreeMap<String, String>>>,
        /// Refuses the next `connection modify` before it changes anything.
        fail_modify: Arc<std::sync::atomic::AtomicBool>,
    }

    impl FakeNm {
        fn wired(profile: &Arc<Mutex<Snapshot>>) -> Self {
            Self {
                profile: Arc::clone(profile),
                wlan: None,
                calls: Arc::new(Mutex::new(Vec::new())),
                ipv6: Arc::new(Mutex::new(
                    [("ipv6.method".to_string(), "auto".to_string())].into(),
                )),
                fail_modify: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }
        }

        fn profile_of(&self, uuid: &str) -> Arc<Mutex<Snapshot>> {
            match (&self.wlan, uuid) {
                (Some(wlan), WIRELESS_UUID) => Arc::clone(wlan),
                _ => Arc::clone(&self.profile),
            }
        }
    }

    impl WifiRunner for FakeNm {
        fn nmcli(&mut self, args: &[&str]) -> Result<String, String> {
            self.calls.lock().unwrap().push(args.join(" "));
            let uuid = match args {
                ["-g", _, "connection", "show", uuid] | ["connection", "modify", uuid, ..] => *uuid,
                _ => WIRED_UUID,
            };
            let selected = self.profile_of(uuid);
            let profile = selected.lock().unwrap().clone();
            Ok(match args {
                ["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device"] => {
                    if self.wlan.is_some() {
                        "eth0:ethernet:connected:Wired connection 1\nwlan0:wifi:connected:Home\nlo:loopback:unmanaged:\n".into()
                    } else {
                        "eth0:ethernet:connected:Wired connection 1\nwlan0:wifi:disconnected:\nlo:loopback:unmanaged:\n".into()
                    }
                }
                ["-g", "connection.uuid", "connection", "show", "Wired connection 1"] => {
                    format!("{WIRED_UUID}\n")
                }
                ["-g", "connection.uuid", "connection", "show", "Home"] if self.wlan.is_some() => {
                    format!("{WIRELESS_UUID}\n")
                }
                ["-g", "ipv6.method,ipv6.addresses,ipv6.gateway", "connection", "show", _] => {
                    let ipv6 = self.ipv6.lock().unwrap();
                    let get = |key: &str| ipv6.get(key).cloned().unwrap_or_default();
                    format!(
                        "{}\n{}\n{}\n",
                        get("ipv6.method"),
                        get("ipv6.addresses"),
                        get("ipv6.gateway")
                    )
                }
                ["-g", fields, "connection", "show", _] if fields.contains("ipv6.method") => {
                    let ipv6 = self.ipv6.lock().unwrap();
                    let get = |key: &str| ipv6.get(key).cloned().unwrap_or_default();
                    format!(
                        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
                        profile.method,
                        profile.addresses,
                        profile.gateway,
                        profile.dns,
                        profile.ignore_auto_dns,
                        get("ipv6.method"),
                        get("ipv6.addresses"),
                        get("ipv6.gateway")
                    )
                }
                ["-g", _, "connection", "show", _] => format!(
                    "{}\n{}\n{}\n{}\n{}\n",
                    profile.method,
                    profile.addresses,
                    profile.gateway,
                    profile.dns,
                    profile.ignore_auto_dns
                ),
                ["-g", _, "device", "show", "eth0"] => "192.0.2.118/24\n192.0.2.5\n192.0.2.5\nint\nrouters = 192.0.2.5 | domain_name_servers = 192.0.2.5 192.0.2.6 | subnet_mask = 255.255.255.0\n\n\n8fe436fc-dbd5-3398-9d6d-669fa69d8a73\n".into(),
                ["-g", _, "device", "show", "wlan0"] if self.wlan.is_some() => {
                    format!("198.51.100.20/24\n198.51.100.1\n198.51.100.1\n\n\n\n\n{WIRELESS_UUID}\n")
                }
                ["connection", "modify", _, rest @ ..] => {
                    if self
                        .fail_modify
                        .swap(false, std::sync::atomic::Ordering::SeqCst)
                    {
                        return Err("modify refused".into());
                    }
                    let mut profile = selected.lock().unwrap();
                    for pair in rest.chunks(2) {
                        let value = pair[1].to_string();
                        match pair[0] {
                            "ipv4.method" => profile.method = value,
                            "ipv4.addresses" => profile.addresses = value,
                            "ipv4.gateway" => profile.gateway = value,
                            "ipv4.dns" => profile.dns = value,
                            "ipv4.ignore-auto-dns" => profile.ignore_auto_dns = value,
                            key if key.starts_with("ipv6.") => {
                                if key == "ipv6.method"
                                    && value == "manual"
                                    && self
                                        .ipv6
                                        .lock()
                                        .unwrap()
                                        .get("ipv6.addresses")
                                        .is_none_or(String::is_empty)
                                {
                                    return Err("manual needs an address".into());
                                }
                                self.ipv6.lock().unwrap().insert(key.into(), value);
                            }
                            _ => return Err("unknown setting".into()),
                        }
                    }
                    String::new()
                }
                ["device", "reapply", "eth0" | "wlan0"] => String::new(),
                _ => return Err(format!("unexpected {}", args.join(" "))),
            })
        }

        fn write_keyfile(
            &mut self,
            _name: &str,
            _contents: &str,
        ) -> Result<std::path::PathBuf, String> {
            Err("not used".into())
        }

        fn remove_keyfile(&mut self, _path: &std::path::Path) -> Result<(), String> {
            Err("not used".into())
        }
    }

    fn dhcp_profile() -> Snapshot {
        Snapshot {
            method: "auto".into(),
            addresses: String::new(),
            gateway: String::new(),
            dns: String::new(),
            ignore_auto_dns: "no".into(),
        }
    }

    fn settings(profile: &Arc<Mutex<Snapshot>>, revert_after: Duration) -> Ipv4Settings {
        Ipv4Settings::open(Box::new(FakeNm::wired(profile))).with_revert_after(revert_after)
    }

    fn manual(address: Option<&str>) -> Ipv4Change {
        Ipv4Change {
            interface: "eth0".into(),
            dhcp: false,
            dns: vec!["192.0.2.53".into()],
            address: address.map(str::to_string),
            prefix: address.map(|_| 24),
            gateway: address.map(|_| "192.0.2.5".to_string()),
        }
    }

    #[test]
    fn interfaces_are_read_from_network_manager() {
        let profile = Arc::new(Mutex::new(dhcp_profile()));
        let mut ipv4 = settings(&profile, REVERT_AFTER);
        let ControlResponse::NetworkInfo {
            interfaces,
            revert_in_seconds,
        } = ipv4.info()
        else {
            panic!("no info");
        };
        assert_eq!(revert_in_seconds, 0);
        assert_eq!(interfaces.len(), 1);
        let eth0 = &interfaces[0];
        assert_eq!(
            (eth0.name.as_str(), eth0.kind.as_str(), eth0.method.as_str()),
            ("eth0", "ethernet", "auto")
        );
        assert_eq!((eth0.address.as_str(), eth0.prefix), ("192.0.2.118", 24));
        assert_eq!(eth0.gateway, "192.0.2.5");
        assert_eq!(eth0.dhcp_dns, ["192.0.2.5", "192.0.2.6"]);
        assert_eq!(eth0.search, ["int"]);
        assert!(!eth0.manual_dns);
    }

    #[test]
    fn one_reading_serves_the_pages_that_ask_together() {
        let profile = Arc::new(Mutex::new(dhcp_profile()));
        let nm = FakeNm::wired(&profile);
        let calls = Arc::clone(&nm.calls);
        let mut ipv4 = Ipv4Settings::open(Box::new(nm));
        let first = ipv4.info();
        let reads = calls.lock().unwrap().len();
        // The device list, the device (with its profile's UUID) and one
        // reading of the profile's IPv4 and IPv6 settings.
        assert_eq!(reads, 3, "{:?}", calls.lock().unwrap());
        assert_eq!(
            ipv4.dispatch(&PlatformControlRequestV2::NetworkInfo {}),
            first
        );
        assert_eq!(calls.lock().unwrap().len(), reads);
        // A change is read afresh.
        let ControlResponse::NetworkInfo { .. } = ipv4.set(&manual(None)) else {
            panic!("not applied");
        };
        assert!(calls.lock().unwrap().len() > reads + 3);
    }

    #[test]
    fn resolvers_apply_at_once_and_addressing_reverts_unless_confirmed() {
        let profile = Arc::new(Mutex::new(dhcp_profile()));
        let mut ipv4 = settings(&profile, Duration::from_millis(300));
        // Resolvers only: no revert.
        let ControlResponse::NetworkInfo {
            revert_in_seconds, ..
        } = ipv4.set(&manual(None))
        else {
            panic!("not applied");
        };
        assert_eq!(revert_in_seconds, 0);
        assert_eq!(profile.lock().unwrap().dns, "192.0.2.53");
        assert_eq!(profile.lock().unwrap().ignore_auto_dns, "yes");
        assert_eq!(ipv4.confirm(), reject(ControlRejectionCode::InvalidState));

        // A static address reverts when nobody confirms it.
        let ControlResponse::NetworkInfo {
            revert_in_seconds, ..
        } = ipv4.set(&manual(Some("192.0.2.200")))
        else {
            panic!("not applied");
        };
        assert!(revert_in_seconds > 0);
        assert_eq!(profile.lock().unwrap().method, "manual");
        std::thread::sleep(Duration::from_millis(800));
        assert_eq!(profile.lock().unwrap().method, "auto");
        assert_eq!(profile.lock().unwrap().dns, "192.0.2.53");

        // Confirmed in time, it stays.
        ipv4.set(&manual(Some("192.0.2.200")));
        assert_eq!(ipv4.confirm(), ControlResponse::Applied);
        std::thread::sleep(Duration::from_millis(800));
        assert_eq!(profile.lock().unwrap().addresses, "192.0.2.200/24");

        // Back to DHCP clears the manual settings.
        ipv4.set(&Ipv4Change {
            interface: "eth0".into(),
            dhcp: true,
            dns: Vec::new(),
            address: None,
            prefix: None,
            gateway: None,
        });
        assert_eq!(ipv4.confirm(), ControlResponse::Applied);
        assert_eq!(*profile.lock().unwrap(), dhcp_profile());
    }

    #[test]
    fn an_unconfirmed_addressing_change_is_reverted_by_the_next_owner() {
        let dir = tempfile::tempdir().unwrap();
        let journal = RevertJournal::new(dir.path().join("ipv4.json"));
        let profile = Arc::new(Mutex::new(dhcp_profile()));
        // A long window: the owner "restarts" before the timer would fire.
        let mut ipv4 = settings(&profile, Duration::from_secs(3600)).with_journal(journal.clone());
        ipv4.set(&manual(Some("192.0.2.200")));
        assert_eq!(profile.lock().unwrap().method, "manual");
        assert!(journal.read::<RevertRecord>().is_some());
        drop(ipv4);

        let next = settings(&profile, REVERT_AFTER).with_journal(journal.clone());
        assert!(next.reconcile());
        for _ in 0..50 {
            if journal.read::<RevertRecord>().is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(journal.read::<RevertRecord>().is_none());
        assert_eq!(profile.lock().unwrap().method, "auto");

        // A resolver-only change and a confirmed change leave no record.
        let mut ipv4 = settings(&profile, REVERT_AFTER).with_journal(journal.clone());
        ipv4.set(&manual(None));
        assert!(journal.read::<RevertRecord>().is_none());
        ipv4.set(&manual(Some("192.0.2.200")));
        assert_eq!(ipv4.confirm(), ControlResponse::Applied);
        assert!(journal.read::<RevertRecord>().is_none());
    }

    #[test]
    fn a_second_change_in_the_window_still_reverts_to_the_first_state() {
        let profile = Arc::new(Mutex::new(dhcp_profile()));
        let mut ipv4 = settings(&profile, Duration::from_millis(400));
        ipv4.set(&manual(Some("192.0.2.50")));
        ipv4.set(&manual(Some("192.0.2.60")));
        assert_eq!(profile.lock().unwrap().addresses, "192.0.2.60/24");
        assert!(ipv4.revert_in_seconds() > 0);
        std::thread::sleep(Duration::from_millis(1_000));
        assert_eq!(*profile.lock().unwrap(), dhcp_profile());
        assert_eq!(ipv4.revert_in_seconds(), 0);
    }

    #[test]
    fn a_resolver_edit_in_the_window_keeps_the_revert_across_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let journal = RevertJournal::new(dir.path().join("ipv4.json"));
        let profile = Arc::new(Mutex::new(dhcp_profile()));
        let mut ipv4 = settings(&profile, Duration::from_secs(3600)).with_journal(journal.clone());
        ipv4.set(&manual(Some("192.0.2.200")));
        let mut resolvers = manual(None);
        resolvers.dns = vec!["192.0.2.54".into()];
        ipv4.set(&resolvers);
        assert!(ipv4.revert_in_seconds() > 0);
        assert!(journal.read::<RevertRecords>().is_some());
        drop(ipv4);

        let next = settings(&profile, REVERT_AFTER).with_journal(journal.clone());
        assert!(next.reconcile());
        for _ in 0..50 {
            if journal.read::<RevertRecords>().is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(*profile.lock().unwrap(), dhcp_profile());
    }

    #[test]
    fn a_change_on_another_interface_keeps_the_first_ones_revert() {
        let wired = Arc::new(Mutex::new(dhcp_profile()));
        let wireless = Arc::new(Mutex::new(dhcp_profile()));
        let mut nm = FakeNm::wired(&wired);
        nm.wlan = Some(Arc::clone(&wireless));
        let mut ipv4 =
            Ipv4Settings::open(Box::new(nm)).with_revert_after(Duration::from_millis(400));
        ipv4.set(&manual(Some("192.0.2.200")));
        let mut other = manual(Some("198.51.100.77"));
        other.interface = "wlan0".into();
        other.gateway = Some("198.51.100.1".into());
        ipv4.set(&other);
        assert_eq!(wired.lock().unwrap().method, "manual");
        assert_eq!(wireless.lock().unwrap().method, "manual");
        std::thread::sleep(Duration::from_millis(1_000));
        assert_eq!(*wired.lock().unwrap(), dhcp_profile());
        assert_eq!(*wireless.lock().unwrap(), dhcp_profile());
    }

    #[test]
    fn a_failed_change_in_the_window_keeps_the_pending_revert() {
        let dir = tempfile::tempdir().unwrap();
        let journal = RevertJournal::new(dir.path().join("ipv4.json"));
        let profile = Arc::new(Mutex::new(dhcp_profile()));
        let nm = FakeNm::wired(&profile);
        let fail = Arc::clone(&nm.fail_modify);
        let mut ipv4 = Ipv4Settings::open(Box::new(nm))
            .with_revert_after(Duration::from_millis(600))
            .with_journal(journal.clone());
        ipv4.set(&manual(Some("192.0.2.200")));
        fail.store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            ipv4.set(&manual(Some("192.0.2.60"))),
            reject(ControlRejectionCode::BackendRejected)
        );
        assert_eq!(profile.lock().unwrap().addresses, "192.0.2.200/24");
        assert!(ipv4.revert_in_seconds() > 0);
        assert!(journal.read::<RevertRecords>().is_some());
        std::thread::sleep(Duration::from_millis(1_200));
        assert_eq!(*profile.lock().unwrap(), dhcp_profile());
        assert!(journal.read::<RevertRecords>().is_none());
    }

    #[test]
    fn an_older_owners_single_record_is_still_reverted() {
        let dir = tempfile::tempdir().unwrap();
        let journal = RevertJournal::new(dir.path().join("ipv4.json"));
        let profile = Arc::new(Mutex::new(Snapshot {
            method: "manual".into(),
            addresses: "192.0.2.200/24".into(),
            gateway: "192.0.2.5".into(),
            dns: "192.0.2.53".into(),
            ignore_auto_dns: "yes".into(),
        }));
        journal
            .write(&RevertRecord {
                device: "eth0".into(),
                uuid: WIRED_UUID.into(),
                before: dhcp_profile(),
            })
            .unwrap();
        let next = settings(&profile, REVERT_AFTER).with_journal(journal.clone());
        assert!(next.reconcile());
        for _ in 0..50 {
            if journal.read::<RevertRecords>().is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(*profile.lock().unwrap(), dhcp_profile());
    }

    #[test]
    fn ipv6_modes_map_to_network_manager_methods() {
        let profile = Arc::new(Mutex::new(dhcp_profile()));
        let mut ipv4 = settings(&profile, REVERT_AFTER);
        let mode = |response: ControlResponse| match response {
            ControlResponse::NetworkInfo { interfaces, .. } => (
                interfaces[0].ipv6_mode.clone(),
                interfaces[0].ipv6_manual_address.clone(),
            ),
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(mode(ipv4.info()).0, "slaac");
        let change = |mode: &str, address: Option<&str>| Ipv6Change {
            interface: "eth0".into(),
            mode: mode.into(),
            address: address.map(str::to_string),
            prefix: address.map(|_| 64),
            gateway: address.map(|_| "2001:db8::1".to_string()),
        };
        assert_eq!(
            mode(ipv4.set_ipv6(&change("manual", Some("2001:db8::10")))),
            ("manual".to_string(), "2001:db8::10/64".to_string())
        );
        assert_eq!(mode(ipv4.set_ipv6(&change("dhcpv6", None))).0, "dhcpv6");
        assert_eq!(mode(ipv4.set_ipv6(&change("disabled", None))).0, "disabled");
        assert_eq!(
            ipv4.set_ipv6(&change("manual", None)),
            reject(ControlRejectionCode::InvalidState)
        );
        // IPv6 never arms the IPv4 revert.
        assert_eq!(ipv4.revert_in_seconds(), 0);
    }

    #[test]
    fn escaped_values_are_unescaped() {
        assert_eq!(
            values("fe80\\:\\:1/64 | 2001\\:db8\\:\\:10/64", " | "),
            ["fe80::1/64", "2001:db8::10/64"]
        );
        assert_eq!(unescape("a\\\\b"), "a\\b");
    }

    #[test]
    fn invalid_changes_are_refused() {
        let profile = Arc::new(Mutex::new(dhcp_profile()));
        let mut ipv4 = settings(&profile, REVERT_AFTER);
        let mut change = manual(Some("192.0.2.200"));
        change.gateway = Some("198.51.100.1".into());
        assert_eq!(
            ipv4.set(&change),
            reject(ControlRejectionCode::InvalidState)
        );
        let mut change = manual(None);
        change.interface = "wlan9".into();
        assert_eq!(
            ipv4.set(&change),
            reject(ControlRejectionCode::InvalidState)
        );
        assert_eq!(*profile.lock().unwrap(), dhcp_profile());
    }
}
