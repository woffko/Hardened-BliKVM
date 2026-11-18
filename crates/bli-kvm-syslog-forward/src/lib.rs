//! The board's system log forwarder (docs/baselines/work-plan-20261117-system-log.md, L2).
//!
//! journald keeps the log in memory (Armbian's ramlog). This program gets a
//! copy of each message journald forwards to the syslog socket, and the
//! kernel's from `/dev/kmsg`, which journald does not forward. It keeps the
//! messages of priority err and above in a bounded file on the card, so the
//! last errors before a hang survive it, and sends the messages up to the
//! configured priority to a remote syslog host over UDP when one is set.
#![forbid(unsafe_code)]

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The most urgent severities kept on the card: emerg, alert, crit, err.
pub const KEPT_SEVERITY: u8 = 3;
/// Severities synced to the card at once: emerg, alert, crit.
pub const SYNCED_SEVERITY: u8 = 2;
/// The card file's size before it is rotated once.
pub const STORE_LIMIT_BYTES: u64 = 256 * 1024;
/// The longest message line kept or sent.
pub const MAX_LINE_BYTES: usize = 2048;
/// Lines the card takes in a burst, and how many a second refill it: a storm
/// of errors cannot keep the card busy.
pub const STORE_BURST: u32 = 100;
pub const STORE_PER_SECOND: u32 = 10;

const KERNEL_FACILITY: u8 = 0;

/// The settings the board owner writes (`/etc/hardened-blikvm/syslog.env`,
/// passed by systemd as the environment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// `host:port` of the remote syslog receiver, or none.
    pub remote: Option<(String, u16)>,
    /// Messages less urgent than this are not sent (busybox's `-l`: 1 to 8,
    /// 8 sends every severity).
    pub priority: u8,
    /// Whether the kernel's lines are sent to the remote host too.
    pub kernel: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            remote: None,
            priority: 8,
            kernel: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsError {
    Remote,
    Priority,
    Kernel,
}

impl Settings {
    /// Reads the settings from the environment's values; an absent value
    /// keeps its default, an invalid one is refused.
    pub fn from_values(
        remote: Option<&str>,
        priority: Option<&str>,
        kernel: Option<&str>,
    ) -> Result<Self, SettingsError> {
        let mut settings = Self::default();
        if let Some(remote) = remote.map(str::trim).filter(|value| !value.is_empty()) {
            let (host, port) = remote.rsplit_once(':').ok_or(SettingsError::Remote)?;
            let port: u16 = port.parse().map_err(|_| SettingsError::Remote)?;
            if port == 0 || !valid_host(host) {
                return Err(SettingsError::Remote);
            }
            settings.remote = Some((host.to_string(), port));
        }
        if let Some(priority) = priority.map(str::trim).filter(|value| !value.is_empty()) {
            let value: u8 = priority.parse().map_err(|_| SettingsError::Priority)?;
            if !(1..=8).contains(&value) {
                return Err(SettingsError::Priority);
            }
            settings.priority = value;
        }
        if let Some(kernel) = kernel.map(str::trim).filter(|value| !value.is_empty()) {
            settings.kernel = match kernel {
                "1" | "yes" | "true" => true,
                "0" | "no" | "false" => false,
                _ => return Err(SettingsError::Kernel),
            };
        }
        Ok(settings)
    }
}

/// A host name or an address: letters, digits, dots, hyphens, and the colons
/// of an IPv6 address in brackets.
pub fn valid_host(host: &str) -> bool {
    let bare = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host);
    !bare.is_empty()
        && bare.len() <= 253
        && bare
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':'))
        && (bare == host || bare.contains(':'))
        && !bare.starts_with('-')
}

/// One message: where it came from, its severity and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub facility: u8,
    pub severity: u8,
    /// `tag: text`, without the priority and the timestamp.
    pub body: String,
}

impl Message {
    pub fn is_kernel(&self) -> bool {
        self.facility == KERNEL_FACILITY
    }
}

/// Parses a datagram journald forwards: `<PRI>Mmm dd hh:mm:ss tag: text`.
pub fn parse_forwarded(datagram: &[u8]) -> Option<Message> {
    let text = String::from_utf8_lossy(datagram);
    let rest = text.strip_prefix('<')?;
    let (priority, rest) = rest.split_once('>')?;
    let priority: u16 = priority.parse().ok()?;
    if priority > 191 {
        return None;
    }
    // The RFC 3164 timestamp is 15 characters and a space.
    let body = if rest.len() > 16 && rest.as_bytes()[3] == b' ' && rest.as_bytes()[15] == b' ' {
        &rest[16..]
    } else {
        rest
    };
    Some(Message {
        facility: (priority / 8) as u8,
        severity: (priority % 8) as u8,
        body: clean(body),
    })
}

