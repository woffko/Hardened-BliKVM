//! Bounded network writes owned by the board owner.
//!
//! The unprivileged server cannot write `/etc/resolv.conf` or the sysctl tree,
//! so the reviewed writes live behind the owner control socket with a
//! capability gate. Every write is validated, applied once, and read back; the
//! read-back is what the caller reports, never the intent.

use std::{
    fs,
    net::IpAddr,
    path::{Path, PathBuf},
};

/// The reference GUI allows at most three resolvers.
pub const MAX_DNS_SERVERS: usize = 3;
pub const MAX_NAMESERVER_LINE_BYTES: usize = 64;
pub const MAX_RESOLV_CONF_BYTES: usize = 512;
pub const MAX_INTERFACE_NAME_BYTES: usize = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkWriteError {
    InvalidRequest,
    Refused,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkWriteOutcome {
    Applied { read_back: String },
    Refused,
    Unavailable,
}

pub trait NetworkWriter {
    /// Replace the resolver configuration and return what the file now holds.
    fn write_resolv_conf(&self, content: &str) -> Result<String, NetworkWriteError>;

    /// Set one interface's `disable_ipv6` and return the value the kernel now
    /// reports.
    fn write_ipv6_disabled(
        &self,
        interface: &str,
        disabled: bool,
    ) -> Result<String, NetworkWriteError>;
}

/// A writer that never writes: an unconfigured profile must refuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefusingWriter;

impl NetworkWriter for RefusingWriter {
    fn write_resolv_conf(&self, _content: &str) -> Result<String, NetworkWriteError> {
        Err(NetworkWriteError::Refused)
    }

    fn write_ipv6_disabled(
        &self,
        _interface: &str,
        _disabled: bool,
    ) -> Result<String, NetworkWriteError> {
        Err(NetworkWriteError::Refused)
    }
}

/// The reviewed writer: one atomic resolver replacement and one sysctl write.
#[derive(Debug)]
pub struct FileWriter {
    resolv_conf: PathBuf,
    ipv6_root: PathBuf,
}

impl FileWriter {
    pub fn new(resolv_conf: impl Into<PathBuf>, ipv6_root: impl Into<PathBuf>) -> Self {
        Self {
            resolv_conf: resolv_conf.into(),
            ipv6_root: ipv6_root.into(),
        }
    }
}

impl NetworkWriter for FileWriter {
    fn write_resolv_conf(&self, content: &str) -> Result<String, NetworkWriteError> {
        let metadata =
            fs::symlink_metadata(&self.resolv_conf).map_err(|_| NetworkWriteError::Refused)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(NetworkWriteError::Refused);
        }
        let directory = self
            .resolv_conf
            .parent()
            .ok_or(NetworkWriteError::Refused)?;
        let temporary = directory.join(".resolv.conf.hardened-blikvm");
        fs::write(&temporary, content).map_err(|_| NetworkWriteError::Unavailable)?;
        fs::rename(&temporary, &self.resolv_conf).map_err(|_| NetworkWriteError::Unavailable)?;
        fs::read_to_string(&self.resolv_conf).map_err(|_| NetworkWriteError::Unavailable)
    }

    fn write_ipv6_disabled(
        &self,
        interface: &str,
        disabled: bool,
    ) -> Result<String, NetworkWriteError> {
        let path = self.ipv6_root.join(interface).join("disable_ipv6");
        fs::write(&path, if disabled { "1\n" } else { "0\n" })
            .map_err(|_| NetworkWriteError::Unavailable)?;
        let value = fs::read_to_string(&path).map_err(|_| NetworkWriteError::Unavailable)?;
        Ok(value.trim().to_string())
    }
}

