//! The switches the board owner keeps in system files that a RAW system
//! update does not carry: whether the OpenSSH server, the QUIC gateway and
//! the Tailscale daemon are enabled (unit enablement under
//! `/etc/systemd/system`), and mDNS (`MulticastDNS` in `resolved.conf`).
//!
//! A RAW system update writes a new root file system and carries a fixed list
//! of state paths across, the owner's state directory among them, but not
//! these files: without more the board would come back with the new image's
//! choices (the OpenSSH server on, the QUIC gateway off). The owner records
//! the four in its state directory, with the system image they were recorded
//! under (`/etc/hardened-blikvm-image`). At its start, when the image is
//! another one than the record names, the root was replaced and the owner sets
//! the switches as the record has them; otherwise the record only follows the
//! board. Every change the owner makes is recorded at once, and the Tailscale
//! daemon's enablement, which the administration helper changes, whenever the
//! owner reads it (docs/design/system-raw-update-v1.md).

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::device_settings::MdnsState;
use crate::quic_settings::QUIC_UNIT;
use crate::ssh_settings::SSH_UNIT;
use crate::tailscale::{SYSTEMCTL, TAILSCALED_UNIT, TailscaleRunner};

/// The record, in the owner's state directory (carried by a RAW update and
/// left out of a new card's image).
#[cfg_attr(
    not(all(
        feature = "gadget-media-profile",
        feature = "gadget-hid-profile",
        feature = "gpio-atx-profile"
    )),
    allow(dead_code)
)]
pub const SWITCHES_RECORD: &str = "/var/lib/hardened-blikvm-gadget/system-switches.json";
/// The system image's name and date, written by the SD image builder.
#[cfg_attr(
    not(all(
        feature = "gadget-media-profile",
        feature = "gadget-hid-profile",
        feature = "gpio-atx-profile"
    )),
    allow(dead_code)
)]
pub const IMAGE_IDENTITY: &str = "/etc/hardened-blikvm-image";
const RECORD_SCHEMA: u32 = 1;
const MAX_RECORD_BYTES: u64 = 4096;
const MAX_IDENTITY_BYTES: u64 = 512;
/// `systemctl enable/disable` without the SysV step, which would write
/// outside the owner's sandbox (as `ssh_settings` does).
const ENV: &str = "/usr/bin/env";
const SKIP_SYSV: &str = "SYSTEMCTL_SKIP_SYSV=1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    schema_version: u32,
    /// The image the switches were recorded under.
    image: String,
    ssh: bool,
    quic: bool,
    tailscaled: bool,
    mdns: bool,
}

/// What the owner did at its start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reconciled {
    /// No record (or one that could not be read): one was made from the board.
    Recorded,
    /// The same image: the record follows the board.
    Followed,
    /// Another image: the switches were set as recorded; the units that could
    /// not be set are named.
    Restored { failed: Vec<String> },
}

pub struct SystemSwitches {
    record: PathBuf,
    identity: PathBuf,
    runner: Box<dyn TailscaleRunner>,
}

impl SystemSwitches {
    pub fn open(
        record: impl Into<PathBuf>,
        identity: impl Into<PathBuf>,
        runner: Box<dyn TailscaleRunner>,
    ) -> Self {
        Self {
            record: record.into(),
            identity: identity.into(),
            runner,
        }
    }

    /// `Some(enabled)` for a unit whose enablement systemd states as
    /// `enabled` or `disabled`; `None` for one it does not have, or that is
    /// static, masked or otherwise not the owner's to switch.
    fn enablement(&mut self, unit: &str) -> Option<bool> {
        // `show` answers for a disabled unit and for one that does not exist
        // alike with success (`is-enabled` fails for both, and says why on
        // standard output, which a failed command's answer here does not
        // carry); the latter has no unit file state.
        let answer = self
            .runner
            .run(
                SYSTEMCTL,
                &["show", "--property=UnitFileState", "--value", unit],
            )
            .ok()?;
        match answer.trim() {
            "enabled" => Some(true),
            "disabled" => Some(false),
            _ => None,
        }
    }

    fn set_unit(&mut self, unit: &str, enabled: bool) -> Result<(), String> {
        let verb = if enabled { "enable" } else { "disable" };
        self.runner
            .run(ENV, &[SKIP_SYSV, SYSTEMCTL, verb, "--now", unit])
            .map(|_| ())
    }

