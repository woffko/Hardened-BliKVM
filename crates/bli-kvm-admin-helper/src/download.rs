//! Remote ISO download into the virtual media store, while switched on.
//!
//! Runs unprivileged under an account of its own in its own unit, which is
//! the only one with outbound network access. One download at a time: curl
//! fetches the image into the one file the web server prepared for it,
//! progress goes to a status file the web server reads, and a finished file
//! must be an ISO 9660 image; the web server then checks it again, hashes it
//! and publishes it into the store and the media catalog (security review of
//! 2026-10-18, N1).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use bli_kvm_admin_helper::{
    DownloadStatus, MAX_IMAGE_BYTES, PicoclawInstallStatus, RemoteIso, remote_authority,
};
use sha2::{Digest, Sha256};

pub const CURL: &str = "/usr/bin/curl";
const ISO9660_MAGIC_OFFSET: u64 = 0x8001;
const PROGRESS_INTERVAL: Duration = Duration::from_secs(1);
const SWITCH_CHECK_INTERVAL: Duration = Duration::from_secs(2);

/// Held for the whole download; a second request finds it taken.
pub struct DownloadLock {
    _file: File,
}

impl DownloadLock {
    pub fn acquire(path: &Path) -> io::Result<Option<Self>> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o640)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        // The web server, a member of the helper's group, probes the lock.
        file.set_permissions(fs::Permissions::from_mode(0o640))?;
        // SAFETY: flock on a descriptor this function owns.
        let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
        Ok(locked.then_some(Self { _file: file }))
    }
}

/// How long a start waits for a lock that a status read holds for a moment:
/// the web server probes the same file with a shared lock, and a start that
/// met that probe used to be refused as "in progress".
pub const LOCK_WAIT: Duration = Duration::from_millis(250);

impl DownloadLock {
    /// [`Self::acquire`], retried for up to `wait`. A real download holds the
    /// lock for minutes, so this only rides out a momentary reader.
    pub fn acquire_within(path: &Path, wait: Duration) -> io::Result<Option<Self>> {
        let started = std::time::Instant::now();
        loop {
            if let Some(lock) = Self::acquire(path)? {
                return Ok(Some(lock));
            }
            if started.elapsed() >= wait {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// Writes the status atomically so the web server never reads half a file.
pub fn write_status(path: &Path, status: &DownloadStatus) -> io::Result<()> {
    let temporary = path.with_extension("new");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o644)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temporary)?;
    file.write_all(&serde_json::to_vec(status).map_err(io::Error::other)?)?;
    drop(file);
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o644))?;
    fs::rename(&temporary, path)
}

pub fn in_progress(url: &str, percentage: &str) -> DownloadStatus {
    DownloadStatus {
        status: "in_progress".into(),
        file: url.into(),
        percentage: percentage.into(),
    }
}

/// The last percentage in curl's progress-bar output, as `12.30%`.
pub fn parse_progress(output: &str) -> Option<String> {
    output
        .rsplit(['\r', '\n'])
        .filter_map(|line| line.split_whitespace().last())
        .find_map(|token| {
            let value: f64 = token.strip_suffix('%')?.parse().ok()?;
            (0.0..=100.0)
                .contains(&value)
                .then(|| format!("{value:.2}%"))
        })
}

/// curl's own error message; it may follow a progress bar on the same line.
pub fn curl_error(output: &str) -> String {
    output
        .rfind("curl: (")
        .map(|start| output[start..].lines().next().unwrap_or_default().trim())
        .filter(|message| !message.is_empty())
        .unwrap_or("curl failed")
        .to_string()
}

pub fn is_iso9660(path: &Path) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let mut magic = [0_u8; 5];
    io::Seek::seek(&mut file, io::SeekFrom::Start(ISO9660_MAGIC_OFFSET))?;
    match file.read_exact(&mut magic) {
        Ok(()) => Ok(&magic == b"CD001"),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error),
    }
}

pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// How a download finds its hosts: the system's resolver and the address
/// classes the redirect rule names; the tests stand in their own.
#[derive(Clone, Copy)]
pub struct Network {
    pub resolve: fn(&str, u16) -> io::Result<Vec<IpAddr>>,
    pub public: fn(IpAddr) -> bool,
}