/// A resolver list is accepted only when every entry is a literal address, so a
/// write cannot smuggle configuration into the file.
pub fn render_resolv_conf(servers: &[String]) -> Result<String, NetworkWriteError> {
    if servers.is_empty() || servers.len() > MAX_DNS_SERVERS {
        return Err(NetworkWriteError::InvalidRequest);
    }
    let mut content = String::new();
    for server in servers {
        if server.trim() != server || server.len() > MAX_NAMESERVER_LINE_BYTES {
            return Err(NetworkWriteError::InvalidRequest);
        }
        let address: IpAddr = server
            .parse()
            .map_err(|_| NetworkWriteError::InvalidRequest)?;
        content.push_str("nameserver ");
        content.push_str(&address.to_string());
        content.push('\n');
    }
    if content.len() > MAX_RESOLV_CONF_BYTES {
        return Err(NetworkWriteError::InvalidRequest);
    }
    Ok(content)
}

/// A network interface name is the kernel's own restricted charset.
pub fn normalize_interface(interface: &str) -> Result<&str, NetworkWriteError> {
    if interface.is_empty()
        || interface.len() > MAX_INTERFACE_NAME_BYTES
        || !interface
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
    {
        return Err(NetworkWriteError::InvalidRequest);
    }
    Ok(interface)
}

pub struct NetworkWriteState<W: NetworkWriter> {
    writer: W,
    enabled: bool,
}

impl<W: NetworkWriter> NetworkWriteState<W> {
    pub const fn new(writer: W, enabled: bool) -> Self {
        Self { writer, enabled }
    }

    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_dns(&self, servers: &[String]) -> NetworkWriteOutcome {
        if !self.enabled {
            return NetworkWriteOutcome::Unavailable;
        }
        let content = match render_resolv_conf(servers) {
            Ok(content) => content,
            Err(_) => return NetworkWriteOutcome::Refused,
        };
        match self.writer.write_resolv_conf(&content) {
            Ok(read_back) => NetworkWriteOutcome::Applied { read_back },
            Err(NetworkWriteError::Unavailable) => NetworkWriteOutcome::Unavailable,
            Err(_) => NetworkWriteOutcome::Refused,
        }
    }

    pub fn set_ipv6(&self, interface: &str, disabled: bool) -> NetworkWriteOutcome {
        if !self.enabled {
            return NetworkWriteOutcome::Unavailable;
        }
        let interface = match normalize_interface(interface) {
            Ok(interface) => interface,
            Err(_) => return NetworkWriteOutcome::Refused,
        };
        match self.writer.write_ipv6_disabled(interface, disabled) {
            Ok(read_back) => NetworkWriteOutcome::Applied { read_back },
            Err(NetworkWriteError::Unavailable) => NetworkWriteOutcome::Unavailable,
            Err(_) => NetworkWriteOutcome::Refused,
        }
    }
}

/// The reviewed device paths.
pub fn file_writer() -> FileWriter {
    FileWriter::new("/etc/resolv.conf", "/proc/sys/net/ipv6/conf")
}

