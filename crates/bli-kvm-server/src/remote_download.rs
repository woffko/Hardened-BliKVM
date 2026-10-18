//! Remote ISO download (reference `POST /api/download/image` and its status),
//! switched off by default.
//!
//! The server validates and forwards; the download helper, the only unit with
//! outbound network access, fetches the image, re-checks the switch and
//! reports progress through a status file this module reads. The helper runs
//! under an account of its own and may write one file only: the one this
//! module prepares for each download in the drop directory on the image
//! store's partition. When the helper reports the download finished, the
//! server checks the file again, hashes it and publishes it into the store
//! and the catalog, as it does an upload (security review of 2026-10-18, N1).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use axum::{Extension, Json, extract::State};
use bli_kvm_admin_helper::{
    AdminReply, AdminRequest, DOWNLOAD_STATUS, DownloadStatus, MAX_IMAGE_BYTES, validate_remote_iso,
};
use bli_kvm_core::AdminFeature;
use bli_kvm_media_catalog::{CatalogImage, MediaCatalog};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{
    AppState,
    auth::middleware::CurrentSession,
    download::{UploadKind, refuse_attached, upload_allows, upload_directory},
    error::{ApiResponse, AppError, AppResult},
};

/// A failed download is reported this long, then the status reads idle.
const FAILED_STATUS_TTL: Duration = Duration::from_secs(30);
const MAX_STATUS_BYTES: u64 = 4096;
/// The drop directory in the image store, bound into the helper's unit.
pub(crate) const DROP_DIRECTORY: &str = ".downloads";
/// The helper's own group; the server is its one other member.
const HELPER_GROUP: &str = "hardened-blikvm-download";
/// How often the server looks whether the helper is done.
const FINISH_POLL: Duration = Duration::from_secs(2);
const ISO9660_MAGIC_OFFSET: u64 = 0x8001;

/// The download the server is to publish, from its start until its publish,
/// and the last publish it refused.
struct Publications {
    pending: Option<String>,
    refused: Option<(DownloadStatus, Instant)>,
}

static PUBLICATIONS: Mutex<Publications> = Mutex::new(Publications {
    pending: None,
    refused: None,
});

fn publications() -> std::sync::MutexGuard<'static, Publications> {
    PUBLICATIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Takes the one download slot for `url`; false while another holds it.
fn begin(url: &str) -> bool {
    let mut publications = publications();
    if publications.pending.is_some() {
        return false;
    }
    publications.pending = Some(url.to_string());
    publications.refused = None;
    true
}

/// Ends the download of `url`, reporting a refused publish for a while.
fn settle(url: &str, refusal: Option<String>) {
    let mut publications = publications();
    publications.pending = None;
    if let Some(reason) = refusal {
        eprintln!("remote download: {url} was not published: {reason}");
        publications.refused = Some((failed(url), Instant::now()));
    }
}

