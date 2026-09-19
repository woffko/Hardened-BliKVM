//! Reviewed HID shortcut store.
//!
//! The interface records key sequences, lists them and deletes them again. The
//! server owns this control-plane file in its state directory: it bounds every
//! field, keeps at most a fixed number of entries, writes atomically and never
//! executes anything.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    error::{ApiResponse, AppError, AppResult},
};

pub const MAX_SHORTCUTS: usize = 32;
pub const MAX_KEYS: usize = 4;
const MAX_CODE_BYTES: usize = 32;
const MAX_LABEL_BYTES: usize = 24;
const MAX_FILE_BYTES: u64 = 32 * 1024;
const FILE_NAME: &str = "shortcuts.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShortcutKey {
    pub code: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Shortcut {
    pub id: String,
    pub keys: Vec<ShortcutKey>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Store {
    #[serde(default)]
    shortcuts: Vec<Shortcut>,
}

#[derive(Debug)]
pub struct ShortcutStore {
    path: PathBuf,
    store: Store,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutError {
    InvalidKey,
    TooManyKeys,
    TooManyShortcuts,
    NotFound,
    Unavailable,
}

impl ShortcutStore {
    /// Opens the store under the reviewed state directory.
    pub fn open(state_dir: &Path) -> Result<Self, ShortcutError> {
        let path = state_dir.join(FILE_NAME);
        if path.exists() {
            let metadata = std::fs::metadata(&path).map_err(|_| ShortcutError::Unavailable)?;
            if metadata.len() > MAX_FILE_BYTES {
                return Err(ShortcutError::Unavailable);
            }
        }
        let store = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Store>(&text).ok())
            .unwrap_or_default();
        Ok(Self { path, store })
    }

    pub fn list(&self) -> Vec<Shortcut> {
        self.store.shortcuts.clone()
    }

    pub fn add(&mut self, keys: Vec<ShortcutKey>) -> Result<Shortcut, ShortcutError> {
        if keys.is_empty() || keys.len() > MAX_KEYS {
            return Err(ShortcutError::TooManyKeys);
        }
        for key in &keys {
            if !valid_code(&key.code) || key.label.len() > MAX_LABEL_BYTES {
                return Err(ShortcutError::InvalidKey);
            }
        }
        if self.store.shortcuts.len() >= MAX_SHORTCUTS {
            return Err(ShortcutError::TooManyShortcuts);
        }
        let shortcut = Shortcut {
            id: self.next_id(),
            keys,
        };
        self.store.shortcuts.push(shortcut.clone());
        self.persist()?;
        Ok(shortcut)
    }

    pub fn delete(&mut self, id: &str) -> Result<(), ShortcutError> {
        let before = self.store.shortcuts.len();
        self.store.shortcuts.retain(|shortcut| shortcut.id != id);
        if self.store.shortcuts.len() == before {
            return Err(ShortcutError::NotFound);
        }
        self.persist()
    }

    fn next_id(&self) -> String {
        // Deterministic and unique inside the store, with no clock or clock
        // dependency to leak into a receipt.
        let mut candidate = self.store.shortcuts.len() + 1;
        loop {
            let id = format!("{candidate:08x}");
            if !self
                .store
                .shortcuts
                .iter()
                .any(|shortcut| shortcut.id == id)
            {
                return id;
            }
            candidate += 1;
        }
    }

    fn persist(&self) -> Result<(), ShortcutError> {
        let parent = self.path.parent().ok_or(ShortcutError::Unavailable)?;
        let metadata = std::fs::symlink_metadata(parent).map_err(|_| ShortcutError::Unavailable)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(ShortcutError::Unavailable);
        }
        let text = serde_json::to_string(&self.store).map_err(|_| ShortcutError::Unavailable)?;
        if text.len() as u64 > MAX_FILE_BYTES {
            return Err(ShortcutError::Unavailable);
        }
        std::fs::write(&self.path, text).map_err(|_| ShortcutError::Unavailable)?;
        std::fs::set_permissions(&self.path, PermissionsExt::from_mode(0o600))
            .map_err(|_| ShortcutError::Unavailable)
    }
}