    fn image(&self) -> String {
        read_bounded(&self.identity, MAX_IDENTITY_BYTES)
            .map(|text| text.trim().to_string())
            .unwrap_or_default()
    }

    fn observe(
        &mut self,
        image: String,
        mdns: Option<&MdnsState>,
        keep: Option<&Record>,
    ) -> Record {
        // A unit the board does not have keeps what was recorded of it.
        let ssh = self
            .enablement(SSH_UNIT)
            .or(keep.map(|record| record.ssh))
            .unwrap_or(false);
        let quic = self
            .enablement(QUIC_UNIT)
            .or(keep.map(|record| record.quic))
            .unwrap_or(false);
        let tailscaled = self
            .enablement(TAILSCALED_UNIT)
            .or(keep.map(|record| record.tailscaled))
            .unwrap_or(false);
        let mdns = mdns
            .map(|state| state.current())
            .or(keep.map(|record| record.mdns))
            .unwrap_or(false);
        Record {
            schema_version: RECORD_SCHEMA,
            image,
            ssh,
            quic,
            tailscaled,
            mdns,
        }
    }

    fn load(&self) -> Option<Record> {
        let text = read_bounded(&self.record, MAX_RECORD_BYTES)?;
        let record: Record = serde_json::from_str(&text).ok()?;
        (record.schema_version == RECORD_SCHEMA).then_some(record)
    }

    fn store(&self, record: &Record) {
        if let Err(reason) = write_record(&self.record, record) {
            eprintln!("system switches: the record could not be written: {reason}");
        }
    }

    /// At the owner's start: after a new image, set the switches as recorded;
    /// otherwise record what the board has.
    pub fn reconcile(&mut self, mut mdns: Option<&mut MdnsState>) -> Reconciled {
        let image = self.image();
        let Some(recorded) = self.load() else {
            let record = self.observe(image, mdns.as_deref(), None);
            self.store(&record);
            eprintln!("system switches: recorded {record:?}");
            return Reconciled::Recorded;
        };
        if recorded.image == image {
            let record = self.observe(image, mdns.as_deref(), Some(&recorded));
            if record != recorded {
                self.store(&record);
            }
            return Reconciled::Followed;
        }
        let mut failed = Vec::new();
        for (unit, wanted) in [
            (SSH_UNIT, recorded.ssh),
            (QUIC_UNIT, recorded.quic),
            (TAILSCALED_UNIT, recorded.tailscaled),
        ] {
            match self.enablement(unit) {
                Some(enabled) if enabled != wanted => {
                    if let Err(reason) = self.set_unit(unit, wanted) {
                        eprintln!("system switches: {unit} could not be set: {reason}");
                        failed.push(unit.to_string());
                    } else {
                        eprintln!(
                            "system switches: {unit} {} as before the new image",
                            if wanted { "enabled" } else { "disabled" }
                        );
                    }
                }
                Some(_) => {}
                // Not a unit of this image: nothing to set.
                None => {}
            }
        }
        if let Some(state) = mdns.as_deref_mut()
            && state.current() != recorded.mdns
        {
            if matches!(
                state.handle(recorded.mdns),
                bli_kvm_control_protocol::ControlResponse::Applied
            ) {
                eprintln!("system switches: mDNS set as before the new image");
            } else {
                failed.push("mdns".to_string());
            }
        }
        let record = self.observe(image, mdns.as_deref(), Some(&recorded));
        self.store(&record);
        Reconciled::Restored { failed }
    }