fn failed(url: &str) -> DownloadStatus {
    DownloadStatus {
        status: "failed".into(),
        file: url.into(),
        percentage: String::new(),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DownloadBody {
    file: String,
}

pub(crate) async fn start(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<DownloadBody>,
) -> AppResult<Json<ApiResponse<DownloadStatus>>> {
    crate::admin_features::require(AdminFeature::RemoteDownload)?;
    let socket = state.options.download_socket.as_deref().ok_or_else(|| {
        AppError::ServiceUnavailable("the download helper is not configured".into())
    })?;
    let iso = validate_remote_iso(&body.file)
        .ok_or_else(|| AppError::BadRequest("invalid url".into()))?;
    let images = upload_directory(&state)?;
    if state.options.media_catalog.is_none() {
        return Err(AppError::ServiceUnavailable(
            "image catalog is not configured".into(),
        ));
    }
    // As for an upload: the attached image is not replaced under the target.
    refuse_attached(&state, &session.token, &iso.filename).await?;
    resume(&state);
    if download_running() || !begin(&iso.url) {
        return Err(AppError::BadRequest("download in progress".into()));
    }
    let prepared = {
        let images = images.clone();
        let name = iso.filename.clone();
        tokio::task::spawn_blocking(move || prepare_drop(&images, &name, helper_group())).await
    };
    if let Err(error) = prepared
        .map_err(io::Error::other)
        .and_then(|prepared| prepared)
    {
        settle(&iso.url, None);
        eprintln!("remote download: the drop directory is unusable: {error}");
        return Err(AppError::Conflict(
            "the image store on the board is not writable".into(),
        ));
    }
    let request = AdminRequest::ImageDownload {
        url: iso.url.clone(),
    };
    let answer = crate::scripts::call(socket, &request, None).await;
    if !matches!(answer, Ok(AdminReply::Ready)) {
        discard(&images, &iso.filename);
        settle(&iso.url, None);
    }
    match answer? {
        AdminReply::Ready => {
            tokio::spawn(publish_when_finished(
                state.clone(),
                session.token.clone(),
                iso.url.clone(),
                iso.filename,
            ));
            Ok(Json(ApiResponse::ok(DownloadStatus {
                status: "in_progress".into(),
                file: iso.url,
                percentage: String::new(),
            })))
        }
        AdminReply::Refused { reason } => Err(AppError::BadRequest(reason)),
        _ => Err(AppError::ServiceUnavailable(
            "unexpected download helper answer".into(),
        )),
    }
}

pub(crate) async fn status(State(state): State<AppState>) -> Json<ApiResponse<DownloadStatus>> {
    resume(&state);
    let worker = read_status(Path::new(DOWNLOAD_STATUS));
    Json(ApiResponse::ok(current_status(worker, &publications())))
}

/// The helper's status, or the server's while it has a download to publish
/// or has just refused one.
fn current_status(worker: DownloadStatus, publications: &Publications) -> DownloadStatus {
    if worker.status != "idle" {
        return worker;
    }
    if let Some(url) = &publications.pending {
        return DownloadStatus {
            status: "in_progress".into(),
            file: url.clone(),
            percentage: "100.00%".into(),
        };
    }
    match &publications.refused {
        Some((refused, at)) if at.elapsed() < FAILED_STATUS_TTL => refused.clone(),
        _ => worker,
    }
}

/// A server restarted during a download finds the helper's status naming it
/// and its file in the drop directory: the publish is taken up again.
fn resume(state: &AppState) {
    let Ok(images) = upload_directory(state) else {
        return;
    };
    let Some(raw) = raw_status(Path::new(DOWNLOAD_STATUS)) else {
        return;
    };
    if !matches!(raw.status.as_str(), "in_progress" | "finished") {
        return;
    }
    let Some(iso) = validate_remote_iso(&raw.file) else {
        return;
    };
    let left = fs::symlink_metadata(images.join(DROP_DIRECTORY).join(&iso.filename))
        .is_ok_and(|metadata| metadata.is_file());
    if !left || !begin(&raw.file) {
        return;
    }
    tokio::spawn(publish_when_finished(
        state.clone(),
        crate::auth::token::random_token(16),
        raw.file,
        iso.filename,
    ));
}

/// Waits for the helper to let its lock go, then publishes what it left.
async fn publish_when_finished(state: AppState, token: String, url: String, name: String) {
    while download_running() {
        tokio::time::sleep(FINISH_POLL).await;
    }
    let refusal = publish(&state, &token, &url, &name).await.err();
    settle(&url, refusal);
}

/// Publishes the file the helper reports finished, and removes what is
/// left of it.
async fn publish(state: &AppState, token: &str, url: &str, name: &str) -> Result<(), String> {
    let images = upload_directory(state).map_err(|error| error.to_string())?;
    let catalog = state
        .options
        .media_catalog
        .clone()
        .ok_or("image catalog is not configured")?;
    let result = Publish {
        images: &images,
        catalog: &catalog,
        status: Path::new(DOWNLOAD_STATUS),
        url,
        name,
    }
    .run(
        || crate::admin_features::switches().remote_download,
        refuse_attached(state, token, name),
    )
    .await;
    discard(&images, name);
    result
}

pub(crate) struct Publish<'a> {
    pub images: &'a Path,
    pub catalog: &'a Path,
    pub status: &'a Path,
    pub url: &'a str,
    pub name: &'a str,
}

impl Publish<'_> {
    /// Checks, hashes and publishes the file. A failure the helper reported
    /// itself, and a download an older helper published on its own, leave
    /// nothing to publish. The switch and the attached image are looked at
    /// once the file is hashed, which takes a while for a large image.
    pub(crate) async fn run(
        &self,
        switched_on: impl Fn() -> bool,
        not_attached: impl Future<Output = AppResult<()>>,
    ) -> Result<(), String> {
        match raw_status(self.status) {
            Some(raw) if raw.status == "finished" && raw.file == self.url => {}
            Some(raw) if raw.status == "in_progress" => {
                return Err("the download helper stopped before the end".into());
            }
            _ => return Ok(()),
        }
        let checked = {
            let images = self.images.to_path_buf();
            let name = self.name.to_string();
            tokio::task::spawn_blocking(move || check_drop(&images, &name))
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?
        };
        if !switched_on() {
            return Err("remote download was switched off".into());
        }
        // The drive may have been attached to this image during the
        // download; the rename would swap the file under the target.
        not_attached.await.map_err(|error| error.to_string())?;
        let (cdrom_allowed, removable_allowed, writable_allowed) = upload_allows(UploadKind::Iso);
        let image = CatalogImage {
            image_name: self.name.to_string(),
            size: checked.size,
            sha256: checked.sha256,
            cdrom_allowed,
            removable_allowed,
            writable_allowed,
        };
        let images = self.images.to_path_buf();
        let catalog = self.catalog.to_path_buf();
        tokio::task::spawn_blocking(move || {
            move_into_store(&images, &image.image_name)?;
            MediaCatalog::upsert_image(&catalog, &image)
                .map(|_| ())
                .map_err(|error| io::Error::other(error.to_string()))
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
    }
}

/// The helper's group, when the system has one (an older system's helper
/// runs as the server and publishes on its own).
fn helper_group() -> Option<u32> {
    nix::unistd::Group::from_name(HELPER_GROUP)
        .ok()
        .flatten()
        .map(|group| group.gid.as_raw())
}

/// The drop directory, opened without following a link and only when it is
/// the server's; its entries are reached through the descriptor's path.
fn open_drop(images: &Path) -> io::Result<(File, PathBuf)> {
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(images.join(DROP_DIRECTORY))?;
    let metadata = directory.metadata()?;
    if !metadata.is_dir() || metadata.uid() != nix::unistd::geteuid().as_raw() {
        return Err(io::Error::other(
            "the download directory is not the server's",
        ));
    }
    let path = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    Ok((directory, path))
}

/// Empties the drop directory and prepares in it the one file the helper
/// may write for `name`: the server's, writable by the helper's group.
pub(crate) fn prepare_drop(images: &Path, name: &str, group: Option<u32>) -> io::Result<()> {
    match fs::create_dir(images.join(DROP_DIRECTORY)) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let (directory, at) = open_drop(images)?;
    if let Some(group) = group {
        std::os::unix::fs::fchown(&directory, None, Some(group))?;
    }
    directory.set_permissions(fs::Permissions::from_mode(0o750))?;
    for entry in fs::read_dir(&at)? {
        fs::remove_file(at.join(entry?.file_name()))?;
    }
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o660)
        .custom_flags(libc::O_NOFOLLOW)
        .open(at.join(name))?;
    if let Some(group) = group {
        std::os::unix::fs::fchown(&file, None, Some(group))?;
    }
    file.set_permissions(fs::Permissions::from_mode(0o660))
}

/// Removes what a download left in the drop directory.
fn discard(images: &Path, name: &str) {
    if let Ok((_directory, at)) = open_drop(images) {
        let _ = fs::remove_file(at.join(name));
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Checked {
    size: u64,
    sha256: String,
}

/// Checks the file the server prepared and the helper filled: still the
/// server's one link, within the size bound, an ISO 9660 image. It is
/// hashed and made read-only for the helper's group.
pub(crate) fn check_drop(images: &Path, name: &str) -> io::Result<Checked> {
    let (_directory, at) = open_drop(images)?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(at.join(name))?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != nix::unistd::geteuid().as_raw()
    {
        return Err(io::Error::other(
            "the downloaded file is not the one prepared",
        ));
    }
    file.set_permissions(fs::Permissions::from_mode(0o644))?;
    let size = metadata.len();
    if size == 0 || size > MAX_IMAGE_BYTES {
        return Err(io::Error::other("downloaded image size out of bounds"));
    }
    let mut magic = [0_u8; 5];
    match file.read_exact_at(&mut magic, ISO9660_MAGIC_OFFSET) {
        Ok(()) if &magic == b"CD001" => {}
        Ok(()) => return Err(io::Error::other("file is not a valid ISO image")),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
            return Err(io::Error::other("file is not a valid ISO image"));
        }
        Err(error) => return Err(error),
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut read: u64 = 0;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        read += count as u64;
        hasher.update(&buffer[..count]);
    }
    if read != size {
        return Err(io::Error::other(
            "the downloaded file changed while it was checked",
        ));
    }
    let sha256 = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(Checked { size, sha256 })
}

/// Renames the checked file into the image store.
pub(crate) fn move_into_store(images: &Path, name: &str) -> io::Result<()> {
    let (_directory, at) = open_drop(images)?;
    fs::rename(at.join(name), images.join(name))
}

/// The helper's status as it wrote it.
fn raw_status(path: &Path) -> Option<DownloadStatus> {
    let file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_STATUS_BYTES).read_to_end(&mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// True while the download helper runs a job (an image or the PicoClaw
/// runtime).
pub(crate) fn download_running() -> bool {
    lock_held(Path::new(DOWNLOAD_STATUS))
}

/// True while the helper holds the download lock next to the status file.
fn lock_held(status_path: &Path) -> bool {
    let Ok(file) = File::open(status_path.with_file_name("download.lock")) else {
        return false;
    };
    // SAFETY: flock on a descriptor this function owns; a shared lock that
    // succeeds is released when the file closes.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) != 0 }
}

/// The helper's status, or idle when there is none, when a failure is old
/// or when an `in_progress` status has no running download behind it.
pub(crate) fn read_status(path: &Path) -> DownloadStatus {
    let Ok(file) = File::open(path) else {
        return DownloadStatus::idle();
    };
    let modified = file
        .metadata()
        .and_then(|metadata| metadata.modified())
        .ok();
    let mut bytes = Vec::new();
    if file.take(MAX_STATUS_BYTES).read_to_end(&mut bytes).is_err() {
        return DownloadStatus::idle();
    }
    let Ok(status) = serde_json::from_slice::<DownloadStatus>(&bytes) else {
        return DownloadStatus::idle();
    };
    let age = modified
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .unwrap_or_default();
    match status.status.as_str() {
        "in_progress" if lock_held(path) => status,
        "failed" if age < FAILED_STATUS_TTL => status,
        _ => DownloadStatus::idle(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// An image store and a catalog the server owns, as on the board.
    fn store() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let images = directory.path().join("images");
        fs::create_dir(&images).unwrap();
        let catalog = directory.path().join("catalog.json");
        fs::write(&catalog, br#"{"schemaVersion":1,"entries":[]}"#).unwrap();
        fs::set_permissions(&catalog, fs::Permissions::from_mode(0o640)).unwrap();
        (directory, images, catalog)
    }

    fn iso_bytes() -> Vec<u8> {
        let mut bytes = vec![0_u8; ISO9660_MAGIC_OFFSET as usize];
        bytes.extend_from_slice(b"CD001 the rescue image");
        bytes
    }

    fn write_status(path: &Path, status: &str, url: &str) {
        let status = DownloadStatus {
            status: status.into(),
            file: url.into(),
            percentage: String::new(),
        };
        fs::write(path, serde_json::to_vec(&status).unwrap()).unwrap();
    }

    #[tokio::test]
    async fn a_finished_download_reaches_the_store_only_through_the_servers_publish() {
        // The helper may write only the file the server prepares; the server
        // checks it again, refuses a name that is attached and publishes it
        // as it does an upload (security review of 2026-10-18, N1, MED-06).
        let (directory, images, catalog) = store();
        let url = "https://mirror.example/rescue.iso";
        let drop = images.join(DROP_DIRECTORY);
        fs::create_dir(&drop).unwrap();
        fs::write(drop.join("left.iso"), b"an earlier download").unwrap();
        prepare_drop(&images, "rescue.iso", None).unwrap();
        let names: Vec<_> = fs::read_dir(&drop)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["rescue.iso"]);
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode(&drop), 0o750);
        assert_eq!(mode(&drop.join("rescue.iso")), 0o660);
        assert_eq!(fs::metadata(drop.join("rescue.iso")).unwrap().len(), 0);
        fs::write(drop.join("rescue.iso"), iso_bytes()).unwrap();

        let status = directory.path().join("status.json");
        let publish = Publish {
            images: &images,
            catalog: &catalog,
            status: &status,
            url,
            name: "rescue.iso",
        };
        let published = || images.join("rescue.iso").exists();
        // The helper reported its own failure: nothing to publish.
        write_status(&status, "failed", url);
        assert_eq!(publish.run(|| true, async { Ok(()) }).await, Ok(()));
        assert!(!published());
        // The helper stopped without a word.
        write_status(&status, "in_progress", url);
        let error = publish.run(|| true, async { Ok(()) }).await.unwrap_err();
        assert!(error.contains("stopped"), "{error}");
        assert!(!published());
        write_status(&status, "finished", url);
        let error = publish.run(|| false, async { Ok(()) }).await.unwrap_err();
        assert!(error.contains("switched off"), "{error}");
        let attached = async {
            Err(AppError::Conflict(
                "detach the drive before replacing its image".into(),
            ))
        };
        let error = publish.run(|| true, attached).await.unwrap_err();
        assert!(error.contains("detach the drive"), "{error}");
        assert!(!published());
        assert!(MediaCatalog::open(&catalog).unwrap().summaries().is_empty());

        publish.run(|| true, async { Ok(()) }).await.unwrap();
        assert_eq!(fs::read(images.join("rescue.iso")).unwrap(), iso_bytes());
        assert_eq!(mode(&images.join("rescue.iso")), 0o644);
        assert_eq!(fs::read_dir(&drop).unwrap().count(), 0);
        let summaries = MediaCatalog::open(&catalog).unwrap().summaries();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].image_name, "rescue.iso");
        assert_eq!(summaries[0].size, iso_bytes().len() as u64);
        assert!(summaries[0].cdrom_allowed && !summaries[0].writable_allowed);
    }

    #[test]
    fn the_servers_check_takes_only_the_file_it_prepared() {
        let (directory, images, _catalog) = store();
        let drop = images.join(DROP_DIRECTORY);
        prepare_drop(&images, "a.iso", None).unwrap();
        let refused = |what: &str| {
            let error = check_drop(&images, "a.iso").unwrap_err();
            assert!(error.to_string().contains(what), "{error}");
        };
        refused("size out of bounds");
        fs::write(drop.join("a.iso"), b"hello").unwrap();
        refused("not a valid ISO");
        fs::write(drop.join("a.iso"), iso_bytes()).unwrap();
        // A second link: the file is not the one the server prepared.
        fs::hard_link(drop.join("a.iso"), directory.path().join("kept.iso")).unwrap();
        refused("not the one prepared");
        fs::remove_file(directory.path().join("kept.iso")).unwrap();
        let checked = check_drop(&images, "a.iso").unwrap();
        assert_eq!(checked.size, iso_bytes().len() as u64);
        let expected: String = Sha256::digest(iso_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(checked.sha256, expected);
        // A link in the file's place is not followed.
        fs::remove_file(drop.join("a.iso")).unwrap();
        std::os::unix::fs::symlink(directory.path().join("kept.iso"), drop.join("a.iso")).unwrap();
        assert!(check_drop(&images, "a.iso").is_err());
        // Nor a link in the directory's place.
        fs::remove_file(drop.join("a.iso")).unwrap();
        fs::remove_dir(&drop).unwrap();
        let elsewhere = directory.path().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &drop).unwrap();
        assert!(prepare_drop(&images, "a.iso", None).is_err());
        assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
    }

    #[test]
    fn the_status_reads_in_progress_until_the_publish_ends() {
        let url = "https://mirror.example/a.iso";
        let running = DownloadStatus {
            status: "in_progress".into(),
            file: url.into(),
            percentage: "12.00%".into(),
        };
        let mut publications = Publications {
            pending: Some(url.into()),
            refused: None,
        };
        assert_eq!(current_status(running.clone(), &publications), running);
        let publishing = current_status(DownloadStatus::idle(), &publications);
        assert_eq!(
            (publishing.status.as_str(), publishing.percentage.as_str()),
            ("in_progress", "100.00%")
        );
        publications.pending = None;
        publications.refused = Some((failed(url), Instant::now()));
        assert_eq!(
            current_status(DownloadStatus::idle(), &publications),
            failed(url)
        );
        publications.refused = Some((failed(url), Instant::now() - FAILED_STATUS_TTL));
        assert_eq!(
            current_status(DownloadStatus::idle(), &publications),
            DownloadStatus::idle()
        );
    }

    #[test]
    fn status_reads_idle_unless_a_download_runs_or_just_failed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("status.json");
        assert_eq!(read_status(&path), DownloadStatus::idle());
        let running = DownloadStatus {
            status: "in_progress".into(),
            file: "https://mirror.example/a.iso".into(),
            percentage: "12.00%".into(),
        };
        fs::write(&path, serde_json::to_vec(&running).unwrap()).unwrap();
        // Nobody holds the lock: a stale status after a helper crash.
        assert_eq!(read_status(&path), DownloadStatus::idle());
        let lock = File::create(dir.path().join("download.lock")).unwrap();
        // SAFETY: flock on an open descriptor owned by the test.
        assert_eq!(
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        assert_eq!(read_status(&path), running);
        let failed = DownloadStatus {
            status: "failed".into(),
            ..running
        };
        fs::write(&path, serde_json::to_vec(&failed).unwrap()).unwrap();
        assert_eq!(read_status(&path), failed);
        fs::write(&path, b"{not json").unwrap();
        assert_eq!(read_status(&path), DownloadStatus::idle());
    }
}