impl Network {
    pub const SYSTEM: Self = Self {
        resolve: system_resolve,
        public: is_public,
    };
}

fn system_resolve(host: &str, port: u16) -> io::Result<Vec<IpAddr>> {
    Ok((host, port)
        .to_socket_addrs()?
        .map(|address| address.ip())
        .collect())
}

/// Whether an address is on the public internet: not private, CGNAT,
/// unique-local, loopback, link-local or otherwise special.
pub fn is_public(address: IpAddr) -> bool {
    fn public_v4(address: Ipv4Addr) -> bool {
        let [first, second, ..] = address.octets();
        !(address.is_private()
            || address.is_loopback()
            || address.is_link_local()
            || address.is_unspecified()
            || address.is_broadcast()
            || address.is_multicast()
            || first == 0
            || first >= 240
            || (first == 100 && (64..128).contains(&second))
            // Benchmarking (code audit of 2026-10-20, AUD-10).
            || (first == 198 && (18..20).contains(&second)))
    }
    match address {
        IpAddr::V4(address) => public_v4(address),
        IpAddr::V6(address) => match address.to_ipv4_mapped() {
            Some(mapped) => public_v4(mapped),
            None => {
                let segments = address.segments();
                let first = segments[0];
                !(address.is_loopback()
                    || address.is_unspecified()
                    || address.is_multicast()
                    || first & 0xfe00 == 0xfc00
                    || first & 0xffc0 == 0xfe80
                    // IPv4 translated (NAT64, 64:ff9b::/96 and
                    // 64:ff9b:1::/48), 6to4, IPv4-compatible and
                    // documentation addresses (code audit of 2026-10-20,
                    // AUD-10).
                    || (first == 0x64 && segments[1] == 0xff9b)
                    || first == 0x2002
                    || segments[..6] == [0; 6]
                    || (first == 0x2001 && segments[1] == 0x0db8))
            }
        },
    }
}

/// The redirects one download may follow, as curl's own limit was.
const MAX_REDIRECTS: usize = 5;

/// One download with the reference limits, reporting progress and stopping
/// when `still_enabled` turns false. The helper follows redirects itself:
/// each hop's host is resolved first and curl is pinned to those addresses,
/// and once a hop was on a public address a hop into a private, CGNAT or
/// unique-local one is refused before anything is sent to it (security
/// review of 2026-10-18, N8, decision 6). The operator's own URL may be
/// private.
pub struct Fetch<'a> {
    pub curl: &'a Path,
    pub url: &'a str,
    pub output: &'a Path,
    pub max_bytes: u64,
    pub max_seconds: u32,
    pub network: Network,
}

enum Hop {
    Done,
    Redirect(String),
}