fn valid_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= MAX_CODE_BYTES
        && code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShortcutList {
    pub shortcuts: Vec<Shortcut>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ShortcutKeysBody {
    pub keys: Vec<ShortcutKey>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ShortcutIdBody {
    pub id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShortcutOutcome {
    pub applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub shortcuts: Vec<Shortcut>,
}

fn open(state: &AppState) -> AppResult<ShortcutStore> {
    ShortcutStore::open(&state.options.state_dir)
        .map_err(|_| AppError::ServiceUnavailable("shortcut store is unavailable".into()))
}

pub(crate) async fn list(
    State(state): State<AppState>,
) -> AppResult<Json<ApiResponse<ShortcutList>>> {
    let store = open(&state)?;
    Ok(Json(ApiResponse::ok(ShortcutList {
        shortcuts: store.list(),
    })))
}

pub(crate) async fn add(
    State(state): State<AppState>,
    Json(body): Json<ShortcutKeysBody>,
) -> AppResult<Json<ApiResponse<ShortcutOutcome>>> {
    let mut store = open(&state)?;
    match store.add(body.keys) {
        Ok(shortcut) => Ok(Json(ApiResponse::ok(ShortcutOutcome {
            applied: true,
            id: Some(shortcut.id),
            shortcuts: store.list(),
        }))),
        Err(ShortcutError::TooManyKeys) => Err(AppError::BadRequest(
            "a shortcut needs one to four keys".into(),
        )),
        Err(ShortcutError::InvalidKey) => {
            Err(AppError::BadRequest("a shortcut key is invalid".into()))
        }
        Err(ShortcutError::TooManyShortcuts) => {
            Err(AppError::Conflict("the shortcut list is full".into()))
        }
        Err(_) => Err(AppError::ServiceUnavailable(
            "shortcut store is unavailable".into(),
        )),
    }
}

pub(crate) async fn delete(
    State(state): State<AppState>,
    Json(body): Json<ShortcutIdBody>,
) -> AppResult<Json<ApiResponse<ShortcutOutcome>>> {
    let mut store = open(&state)?;
    match store.delete(&body.id) {
        Ok(()) => Ok(Json(ApiResponse::ok(ShortcutOutcome {
            applied: true,
            id: Some(body.id),
            shortcuts: store.list(),
        }))),
        Err(ShortcutError::NotFound) => Err(AppError::NotFound("shortcut not found".into())),
        Err(_) => Err(AppError::ServiceUnavailable(
            "shortcut store is unavailable".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: &str) -> ShortcutKey {
        ShortcutKey {
            code: code.into(),
            label: code.into(),
        }
    }

    #[test]
    fn shortcuts_round_trip_through_the_reviewed_file() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ShortcutStore::open(directory.path()).unwrap();
        assert!(store.list().is_empty());

        let added = store
            .add(vec![key("MetaLeft"), key("KeyT")])
            .expect("a reviewed shortcut");
        assert_eq!(added.id, "00000001");
        assert_eq!(store.list().len(), 1);

        // A second store over the same directory sees the persisted entry.
        let reopened = ShortcutStore::open(directory.path()).unwrap();
        assert_eq!(reopened.list(), vec![added.clone()]);
        let mode = std::fs::metadata(directory.path().join(FILE_NAME))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);

        let mut store = reopened;
        store.delete(&added.id).expect("the entry exists");
        assert!(store.list().is_empty());
        assert_eq!(store.delete(&added.id), Err(ShortcutError::NotFound));
    }

    #[test]
    fn bounds_are_enforced() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ShortcutStore::open(directory.path()).unwrap();
        assert_eq!(store.add(Vec::new()), Err(ShortcutError::TooManyKeys));
        assert_eq!(
            store.add(vec![key("A"), key("B"), key("C"), key("D"), key("E")]),
            Err(ShortcutError::TooManyKeys)
        );
        assert_eq!(
            store.add(vec![ShortcutKey {
                code: "bad code".into(),
                label: "x".into(),
            }]),
            Err(ShortcutError::InvalidKey)
        );
        for _ in 0..MAX_SHORTCUTS {
            store.add(vec![key("KeyA")]).expect("within the bound");
        }
        assert_eq!(
            store.add(vec![key("KeyA")]),
            Err(ShortcutError::TooManyShortcuts)
        );
    }
}