pub fn resolv_conf_path(writer: &FileWriter) -> &Path {
    &writer.resolv_conf
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[derive(Default)]
    struct RecordingWriter {
        resolv: RefCell<Vec<String>>,
        ipv6: RefCell<Vec<(String, bool)>>,
        fail: Option<NetworkWriteError>,
    }

    impl NetworkWriter for RecordingWriter {
        fn write_resolv_conf(&self, content: &str) -> Result<String, NetworkWriteError> {
            if let Some(error) = self.fail {
                return Err(error);
            }
            self.resolv.borrow_mut().push(content.to_string());
            Ok(content.to_string())
        }

        fn write_ipv6_disabled(
            &self,
            interface: &str,
            disabled: bool,
        ) -> Result<String, NetworkWriteError> {
            if let Some(error) = self.fail {
                return Err(error);
            }
            self.ipv6
                .borrow_mut()
                .push((interface.to_string(), disabled));
            Ok(if disabled { "1" } else { "0" }.to_string())
        }
    }

    #[test]
    fn resolver_lists_are_literal_and_bounded() {
        let servers = vec!["1.1.1.1".to_string(), "2606:4700:4700::1111".to_string()];
        assert_eq!(
            render_resolv_conf(&servers).unwrap(),
            "nameserver 1.1.1.1\nnameserver 2606:4700:4700::1111\n"
        );
        for invalid in [
            vec![],
            vec!["1.1.1.1".to_string(); MAX_DNS_SERVERS + 1],
            vec!["1.1.1.1 # comment".to_string()],
            vec![" 1.1.1.1".to_string()],
            vec!["search example.org".to_string()],
            vec!["nameserver 1.1.1.1".to_string()],
            vec!["1.1.1.1\nnameserver 8.8.8.8".to_string()],
        ] {
            assert_eq!(
                render_resolv_conf(&invalid),
                Err(NetworkWriteError::InvalidRequest)
            );
        }
    }

    #[test]
    fn interface_names_use_the_kernel_charset() {
        assert_eq!(normalize_interface("wlan0").unwrap(), "wlan0");
        assert_eq!(normalize_interface("eth0.10").unwrap(), "eth0.10");
        for invalid in ["", "wlan 0", "wlan0;id", "a".repeat(16).as_str(), "../etc"] {
            assert_eq!(
                normalize_interface(invalid),
                Err(NetworkWriteError::InvalidRequest)
            );
        }
    }

    #[test]
    fn writes_apply_once_and_report_the_read_back() {
        let state = NetworkWriteState::new(RecordingWriter::default(), true);
        assert_eq!(
            state.set_dns(&["1.1.1.1".to_string()]),
            NetworkWriteOutcome::Applied {
                read_back: "nameserver 1.1.1.1\n".to_string()
            }
        );
        assert_eq!(
            state.set_ipv6("wlan0", true),
            NetworkWriteOutcome::Applied {
                read_back: "1".to_string()
            }
        );
        assert_eq!(
            state.set_dns(&["not-an-address".to_string()]),
            NetworkWriteOutcome::Refused
        );
    }

    #[test]
    fn a_disabled_or_unavailable_writer_never_applies() {
        let disabled = NetworkWriteState::new(RecordingWriter::default(), false);
        assert_eq!(
            disabled.set_dns(&["1.1.1.1".to_string()]),
            NetworkWriteOutcome::Unavailable
        );
        let refusing = NetworkWriteState::new(RefusingWriter, true);
        assert_eq!(
            refusing.set_dns(&["1.1.1.1".to_string()]),
            NetworkWriteOutcome::Refused
        );
        let mut broken = RecordingWriter::default();
        broken.fail = Some(NetworkWriteError::Unavailable);
        let state = NetworkWriteState::new(broken, true);
        assert_eq!(
            state.set_ipv6("wlan0", false),
            NetworkWriteOutcome::Unavailable
        );
    }

    #[test]
    fn the_file_writer_replaces_atomically_and_refuses_symlinks() {
        let directory = tempfile::tempdir().unwrap();
        let resolv = directory.path().join("resolv.conf");
        let ipv6 = directory.path().join("ipv6");
        std::fs::create_dir(&ipv6).unwrap();
        std::fs::write(&resolv, "nameserver 9.9.9.9\n").unwrap();
        let writer = FileWriter::new(&resolv, &ipv6);
        assert_eq!(
            writer.write_resolv_conf("nameserver 1.1.1.1\n").unwrap(),
            "nameserver 1.1.1.1\n"
        );

        let interface = ipv6.join("wlan0");
        std::fs::create_dir(&interface).unwrap();
        std::fs::write(interface.join("disable_ipv6"), "0\n").unwrap();
        assert_eq!(writer.write_ipv6_disabled("wlan0", true).unwrap(), "1");

        let target = directory.path().join("target.conf");
        std::fs::write(&target, "nameserver 9.9.9.9\n").unwrap();
        let link = directory.path().join("link.conf");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let linked = FileWriter::new(&link, &ipv6);
        assert_eq!(
            linked.write_resolv_conf("nameserver 1.1.1.1\n"),
            Err(NetworkWriteError::Refused)
        );
        assert_eq!(
            writer.write_ipv6_disabled("missing", true),
            Err(NetworkWriteError::Unavailable)
        );
    }
}