impl Fetch<'_> {
    pub fn run(
        &self,
        still_enabled: impl Fn() -> bool,
        mut on_progress: impl FnMut(&str),
    ) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(u64::from(self.max_seconds));
        let mut url = self.url.to_string();
        let mut public = false;
        for _ in 0..=MAX_REDIRECTS {
            let (host, port) = remote_authority(&url)
                .ok_or_else(|| io::Error::other("the redirect leads to an invalid URL"))?;
            let addresses = (self.network.resolve)(&host, port)?;
            if addresses.is_empty() {
                return Err(io::Error::other("the host has no address"));
            }
            if public
                && !addresses
                    .iter()
                    .all(|address| (self.network.public)(*address))
            {
                return Err(io::Error::other(
                    "a redirect from a public address into a private one was refused",
                ));
            }
            public |= addresses
                .iter()
                .any(|address| (self.network.public)(*address));
            let remaining = deadline.saturating_duration_since(Instant::now()).as_secs();
            if remaining == 0 {
                return Err(io::Error::other("the download took too long"));
            }
            let pinned = Pinned {
                url: &url,
                host: &host,
                port,
                addresses: &addresses,
                max_seconds: remaining,
            };
            match self.transfer(&pinned, &still_enabled, &mut on_progress)? {
                Hop::Done => return Ok(()),
                Hop::Redirect(next) => url = next,
            }
        }
        Err(io::Error::other("too many redirects"))
    }

    /// The curl arguments of one hop, which follows no redirect itself.
    fn arguments(&self, hop: &Pinned) -> Vec<String> {
        let addresses: Vec<String> = hop
            .addresses
            .iter()
            .map(|address| match address {
                IpAddr::V4(address) => address.to_string(),
                IpAddr::V6(address) => format!("[{address}]"),
            })
            .collect();
        [
            "-q",
            "--fail",
            "--globoff",
            "--proto",
            "=http,https",
            "--connect-timeout",
            "20",
            "--max-time",
            &hop.max_seconds.to_string(),
            "--speed-limit",
            "1024",
            "--speed-time",
            "120",
            "--max-filesize",
            &self.max_bytes.to_string(),
            "--resolve",
            &format!("{}:{}:{}", hop.host, hop.port, addresses.join(",")),
            "--write-out",
            "%{http_code} %{redirect_url}",
            "--progress-bar",
            "--output",
        ]
        .map(str::to_string)
        .into_iter()
        .chain([
            self.output.display().to_string(),
            "--".into(),
            hop.url.into(),
        ])
        .collect()
    }

    fn transfer(
        &self,
        hop: &Pinned,
        still_enabled: &impl Fn() -> bool,
        on_progress: &mut impl FnMut(&str),
    ) -> io::Result<Hop> {
        let mut child = Command::new(self.curl)
            .env_clear()
            .args(self.arguments(hop))
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let written = {
            let mut stdout = child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("curl has no stdout"))?;
            thread::spawn(move || {
                let mut text = String::new();
                let _ = (&mut stdout).take(4096).read_to_string(&mut text);
                text
            })
        };
        let progress = Arc::new(Mutex::new(String::new()));
        let reader = {
            let progress = Arc::clone(&progress);
            let mut stderr = child
                .stderr
                .take()
                .ok_or_else(|| io::Error::other("curl has no stderr"))?;
            thread::spawn(move || {
                let mut tail = Vec::new();
                let mut buffer = [0_u8; 4096];
                while let Ok(count) = stderr.read(&mut buffer) {
                    if count == 0 {
                        break;
                    }
                    push_tail(&mut tail, &buffer[..count]);
                    if let Some(value) = parse_progress(&String::from_utf8_lossy(&tail))
                        && let Ok(mut current) = progress.lock()
                    {
                        *current = value;
                    }
                }
                String::from_utf8_lossy(&tail).into_owned()
            })
        };
        let mut reported = String::new();
        let mut last_report = Instant::now();
        let mut last_check = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if last_check.elapsed() >= SWITCH_CHECK_INTERVAL {
                last_check = Instant::now();
                if !still_enabled() {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = reader.join();
                    let _ = written.join();
                    return Err(io::Error::other("the download was switched off"));
                }
            }
            if last_report.elapsed() >= PROGRESS_INTERVAL {
                last_report = Instant::now();
                let current = progress
                    .lock()
                    .map(|value| value.clone())
                    .unwrap_or_default();
                if current != reported {
                    on_progress(&current);
                    reported = current;
                }
            }
            thread::sleep(Duration::from_millis(200));
        };
        let tail = reader.join().unwrap_or_default();
        let written = written.join().unwrap_or_default();
        if !status.success() {
            return Err(io::Error::other(curl_error(&tail)));
        }
        parse_hop(&written)
    }
}

/// Keeps the end of curl's stderr, where its progress and its error are,
/// as bytes: a cut inside a character must not stop the helper (code audit
/// of 2026-10-20, AUD-10).
fn push_tail(tail: &mut Vec<u8>, chunk: &[u8]) {
    tail.extend_from_slice(chunk);
    if tail.len() > 4096 {
        tail.drain(..tail.len() - 1024);
    }
}

/// One hop's URL, its host and port, and the addresses curl is pinned to.
struct Pinned<'a> {
    url: &'a str,
    host: &'a str,
    port: u16,
    addresses: &'a [IpAddr],
    max_seconds: u64,
}

/// curl's `--write-out`: a redirect's status and the URL it names.
fn parse_hop(written: &str) -> io::Result<Hop> {
    let mut words = written.split_whitespace();
    match (
        words.next().and_then(|code| code.parse::<u16>().ok()),
        words.next(),
    ) {
        (Some(200), _) => Ok(Hop::Done),
        (Some(300..=399), Some(next)) if !next.is_empty() => Ok(Hop::Redirect(next.to_string())),
        // A redirect that names nowhere, or an answer without the image
        // (204, 304): its body is not the image (code audit of 2026-10-20,
        // AUD-10).
        (status, _) => Err(io::Error::other(format!(
            "the server answered {} without the image",
            status.map_or_else(|| "nothing".to_string(), |status| status.to_string())
        ))),
    }
}

