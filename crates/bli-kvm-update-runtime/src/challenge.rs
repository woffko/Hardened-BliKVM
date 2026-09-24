use std::{
    fs::{self, File, OpenOptions, Permissions},
    io::{Read, Write},
    os::fd::AsRawFd,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use bli_kvm_update_manager::ClockSnapshot;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

use crate::UpdateRuntimeError;

const CHALLENGE_SCHEMA_VERSION: u32 = 1;
const STATE_FILE_NAME: &str = "health-challenge.json";
const TOKEN_FILE_NAME: &str = "health-token";
const MAX_STATE_BYTES: u64 = 4 * 1024;
const TOKEN_BYTES: u64 = 32;
pub const CONFIRMED_APPLY_FILE_NAME: &str = "confirmed-apply-request.json";
pub const MAX_APPLY_REQUEST_BYTES: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfirmedApplyRequest {
    schema_version: u32,
    pub generation: u64,
    pub slot_id: String,
}

impl ConfirmedApplyRequest {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, UpdateRuntimeError> {
        if bytes.is_empty() || bytes.len() > MAX_APPLY_REQUEST_BYTES {
            return Err(UpdateRuntimeError::InvalidApplyRequest);
        }
        let value: Self =
            serde_json::from_slice(bytes).map_err(|_| UpdateRuntimeError::InvalidApplyRequest)?;
        if value.schema_version != 1 || value.generation == 0 || !valid_slot_id(&value.slot_id) {
            return Err(UpdateRuntimeError::InvalidApplyRequest);
        }
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedHealthChallenge {
    pub operation_id: [u8; 16],
    pub health_binding: [u8; 32],
    pub target_slot_id: String,
    pub boot_id: [u8; 16],
    pub issued_monotonic_ms: u64,
    pub deadline_monotonic_ms: u64,
    pub credential_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthChallengeStatus {
    pub operation_id: [u8; 16],
    pub target_slot_id: String,
    pub boot_id: [u8; 16],
    pub issued_monotonic_ms: u64,
    pub deadline_monotonic_ms: u64,
    pub credential_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedHealthResponse {
    pub operation_id: [u8; 16],
    pub health_binding: [u8; 32],
    pub target_slot_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChallengeDocument {
    schema_version: u32,
    operation_id: String,
    health_binding: String,
    target_slot_id: String,
    boot_id: String,
    issued_monotonic_ms: u64,
    deadline_monotonic_ms: u64,
}

#[derive(Debug)]
struct LoadedChallenge {
    document: ChallengeDocument,
    operation_id: [u8; 16],
    health_binding: [u8; 32],
    boot_id: [u8; 16],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuntimeIdentity {
    device: u64,
    inode: u64,
    owner: u32,
}

#[derive(Debug)]
pub struct HealthChallengeStore {
    root: PathBuf,
    state_path: PathBuf,
    token_path: PathBuf,
    root_directory: File,
    identity: RuntimeIdentity,
}

/// Validated file presence for one exact journal-bound challenge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChallengePresence {
    Absent,
    StateOnly,
    TokenOnly,
    Complete,
}

impl HealthChallengeStore {
    /// Bind a trusted native coordinator to the complete stored challenge pair.
    /// The manager independently checks journal identity and the fresh clock.
    pub(crate) fn native_confirmation_request(
        &self,
        operation_id: [u8; 16],
        target_slot_id: &str,
        clock: ClockSnapshot,
    ) -> Result<bli_kvm_update_manager::ActivationRequest, UpdateRuntimeError> {
        let loaded = self.load_required()?;
        if loaded.operation_id != operation_id || loaded.document.target_slot_id != target_slot_id {
            return Err(UpdateRuntimeError::InvalidChallengeState);
        }
        Ok(bli_kvm_update_manager::ActivationRequest {
            operation_id,
            target_slot_id: target_slot_id.to_owned(),
            health_binding: loaded.health_binding,
            clock,
        })
    }

    /// Native idle admission cannot silently discard credentials or legacy work.
    /// Inspect node presence even when only one challenge file survived a crash.
    pub(crate) fn require_empty_native_runtime(&self) -> Result<(), UpdateRuntimeError> {
        self.validate_root()?;
        if self.confirmed_apply_request()?.is_some() {
            return Err(UpdateRuntimeError::InvalidApplyRequest);
        }
        if self.nodes_present()? != (false, false) {
            return Err(UpdateRuntimeError::ChallengeExists);
        }
        Ok(())
    }

    pub(crate) fn confirmed_apply_request(
        &self,
    ) -> Result<Option<ConfirmedApplyRequest>, UpdateRuntimeError> {
        self.validate_root()?;
        let path = self.root.join(CONFIRMED_APPLY_FILE_NAME);
        if !node_exists(&path)? {
            return Ok(None);
        }
        let bytes = self.read_safe_file(&path, 0o600, MAX_APPLY_REQUEST_BYTES as u64)?;
        Ok(Some(ConfirmedApplyRequest::from_bytes(&bytes)?))
    }

    pub(crate) fn request_confirmed_apply(
        &self,
        generation: u64,
        slot_id: &str,
    ) -> Result<(), UpdateRuntimeError> {
        if generation == 0 || !valid_slot_id(slot_id) {
            return Err(UpdateRuntimeError::InvalidApplyRequest);
        }
        let request = ConfirmedApplyRequest {
            schema_version: 1,
            generation,
            slot_id: slot_id.into(),
        };
        let previous = self.confirmed_apply_request()?;
        if previous.as_ref() == Some(&request) {
            return Ok(());
        }
        let bytes = serde_json::to_vec(&request).map_err(|_| UpdateRuntimeError::Serialization)?;
        self.write_file(
            &self.root.join(CONFIRMED_APPLY_FILE_NAME),
            &bytes,
            0o600,
            previous.is_some(),
        )
    }

    pub(crate) fn acknowledge_confirmed_apply(
        &self,
        expected: &ConfirmedApplyRequest,
    ) -> Result<bool, UpdateRuntimeError> {
        if self.confirmed_apply_request()?.as_ref() != Some(expected) {
            return Ok(false);
        }
        self.remove_safe_file(
            &self.root.join(CONFIRMED_APPLY_FILE_NAME),
            0o600,
            MAX_APPLY_REQUEST_BYTES as u64,
        )?;
        Ok(true)
    }
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, UpdateRuntimeError> {
        let root = root.into();
        let identity = validate_runtime_root(&root)?;
        let root_directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&root)
            .map_err(|_| UpdateRuntimeError::UnsafeRuntimeRoot)?;
        if identity_from_metadata(&root_directory.metadata()?) != identity {
            return Err(UpdateRuntimeError::UnsafeRuntimeRoot);
        }
        // Separate managers may share this runtime despite having different
        // slot/state locks. Hold an advisory lock on the already validated
        // directory inode; never truncate or create a replaceable lock file.
        // SAFETY: the descriptor belongs to a live File; flock takes no pointers.
        // LOCK_NB prevents cross-manager lock-order waits. File drop releases it.
        if unsafe { libc::flock(root_directory.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            return Err(if error.kind() == std::io::ErrorKind::WouldBlock {
                UpdateRuntimeError::RuntimeBusy
            } else {
                error.into()
            });
        }
        Ok(Self {
            state_path: root.join(STATE_FILE_NAME),
            token_path: root.join(TOKEN_FILE_NAME),
            root,
            root_directory,
            identity,
        })
    }

    pub fn issue(
        &self,
        target_slot_id: &str,
        clock: ClockSnapshot,
        deadline_monotonic_ms: u64,
    ) -> Result<IssuedHealthChallenge, UpdateRuntimeError> {
        self.issue_after_admission(target_slot_id, clock, deadline_monotonic_ms, |_| Ok(()))
    }

    /// Generate identity in memory, then let the trusted caller durably admit
    /// it before any credential file is published. An ambiguous admission error
    /// leaves no newly published files; the caller must inspect its journal.
    pub(crate) fn issue_after_admission(
        &self,
        target_slot_id: &str,
        clock: ClockSnapshot,
        deadline_monotonic_ms: u64,
        admit: impl FnOnce(&IssuedHealthChallenge) -> Result<(), UpdateRuntimeError>,
    ) -> Result<IssuedHealthChallenge, UpdateRuntimeError> {
        self.validate_root()?;
        if !valid_slot_id(target_slot_id)
            || clock.boot_id.iter().all(|byte| *byte == 0)
            || deadline_monotonic_ms <= clock.monotonic_ms
        {
            return Err(UpdateRuntimeError::InvalidChallengeState);
        }
        if self.nodes_present()? != (false, false) {
            return Err(UpdateRuntimeError::ChallengeExists);
        }
        let mut operation_id = [0_u8; 16];
        let mut token = [0_u8; 32];
        let mut rng = OsRng;
        rng.try_fill_bytes(&mut operation_id)
            .map_err(|_| UpdateRuntimeError::EntropyUnavailable)?;
        if rng.try_fill_bytes(&mut token).is_err() {
            token.zeroize();
            return Err(UpdateRuntimeError::EntropyUnavailable);
        }
        if operation_id.iter().all(|byte| *byte == 0) || token.iter().all(|byte| *byte == 0) {
            token.zeroize();
            return Err(UpdateRuntimeError::EntropyUnavailable);
        }
        let health_binding: [u8; 32] = Sha256::digest(token).into();
        let document = ChallengeDocument {
            schema_version: CHALLENGE_SCHEMA_VERSION,
            operation_id: encode_hex(&operation_id),
            health_binding: encode_hex(&health_binding),
            target_slot_id: target_slot_id.to_string(),
            boot_id: encode_hex(&clock.boot_id),
            issued_monotonic_ms: clock.monotonic_ms,
            deadline_monotonic_ms,
        };
        let state_bytes = match serde_json::to_vec(&document) {
            Ok(bytes) => bytes,
            Err(_) => {
                token.zeroize();
                return Err(UpdateRuntimeError::Serialization);
            }
        };
        if state_bytes.is_empty() || state_bytes.len() as u64 > MAX_STATE_BYTES {
            token.zeroize();
            return Err(UpdateRuntimeError::Serialization);
        }
        let issued = IssuedHealthChallenge {
            operation_id,
            health_binding,
            target_slot_id: target_slot_id.to_string(),
            boot_id: clock.boot_id,
            issued_monotonic_ms: clock.monotonic_ms,
            deadline_monotonic_ms,
            credential_path: self.token_path.clone(),
        };
        let result = (|| {
            admit(&issued)?;
            self.write_noclobber(&self.token_path, &token, 0o400)?;
            if let Err(error) = self.write_noclobber(&self.state_path, &state_bytes, 0o600) {
                let _ = self.remove_safe_file(&self.token_path, 0o400, TOKEN_BYTES);
                return Err(error);
            }
            let loaded = self.load_required()?;
            if loaded.document != document
                || loaded.operation_id != operation_id
                || loaded.health_binding != health_binding
            {
                return Err(UpdateRuntimeError::InvalidChallengeState);
            }
            Ok(issued)
        })();
        token.zeroize();
        result
    }

    pub fn status(&self) -> Result<Option<HealthChallengeStatus>, UpdateRuntimeError> {
        self.validate_root()?;
        match self.nodes_present()? {
            (false, false) => Ok(None),
            (true, true) => {
                let loaded = self.load_required()?;
                Ok(Some(HealthChallengeStatus {
                    operation_id: loaded.operation_id,
                    target_slot_id: loaded.document.target_slot_id,
                    boot_id: loaded.boot_id,
                    issued_monotonic_ms: loaded.document.issued_monotonic_ms,
                    deadline_monotonic_ms: loaded.document.deadline_monotonic_ms,
                    credential_path: self.token_path.clone(),
                }))
            }
            _ => Err(UpdateRuntimeError::InvalidChallengeState),
        }
    }

    pub fn verify_response(
        &self,
        operation_id: [u8; 16],
        response: &mut [u8; 32],
        clock: ClockSnapshot,
    ) -> Result<VerifiedHealthResponse, UpdateRuntimeError> {
        let result = (|| {
            let loaded = self.load_required()?;
            if operation_id != loaded.operation_id {
                return Err(UpdateRuntimeError::OperationMismatch);
            }
            if clock.boot_id != loaded.boot_id {
                return Err(UpdateRuntimeError::BootMismatch);
            }
            if clock.monotonic_ms > loaded.document.deadline_monotonic_ms {
                return Err(UpdateRuntimeError::ChallengeExpired);
            }
            let response_binding: [u8; 32] = Sha256::digest(response.as_slice()).into();
            if !bool::from(response_binding.ct_eq(&loaded.health_binding)) {
                return Err(UpdateRuntimeError::InvalidHealthResponse);
            }
            Ok(VerifiedHealthResponse {
                operation_id,
                health_binding: loaded.health_binding,
                target_slot_id: loaded.document.target_slot_id,
            })
        })();
        response.zeroize();
        result
    }

    pub fn consume(&self, operation_id: [u8; 16]) -> Result<(), UpdateRuntimeError> {
        let loaded = self.load_required()?;
        if operation_id != loaded.operation_id {
            return Err(UpdateRuntimeError::OperationMismatch);
        }
        self.remove_safe_file(&self.token_path, 0o400, TOKEN_BYTES)?;
        self.remove_safe_file(&self.state_path, 0o600, MAX_STATE_BYTES)?;
        Ok(())
    }

    pub fn discard(&self, operation_id: [u8; 16]) -> Result<(), UpdateRuntimeError> {
        self.consume(operation_id)
    }

    /// Inspect surviving evidence without changing it. The caller supplies the
    /// authoritative journal identity and holds manager/slot locks. The result
    /// grants neither confirmation nor cleanup authority.
    pub fn inspect_exact(
        &self,
        operation_id: [u8; 16],
        health_binding: [u8; 32],
        target_slot_id: &str,
    ) -> Result<ChallengePresence, UpdateRuntimeError> {
        self.validate_root()?;
        if operation_id == [0; 16] || health_binding == [0; 32] || !valid_slot_id(target_slot_id) {
            return Err(UpdateRuntimeError::InvalidChallengeState);
        }
        let (state_present, token_present) = self.nodes_present()?;
        if state_present {
            let loaded = self.load_document()?;
            if loaded.operation_id != operation_id
                || loaded.document.target_slot_id != target_slot_id
                || !bool::from(loaded.health_binding.ct_eq(&health_binding))
            {
                return Err(UpdateRuntimeError::OperationMismatch);
            }
        }
        if token_present {
            let mut token = self.read_safe_token()?;
            let observed: [u8; 32] = Sha256::digest(token).into();
            token.zeroize();
            if !bool::from(observed.ct_eq(&health_binding)) {
                return Err(UpdateRuntimeError::InvalidChallengeState);
            }
        }
        Ok(match (state_present, token_present) {
            (false, false) => ChallengePresence::Absent,
            (true, false) => ChallengePresence::StateOnly,
            (false, true) => ChallengePresence::TokenOnly,
            (true, true) => ChallengePresence::Complete,
        })
    }

    /// Idempotent cleanup of one completed operation. The caller must hold
    /// manager/slot locks and validate durable completion before calling.
    /// All surviving evidence is checked before either file is removed.
    pub fn consume_exact(
        &self,
        operation_id: [u8; 16],
        health_binding: [u8; 32],
        target_slot_id: &str,
    ) -> Result<bool, UpdateRuntimeError> {
        let presence = self.inspect_exact(operation_id, health_binding, target_slot_id)?;
        if matches!(
            presence,
            ChallengePresence::TokenOnly | ChallengePresence::Complete
        ) {
            self.remove_safe_file(&self.token_path, 0o400, TOKEN_BYTES)?;
        }
        if matches!(
            presence,
            ChallengePresence::StateOnly | ChallengePresence::Complete
        ) {
            self.remove_safe_file(&self.state_path, 0o600, MAX_STATE_BYTES)?;
        }
        // A previous attempt may have unlinked the final file but failed its
        // directory sync. Replaying an empty pair must finish that durability step.
        self.root_directory.sync_all()?;
        Ok(presence != ChallengePresence::Absent)
    }

    pub fn clear_incomplete_for_recovery(&self) -> Result<bool, UpdateRuntimeError> {
        self.validate_root()?;
        let presence = self.nodes_present()?;
        if presence == (false, false) {
            return Ok(false);
        }
        if presence == (true, true) {
            return Err(UpdateRuntimeError::ChallengeExists);
        }
        if presence.1 {
            self.remove_safe_file(&self.token_path, 0o400, TOKEN_BYTES)?;
        }
        if presence.0 {
            self.remove_safe_file(&self.state_path, 0o600, MAX_STATE_BYTES)?;
        }
        Ok(true)
    }

    pub(crate) fn clear_stale_after_rollback(&self) -> Result<bool, UpdateRuntimeError> {
        self.validate_root()?;
        let presence = self.nodes_present()?;
        if presence.1 {
            self.remove_safe_file(&self.token_path, 0o400, TOKEN_BYTES)?;
        }
        if presence.0 {
            self.remove_safe_file(&self.state_path, 0o600, MAX_STATE_BYTES)?;
        }
        Ok(presence != (false, false))
    }

    fn load_required(&self) -> Result<LoadedChallenge, UpdateRuntimeError> {
        self.validate_root()?;
        if self.nodes_present()? != (true, true) {
            return Err(UpdateRuntimeError::ChallengeMissing);
        }
        let loaded = self.load_document()?;
        let mut token = self.read_safe_token()?;
        let observed_binding: [u8; 32] = Sha256::digest(token).into();
        token.zeroize();
        if !bool::from(observed_binding.ct_eq(&loaded.health_binding)) {
            return Err(UpdateRuntimeError::InvalidChallengeState);
        }
        Ok(loaded)
    }

    fn load_document(&self) -> Result<LoadedChallenge, UpdateRuntimeError> {
        let state_bytes = self.read_safe_file(&self.state_path, 0o600, MAX_STATE_BYTES)?;
        let document: ChallengeDocument = serde_json::from_slice(&state_bytes)
            .map_err(|_| UpdateRuntimeError::InvalidChallengeState)?;
        let operation_id = decode_array::<16>(&document.operation_id)?;
        let health_binding = decode_array::<32>(&document.health_binding)?;
        let boot_id = decode_array::<16>(&document.boot_id)?;
        if document.schema_version != CHALLENGE_SCHEMA_VERSION
            || !valid_slot_id(&document.target_slot_id)
            || operation_id.iter().all(|byte| *byte == 0)
            || health_binding.iter().all(|byte| *byte == 0)
            || boot_id.iter().all(|byte| *byte == 0)
            || document.deadline_monotonic_ms <= document.issued_monotonic_ms
        {
            return Err(UpdateRuntimeError::InvalidChallengeState);
        }
        Ok(LoadedChallenge {
            document,
            operation_id,
            health_binding,
            boot_id,
        })
    }

    fn nodes_present(&self) -> Result<(bool, bool), UpdateRuntimeError> {
        Ok((
            node_exists(&self.state_path)?,
            node_exists(&self.token_path)?,
        ))
    }

    fn write_noclobber(
        &self,
        path: &Path,
        bytes: &[u8],
        mode: u32,
    ) -> Result<(), UpdateRuntimeError> {
        self.write_file(path, bytes, mode, false)
    }

    fn write_file(
        &self,
        path: &Path,
        bytes: &[u8],
        mode: u32,
        replace: bool,
    ) -> Result<(), UpdateRuntimeError> {
        self.validate_root()?;
        let mut temporary = tempfile::NamedTempFile::new_in(&self.root)?;
        fs::set_permissions(temporary.path(), Permissions::from_mode(mode))?;
        let metadata = temporary.as_file().metadata()?;
        if !metadata.is_file()
            || metadata.dev() != self.identity.device
            || metadata.uid() != self.identity.owner
            || metadata.nlink() != 1
            || metadata.permissions().mode() & 0o7777 != mode
        {
            return Err(UpdateRuntimeError::UnsafeRuntimeRoot);
        }
        temporary.write_all(bytes)?;
        temporary.flush()?;
        temporary.as_file().sync_all()?;
        if replace {
            temporary
                .persist(path)
                .map_err(|error| UpdateRuntimeError::Io(error.error))?;
        } else {
            temporary
                .persist_noclobber(path)
                .map_err(|error| UpdateRuntimeError::Io(error.error))?;
        }
        self.root_directory.sync_all()?;
        Ok(())
    }

    fn read_safe_file(
        &self,
        path: &Path,
        mode: u32,
        maximum_size: u64,
    ) -> Result<Vec<u8>, UpdateRuntimeError> {
        let (file, size) = self.open_safe_file(path, mode, maximum_size)?;
        let mut bytes = Vec::with_capacity(size as usize);
        file.take(maximum_size + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 != size {
            return Err(UpdateRuntimeError::InvalidChallengeState);
        }
        Ok(bytes)
    }

    fn read_safe_token(&self) -> Result<[u8; 32], UpdateRuntimeError> {
        let (mut file, size) = self.open_safe_file(&self.token_path, 0o400, TOKEN_BYTES)?;
        if size != TOKEN_BYTES {
            return Err(UpdateRuntimeError::InvalidChallengeState);
        }
        let mut token = [0_u8; 32];
        if let Err(error) = file.read_exact(&mut token) {
            token.zeroize();
            return Err(UpdateRuntimeError::Io(error));
        }
        let mut extra = [0_u8; 1];
        match file.read(&mut extra) {
            Ok(0) => {}
            Ok(_) => {
                token.zeroize();
                return Err(UpdateRuntimeError::InvalidChallengeState);
            }
            Err(error) => {
                token.zeroize();
                return Err(UpdateRuntimeError::Io(error));
            }
        }
        Ok(token)
    }

    fn open_safe_file(
        &self,
        path: &Path,
        mode: u32,
        maximum_size: u64,
    ) -> Result<(File, u64), UpdateRuntimeError> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| UpdateRuntimeError::InvalidChallengeState)?;
        let metadata = file
            .metadata()
            .map_err(|_| UpdateRuntimeError::InvalidChallengeState)?;
        if !metadata.is_file()
            || metadata.dev() != self.identity.device
            || metadata.uid() != self.identity.owner
            || metadata.nlink() != 1
            || metadata.permissions().mode() & 0o7777 != mode
            || metadata.len() == 0
            || metadata.len() > maximum_size
        {
            return Err(UpdateRuntimeError::InvalidChallengeState);
        }
        Ok((file, metadata.len()))
    }

    fn remove_safe_file(
        &self,
        path: &Path,
        mode: u32,
        maximum_size: u64,
    ) -> Result<(), UpdateRuntimeError> {
        let _ = self.open_safe_file(path, mode, maximum_size)?;
        fs::remove_file(path)?;
        self.root_directory.sync_all()?;
        Ok(())
    }

    fn validate_root(&self) -> Result<(), UpdateRuntimeError> {
        if validate_runtime_root(&self.root)? != self.identity
            || identity_from_metadata(&self.root_directory.metadata()?) != self.identity
        {
            return Err(UpdateRuntimeError::UnsafeRuntimeRoot);
        }
        Ok(())
    }
}

fn node_exists(path: &Path) -> Result<bool, UpdateRuntimeError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(UpdateRuntimeError::Io(error)),
    }
}

fn validate_runtime_root(path: &Path) -> Result<RuntimeIdentity, UpdateRuntimeError> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(UpdateRuntimeError::InvalidRuntimeRoot);
    }
    let canonical = fs::canonicalize(path).map_err(|_| UpdateRuntimeError::UnsafeRuntimeRoot)?;
    if canonical != path {
        return Err(UpdateRuntimeError::UnsafeRuntimeRoot);
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| UpdateRuntimeError::UnsafeRuntimeRoot)?;
    let identity = identity_from_metadata(&metadata);
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o022 != 0
        || identity.owner != effective_uid()
    {
        return Err(UpdateRuntimeError::UnsafeRuntimeRoot);
    }
    Ok(identity)
}

fn identity_from_metadata(metadata: &fs::Metadata) -> RuntimeIdentity {
    RuntimeIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        owner: metadata.uid(),
    }
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and does not dereference pointers.
    unsafe { libc::geteuid() }
}