    /// A switch the owner has just changed.
    pub fn note(&mut self, switch: Switch, enabled: bool) {
        let image = self.image();
        let mut record = self.load().unwrap_or(Record {
            schema_version: RECORD_SCHEMA,
            image: image.clone(),
            ssh: false,
            quic: false,
            tailscaled: false,
            mdns: false,
        });
        // A change made under a new image that the owner has not reconciled
        // yet cannot happen: reconciling is the first thing it does.
        record.image = image;
        match switch {
            Switch::Ssh => record.ssh = enabled,
            Switch::Quic => record.quic = enabled,
            Switch::Tailscaled => record.tailscaled = enabled,
            Switch::Mdns => record.mdns = enabled,
        }
        if self.load().as_ref() != Some(&record) {
            self.store(&record);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switch {
    Ssh,
    Quic,
    Tailscaled,
    Mdns,
}

fn read_bounded(path: &Path, limit: u64) -> Option<String> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut text = String::new();
    file.take(limit + 1).read_to_string(&mut text).ok()?;
    (text.len() as u64 <= limit).then_some(text)
}

fn write_record(path: &Path, record: &Record) -> Result<(), String> {
    let text = serde_json::to_vec_pretty(record).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.next");
    let mut file = std::fs::File::create(&temporary).map_err(|error| error.to_string())?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|error| error.to_string())?;
    file.write_all(&text).map_err(|error| error.to_string())?;
    file.write_all(b"\n").map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())?;
    if let Some(directory) = path.parent()
        && let Ok(directory) = std::fs::File::open(directory)
    {
        let _ = directory.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video_settings::VideoRestartRunner;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// systemd as far as the owner sees it: the units' enablement, and every
    /// call made.
    #[derive(Clone, Default)]
    struct Systemd {
        units: Arc<Mutex<HashMap<String, bool>>>,
        calls: Arc<Mutex<Vec<String>>>,
        refuse: Arc<Mutex<Vec<String>>>,
    }

    impl Systemd {
        fn with(units: &[(&str, bool)]) -> Self {
            let systemd = Self::default();
            for (unit, enabled) in units {
                systemd
                    .units
                    .lock()
                    .unwrap()
                    .insert(unit.to_string(), *enabled);
            }
            systemd
        }

        fn enabled(&self, unit: &str) -> Option<bool> {
            self.units.lock().unwrap().get(unit).copied()
        }

        fn changes(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|call| !call.contains(" show "))
                .cloned()
                .collect()
        }
    }

    impl TailscaleRunner for Systemd {
        fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{program} {}", args.join(" ")));
            match (program, args) {
                (SYSTEMCTL, ["show", "--property=UnitFileState", "--value", unit]) => {
                    Ok(match self.enabled(unit) {
                        Some(true) => "enabled\n".into(),
                        Some(false) => "disabled\n".into(),
                        None => "\n".into(),
                    })
                }
                (ENV, [SKIP_SYSV, SYSTEMCTL, verb, "--now", unit]) => {
                    if self.refuse.lock().unwrap().iter().any(|u| u == unit) {
                        return Err("refused".into());
                    }
                    let mut units = self.units.lock().unwrap();
                    if !units.contains_key(*unit) {
                        return Err("no such unit".into());
                    }
                    units.insert(unit.to_string(), *verb == "enable");
                    Ok(String::new())
                }
                _ => Err(format!("unexpected call {program} {args:?}")),
            }
        }

