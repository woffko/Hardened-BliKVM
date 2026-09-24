//! Canonical application archive rebuilt from an updater-owned slot tree.
//! The slot is untrusted input for a privileged reader: its signed manifest is
//! verified before any payload file is opened, every component is opened
//! relative to a held directory without following links, and the stream still
//! passes the normal snapshot, signature and hash verification when staged.

use std::{
    collections::{BTreeMap, VecDeque},
    ffi::CString,
    fs::File,
    io::{self, Cursor, Read},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::Path,
};

use bli_kvm_update_stager::{
    ApplicationStageError, SLOT_MANIFEST_PATH, SLOT_METADATA_DIRECTORY, SLOT_SIGNATURE_PATH,
};
use bli_kvm_update_verifier::{
    APPLICATION_MANIFEST_PATH, APPLICATION_PAYLOAD_ROOT, APPLICATION_SIGNATURE_PATH,
    ApplicationTrustRoot, MAX_ARCHIVE_BYTES, MAX_MANIFEST_BYTES, UpdateVerifyError,
    verify_application_manifest_bytes,
};
use sha2::{Digest, Sha256};

const BLOCK: u64 = 512;
const END_OF_ARCHIVE: usize = 2 * BLOCK as usize;

enum Segment {
    Bytes(Cursor<Vec<u8>>),
    File { file: File, remaining: u64 },
}

/// A finite reader whose exact length is known before staging.
pub struct SlotArchive {
    segments: VecDeque<Segment>,
    size: u64,
}

impl SlotArchive {
    pub fn size(&self) -> u64 {
        self.size
    }
}

impl Read for SlotArchive {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        while let Some(segment) = self.segments.front_mut() {
            let count = match segment {
                Segment::Bytes(cursor) => cursor.read(buffer)?,
                Segment::File { file, remaining } => {
                    if *remaining == 0 {
                        0
                    } else {
                        let limit = buffer
                            .len()
                            .min(usize::try_from(*remaining).unwrap_or(usize::MAX));
                        let count = file.read(&mut buffer[..limit])?;
                        if count == 0 {
                            // A payload file shrank after it was opened.
                            return Err(io::ErrorKind::UnexpectedEof.into());
                        }
                        *remaining -= count as u64;
                        count
                    }
                }
            };
            if count != 0 || buffer.is_empty() {
                return Ok(count);
            }
            self.segments.pop_front();
        }
        Ok(0)
    }
}

fn open_at(parent: &File, name: &str, directory: bool) -> io::Result<File> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let name = CString::new(name).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    if directory {
        flags |= libc::O_DIRECTORY;
    } else {
        flags |= libc::O_NONBLOCK;
    }
    // SAFETY: parent is a live directory descriptor and name is NUL-terminated.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat returned a fresh descriptor owned by nobody else.
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file.metadata()?;
    if (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(file)
}

fn open_relative(root: &File, relative: &str) -> io::Result<File> {
    let mut components = relative.split('/').peekable();
    let mut parent = root.try_clone()?;
    while let Some(component) = components.next() {
        if components.peek().is_some() {
            parent = open_at(&parent, component, true)?;
        } else {
            return open_at(&parent, component, false);
        }
    }
    Err(io::ErrorKind::InvalidInput.into())
}