/// A kernel record read from `/dev/kmsg`: `pri,seq,usec,flags;text`, with
/// continuation lines (` KEY=value`) dropped.
pub fn parse_kmsg(record: &[u8]) -> Option<Message> {
    let text = String::from_utf8_lossy(record);
    let (fields, rest) = text.split_once(';')?;
    let priority: u16 = fields.split(',').next()?.parse().ok()?;
    let line = rest.split('\n').next().unwrap_or("");
    Some(Message {
        facility: (priority / 8).min(23) as u8,
        severity: (priority % 8) as u8,
        body: clean(&format!("kernel: {line}")),
    })
}

/// One line of printable text, bounded.
fn clean(text: &str) -> String {
    let mut out: String = text
        .trim_end_matches(['\n', '\r', '\0'])
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if out.len() > MAX_LINE_BYTES {
        let mut end = MAX_LINE_BYTES;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
    }
    out
}

/// Whether a message goes to the remote host under these settings.
pub fn sent(settings: &Settings, message: &Message) -> bool {
    settings.remote.is_some()
        && message.severity < settings.priority
        && (settings.kernel || !message.is_kernel())
}

/// Whether a message is kept on the card.
pub fn kept(message: &Message) -> bool {
    message.severity <= KEPT_SEVERITY
}

/// An RFC 3164 line for the remote host: `<PRI>Mmm dd hh:mm:ss host body`.
pub fn remote_line(message: &Message, hostname: &str, at: SystemTime) -> String {
    let (_, month, day, hour, minute, second) = civil(at);
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "<{}>{} {:>2} {:02}:{:02}:{:02} {} {}",
        u16::from(message.facility) * 8 + u16::from(message.severity),
        MONTHS[usize::from(month - 1)],
        day,
        hour,
        minute,
        second,
        hostname,
        message.body
    )
}

/// UTC calendar fields of a time: year, month, day, hour, minute, second.
pub fn civil(at: SystemTime) -> (i64, u8, u8, u8, u8, u8) {
    let seconds = at
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs() as i64;
    let days = seconds.div_euclid(86_400);
    let of_day = seconds.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u8;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (
        year,
        month,
        day,
        (of_day / 3600) as u8,
        ((of_day % 3600) / 60) as u8,
        (of_day % 60) as u8,
    )
}

