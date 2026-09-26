//! Remote ISO download into the virtual media store, while switched on.
//!
//! Runs unprivileged as the web-server account in its own unit, which is the
//! only one with outbound network access. One download at a time: curl
//! fetches the image into a hidden temporary file, progress goes to a status
//! file the web server reads, and a finished file must be an ISO 9660 image
//! before it is hashed, published and recorded in the media catalog.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use bli_kvm_admin_helper::{DownloadStatus, MAX_IMAGE_BYTES, PicoclawInstallStatus, RemoteIso};
use bli_kvm_media_catalog::{CatalogImage, MediaCatalog};
use sha2::{Digest, Sha256};

pub const IMAGE_DIRECTORY: &str = "/var/lib/hardened-blikvm-gadget/images";
pub const MEDIA_CATALOG: &str = "/etc/hardened-blikvm-gadget/catalog.json";
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
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        // SAFETY: flock on a descriptor this function owns.
        let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
        Ok(locked.then_some(Self { _file: file }))
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

/// One curl transfer with the reference limits, reporting progress and
/// stopping when `still_enabled` turns false.
pub struct Fetch<'a> {
    pub curl: &'a Path,
    pub url: &'a str,
    pub output: &'a Path,
    pub max_bytes: u64,
    pub max_seconds: u32,
}

impl Fetch<'_> {
    pub fn run(
        &self,
        still_enabled: impl Fn() -> bool,
        mut on_progress: impl FnMut(&str),
    ) -> io::Result<()> {
        let mut child = Command::new(self.curl)
            .env_clear()
            .args([
                "-q",
                "--fail",
                "--location",
                "--proto",
                "=http,https",
                "--proto-redir",
                "=http,https",
                "--max-redirs",
                "5",
                "--connect-timeout",
                "20",
                "--max-time",
                &self.max_seconds.to_string(),
                "--speed-limit",
                "1024",
                "--speed-time",
                "120",
                "--max-filesize",
                &self.max_bytes.to_string(),
                "--progress-bar",
                "--output",
            ])
            .arg(self.output)
            .arg("--")
            .arg(self.url)
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        let progress = Arc::new(Mutex::new(String::new()));
        let reader = {
            let progress = Arc::clone(&progress);
            let mut stderr = child
                .stderr
                .take()
                .ok_or_else(|| io::Error::other("curl has no stderr"))?;
            thread::spawn(move || {
                let mut tail = String::new();
                let mut buffer = [0_u8; 4096];
                while let Ok(count) = stderr.read(&mut buffer) {
                    if count == 0 {
                        break;
                    }
                    tail.push_str(&String::from_utf8_lossy(&buffer[..count]));
                    if tail.len() > 4096 {
                        tail.drain(..tail.len() - 1024);
                    }
                    if let Some(value) = parse_progress(&tail)
                        && let Ok(mut current) = progress.lock()
                    {
                        *current = value;
                    }
                }
                tail
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
        if !status.success() {
            return Err(io::Error::other(curl_error(&tail)));
        }
        Ok(())
    }
}

pub struct Download<'a> {
    pub iso: &'a RemoteIso,
    pub images: &'a Path,
    pub catalog: &'a Path,
    pub status: &'a Path,
    pub curl: &'a Path,
}

impl Download<'_> {
    fn temporary(&self) -> PathBuf {
        self.images.join(format!(".{}.download", self.iso.filename))
    }

    /// Fetches, checks and publishes the image; the temporary file never
    /// survives a failure.
    pub fn run(&self, still_enabled: impl Fn() -> bool) -> io::Result<()> {
        let temporary = self.temporary();
        let _ = fs::remove_file(&temporary);
        let result = self
            .fetch(&temporary, still_enabled)
            .and_then(|()| self.publish(&temporary));
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
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
        };
        request.run(still_enabled, |current| {
            let _ = write_status(self.status, &in_progress(&self.iso.url, current));
        })
    }

    fn publish(&self, temporary: &Path) -> io::Result<()> {
        let size = fs::symlink_metadata(temporary)?.len();
        if size == 0 || size > MAX_IMAGE_BYTES {
            return Err(io::Error::other("downloaded image size out of bounds"));
        }
        if !is_iso9660(temporary)? {
            return Err(io::Error::other("file is not a valid ISO image"));
        }
        let sha256 = sha256_file(temporary)?;
        fs::set_permissions(temporary, fs::Permissions::from_mode(0o644))?;
        fs::rename(temporary, self.images.join(&self.iso.filename))?;
        MediaCatalog::upsert_image(
            self.catalog,
            &CatalogImage {
                image_name: self.iso.filename.clone(),
                size,
                sha256,
                cdrom_allowed: true,
                removable_allowed: true,
                writable_allowed: false,
            },
        )
        .map_err(|error| io::Error::other(error.to_string()))?;
        Ok(())
    }
}

/// Fetches the pinned PicoClaw release, checks its size and digest, and
/// installs only its `picoclaw` executable.
pub struct RuntimeInstall<'a> {
    pub curl: &'a Path,
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
        assert!(DownloadLock::acquire(&path).unwrap().is_none());
        drop(first);
        assert!(DownloadLock::acquire(&path).unwrap().is_some());
    }

    fn catalog(dir: &Path) -> PathBuf {
        let path = dir.join("catalog.json");
        fs::write(&path, br#"{"schemaVersion":1,"entries":[]}"#).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// A stand-in for curl that writes `content` to the `--output` path.
    fn fake_curl(dir: &Path, script: &str) -> PathBuf {
        let path = dir.join("curl");
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn only_a_finished_iso_is_published_and_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let images = dir.path().join("images");
        fs::create_dir(&images).unwrap();
        let etc = dir.path().join("etc");
        fs::create_dir(&etc).unwrap();
        let catalog = catalog(&etc);
        let status = dir.path().join("status.json");
        let iso = validate_remote_iso("https://mirror.example/rescue.iso").unwrap();
        // Writes 0x8001 zero bytes then the ISO 9660 magic to the output path.
        let write_iso = r#"while [ "$1" != "--output" ]; do shift; done
head -c 32769 /dev/zero > "$2"; printf 'CD001' >> "$2"; printf '\r##  50.0%%' >&2"#;
        let curl = fake_curl(dir.path(), write_iso);
        let download = Download {
            iso: &iso,
            images: &images,
            catalog: &catalog,
            status: &status,
            curl: &curl,
        };
        download.run(|| true).unwrap();
        let published = images.join("rescue.iso");
        assert_eq!(fs::metadata(&published).unwrap().len(), 32774);
        assert_eq!(
            fs::metadata(&published).unwrap().permissions().mode() & 0o777,
            0o644
        );
        let entry = MediaCatalog::open(&catalog)
            .unwrap()
            .find_by_name("rescue.iso")
            .unwrap();
        assert!(entry.cdrom_allowed && !entry.writable_allowed);

        let not_iso = r#"while [ "$1" != "--output" ]; do shift; done
printf 'hello' > "$2""#;
        let curl = fake_curl(dir.path(), not_iso);
        let other = validate_remote_iso("https://mirror.example/other.iso").unwrap();
        let download = Download {
            iso: &other,
            curl: &curl,
            ..download
        };
        let error = download.run(|| true).unwrap_err();
        assert!(error.to_string().contains("not a valid ISO"), "{error}");
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
        let mut names: Vec<_> = fs::read_dir(&images)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["rescue.iso"]);
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