fn read_bounded(file: &mut File, limit: u64) -> io::Result<Vec<u8>> {
    let size = file.metadata()?.len();
    if size == 0 || size > limit {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != size {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(bytes)
}

fn header(
    path: &str,
    mode: u32,
    size: u64,
    directory: bool,
) -> Result<Vec<u8>, ApplicationStageError> {
    let mut header = tar::Header::new_ustar();
    header
        .set_path(path)
        .map_err(|_| ApplicationStageError::InvalidArchive)?;
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_size(size);
    header.set_entry_type(if directory {
        tar::EntryType::Directory
    } else {
        tar::EntryType::Regular
    });
    header.set_cksum();
    Ok(header.as_bytes().to_vec())
}

fn padding(size: u64) -> usize {
    ((BLOCK - size % BLOCK) % BLOCK) as usize
}

enum Entry {
    Directory,
    Bytes(Vec<u8>),
    File { file: File, mode: u32, size: u64 },
}

/// Rebuild the canonical archive of `<slot_root>/<slot_id>`. `slot_root` and
/// `trust` must come from privileged local configuration, never from IPC.
pub fn open_slot_archive(
    slot_root: &Path,
    slot_id: &str,
    trust: &ApplicationTrustRoot,
) -> Result<SlotArchive, ApplicationStageError> {
    if !slot_root.is_absolute() {
        return Err(ApplicationStageError::InvalidRoot);
    }
    let root = {
        let path = CString::new(slot_root.as_os_str().as_bytes())
            .map_err(|_| ApplicationStageError::InvalidRoot)?;
        // SAFETY: path is NUL-terminated; the result is checked below.
        let fd = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(ApplicationStageError::UnsafeRoot);
        }
        // SAFETY: open returned a fresh descriptor owned by nobody else.
        unsafe { File::from_raw_fd(fd) }
    };
    let slot = open_at(&root, slot_id, true).map_err(|_| ApplicationStageError::SlotNotFound)?;
    let metadata = open_at(&slot, SLOT_METADATA_DIRECTORY, true)
        .map_err(|_| ApplicationStageError::UnsafeExistingSlot)?;
    let metadata_name = |path: &'static str| {
        path.strip_prefix(SLOT_METADATA_DIRECTORY)
            .and_then(|name| name.strip_prefix('/'))
            .ok_or(ApplicationStageError::UnsafeExistingSlot)
    };
    let manifest_bytes = open_at(&metadata, metadata_name(SLOT_MANIFEST_PATH)?, false)
        .and_then(|mut file| read_bounded(&mut file, MAX_MANIFEST_BYTES))
        .map_err(|_| ApplicationStageError::UnsafeExistingSlot)?;
    let signature_bytes: [u8; 64] = open_at(&metadata, metadata_name(SLOT_SIGNATURE_PATH)?, false)
        .and_then(|mut file| read_bounded(&mut file, 64))
        .map_err(|_| ApplicationStageError::UnsafeExistingSlot)?
        .try_into()
        .map_err(|_| ApplicationStageError::Verify(UpdateVerifyError::InvalidSignature))?;
    // Authenticate the manifest before trusting any path or size it names.
    let manifest = verify_application_manifest_bytes(&manifest_bytes, &signature_bytes, trust)?;
    let digest: String = Sha256::digest(&manifest_bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if slot_id != format!("app-{:020}-{digest}", manifest.sequence) {
        return Err(ApplicationStageError::SlotConflict);
    }

    let mut entries = BTreeMap::new();
    entries.insert(APPLICATION_PAYLOAD_ROOT.to_string(), Entry::Directory);
    for file in &manifest.files {
        let components = file.path.split('/').collect::<Vec<_>>();
        for depth in 1..components.len() {
            entries.insert(
                format!(
                    "{APPLICATION_PAYLOAD_ROOT}/{}",
                    components[..depth].join("/")
                ),
                Entry::Directory,
            );
        }
        let opened = open_relative(&slot, &file.path)
            .map_err(|_| ApplicationStageError::UnsafeExistingSlot)?;
        if opened.metadata()?.size() != file.size {
            return Err(ApplicationStageError::UnsafeExistingSlot);
        }
        entries.insert(
            format!("{APPLICATION_PAYLOAD_ROOT}/{}", file.path),
            Entry::File {
                file: opened,
                mode: file.mode,
                size: file.size,
            },
        );
    }
    entries.insert(
        APPLICATION_MANIFEST_PATH.to_string(),
        Entry::Bytes(manifest_bytes),
    );
    entries.insert(
        APPLICATION_SIGNATURE_PATH.to_string(),
        Entry::Bytes(signature_bytes.to_vec()),
    );

    let mut segments = VecDeque::new();
    let mut size = END_OF_ARCHIVE as u64;
    // BTreeMap order is byte order of the normalized paths, as the verifier requires.
    for (path, entry) in entries {
        match entry {
            Entry::Directory => {
                segments.push_back(Segment::Bytes(Cursor::new(header(&path, 0o755, 0, true)?)));
                size += BLOCK;
            }
            Entry::Bytes(bytes) => {
                let length = bytes.len() as u64;
                let mut block = header(&path, 0o644, length, false)?;
                block.extend_from_slice(&bytes);
                block.resize(block.len() + padding(length), 0);
                size += block.len() as u64;
                segments.push_back(Segment::Bytes(Cursor::new(block)));
            }
            Entry::File {
                file,
                mode,
                size: length,
            } => {
                segments.push_back(Segment::Bytes(Cursor::new(header(
                    &path, mode, length, false,
                )?)));
                segments.push_back(Segment::File {
                    file,
                    remaining: length,
                });
                segments.push_back(Segment::Bytes(Cursor::new(vec![0; padding(length)])));
                size += BLOCK + length + padding(length) as u64;
            }
        }
        if size > MAX_ARCHIVE_BYTES {
            return Err(ApplicationStageError::ArchiveTooLarge);
        }
    }
    segments.push_back(Segment::Bytes(Cursor::new(vec![0; END_OF_ARCHIVE])));
    Ok(SlotArchive { segments, size })
}

#[cfg(test)]
pub(crate) mod fixture {
    use std::collections::BTreeMap;

    use bli_kvm_update_verifier::{
        APPLICATION_ARCHITECTURE, APPLICATION_FORMAT_V1, APPLICATION_PRODUCT, APPLICATION_TARGET,
        ApplicationFile, ApplicationManifest, ApplicationTrustRoot,
    };
    use ed25519_dalek::{Signer, SigningKey};
    use sha2::{Digest, Sha256};

    use crate::native_identity::FILES;

    pub(crate) fn content(path: &str) -> Vec<u8> {
        format!("bytes of {path}").into_bytes()
    }

    /// A signed native composition archive in the canonical builder layout.
    pub(crate) fn signed_archive(
        sequence: u64,
        minimum_system_sequence: u64,
    ) -> (ApplicationTrustRoot, ApplicationManifest, Vec<u8>) {
        let signing = SigningKey::from_bytes(&[37; 32]);
        let trust =
            ApplicationTrustRoot::new("app-test", signing.verifying_key().to_bytes()).unwrap();
        let mut paths = FILES.to_vec();
        paths.push("libexec/bli-kvm-platform-broker");
        paths.sort_unstable();
        let manifest = ApplicationManifest {
            format: APPLICATION_FORMAT_V1.into(),
            product: APPLICATION_PRODUCT.into(),
            target: APPLICATION_TARGET.into(),
            architecture: APPLICATION_ARCHITECTURE.into(),
            key_id: "app-test".into(),
            version: "1.0.0".into(),
            sequence,
            minimum_system_sequence,
            source_commit: "1".repeat(40),
            files: paths
                .iter()
                .map(|path| {
                    let bytes = content(path);
                    ApplicationFile {
                        path: (*path).into(),
                        mode: if path.starts_with("share/") {
                            0o644
                        } else {
                            0o755
                        },
                        size: bytes.len() as u64,
                        sha256: Sha256::digest(&bytes)
                            .iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect(),
                    }
                })
                .collect(),
        };
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let signature = signing.sign(&manifest_bytes).to_bytes();
        let mut builder = tar::Builder::new(Vec::new());
        let mut append = |path: &str, mode: u32, bytes: &[u8], directory: bool| {
            let mut header = tar::Header::new_ustar();
            header.set_path(path).unwrap();
            header.set_mode(mode);
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            header.set_size(bytes.len() as u64);
            header.set_entry_type(if directory {
                tar::EntryType::Directory
            } else {
                tar::EntryType::Regular
            });
            header.set_cksum();
            builder.append(&header, bytes).unwrap();
        };
        append("manifest.json", 0o644, &manifest_bytes, false);
        append("manifest.sig", 0o644, &signature, false);
        let mut entries = BTreeMap::new();
        for file in &manifest.files {
            let components = file.path.split('/').collect::<Vec<_>>();
            entries.insert("payload".to_string(), None);
            for depth in 1..components.len() {
                entries.insert(format!("payload/{}", components[..depth].join("/")), None);
            }
            entries.insert(format!("payload/{}", file.path), Some(file.mode));
        }
        for (path, mode) in entries {
            match mode {
                None => append(&path, 0o755, b"", true),
                Some(mode) => append(&path, mode, &content(&path["payload/".len()..]), false),
            }
        }
        (trust, manifest, builder.into_inner().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, OpenOptions, Permissions},
        io::Write,
        os::unix::fs::{PermissionsExt, symlink},
    };

    use bli_kvm_update_stager::ApplicationSlotStager;
    use bli_kvm_update_verifier::verify_application_reader;

    use super::*;

    fn staged() -> (tempfile::TempDir, ApplicationTrustRoot, String, Vec<u8>) {
        let (trust, _, archive) = fixture::signed_archive(15, 1);
        let root = tempfile::tempdir().unwrap();
        let slot_id = ApplicationSlotStager::open(root.path())
            .unwrap()
            .stage_reader(archive.as_slice(), archive.len() as u64, &trust)
            .unwrap()
            .slot_id;
        (root, trust, slot_id, archive)
    }

    fn writable(path: &Path) {
        fs::set_permissions(path, Permissions::from_mode(0o700)).unwrap();
    }

    fn rebuilt(root: &Path, slot_id: &str, trust: &ApplicationTrustRoot) -> Vec<u8> {
        let mut archive = open_slot_archive(root, slot_id, trust).unwrap();
        let size = archive.size();
        let mut bytes = Vec::new();
        archive.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes.len() as u64, size);
        bytes
    }

    #[test]
    fn rebuilt_archive_is_byte_identical_to_the_canonical_builder_output() {
        let (root, trust, slot_id, original) = staged();
        let bytes = rebuilt(root.path(), &slot_id, &trust);
        assert_eq!(bytes, original);
        let verified =
            verify_application_reader(bytes.as_slice(), bytes.len() as u64, &trust).unwrap();
        assert_eq!(verified.manifest.sequence, 15);
    }

    #[test]
    fn same_inode_modification_is_detected_by_verification() {
        let (root, trust, slot_id, _) = staged();
        let slot = root.path().join(&slot_id);
        let target = slot.join("bin/bli-kvm-server");
        writable(&slot.join("bin"));
        writable(&target);
        let length = fs::metadata(&target).unwrap().len() as usize;
        let mut file = OpenOptions::new().write(true).open(&target).unwrap();
        file.write_all(&vec![b'x'; length]).unwrap();
        drop(file);
        let bytes = rebuilt(root.path(), &slot_id, &trust);
        assert!(matches!(
            verify_application_reader(bytes.as_slice(), bytes.len() as u64, &trust),
            Err(UpdateVerifyError::PayloadMismatch)
        ));
    }

    #[test]
    fn links_truncation_and_foreign_identifiers_are_refused() {
        let (root, trust, slot_id, _) = staged();
        let slot = root.path().join(&slot_id);
        writable(&slot.join("bin"));
        let target = slot.join("bin/bli-kvm-server");
        writable(&target);
        let original = fs::read(&target).unwrap();

        fs::write(&target, &original[..original.len() - 1]).unwrap();
        assert!(matches!(
            open_slot_archive(root.path(), &slot_id, &trust),
            Err(ApplicationStageError::UnsafeExistingSlot)
        ));

        let outside = root.path().join("outside");
        fs::write(&outside, &original).unwrap();
        fs::remove_file(&target).unwrap();
        symlink(&outside, &target).unwrap();
        assert!(matches!(
            open_slot_archive(root.path(), &slot_id, &trust),
            Err(ApplicationStageError::UnsafeExistingSlot)
        ));
        fs::remove_file(&target).unwrap();
        fs::write(&target, &original).unwrap();
        assert!(open_slot_archive(root.path(), &slot_id, &trust).is_ok());

        let other = format!("app-{:020}-{}", 15, "c".repeat(64));
        assert!(matches!(
            open_slot_archive(root.path(), &other, &trust),
            Err(ApplicationStageError::SlotNotFound)
        ));
        writable(root.path());
        writable(&slot);
        fs::rename(&slot, root.path().join(&other)).unwrap();
        assert!(matches!(
            open_slot_archive(root.path(), &other, &trust),
            Err(ApplicationStageError::SlotConflict)
        ));
        assert!(matches!(
            open_slot_archive(Path::new("relative"), &other, &trust),
            Err(ApplicationStageError::InvalidRoot)
        ));
    }

    #[test]
    fn signature_is_verified_before_any_payload_file_is_opened() {
        let (root, trust, slot_id, _) = staged();
        let slot = root.path().join(&slot_id);
        // A missing payload file would be reported as an unsafe slot; a bad
        // signature must be reported first because no payload is opened.
        writable(&slot.join("bin"));
        fs::remove_file(slot.join("bin/bli-kvm-server")).unwrap();
        let metadata = slot.join(SLOT_METADATA_DIRECTORY);
        writable(&metadata);
        let signature = slot.join(SLOT_SIGNATURE_PATH);
        writable(&signature);
        let mut bytes = fs::read(&signature).unwrap();
        bytes[0] ^= 1;
        fs::write(&signature, bytes).unwrap();
        assert!(matches!(
            open_slot_archive(root.path(), &slot_id, &trust),
            Err(ApplicationStageError::Verify(_))
        ));
    }
}
