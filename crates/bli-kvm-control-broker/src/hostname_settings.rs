//! The reference's hostname setting, applied by the board owner.
//!
//! The new name goes to `systemd-hostnamed` (`hostnamectl`), which writes
//! `/etc/hostname` and applies it at once, mDNS included; the old name is
//! replaced as a whole word in `/etc/hosts` so the host still resolves itself.

use std::path::{Path, PathBuf};

use bli_kvm_control_protocol::{ControlRejectionCode, ControlResponse};

use crate::tailscale::TailscaleRunner;

pub const HOSTS: &str = "/etc/hosts";
pub const HOSTNAME: &str = "/etc/hostname";
const HOSTNAMECTL: &str = "/usr/bin/hostnamectl";

pub struct HostnameSettings {
    runner: Box<dyn TailscaleRunner>,
    hosts: PathBuf,
    hostname: PathBuf,
}

/// Replaces `old` where it is a whole whitespace-separated word.
fn rename_in_hosts(text: &str, old: &str, new: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let (content, ending) = match line.strip_suffix('\n') {
            Some(content) => (content, "\n"),
            None => (line, ""),
        };
        if content.trim_start().starts_with('#') {
            out.push_str(line);
            continue;
        }
        let mut rebuilt = String::with_capacity(content.len());
        let mut word = String::new();
        let flush = |word: &mut String, rebuilt: &mut String| {
            if word == old {
                rebuilt.push_str(new);
            } else {
                rebuilt.push_str(word);
            }
            word.clear();
        };
        for ch in content.chars() {
            if ch.is_whitespace() {
                flush(&mut word, &mut rebuilt);
                rebuilt.push(ch);
            } else {
                word.push(ch);
            }
        }
        flush(&mut word, &mut rebuilt);
        out.push_str(&rebuilt);
        out.push_str(ending);
    }
    out
}

impl HostnameSettings {
    pub fn open(runner: Box<dyn TailscaleRunner>) -> Self {
        Self::with_paths(runner, HOSTS, HOSTNAME)
    }

    pub fn with_paths(
        runner: Box<dyn TailscaleRunner>,
        hosts: impl Into<PathBuf>,
        hostname: impl Into<PathBuf>,
    ) -> Self {
        Self {
            runner,
            hosts: hosts.into(),
            hostname: hostname.into(),
        }
    }

    pub fn set(&mut self, name: &str) -> ControlResponse {
        if !bli_kvm_core::valid_hostname(name) {
            return reject(ControlRejectionCode::InvalidState);
        }
        let old = std::fs::read_to_string(&self.hostname)
            .map(|text| text.trim().to_string())
            .unwrap_or_default();
        if old == name {
            return ControlResponse::Applied;
        }
        let hosts_before = std::fs::read_to_string(&self.hosts).unwrap_or_default();
        let renamed = if old.is_empty() {
            hosts_before.clone()
        } else {
            rename_in_hosts(&hosts_before, &old, name)
        };
        if renamed != hosts_before
            && let Err(reason) =
                crate::device_settings::write_conf(Path::new(&self.hosts), &renamed)
        {
            eprintln!("hostname: /etc/hosts could not be updated: {reason}");
            return reject(ControlRejectionCode::BackendRejected);
        }
        match self.runner.run(HOSTNAMECTL, &["set-hostname", name]) {
            Ok(_) => {
                eprintln!("hostname: the device is now called {name}");
                ControlResponse::Applied
            }
            Err(reason) => {
                eprintln!("hostname: hostnamectl failed: {reason}");
                if renamed != hosts_before {
                    let _ =
                        crate::device_settings::write_conf(Path::new(&self.hosts), &hosts_before);
                }
                reject(ControlRejectionCode::BackendRejected)
            }
        }
    }
}

fn reject(code: ControlRejectionCode) -> ControlResponse {
    ControlResponse::Rejected { code }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct Recorder(Arc<Mutex<Vec<String>>>, PathBuf);

    impl TailscaleRunner for Recorder {
        fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String> {
            self.0
                .lock()
                .unwrap()
                .push(format!("{program} {}", args.join(" ")));
            std::fs::write(&self.1, format!("{}\n", args[1])).unwrap();
            Ok(String::new())
        }

        fn path_exists(&mut self, _path: &str) -> bool {
            true
        }
    }

    #[test]
    fn the_hosts_file_follows_the_new_name() {
        assert_eq!(
            rename_in_hosts(
                "127.0.0.1 localhost\n127.0.1.1\tkvm kvm.local\n# kvm stays\n::1 kvm-other\n",
                "kvm",
                "desk"
            ),
            "127.0.0.1 localhost\n127.0.1.1\tdesk kvm.local\n# kvm stays\n::1 kvm-other\n"
        );
        let dir = tempfile::tempdir().unwrap();
        let hosts = dir.path().join("hosts");
        let hostname = dir.path().join("hostname");
        std::fs::write(&hosts, "127.0.1.1 kvm\n").unwrap();
        std::fs::write(&hostname, "kvm\n").unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut settings = HostnameSettings::with_paths(
            Box::new(Recorder(Arc::clone(&calls), hostname.clone())),
            &hosts,
            &hostname,
        );
        assert_eq!(settings.set("desk-1"), ControlResponse::Applied);
        assert_eq!(
            std::fs::read_to_string(&hosts).unwrap(),
            "127.0.1.1 desk-1\n"
        );
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            ["/usr/bin/hostnamectl set-hostname desk-1"]
        );
        assert_eq!(settings.set("desk-1"), ControlResponse::Applied);
        assert_eq!(calls.lock().unwrap().len(), 1);
        assert_eq!(
            settings.set("-bad"),
            reject(ControlRejectionCode::InvalidState)
        );
    }
}