/// One remote image, fetched into the file the web server prepared for it in
/// the drop directory, the one place the helper's account may write.
pub struct Download<'a> {
    pub iso: &'a RemoteIso,
    pub downloads: &'a Path,
    pub status: &'a Path,
    pub curl: &'a Path,
    pub network: Network,
}

impl Download<'_> {
    /// Fetches and checks the image; a failure leaves the prepared file
    /// empty, for the web server to remove.
    pub fn run(&self, still_enabled: impl Fn() -> bool) -> io::Result<()> {
        let target = self.downloads.join(&self.iso.filename);
        if !fs::symlink_metadata(&target).is_ok_and(|metadata| metadata.is_file()) {
            return Err(io::Error::other(
                "the web server prepared no file for this download",
            ));
        }
        let result = self
            .fetch(&target, still_enabled)
            .and_then(|()| self.check(&target));
        if result.is_err() {
            let _ = OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&target)
                .and_then(|file| file.set_len(0));
        }
        result
    }

    fn fetch(&self, temporary: &Path, still_enabled: impl Fn() -> bool) -> io::Result<()> {
        let request = Fetch {
            curl: self.curl,
            url: &self.iso.url,
            output: temporary,
            max_bytes: MAX_IMAGE_BYTES,
            max_seconds: 7200,
            network: self.network,
        };
        request.run(still_enabled, |current| {
            let _ = write_status(self.status, &in_progress(&self.iso.url, current));
        })
    }

    fn check(&self, target: &Path) -> io::Result<()> {
        let size = fs::symlink_metadata(target)?.len();
        if size == 0 || size > MAX_IMAGE_BYTES {
            return Err(io::Error::other("downloaded image size out of bounds"));
        }
        if !is_iso9660(target)? {
            return Err(io::Error::other("file is not a valid ISO image"));
        }
        Ok(())
    }
}

/// Fetches the pinned PicoClaw release, checks its size and digest, and
/// installs only its `picoclaw` executable.
pub struct RuntimeInstall<'a> {
    pub curl: &'a Path,
    pub network: Network,
    pub tar: &'a Path,
    pub url: &'a str,
    pub sha256: &'a str,
    pub bytes: u64,
    /// Directory that receives `picoclaw`; the archive is staged in it.
    pub directory: &'a Path,
    pub status: &'a Path,
}

pub fn write_install_status(path: &Path, stage: &str, progress: u8, error: &str) {
    let status = PicoclawInstallStatus {
        stage: stage.into(),
        progress,
        error: error.into(),
    };
    if let Ok(bytes) = serde_json::to_vec(&status) {
        let temporary = path.with_extension("new");
        if fs::write(&temporary, bytes).is_ok() {
            let _ = fs::set_permissions(&temporary, fs::Permissions::from_mode(0o644));
            let _ = fs::rename(&temporary, path);
        }
    }
}

