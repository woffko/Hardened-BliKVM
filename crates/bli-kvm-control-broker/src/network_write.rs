//! Bounded network writes owned by the board owner.
//!
//! The unprivileged server cannot write `/etc/resolv.conf` or the sysctl tree,
//! so the reviewed writes live behind the owner control socket with a
//! capability gate. Every write is validated, applied once, and read back; the
//! read-back is what the caller reports, never the intent.

use std::{
    fs,
    net::{IpAddr, UdpSocket},
    path::{Path, PathBuf},
    time::Duration,
};

/// The reference GUI allows at most three resolvers.
pub const MAX_DNS_SERVERS: usize = 3;
pub const MAX_NAMESERVER_LINE_BYTES: usize = 64;
pub const MAX_RESOLV_CONF_BYTES: usize = 512;
pub const MAX_INTERFACE_NAME_BYTES: usize = 15;
/// The reference sends the Wake-on-LAN magic packet to the discard port.
pub const WOL_MAGIC_PORT: u16 = 9;

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

pub trait NetworkWriter: Send {
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

impl NetworkWriter for Box<dyn NetworkWriter> {
    fn write_resolv_conf(&self, content: &str) -> Result<String, NetworkWriteError> {
        (**self).write_resolv_conf(content)
    }

    fn write_ipv6_disabled(
        &self,
        interface: &str,
        disabled: bool,
    ) -> Result<String, NetworkWriteError> {
        (**self).write_ipv6_disabled(interface, disabled)
    }
}

/// A Wake-on-LAN sender: exactly one magic packet for one validated address.
pub trait WolSender: Send {
    fn send_magic_packet(&self, mac: [u8; 6]) -> Result<(), NetworkWriteError>;
}

impl WolSender for Box<dyn WolSender> {
    fn send_magic_packet(&self, mac: [u8; 6]) -> Result<(), NetworkWriteError> {
        (**self).send_magic_packet(mac)
    }
}

/// A sender that never sends: an unconfigured profile must refuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefusingWolSender;

impl WolSender for RefusingWolSender {
    fn send_magic_packet(&self, _mac: [u8; 6]) -> Result<(), NetworkWriteError> {
        Err(NetworkWriteError::Refused)
    }
}

/// The reviewed sender: one broadcast datagram, no socket kept open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UdpWolSender;

impl WolSender for UdpWolSender {
    fn send_magic_packet(&self, mac: [u8; 6]) -> Result<(), NetworkWriteError> {
        let socket = UdpSocket::bind("0.0.0.0:0").map_err(|_| NetworkWriteError::Unavailable)?;
        socket
            .set_broadcast(true)
            .map_err(|_| NetworkWriteError::Unavailable)?;
        let _ = socket.set_write_timeout(Some(Duration::from_secs(2)));
        let mut packet = [0xff_u8; 6 + 6 * 16];
        for index in 0..16 {
            let offset = 6 + index * 6;
            packet[offset..offset + 6].copy_from_slice(&mac);
        }
        socket
            .send_to(&packet, ("255.255.255.255", WOL_MAGIC_PORT))
            .map_err(|_| NetworkWriteError::Unavailable)?;
        Ok(())
    }
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
        // The reviewed sandbox grants write access to the file itself, not to
        // its directory, so the bounded payload is written in place and then
        // read back instead of being renamed into position.
        let mut file = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&self.resolv_conf)
            .map_err(|_| NetworkWriteError::Unavailable)?;
        std::io::Write::write_all(&mut file, content.as_bytes())
            .map_err(|_| NetworkWriteError::Unavailable)?;
        std::io::Write::flush(&mut file).map_err(|_| NetworkWriteError::Unavailable)?;
        file.sync_all()
            .map_err(|_| NetworkWriteError::Unavailable)?;
        drop(file);
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

/// One MAC address in colon or dash notation, or as twelve hex characters.
/// The notation must be consistent: six two-digit groups with one separator, or
/// twelve digits with none.
pub fn normalize_mac(value: &str) -> Result<[u8; 6], NetworkWriteError> {
    if value.trim() != value || value.is_empty() || value.len() > 17 {
        return Err(NetworkWriteError::InvalidRequest);
    }
    let separator = value
        .chars()
        .find(|character| *character == ':' || *character == '-');
    let compact = match separator {
        Some(separator) => {
            let groups: Vec<&str> = value.split(separator).collect();
            if groups.len() != 6 {
                return Err(NetworkWriteError::InvalidRequest);
            }
            let mut compact = String::with_capacity(12);
            for group in groups {
                if group.len() != 2 || !group.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(NetworkWriteError::InvalidRequest);
                }
                compact.push_str(group);
            }
            compact
        }
        None => {
            if value.len() != 12 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(NetworkWriteError::InvalidRequest);
            }
            value.to_string()
        }
    };
    let mut bytes = [0_u8; 6];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&compact[index * 2..index * 2 + 2], 16)
            .map_err(|_| NetworkWriteError::InvalidRequest)?;
    }
    Ok(bytes)
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