        fn path_exists(&mut self, _path: &str) -> bool {
            false
        }
    }

    struct Restarts(Arc<Mutex<Vec<String>>>);

    impl VideoRestartRunner for Restarts {
        fn restart(&mut self, unit: &str) -> Result<(), String> {
            self.0.lock().unwrap().push(unit.to_string());
            Ok(())
        }
    }

    struct Board {
        directory: tempfile::TempDir,
        systemd: Systemd,
        restarts: Arc<Mutex<Vec<String>>>,
    }

    impl Board {
        fn new(image: &str, units: &[(&str, bool)], mdns: bool) -> Self {
            let directory = tempfile::tempdir().unwrap();
            std::fs::write(directory.path().join("image"), format!("{image}\n")).unwrap();
            std::fs::write(
                directory.path().join("resolved.conf"),
                format!(
                    "[Resolve]\nDNS=192.0.2.53\nMulticastDNS={}\n",
                    if mdns { "yes" } else { "no" }
                ),
            )
            .unwrap();
            Self {
                directory,
                systemd: Systemd::with(units),
                restarts: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.directory.path().join(name)
        }

        fn switches(&self) -> SystemSwitches {
            SystemSwitches::open(
                self.path("system-switches.json"),
                self.path("image"),
                Box::new(self.systemd.clone()),
            )
        }

        fn mdns(&self) -> MdnsState {
            MdnsState::open(
                self.path("resolved.conf"),
                "systemd-resolved.service",
                Box::new(Restarts(self.restarts.clone())),
                true,
            )
        }

        fn record(&self) -> serde_json::Value {
            serde_json::from_slice(&std::fs::read(self.path("system-switches.json")).unwrap())
                .unwrap()
        }

        /// A RAW update: a new image with its own choices, the owner's state
        /// directory (and with it the record) carried across.
        fn new_image(&mut self, image: &str, units: &[(&str, bool)], mdns: bool) {
            std::fs::write(self.path("image"), format!("{image}\n")).unwrap();
            for (unit, enabled) in units {
                self.systemd
                    .units
                    .lock()
                    .unwrap()
                    .insert(unit.to_string(), *enabled);
            }
            std::fs::write(
                self.path("resolved.conf"),
                format!(
                    "[Resolve]\nMulticastDNS={}\n",
                    if mdns { "yes" } else { "no" }
                ),
            )
            .unwrap();
            self.systemd.calls.lock().unwrap().clear();
        }
    }

    const ALL: [&str; 3] = [SSH_UNIT, QUIC_UNIT, TAILSCALED_UNIT];

    #[test]
    fn a_board_without_a_record_is_recorded_and_left_as_it_is() {
        let board = Board::new(
            "Hardened BliKVM H616 SD image 0.1.0-beta.1 (2026-10-04)",
            &[
                (SSH_UNIT, false),
                (QUIC_UNIT, true),
                (TAILSCALED_UNIT, true),
            ],
            true,
        );
        let mut mdns = board.mdns();
        assert_eq!(
            board.switches().reconcile(Some(&mut mdns)),
            Reconciled::Recorded
        );
        assert_eq!(board.systemd.changes(), Vec::<String>::new());
        let record = board.record();
        assert_eq!(
            (
                &record["image"],
                &record["ssh"],
                &record["quic"],
                &record["tailscaled"],
                &record["mdns"]
            ),
            (
                &serde_json::json!("Hardened BliKVM H616 SD image 0.1.0-beta.1 (2026-10-04)"),
                &serde_json::json!(false),
                &serde_json::json!(true),
                &serde_json::json!(true),
                &serde_json::json!(true)
            )
        );
        let mode = std::fs::metadata(board.path("system-switches.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    /// The image changed under a carried record: the RAW update's new root
    /// had the OpenSSH server on, the gateway off and mDNS off; the board had
    /// them the other way round, and gets them back.
    #[test]
    fn after_a_new_image_the_switches_are_set_as_the_board_had_them() {
        let mut board = Board::new(
            "Hardened BliKVM H616 SD image 0.1.0-beta.1 (2026-10-04)",
            &[
                (SSH_UNIT, false),
                (QUIC_UNIT, true),
                (TAILSCALED_UNIT, false),
            ],
            true,
        );
        board.switches().reconcile(Some(&mut board.mdns()));
        board.new_image(
            "Hardened BliKVM H616 SD image 0.1.0-beta.3 (2026-10-07)",
            &[
                (SSH_UNIT, true),
                (QUIC_UNIT, false),
                (TAILSCALED_UNIT, true),
            ],
            false,
        );
        let mut mdns = board.mdns();
        assert!(!mdns.current());
        assert_eq!(
            board.switches().reconcile(Some(&mut mdns)),
            Reconciled::Restored { failed: vec![] }
        );
        assert_eq!(
            ALL.map(|unit| board.systemd.enabled(unit)),
            [Some(false), Some(true), Some(false)]
        );
        let mut changes = board.systemd.changes();
        changes.sort();
        assert_eq!(
            changes,
            vec![
                format!("{ENV} {SKIP_SYSV} {SYSTEMCTL} disable --now {SSH_UNIT}"),
                format!("{ENV} {SKIP_SYSV} {SYSTEMCTL} disable --now {TAILSCALED_UNIT}"),
                format!("{ENV} {SKIP_SYSV} {SYSTEMCTL} enable --now {QUIC_UNIT}"),
            ]
        );
        assert!(mdns.current());
        assert_eq!(
            board.restarts.lock().unwrap().as_slice(),
            ["systemd-resolved.service"]
        );
        // The record names the new image now, and the next start leaves
        // everything as it is.
        assert_eq!(
            board.record()["image"],
            "Hardened BliKVM H616 SD image 0.1.0-beta.3 (2026-10-07)"
        );
        board.systemd.calls.lock().unwrap().clear();
        assert_eq!(
            board.switches().reconcile(Some(&mut board.mdns())),
            Reconciled::Followed
        );
        assert_eq!(board.systemd.changes(), Vec::<String>::new());
    }

    /// Under the same image the owner never switches anything: a unit
    /// changed behind its back is recorded, not reverted.
    #[test]
    fn under_the_same_image_the_record_follows_the_board() {
        let board = Board::new(
            "image A",
            &[
                (SSH_UNIT, true),
                (QUIC_UNIT, false),
                (TAILSCALED_UNIT, true),
            ],
            false,
        );
        board.switches().reconcile(Some(&mut board.mdns()));
        board
            .systemd
            .units
            .lock()
            .unwrap()
            .insert(TAILSCALED_UNIT.to_string(), false);
        assert_eq!(
            board.switches().reconcile(Some(&mut board.mdns())),
            Reconciled::Followed
        );
        assert_eq!(board.systemd.changes(), Vec::<String>::new());
        assert_eq!(board.record()["tailscaled"], false);
    }

    #[test]
    fn a_change_the_owner_makes_is_recorded_at_once() {
        let board = Board::new(
            "image A",
            &[
                (SSH_UNIT, true),
                (QUIC_UNIT, false),
                (TAILSCALED_UNIT, true),
            ],
            false,
        );
        let mut switches = board.switches();
        switches.reconcile(Some(&mut board.mdns()));
        switches.note(Switch::Quic, true);
        switches.note(Switch::Ssh, false);
        switches.note(Switch::Mdns, true);
        switches.note(Switch::Tailscaled, false);
        let record = board.record();
        assert_eq!(
            (
                &record["ssh"],
                &record["quic"],
                &record["tailscaled"],
                &record["mdns"]
            ),
            (
                &serde_json::json!(false),
                &serde_json::json!(true),
                &serde_json::json!(false),
                &serde_json::json!(true)
            )
        );
    }

    /// A unit the new image does not have, or refuses to switch, is left out
    /// and named; the others are set, and what was recorded of the missing
    /// one is kept for an image that has it.
    #[test]
    fn a_unit_that_cannot_be_set_does_not_stop_the_others() {
        let mut board = Board::new(
            "image A",
            &[
                (SSH_UNIT, false),
                (QUIC_UNIT, true),
                (TAILSCALED_UNIT, true),
            ],
            false,
        );
        board.switches().reconcile(None);
        board.new_image("image B", &[(SSH_UNIT, true), (QUIC_UNIT, false)], false);
        board.systemd.units.lock().unwrap().remove(TAILSCALED_UNIT);
        board
            .systemd
            .refuse
            .lock()
            .unwrap()
            .push(SSH_UNIT.to_string());
        assert_eq!(
            board.switches().reconcile(None),
            Reconciled::Restored {
                failed: vec![SSH_UNIT.to_string()]
            }
        );
        assert_eq!(board.systemd.enabled(QUIC_UNIT), Some(true));
        assert_eq!(board.record()["tailscaled"], true);
    }

    #[test]
    fn a_record_that_cannot_be_read_is_made_again_and_nothing_is_switched() {
        for content in [
            &b"not json"[..],
            br#"{"schemaVersion":2,"image":"x","ssh":true,"quic":true,"tailscaled":true,"mdns":true}"#,
            br#"{"schemaVersion":1,"image":"x","ssh":true,"quic":true,"tailscaled":true,"mdns":true,"more":1}"#,
            &[b'{'; 5000][..],
        ] {
            let board = Board::new(
                "image A",
                &[(SSH_UNIT, true), (QUIC_UNIT, false), (TAILSCALED_UNIT, false)],
                false,
            );
            std::fs::write(board.path("system-switches.json"), content).unwrap();
            assert_eq!(board.switches().reconcile(None), Reconciled::Recorded);
            assert_eq!(board.systemd.changes(), Vec::<String>::new());
            assert_eq!(board.record()["image"], "image A");
        }
    }

    /// A board whose image has no name (built by hand) still tells a RAW
    /// update's image from its own.
    #[test]
    fn a_board_without_an_image_name_is_recorded_under_none() {
        let mut board = Board::new(
            "",
            &[
                (SSH_UNIT, true),
                (QUIC_UNIT, true),
                (TAILSCALED_UNIT, false),
            ],
            false,
        );
        std::fs::remove_file(board.path("image")).unwrap();
        board.switches().reconcile(None);
        assert_eq!(board.record()["image"], "");
        board.new_image("image B", &[(QUIC_UNIT, false)], false);
        assert_eq!(
            board.switches().reconcile(None),
            Reconciled::Restored { failed: vec![] }
        );
        assert_eq!(board.systemd.enabled(QUIC_UNIT), Some(true));
    }
}