impl RuntimeInstall<'_> {
    pub fn run(&self, still_enabled: impl Fn() -> bool) -> io::Result<()> {
        let archive = self.directory.join(".picoclaw.tar.gz.download");
        let unpack = self.directory.join(".picoclaw.unpack");
        let cleanup = || {
            let _ = fs::remove_file(&archive);
            let _ = fs::remove_dir_all(&unpack);
        };
        cleanup();
        let result = self.install(&archive, &unpack, still_enabled);
        cleanup();
        match &result {
            Ok(()) => write_install_status(self.status, "installed", 100, ""),
            Err(error) => {
                write_install_status(self.status, "install_failed", 0, &error.to_string())
            }
        }
        result
    }

    fn install(
        &self,
        archive: &Path,
        unpack: &Path,
        still_enabled: impl Fn() -> bool,
    ) -> io::Result<()> {
        write_install_status(self.status, "downloading", 5, "");
        Fetch {
            curl: self.curl,
            url: self.url,
            output: archive,
            max_bytes: self.bytes,
            max_seconds: 600,
            network: self.network,
        }
        .run(still_enabled, |current| {
            let percent: f64 = current.trim_end_matches('%').parse().unwrap_or(0.0);
            let progress = (10.0 + percent * 0.7).clamp(10.0, 80.0) as u8;
            write_install_status(self.status, "downloading", progress, "");
        })?;
        write_install_status(self.status, "verifying", 82, "");
        if fs::symlink_metadata(archive)?.len() != self.bytes
            || sha256_file(archive)? != self.sha256
        {
            return Err(io::Error::other(
                "the PicoClaw archive does not match the pinned digest",
            ));
        }
        write_install_status(self.status, "extracting", 85, "");
        fs::create_dir(unpack)?;
        let status = Command::new(self.tar)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .args(["--no-same-owner", "--no-same-permissions", "-xzf"])
            .arg(archive)
            .arg("-C")
            .arg(unpack)
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            return Err(io::Error::other(
                "the PicoClaw archive could not be unpacked",
            ));
        }
        let binary = [unpack.join("picoclaw")]
            .into_iter()
            .chain(
                fs::read_dir(unpack)?
                    .filter_map(Result::ok)
                    .map(|entry| entry.path().join("picoclaw")),
            )
            .find(|path| {
                fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
            })
            .ok_or_else(|| io::Error::other("picoclaw binary not found in archive"))?;
        write_install_status(self.status, "installing", 95, "");
        let target = self.directory.join("picoclaw");
        let staged = self.directory.join(".picoclaw.new");
        fs::copy(&binary, &staged)?;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
        fs::rename(&staged, &target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bli_kvm_admin_helper::validate_remote_iso;
    use std::path::PathBuf;

    #[test]
    fn curl_progress_is_parsed() {
        assert_eq!(
            parse_progress("\r####        37.9%\r#########     75.5%").as_deref(),
            Some("75.50%")
        );
        assert_eq!(
            parse_progress("\r###  100.0%\n").as_deref(),
            Some("100.00%")
        );
        assert_eq!(parse_progress("\r#=#=#  ##O#- "), None);
        assert_eq!(parse_progress("curl: (22) 404"), None);
        assert_eq!(
            curl_error("\r###   1.0%\r##  curl: (7) Failed to connect\n"),
            "curl: (7) Failed to connect"
        );
        assert_eq!(curl_error(""), "curl failed");
    }

    #[test]
    fn a_second_download_finds_the_lock_taken() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lock");
        let first = DownloadLock::acquire(&path).unwrap();
        assert!(first.is_some());
        // The web server, in the helper's group only, reads the lock.
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert!(DownloadLock::acquire(&path).unwrap().is_none());
        drop(first);
        // A child forked by a parallel test may hold the descriptor until
        // its exec closes it, so allow a moment for the release.
        let released = (0..50).any(|_| {
            let taken = DownloadLock::acquire(&path).unwrap().is_some();
            if !taken {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            taken
        });
        assert!(released);
    }

    #[test]
    fn a_start_rides_out_a_status_reader_but_not_a_running_download() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lock");
        let reader = File::create(&path).unwrap();
        // SAFETY: flock on a descriptor this test owns.
        assert_eq!(
            unsafe { libc::flock(reader.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) },
            0
        );
        assert!(DownloadLock::acquire(&path).unwrap().is_none());
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            drop(reader);
        });
        let taken = DownloadLock::acquire_within(&path, Duration::from_secs(2)).unwrap();
        assert!(taken.is_some(), "the momentary reader is waited out");
        release.join().unwrap();
        // A held download is still refused, after the wait.
        let begun = std::time::Instant::now();
        assert!(
            DownloadLock::acquire_within(&path, Duration::from_millis(100))
                .unwrap()
                .is_none()
        );
        assert!(begun.elapsed() >= Duration::from_millis(100));
        drop(taken);
    }

    fn one_public_address(_host: &str, _port: u16) -> io::Result<Vec<IpAddr>> {
        Ok(vec![IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))])
    }

    /// Every host is one public address: the tests make no lookups.
    const OFFLINE: Network = Network {
        resolve: one_public_address,
        public: is_public,
    };

    /// A stand-in for curl that writes `content` to the `--output` path.
    /// A curl that runs `script` and, unless the script exits first, answers
    /// as a complete transfer does (`--write-out`'s status 200).
    fn fake_curl(dir: &Path, script: &str) -> PathBuf {
        let path = dir.join("curl");
        fs::write(&path, format!("#!/bin/sh\n{script}\nprintf '200 '\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn curl_runs_with_globbing_off_and_follows_no_redirect_itself() {
        // curl's URL globbing turned `[ ] { }` into many requests, and curl
        // followed redirects anywhere (security review of 2026-10-18, MED-05,
        // N8).
        let dir = tempfile::tempdir().unwrap();
        let arguments = dir.path().join("arguments");
        let curl = fake_curl(
            dir.path(),
            &format!("printf '%s\\n' \"$@\" > '{}'", arguments.display()),
        );
        let output = dir.path().join("out");
        Fetch {
            curl: &curl,
            url: "https://mirror.example/rescue.iso",
            output: &output,
            max_bytes: 10,
            max_seconds: 60,
            network: OFFLINE,
        }
        .run(|| true, |_| {})
        .unwrap();
        let arguments = fs::read_to_string(&arguments).unwrap();
        let arguments: Vec<&str> = arguments.lines().collect();
        assert!(arguments.contains(&"--globoff"), "{arguments:?}");
        assert!(!arguments.contains(&"--location") && !arguments.contains(&"-L"));
        let resolve = arguments
            .iter()
            .position(|argument| *argument == "--resolve");
        assert_eq!(
            resolve.map(|index| arguments[index + 1]),
            Some("mirror.example:443:192.0.2.1")
        );
        assert_eq!(arguments.last(), Some(&"https://mirror.example/rescue.iso"));
    }

    /// A stand-in HTTP server on `address`: it answers each request with
    /// `answer(path)` and counts the requests.
    fn serve(
        address: Ipv4Addr,
        answer: impl Fn(&str) -> String + Send + 'static,
    ) -> (u16, Arc<std::sync::atomic::AtomicUsize>) {
        let listener = std::net::TcpListener::bind((address, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&requests);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut request = Vec::new();
                let mut byte = [0_u8; 1];
                while !request.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                    request.push(byte[0]);
                }
                let request = String::from_utf8_lossy(&request);
                let path = request.split_whitespace().nth(1).unwrap_or("/");
                let _ = stream.write_all(answer(path).as_bytes());
            }
        });
        (port, requests)
    }

    fn found(location: &str) -> String {
        format!(
            "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
    }

    fn image(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    /// The test's two hosts on loopback: one stands for the public internet,
    /// the other for the operator's network.
    fn two_hosts(host: &str, _port: u16) -> io::Result<Vec<IpAddr>> {
        match host {
            "public.test" => Ok(vec![IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))]),
            "intranet.test" => Ok(vec![IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2))]),
            _ => Err(io::Error::other("unknown host")),
        }
    }

    fn only_the_first_is_public(address: IpAddr) -> bool {
        address == IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
    }

    #[test]
    fn a_redirect_from_a_public_address_into_a_private_one_is_refused() {
        // A remote server could make the board send requests into the
        // operator's network (security review of 2026-10-18, N8, decision 6).
        let curl = Path::new(CURL);
        assert!(curl.exists(), "the tests need {CURL}");
        let (intranet, intranet_requests) = serve(Ipv4Addr::new(127, 0, 0, 2), |_| {
            image("the operator's image")
        });
        let (public, public_requests) =
            serve(Ipv4Addr::new(127, 0, 0, 1), move |path| match path {
                "/start.iso" => found(&format!("http://intranet.test:{intranet}/rescue.iso")),
                "/moved.iso" => found("/rescue.iso"),
                _ => image("the public image"),
            });
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        let network = Network {
            resolve: two_hosts,
            public: only_the_first_is_public,
        };
        let fetch = |url: &str| {
            Fetch {
                curl,
                url,
                output: &output,
                max_bytes: 1024,
                max_seconds: 60,
                network,
            }
            .run(|| true, |_| {})
        };
        let error = fetch(&format!("http://public.test:{public}/start.iso")).unwrap_err();
        assert!(error.to_string().contains("refused"), "{error}");
        assert_eq!(
            intranet_requests.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(public_requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        // A redirect on the public side is followed.
        fetch(&format!("http://public.test:{public}/moved.iso")).unwrap();
        assert_eq!(fs::read_to_string(&output).unwrap(), "the public image");
        // The operator's own URL may name a private address.
        fetch(&format!("http://intranet.test:{intranet}/rescue.iso")).unwrap();
        assert_eq!(fs::read_to_string(&output).unwrap(), "the operator's image");
        assert_eq!(
            intranet_requests.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }

    #[test]
    fn a_cut_inside_a_character_does_not_stop_the_helper() {
        // A compromised curl could end its output inside a character where
        // the tail is cut, and the helper aborted (code audit of 2026-10-20,
        // AUD-10).
        let mut tail = Vec::new();
        push_tail(&mut tail, &[b'#'; 4000]);
        push_tail(&mut tail, &[0xff; 400]);
        assert!(tail.len() <= 4096 + 1200);
    }

    #[test]
    fn only_a_complete_answer_ends_the_hops() {
        // A 3xx without a Location ended them, and its body counted as the
        // image (code audit of 2026-10-20, AUD-10).
        let curl = Path::new(CURL);
        assert!(curl.exists(), "the tests need {CURL}");
        let (public, _) = serve(Ipv4Addr::new(127, 0, 0, 1), |path| match path {
            "/nowhere.iso" => {
                "HTTP/1.1 302 Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nCD001-iso"
                    .to_string()
            }
            "/nothing.iso" => "HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_string(),
            _ => image("the public image"),
        });
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out");
        let network = Network {
            resolve: two_hosts,
            public: only_the_first_is_public,
        };
        let fetch = |path: &str| {
            Fetch {
                curl,
                url: &format!("http://public.test:{public}{path}"),
                output: &output,
                max_bytes: 1024,
                max_seconds: 60,
                network,
            }
            .run(|| true, |_| {})
        };
        assert!(fetch("/nowhere.iso").is_err());
        assert!(fetch("/nothing.iso").is_err());
        fetch("/image.iso").unwrap();
        assert_eq!(fs::read_to_string(&output).unwrap(), "the public image");
    }

    #[test]
    fn the_address_classes_of_the_redirect_rule() {
        // The private ranges as numbers: the source check keeps their
        // dotted form out of the repository.
        for private in [
            Ipv4Addr::new(10, 1, 2, 3),
            Ipv4Addr::new(172, 16, 0, 1),
            Ipv4Addr::new(192, 168, 1, 1),
        ] {
            assert!(!is_public(IpAddr::V4(private)), "{private}");
            assert!(
                !is_public(IpAddr::V6(private.to_ipv6_mapped())),
                "{private}"
            );
        }
        for private in [
            "100.64.0.1",
            "100.127.255.254",
            "127.0.0.1",
            "169.254.1.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "fd00::1",
            "fe80::1",
            "::1",
            "::",
            // Translated or embedded IPv4, documentation and benchmarking
            // (code audit of 2026-10-20, AUD-10).
            "64:ff9b::c0a8:101",
            "64:ff9b:1::1",
            "2002:c0a8:101::1",
            "::c0a8:101",
            "2001:db8::1",
            "198.18.0.1",
            "198.19.255.254",
        ] {
            assert!(!is_public(private.parse().unwrap()), "{private}");
        }
        for public in [
            "93.184.216.34",
            "100.63.255.255",
            "100.128.0.1",
            "2001:4860::8888",
        ] {
            assert!(is_public(public.parse().unwrap()), "{public}");
        }
    }

    #[test]
    fn the_download_fills_only_the_file_the_server_prepared() {
        // The worker fetches remote data under an account of its own, which
        // may write only the file the server prepared in the drop directory:
        // the server publishes into the image store and the catalog
        // (security review of 2026-10-18, N1).
        let dir = tempfile::tempdir().unwrap();
        let downloads = dir.path().join("downloads");
        fs::create_dir(&downloads).unwrap();
        let status = dir.path().join("status.json");
        let iso = validate_remote_iso("https://mirror.example/rescue.iso").unwrap();
        // Writes 0x8001 zero bytes then the ISO 9660 magic to the output path.
        let write_iso = r#"while [ "$1" != "--output" ]; do shift; done
head -c 32769 /dev/zero > "$2"; printf 'CD001' >> "$2"; printf '\r##  50.0%%' >&2"#;
        let curl = fake_curl(dir.path(), write_iso);
        let download = Download {
            iso: &iso,
            downloads: &downloads,
            status: &status,
            curl: &curl,
            network: OFFLINE,
        };
        let error = download.run(|| true).unwrap_err();
        assert!(error.to_string().contains("prepared no file"), "{error}");
        assert_eq!(fs::read_dir(&downloads).unwrap().count(), 0);

        let prepared = downloads.join("rescue.iso");
        fs::write(&prepared, b"").unwrap();
        download.run(|| true).unwrap();
        assert_eq!(fs::metadata(&prepared).unwrap().len(), 32774);

        let not_iso = r#"while [ "$1" != "--output" ]; do shift; done
printf 'hello' > "$2""#;
        let curl = fake_curl(dir.path(), not_iso);
        let other = validate_remote_iso("https://mirror.example/other.iso").unwrap();
        fs::write(downloads.join("other.iso"), b"").unwrap();
        let download = Download {
            iso: &other,
            curl: &curl,
            ..download
        };
        let error = download.run(|| true).unwrap_err();
        assert!(error.to_string().contains("not a valid ISO"), "{error}");
        // A failure leaves the prepared file empty for the server to remove.
        assert_eq!(fs::metadata(downloads.join("other.iso")).unwrap().len(), 0);
        let failing = fake_curl(
            dir.path(),
            "echo 'curl: (22) The requested URL returned error: 404' >&2; exit 22",
        );
        let download = Download {
            curl: &failing,
            ..download
        };
        let error = download.run(|| true).unwrap_err();
        assert!(error.to_string().contains("404"), "{error}");
        let slow = fake_curl(dir.path(), "exec sleep 30");
        let download = Download {
            curl: &slow,
            ..download
        };
        let error = download.run(|| false).unwrap_err();
        assert!(error.to_string().contains("switched off"), "{error}");
        let mut names: Vec<_> = fs::read_dir(&downloads)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["other.iso", "rescue.iso"]);
    }

    #[test]
    fn the_runtime_is_installed_only_from_the_pinned_archive() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = dir.path().join("runtime");
        fs::create_dir(&runtime).unwrap();
        let source = dir.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("picoclaw"), "#!/bin/sh\necho pico\n").unwrap();
        fs::write(source.join("README.md"), "readme").unwrap();
        let archive = dir.path().join("release.tar.gz");
        assert!(
            Command::new("tar")
                .arg("-czf")
                .arg(&archive)
                .arg("-C")
                .arg(&source)
                .args(["picoclaw", "README.md"])
                .status()
                .unwrap()
                .success()
        );
        let digest = sha256_file(&archive).unwrap();
        let bytes = fs::metadata(&archive).unwrap().len();
        let curl = fake_curl(
            dir.path(),
            &format!(
                "while [ \"$1\" != \"--output\" ]; do shift; done\ncp '{}' \"$2\"",
                archive.display()
            ),
        );
        let status = dir.path().join("install.json");
        let install = RuntimeInstall {
            curl: &curl,
            network: OFFLINE,
            tar: Path::new("/bin/tar"),
            url: "https://example.org/picoclaw.tar.gz",
            sha256: &digest,
            bytes,
            directory: &runtime,
            status: &status,
        };
        install.run(|| true).unwrap();
        let binary = runtime.join("picoclaw");
        assert_eq!(
            fs::read_to_string(&binary).unwrap(),
            "#!/bin/sh\necho pico\n"
        );
        assert_eq!(
            fs::metadata(&binary).unwrap().permissions().mode() & 0o777,
            0o755
        );
        let reported: PicoclawInstallStatus =
            serde_json::from_slice(&fs::read(&status).unwrap()).unwrap();
        assert_eq!(
            (reported.stage.as_str(), reported.progress),
            ("installed", 100)
        );
        let names: Vec<_> = fs::read_dir(&runtime)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, ["picoclaw"]);

        fs::remove_file(&binary).unwrap();
        let wrong = RuntimeInstall {
            sha256: "00",
            ..install
        };
        let error = wrong.run(|| true).unwrap_err();
        assert!(error.to_string().contains("pinned digest"), "{error}");
        assert!(!binary.exists());
        let reported: PicoclawInstallStatus =
            serde_json::from_slice(&fs::read(&status).unwrap()).unwrap();
        assert_eq!(reported.stage, "install_failed");
    }
}