fn valid_slot_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 89 || !bytes.starts_with(b"app-") || bytes[24] != b'-' {
        return false;
    }
    let sequence = &bytes[4..24];
    let digest = &bytes[25..];
    if !sequence.iter().all(u8::is_ascii_digit)
        || digest.iter().all(|byte| *byte == b'0')
        || !digest
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return false;
    }
    let sequence = std::str::from_utf8(sequence).expect("ASCII digits are valid UTF-8");
    let Ok(sequence_number) = sequence.parse::<u64>() else {
        return false;
    };
    sequence_number != 0 && format!("{sequence_number:020}") == sequence
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(char::from(HEX[usize::from(byte >> 4)]));
        value.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    value
}

pub(crate) fn decode_array<const N: usize>(value: &str) -> Result<[u8; N], UpdateRuntimeError> {
    if value.len() != N * 2 {
        return Err(UpdateRuntimeError::InvalidChallengeState);
    }
    let mut output = [0_u8; N];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        output[index] = (decode_nibble(pair[0])? << 4) | decode_nibble(pair[1])?;
    }
    Ok(output)
}

fn decode_nibble(value: u8) -> Result<u8, UpdateRuntimeError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(UpdateRuntimeError::InvalidChallengeState),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn request_is_bounded_closed_and_rejects_unsafe_event_files() {
        let root = tempfile::tempdir().unwrap();
        let store = HealthChallengeStore::open(root.path()).unwrap();
        let slot = slot_id(1, b'a');
        store.request_confirmed_apply(1, &slot).unwrap();
        let path = root.path().join(CONFIRMED_APPLY_FILE_NAME);
        let bytes = fs::read(&path).unwrap();
        let parsed = ConfirmedApplyRequest::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.generation, 1);
        assert_eq!(parsed.slot_id, slot);
        assert!(
            ConfirmedApplyRequest::from_bytes(&vec![b'x'; MAX_APPLY_REQUEST_BYTES + 1]).is_err()
        );
        let mut unknown: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        unknown["path"] = serde_json::json!("/unexpected");
        assert!(ConfirmedApplyRequest::from_bytes(&serde_json::to_vec(&unknown).unwrap()).is_err());
        fs::rename(&path, root.path().join("saved")).unwrap();
        symlink(root.path().join("saved"), &path).unwrap();
        assert!(store.confirmed_apply_request().is_err());
        assert!(store.acknowledge_confirmed_apply(&parsed).is_err());
        assert!(root.path().join("saved").exists());
    }

    fn slot_id(sequence: u64, digest: u8) -> String {
        format!(
            "app-{sequence:020}-{}",
            char::from(digest).to_string().repeat(64)
        )
    }

    fn clock(boot: u8, time: u64) -> ClockSnapshot {
        ClockSnapshot {
            boot_id: [boot; 16],
            monotonic_ms: time,
        }
    }

    #[test]
    fn issues_verifies_zeroizes_and_consumes_one_use_health_evidence() {
        let runtime = tempfile::tempdir().unwrap();
        let store = HealthChallengeStore::open(runtime.path()).unwrap();
        let issued = store
            .issue(&slot_id(1, b'a'), clock(1, 100), 1_100)
            .unwrap();
        assert!(issued.operation_id.iter().any(|byte| *byte != 0));
        assert!(issued.health_binding.iter().any(|byte| *byte != 0));
        assert_eq!(
            fs::symlink_metadata(&issued.credential_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o400
        );
        let status = store.status().unwrap().unwrap();
        assert_eq!(status.operation_id, issued.operation_id);
        assert_eq!(status.target_slot_id, issued.target_slot_id);
        assert!(matches!(
            store.clear_incomplete_for_recovery(),
            Err(UpdateRuntimeError::ChallengeExists)
        ));

        let mut wrong = [9_u8; 32];
        assert!(matches!(
            store.verify_response(issued.operation_id, &mut wrong, clock(1, 200)),
            Err(UpdateRuntimeError::InvalidHealthResponse)
        ));
        assert_eq!(wrong, [0; 32]);

        let mut token: [u8; 32] = fs::read(&issued.credential_path)
            .unwrap()
            .try_into()
            .unwrap();
        let verified = store
            .verify_response(issued.operation_id, &mut token, clock(1, 200))
            .unwrap();
        assert_eq!(token, [0; 32]);
        assert_eq!(verified.health_binding, issued.health_binding);
        store.consume(issued.operation_id).unwrap();
        assert_eq!(store.status().unwrap(), None);
        assert!(matches!(
            store.consume(issued.operation_id),
            Err(UpdateRuntimeError::ChallengeMissing)
        ));
    }

    #[test]
    fn binds_responses_to_operation_boot_and_deadline() {
        let runtime = tempfile::tempdir().unwrap();
        let store = HealthChallengeStore::open(runtime.path()).unwrap();
        let issued = store
            .issue(&slot_id(2, b'b'), clock(2, 500), 1_500)
            .unwrap();
        assert!(matches!(
            store.issue(&slot_id(3, b'c'), clock(2, 600), 1_600),
            Err(UpdateRuntimeError::ChallengeExists)
        ));

        let read_token = || -> [u8; 32] {
            fs::read(&issued.credential_path)
                .unwrap()
                .try_into()
                .unwrap()
        };
        let mut token = read_token();
        assert!(matches!(
            store.verify_response([3; 16], &mut token, clock(2, 600)),
            Err(UpdateRuntimeError::OperationMismatch)
        ));
        assert_eq!(token, [0; 32]);
        let mut token = read_token();
        assert!(matches!(
            store.verify_response(issued.operation_id, &mut token, clock(3, 1)),
            Err(UpdateRuntimeError::BootMismatch)
        ));
        assert_eq!(token, [0; 32]);
        let mut token = read_token();
        assert!(matches!(
            store.verify_response(issued.operation_id, &mut token, clock(2, 1_501)),
            Err(UpdateRuntimeError::ChallengeExpired)
        ));
        assert_eq!(token, [0; 32]);
        store.discard(issued.operation_id).unwrap();
    }

    #[test]
    fn recovery_cleanup_is_marker_exact_and_refuses_symlinks() {
        let runtime = tempfile::tempdir().unwrap();
        let store = HealthChallengeStore::open(runtime.path()).unwrap();
        fs::write(runtime.path().join(TOKEN_FILE_NAME), [1_u8; 32]).unwrap();
        fs::set_permissions(
            runtime.path().join(TOKEN_FILE_NAME),
            Permissions::from_mode(0o400),
        )
        .unwrap();
        assert!(store.clear_incomplete_for_recovery().unwrap());
        assert_eq!(store.status().unwrap(), None);

        let outside = runtime.path().join("outside");
        fs::write(&outside, b"preserve").unwrap();
        symlink(&outside, runtime.path().join(TOKEN_FILE_NAME)).unwrap();
        assert!(matches!(
            store.clear_incomplete_for_recovery(),
            Err(UpdateRuntimeError::InvalidChallengeState)
        ));
        assert_eq!(fs::read(outside).unwrap(), b"preserve");
    }

    #[test]
    fn rejects_unsafe_or_symlinked_runtime_roots() {
        let unsafe_root = tempfile::tempdir().unwrap();
        fs::set_permissions(unsafe_root.path(), Permissions::from_mode(0o770)).unwrap();
        assert!(matches!(
            HealthChallengeStore::open(unsafe_root.path()),
            Err(UpdateRuntimeError::UnsafeRuntimeRoot)
        ));

        let parent = tempfile::tempdir().unwrap();
        let real = parent.path().join("real");
        fs::create_dir(&real).unwrap();
        fs::set_permissions(&real, Permissions::from_mode(0o700)).unwrap();
        let linked = parent.path().join("linked");
        symlink(real, &linked).unwrap();
        assert!(matches!(
            HealthChallengeStore::open(linked),
            Err(UpdateRuntimeError::UnsafeRuntimeRoot)
        ));
    }

    #[test]
    fn exact_cleanup_replays_each_partial_file_state_after_reopen() {
        for (state_present, token_present) in
            [(true, true), (true, false), (false, true), (false, false)]
        {
            let runtime = tempfile::tempdir().unwrap();
            let store = HealthChallengeStore::open(runtime.path()).unwrap();
            let issued = store.issue(&slot_id(2, b'b'), clock(1, 10), 100).unwrap();
            if !state_present {
                fs::remove_file(&store.state_path).unwrap();
            }
            if !token_present {
                fs::remove_file(&store.token_path).unwrap();
            }
            drop(store);
            let reopened = HealthChallengeStore::open(runtime.path()).unwrap();
            let before_state = fs::read(&reopened.state_path).ok();
            let before_token = fs::read(&reopened.token_path).ok();
            let expected = match (state_present, token_present) {
                (true, true) => ChallengePresence::Complete,
                (true, false) => ChallengePresence::StateOnly,
                (false, true) => ChallengePresence::TokenOnly,
                (false, false) => ChallengePresence::Absent,
            };
            assert_eq!(
                reopened
                    .inspect_exact(
                        issued.operation_id,
                        issued.health_binding,
                        &issued.target_slot_id
                    )
                    .unwrap(),
                expected
            );
            assert_eq!(fs::read(&reopened.state_path).ok(), before_state);
            assert_eq!(fs::read(&reopened.token_path).ok(), before_token);
            assert_eq!(
                reopened
                    .consume_exact(
                        issued.operation_id,
                        issued.health_binding,
                        &issued.target_slot_id
                    )
                    .unwrap(),
                state_present || token_present
            );
            assert!(
                !reopened
                    .consume_exact(
                        issued.operation_id,
                        issued.health_binding,
                        &issued.target_slot_id
                    )
                    .unwrap()
            );
            assert_eq!(reopened.status().unwrap(), None);
        }
    }

    #[test]
    fn runtime_lock_excludes_other_stores_and_releases_without_changing_credentials() {
        let runtime = tempfile::tempdir().unwrap();
        let separate = tempfile::tempdir().unwrap();
        let owner = HealthChallengeStore::open(runtime.path()).unwrap();
        let issued = owner.issue(&slot_id(2, b'b'), clock(1, 10), 100).unwrap();
        let state = fs::read(&owner.state_path).unwrap();
        let token = fs::read(&owner.token_path).unwrap();
        for _ in 0..2 {
            assert!(matches!(
                HealthChallengeStore::open(runtime.path()),
                Err(UpdateRuntimeError::RuntimeBusy)
            ));
            assert_eq!(fs::read(&owner.state_path).unwrap(), state);
            assert_eq!(fs::read(&owner.token_path).unwrap(), token);
        }
        let independent = HealthChallengeStore::open(separate.path()).unwrap();
        independent.require_empty_native_runtime().unwrap();
        assert_eq!(fs::read_dir(runtime.path()).unwrap().count(), 2);
        drop(owner);
        let reopened = HealthChallengeStore::open(runtime.path()).unwrap();
        assert_eq!(
            reopened
                .inspect_exact(
                    issued.operation_id,
                    issued.health_binding,
                    &issued.target_slot_id
                )
                .unwrap(),
            ChallengePresence::Complete
        );
        assert_eq!(fs::read(&reopened.state_path).unwrap(), state);
        assert_eq!(fs::read(&reopened.token_path).unwrap(), token);
    }

    #[test]
    fn admission_precedes_publication_and_refusal_leaves_no_credentials() {
        let runtime = tempfile::tempdir().unwrap();
        let store = HealthChallengeStore::open(runtime.path()).unwrap();
        let target = slot_id(2, b'b');
        let rejected = store.issue_after_admission(&target, clock(1, 10), 100, |issued| {
            assert_eq!(store.nodes_present().unwrap(), (false, false));
            assert_eq!(issued.target_slot_id, target);
            Err(UpdateRuntimeError::SupervisorStateMismatch)
        });
        assert!(matches!(
            rejected,
            Err(UpdateRuntimeError::SupervisorStateMismatch)
        ));
        assert_eq!(store.nodes_present().unwrap(), (false, false));
        let mut admitted = None;
        let issued = store
            .issue_after_admission(&target, clock(1, 10), 100, |identity| {
                assert_eq!(store.nodes_present().unwrap(), (false, false));
                admitted = Some((identity.operation_id, identity.health_binding));
                Ok(())
            })
            .unwrap();
        assert_eq!(admitted, Some((issued.operation_id, issued.health_binding)));
        assert_eq!(
            store
                .inspect_exact(issued.operation_id, issued.health_binding, &target)
                .unwrap(),
            ChallengePresence::Complete
        );
    }

    #[test]
    fn exact_cleanup_preserves_newer_or_mismatched_surviving_evidence() {
        for presence in [(true, true), (true, false), (false, true)] {
            let runtime = tempfile::tempdir().unwrap();
            let store = HealthChallengeStore::open(runtime.path()).unwrap();
            let old = store.issue(&slot_id(2, b'b'), clock(1, 10), 100).unwrap();
            store.consume(old.operation_id).unwrap();
            let new = store.issue(&slot_id(3, b'c'), clock(2, 10), 100).unwrap();
            if !presence.0 {
                fs::remove_file(&store.state_path).unwrap();
            }
            if !presence.1 {
                fs::remove_file(&store.token_path).unwrap();
            }
            let before_state = fs::read(&store.state_path).ok();
            let before_token = fs::read(&store.token_path).ok();
            assert!(
                store
                    .consume_exact(old.operation_id, old.health_binding, &old.target_slot_id)
                    .is_err()
            );
            assert_eq!(fs::read(&store.state_path).ok(), before_state);
            assert_eq!(fs::read(&store.token_path).ok(), before_token);
            assert!(
                store
                    .consume_exact(new.operation_id, new.health_binding, &new.target_slot_id)
                    .unwrap()
            );
        }
    }

    #[test]
    fn exact_cleanup_checks_all_evidence_before_deletion() {
        for damage in 0..4 {
            let runtime = tempfile::tempdir().unwrap();
            let store = HealthChallengeStore::open(runtime.path()).unwrap();
            let issued = store.issue(&slot_id(2, b'b'), clock(1, 10), 100).unwrap();
            match damage {
                0 => fs::write(&store.state_path, b"{}").unwrap(),
                1 => {
                    fs::set_permissions(&store.token_path, Permissions::from_mode(0o600)).unwrap();
                    fs::write(&store.token_path, [9; 32]).unwrap();
                    fs::set_permissions(&store.token_path, Permissions::from_mode(0o400)).unwrap();
                }
                2 => {
                    fs::rename(&store.token_path, runtime.path().join("saved-token")).unwrap();
                    symlink(runtime.path().join("saved-token"), &store.token_path).unwrap();
                }
                _ => fs::set_permissions(&store.state_path, Permissions::from_mode(0o644)).unwrap(),
            }
            assert!(
                store
                    .consume_exact(
                        issued.operation_id,
                        issued.health_binding,
                        &issued.target_slot_id
                    )
                    .is_err()
            );
            assert_eq!(store.nodes_present().unwrap(), (true, true));
        }
    }
}