/// `2026-11-17T05:34:56Z`.
pub fn iso(at: SystemTime) -> String {
    let (year, month, day, hour, minute, second) = civil(at);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

const SEVERITY_NAMES: [&str; 8] = [
    "emerg", "alert", "crit", "err", "warning", "notice", "info", "debug",
];

/// The bounded file of kept messages on the card, rotated once at its limit.
pub struct Store {
    path: PathBuf,
    limit: u64,
    tokens: f64,
    refilled: Option<SystemTime>,
    dropped: u64,
}

impl Store {
    pub fn new(directory: &Path) -> Self {
        Self::with_limit(directory, STORE_LIMIT_BYTES)
    }

    pub fn with_limit(directory: &Path, limit: u64) -> Self {
        Self {
            path: directory.join("errors.log"),
            limit,
            tokens: f64::from(STORE_BURST),
            refilled: None,
            dropped: 0,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Marks the start of a boot, once per boot: `marker` names the boot
    /// already marked, so a restart within the boot adds nothing. Returns
    /// whether this boot was new.
    pub fn mark_boot(&mut self, boot_id: &str, marker: &Path, at: SystemTime) -> io::Result<bool> {
        if fs::read_to_string(marker).is_ok_and(|seen| seen.trim() == boot_id) {
            return Ok(false);
        }
        self.append(&format!("{} === boot {boot_id} ===", iso(at)), true)?;
        let next = marker.with_extension("next");
        fs::write(&next, format!("{boot_id}\n"))?;
        fs::rename(&next, marker)?;
        Ok(true)
    }

    /// Keeps a message of priority err or above, synced at once from crit;
    /// beyond the burst and its refill, messages are counted, not written.
    pub fn keep(&mut self, message: &Message, at: SystemTime) -> io::Result<()> {
        if !kept(message) {
            return Ok(());
        }
        self.refill(at);
        if self.tokens < 1.0 {
            self.dropped += 1;
            return Ok(());
        }
        self.tokens -= 1.0;
        if self.dropped > 0 {
            let note = format!("{} {} lines not kept (too many)", iso(at), self.dropped);
            self.dropped = 0;
            self.append(&note, false)?;
        }
        let line = format!(
            "{} {} {}",
            iso(at),
            SEVERITY_NAMES[usize::from(message.severity.min(7))],
            message.body
        );
        self.append(&line, message.severity <= SYNCED_SEVERITY)
    }

    fn refill(&mut self, at: SystemTime) {
        if let Some(last) = self.refilled {
            let elapsed = at
                .duration_since(last)
                .unwrap_or(Duration::ZERO)
                .as_secs_f64();
            self.tokens =
                (self.tokens + elapsed * f64::from(STORE_PER_SECOND)).min(f64::from(STORE_BURST));
        }
        self.refilled = Some(at);
    }

    fn append(&mut self, line: &str, sync: bool) -> io::Result<()> {
        let size = fs::metadata(&self.path).map(|meta| meta.len()).unwrap_or(0);
        if size > 0 && size + line.len() as u64 + 1 > self.limit {
            fs::rename(&self.path, self.path.with_extension("log.1"))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        if sync {
            file.sync_all()?;
        }
        Ok(())
    }
}

/// The boot's identity, as the kernel names it.
pub fn boot_id() -> io::Result<String> {
    Ok(fs::read_to_string("/proc/sys/kernel/random/boot_id")?
        .trim()
        .to_string())
}

/// The board's name for the remote lines.
pub fn hostname() -> String {
    fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|name| name.trim().to_string())
        .ok()
        .filter(|name| valid_host(name))
        .unwrap_or_else(|| "blikvm".to_string())
}

/// Opens `/dev/kmsg` for reading.
pub fn open_kmsg() -> io::Result<File> {
    File::open("/dev/kmsg")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn settings_come_from_the_environment_and_are_checked() {
        assert_eq!(
            Settings::from_values(None, None, None).unwrap(),
            Settings::default()
        );
        let settings =
            Settings::from_values(Some("logs.example.test:514"), Some("5"), Some("0")).unwrap();
        assert_eq!(settings.remote, Some(("logs.example.test".into(), 514)));
        assert_eq!(settings.priority, 5);
        assert!(!settings.kernel);
        assert_eq!(
            Settings::from_values(Some("[2001:db8::1]:514"), None, None)
                .unwrap()
                .remote,
            Some(("[2001:db8::1]".into(), 514))
        );
        for remote in [
            "host",
            "host:0",
            "host:70000",
            "ho st:514",
            "-x:514",
            "a;b:514",
            ":514",
        ] {
            assert_eq!(
                Settings::from_values(Some(remote), None, None),
                Err(SettingsError::Remote),
                "{remote}"
            );
        }
        assert_eq!(
            Settings::from_values(None, Some("9"), None),
            Err(SettingsError::Priority)
        );
        assert_eq!(
            Settings::from_values(None, Some("0"), None),
            Err(SettingsError::Priority)
        );
        assert_eq!(
            Settings::from_values(None, None, Some("maybe")),
            Err(SettingsError::Kernel)
        );
    }

    #[test]
    fn forwarded_and_kernel_messages_are_parsed() {
        let message =
            parse_forwarded(b"<27>Nov 17 05:34:56 bli-kvm-server[812]: socket failed\n").unwrap();
        assert_eq!((message.facility, message.severity), (3, 3));
        assert_eq!(message.body, "bli-kvm-server[812]: socket failed");
        assert!(kept(&message));
        let message = parse_forwarded(b"<38>sshd[9]: no timestamp").unwrap();
        assert_eq!((message.facility, message.severity), (4, 6));
        assert_eq!(message.body, "sshd[9]: no timestamp");
        assert!(!kept(&message));
        assert_eq!(parse_forwarded(b"no priority"), None);
        assert_eq!(parse_forwarded(b"<999>x"), None);

        let kernel =
            parse_kmsg(b"3,1234,5678901,-;mmc0: card stopped responding\n SUBSYSTEM=mmc\n")
                .unwrap();
        assert!(kernel.is_kernel());
        assert_eq!(kernel.severity, 3);
        assert_eq!(kernel.body, "kernel: mmc0: card stopped responding");
        // Control characters do not split a line; a long one is cut.
        let long = format!("<11>x: {}\u{7}", "y".repeat(5000));
        let message = parse_forwarded(long.as_bytes()).unwrap();
        assert!(message.body.len() <= MAX_LINE_BYTES);
        assert!(!message.body.contains('\u{7}'));
    }

    #[test]
    fn the_remote_copy_follows_the_priority_and_the_kernel_switch() {
        let mut settings = Settings::default();
        let error = parse_forwarded(b"<27>Nov 17 05:34:56 server: failed").unwrap();
        let info = parse_forwarded(b"<30>Nov 17 05:34:56 server: started").unwrap();
        let kernel = parse_kmsg(b"6,1,1,-;usb 1-1: new device").unwrap();
        assert!(!sent(&settings, &error), "nothing is sent without a host");
        settings.remote = Some(("192.0.2.10".into(), 514));
        assert!(sent(&settings, &error) && sent(&settings, &info) && sent(&settings, &kernel));
        settings.priority = 5;
        assert!(sent(&settings, &error) && !sent(&settings, &info));
        settings.priority = 8;
        settings.kernel = false;
        assert!(!sent(&settings, &kernel));
        assert_eq!(
            remote_line(&error, "kvm", at(1_794_885_296)),
            "<27>Nov 17 03:14:56 kvm server: failed"
        );
    }

    #[test]
    fn civil_time_is_right_across_years() {
        assert_eq!(iso(at(0)), "1970-01-01T00:00:00Z");
        assert_eq!(iso(at(951_782_400)), "2000-02-29T00:00:00Z");
        assert_eq!(iso(at(1_794_885_296)), "2026-11-17T03:14:56Z");
    }

    #[test]
    fn errors_are_kept_on_the_card_and_the_rest_is_not() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::new(directory.path());
        store
            .keep(&parse_forwarded(b"<30>x: started").unwrap(), at(1))
            .unwrap();
        assert!(!store.path().exists(), "info is not written");
        store
            .keep(&parse_forwarded(b"<27>x: failed").unwrap(), at(2))
            .unwrap();
        store
            .keep(&parse_kmsg(b"2,9,9,-;watchdog: hung").unwrap(), at(3))
            .unwrap();
        let text = fs::read_to_string(store.path()).unwrap();
        assert_eq!(
            text,
            "1970-01-01T00:00:02Z err x: failed\n1970-01-01T00:00:03Z crit kernel: watchdog: hung\n"
        );
    }

    #[test]
    fn each_boot_is_marked_once() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("boot");
        let mut store = Store::new(directory.path());
        assert!(store.mark_boot("aaaa", &marker, at(10)).unwrap());
        assert!(!store.mark_boot("aaaa", &marker, at(20)).unwrap());
        assert!(store.mark_boot("bbbb", &marker, at(30)).unwrap());
        let text = fs::read_to_string(store.path()).unwrap();
        assert_eq!(text.matches("=== boot").count(), 2);
        assert!(text.contains("=== boot aaaa ===") && text.contains("=== boot bbbb ==="));
    }

    #[test]
    fn the_card_file_is_rotated_once_at_its_limit() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::with_limit(directory.path(), 200);
        for second in 0..20 {
            store
                .keep(
                    &parse_forwarded(format!("<27>x: failure {second:02}").as_bytes()).unwrap(),
                    at(second),
                )
                .unwrap();
        }
        let current = fs::read_to_string(store.path()).unwrap();
        let rotated = fs::read_to_string(directory.path().join("errors.log.1")).unwrap();
        assert!(current.len() <= 200 && rotated.len() <= 200);
        assert!(current.ends_with("failure 19\n"));
        assert!(!directory.path().join("errors.log.1.1").exists());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[test]
    fn a_storm_of_errors_is_bounded_and_counted() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = Store::new(directory.path());
        let error = parse_forwarded(b"<27>x: again").unwrap();
        for _ in 0..1000 {
            store.keep(&error, at(100)).unwrap();
        }
        // One second later the bucket has refilled by ten.
        store.keep(&error, at(101)).unwrap();
        let text = fs::read_to_string(store.path()).unwrap();
        assert_eq!(
            text.matches("x: again").count(),
            usize::try_from(STORE_BURST).unwrap() + 1
        );
        assert!(text.contains("900 lines not kept (too many)"), "{text}");
    }
}
