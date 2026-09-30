//! Reviewed Wake-on-LAN address history.
//!
//! The interface lists the addresses that were woken, names and deletes them,
//! as the reference does. The server owns this control-plane file in its state
//! directory: it bounds every field, keeps at most a fixed number of entries,
//! and never sends anything itself. Entries use the reference wire format,
//! `"AA:BB:CC:DD:EE:FF name"`.

// Without the restricted control surface no wake is sent, so nothing records.
#![cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    error::{ApiResponse, AppError, AppResult},
};

pub const MAX_MACS: usize = 32;
const MAX_NAME_CHARS: usize = 32;
const MAX_FILE_BYTES: u64 = 8 * 1024;
const FILE_NAME: &str = "wol-macs.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WolMac {
    mac: String,
    name: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Store {
    macs: Vec<WolMac>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum WolMacError {
    InvalidMac,
    InvalidName,
    NotFound,
    Unavailable,
}

/// `AA:BB:CC:DD:EE:FF` from colon, dash, dot or bare hex notation.
pub fn normalize_mac(value: &str) -> Result<String, WolMacError> {
    let hex: String = value
        .trim()
        .to_ascii_uppercase()
        .chars()
        .filter(|ch| !matches!(ch, '-' | ':' | '.'))
        .collect();
    if hex.len() != 12 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(WolMacError::InvalidMac);
    }
    Ok(hex
        .as_bytes()
        .chunks(2)
        .map(|pair| std::str::from_utf8(pair).unwrap_or_default())
        .collect::<Vec<_>>()
        .join(":"))
}

fn sanitize_name(name: &str) -> Result<String, WolMacError> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty()
        || name.chars().count() > MAX_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        return Err(WolMacError::InvalidName);
    }
    Ok(name)
}

pub struct WolMacStore {
    path: PathBuf,
    store: Store,
}