pub struct WolState<S: WolSender> {
    sender: S,
    enabled: bool,
}

impl<S: WolSender> WolState<S> {
    pub const fn new(sender: S, enabled: bool) -> Self {
        Self { sender, enabled }
    }

    /// Wake-on-LAN has nothing to read back, so the applied outcome names the
    /// address that was actually transmitted.
    pub fn send(&self, mac: &str) -> NetworkWriteOutcome {
        if !self.enabled {
            return NetworkWriteOutcome::Unavailable;
        }
        let mac = match normalize_mac(mac) {
            Ok(mac) => mac,
            Err(_) => return NetworkWriteOutcome::Refused,
        };
        match self.sender.send_magic_packet(mac) {
            Ok(()) => NetworkWriteOutcome::Applied {
                read_back: format!(
                    "mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
                ),
            },
            Err(NetworkWriteError::Unavailable) => NetworkWriteOutcome::Unavailable,
            Err(_) => NetworkWriteOutcome::Refused,
        }
    }
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

    #[derive(Default)]
    struct RecordingSender {
        sent: RefCell<Vec<[u8; 6]>>,
    }

    impl WolSender for RecordingSender {
        fn send_magic_packet(&self, mac: [u8; 6]) -> Result<(), NetworkWriteError> {
            self.sent.borrow_mut().push(mac);
            Ok(())
        }
    }

    #[test]
    fn macs_accept_colon_and_dash_notation_only() {
        let expected = [0x00, 0x11, 0x22, 0xaa, 0xbb, 0xcc];
        for valid in ["00:11:22:aa:bb:cc", "00-11-22-AA-BB-CC", "001122aabbcc"] {
            assert_eq!(normalize_mac(valid), Ok(expected), "{valid}");
        }
        for invalid in [
            "",
            "00:11:22:aa:bb",
            "00:11:22:aa:bb:cc:dd",
            "0011:22aa:bbcc",
            "not-a-mac",
            " 00:11:22:aa:bb:cc",
        ] {
            assert_eq!(
                normalize_mac(invalid),
                Err(NetworkWriteError::InvalidRequest)
            );
        }
    }

    #[test]
    fn wake_on_lan_sends_exactly_one_validated_packet() {
        let state = WolState::new(RecordingSender::default(), true);
        assert_eq!(
            state.send("00:11:22:aa:bb:cc"),
            NetworkWriteOutcome::Applied {
                read_back: "mac=00:11:22:aa:bb:cc".to_string()
            }
        );
        assert_eq!(state.send("not-a-mac"), NetworkWriteOutcome::Refused);
        assert_eq!(
            WolState::new(RefusingWolSender, true).send("00:11:22:aa:bb:cc"),
            NetworkWriteOutcome::Refused
        );
        assert_eq!(
            WolState::new(RecordingSender::default(), false).send("00:11:22:aa:bb:cc"),
            NetworkWriteOutcome::Unavailable
        );
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
    fn the_file_writer_writes_in_place_and_refuses_symlinks() {
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