impl WolMacStore {
    pub fn open(state_dir: &Path) -> Result<Self, WolMacError> {
        let path = state_dir.join(FILE_NAME);
        if path.exists() {
            let metadata = std::fs::metadata(&path).map_err(|_| WolMacError::Unavailable)?;
            if metadata.len() > MAX_FILE_BYTES {
                return Err(WolMacError::Unavailable);
            }
        }
        let store = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Store>(&text).ok())
            .unwrap_or_default();
        Ok(Self { path, store })
    }

    /// Reference wire format: the address, then the name when one is set.
    pub fn lines(&self) -> Vec<String> {
        self.store
            .macs
            .iter()
            .map(|entry| {
                if entry.name.is_empty() {
                    entry.mac.clone()
                } else {
                    format!("{} {}", entry.mac, entry.name)
                }
            })
            .collect()
    }

    /// Remembers a woken address once; the oldest entry makes room.
    pub fn record(&mut self, mac: &str) -> Result<(), WolMacError> {
        let mac = normalize_mac(mac)?;
        if self.store.macs.iter().any(|entry| entry.mac == mac) {
            return Ok(());
        }
        if self.store.macs.len() >= MAX_MACS {
            self.store.macs.remove(0);
        }
        self.store.macs.push(WolMac {
            mac,
            name: String::new(),
        });
        self.persist()
    }

    pub fn rename(&mut self, mac: &str, name: &str) -> Result<(), WolMacError> {
        let mac = normalize_mac(mac)?;
        let name = sanitize_name(name)?;
        let entry = self
            .store
            .macs
            .iter_mut()
            .find(|entry| entry.mac == mac)
            .ok_or(WolMacError::NotFound)?;
        entry.name = name;
        self.persist()
    }

    pub fn delete(&mut self, mac: &str) -> Result<(), WolMacError> {
        let mac = normalize_mac(mac)?;
        let before = self.store.macs.len();
        self.store.macs.retain(|entry| entry.mac != mac);
        if self.store.macs.len() == before {
            return Err(WolMacError::NotFound);
        }
        self.persist()
    }

    fn persist(&self) -> Result<(), WolMacError> {
        let parent = self.path.parent().ok_or(WolMacError::Unavailable)?;
        let metadata = std::fs::symlink_metadata(parent).map_err(|_| WolMacError::Unavailable)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(WolMacError::Unavailable);
        }
        let text = serde_json::to_string(&self.store).map_err(|_| WolMacError::Unavailable)?;
        if text.len() as u64 > MAX_FILE_BYTES {
            return Err(WolMacError::Unavailable);
        }
        std::fs::write(&self.path, text).map_err(|_| WolMacError::Unavailable)?;
        std::fs::set_permissions(&self.path, PermissionsExt::from_mode(0o600))
            .map_err(|_| WolMacError::Unavailable)
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct WolMacList {
    macs: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WolMacBody {
    mac: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WolMacNameBody {
    mac: String,
    name: String,
}

fn open(state: &AppState) -> AppResult<WolMacStore> {
    WolMacStore::open(&state.options.state_dir)
        .map_err(|_| AppError::ServiceUnavailable("the address history is unavailable".into()))
}

fn map_error(error: WolMacError) -> AppError {
    match error {
        WolMacError::InvalidMac => AppError::BadRequest("invalid MAC address".into()),
        WolMacError::InvalidName => AppError::BadRequest("invalid name".into()),
        WolMacError::NotFound => AppError::BadRequest("unknown MAC address".into()),
        WolMacError::Unavailable => {
            AppError::ServiceUnavailable("the address history is unavailable".into())
        }
    }
}

/// Records an address after a successful wake; a storage failure never turns
/// a sent packet into an error.
pub(crate) fn remember(state: &AppState, mac: &str) {
    if let Ok(mut store) = WolMacStore::open(&state.options.state_dir) {
        let _ = store.record(mac);
    }
}

pub(crate) async fn list(
    State(state): State<AppState>,
) -> AppResult<Json<ApiResponse<WolMacList>>> {
    Ok(Json(ApiResponse::ok(WolMacList {
        macs: open(&state)?.lines(),
    })))
}

pub(crate) async fn delete(
    State(state): State<AppState>,
    Json(body): Json<WolMacBody>,
) -> AppResult<Json<ApiResponse<()>>> {
    open(&state)?.delete(&body.mac).map_err(map_error)?;
    Ok(Json(ApiResponse::ok(())))
}

pub(crate) async fn rename(
    State(state): State<AppState>,
    Json(body): Json<WolMacNameBody>,
) -> AppResult<Json<ApiResponse<()>>> {
    open(&state)?
        .rename(&body.mac, &body.name)
        .map_err(map_error)?;
    Ok(Json(ApiResponse::ok(())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_normalized_bounded_named_and_deleted() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            normalize_mac("aa-bb-cc-dd-ee-ff").unwrap(),
            "AA:BB:CC:DD:EE:FF"
        );
        assert_eq!(
            normalize_mac("aabb.ccdd.eeff").unwrap(),
            "AA:BB:CC:DD:EE:FF"
        );
        assert!(normalize_mac("aa:bb:cc:dd:ee").is_err());
        assert!(normalize_mac("gg:bb:cc:dd:ee:ff").is_err());

        let mut store = WolMacStore::open(dir.path()).unwrap();
        store.record("00:11:22:33:44:55").unwrap();
        store.record("00-11-22-33-44-55").unwrap();
        assert_eq!(store.lines(), ["00:11:22:33:44:55"]);
        store.rename("001122334455", "  NAS   box ").unwrap();
        assert!(store.rename("00:11:22:33:44:56", "x").is_err());
        assert!(store.rename("00:11:22:33:44:55", " ").is_err());
        assert!(store.rename("00:11:22:33:44:55", &"n".repeat(33)).is_err());
        let reopened = WolMacStore::open(dir.path()).unwrap();
        assert_eq!(reopened.lines(), ["00:11:22:33:44:55 NAS box"]);

        for index in 0..MAX_MACS {
            store
                .record(&format!("02:00:00:00:00:{index:02X}"))
                .unwrap();
        }
        assert_eq!(store.lines().len(), MAX_MACS);
        // The oldest entry made room.
        assert!(!store.lines()[0].starts_with("00:11:22:33:44:55"));

        store.delete("02:00:00:00:00:1F").unwrap();
        assert_eq!(
            store.delete("02:00:00:00:00:1F"),
            Err(WolMacError::NotFound)
        );
        let mode = std::fs::metadata(dir.path().join(FILE_NAME))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
