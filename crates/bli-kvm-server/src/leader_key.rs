//! Leader-key preference for the on-screen shortcut UI.
//!
//! The reference GUI loads the configured leader key from the server and stores
//! a new one when the operator picks it. The value is a single key code (for
//! example `ControlRight`) kept in the server state directory; an empty value
//! means "no leader key". The endpoint is available only when the restricted
//! control surface is enabled, so a shipped build keeps it unavailable instead
//! of exposing a dead preference.

use std::path::{Path, PathBuf};

use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    error::{ApiResponse, AppError, AppResult},
};

/// Longest accepted key code; the UsbKey names are far shorter.
const MAX_KEY_LENGTH: usize = 32;

#[derive(Debug, Serialize)]
pub(crate) struct LeaderKeyResponse {
    pub key: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetLeaderKeyBody {
    pub key: String,
}

fn surface_available(state: &AppState) -> bool {
    state.options.restricted_controls_enabled && state.control.is_some()
}

pub(crate) fn leader_key_path(state_dir: &Path) -> PathBuf {
    state_dir.join("leader-key")
}

/// Validate and normalise one key code; the empty string clears the setting.
fn validate_key(key: &str) -> Result<&str, AppError> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Ok("");
    }
    if trimmed.len() > MAX_KEY_LENGTH || !trimmed.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(AppError::BadRequest("leader key is invalid".into()));
    }
    Ok(trimmed)
}

pub(crate) fn read_leader_key(state_dir: &Path) -> Result<String, AppError> {
    match std::fs::read_to_string(leader_key_path(state_dir)) {
        Ok(value) => Ok(value
            .chars()
            .filter(|character| *character != '\n' && *character != '\r')
            .collect()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(_) => Err(AppError::ServiceUnavailable(
            "leader key storage is unavailable".into(),
        )),
    }
}

/// Store the key atomically, or remove the file for an empty key.
pub(crate) fn write_leader_key(state_dir: &Path, key: &str) -> Result<(), AppError> {
    let path = leader_key_path(state_dir);
    if key.is_empty() {
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(AppError::ServiceUnavailable(
                "leader key storage is unavailable".into(),
            )),
        };
    }
    std::fs::create_dir_all(state_dir)
        .map_err(|_| AppError::ServiceUnavailable("leader key storage is unavailable".into()))?;
    let temporary = state_dir.join("leader-key.tmp");
    std::fs::write(&temporary, key.as_bytes())
        .map_err(|_| AppError::ServiceUnavailable("leader key storage is unavailable".into()))?;
    std::fs::rename(&temporary, &path)
        .map_err(|_| AppError::ServiceUnavailable("leader key storage is unavailable".into()))
}

pub(crate) async fn get_leader_key(
    State(state): State<AppState>,
) -> AppResult<Json<ApiResponse<LeaderKeyResponse>>> {
    if !surface_available(&state) {
        return Err(AppError::ServiceUnavailable(
            "the shortcut surface is not available".into(),
        ));
    }
    let key = read_leader_key(&state.options.state_dir)?;
    Ok(Json(ApiResponse::ok(LeaderKeyResponse { key })))
}

pub(crate) async fn set_leader_key(
    State(state): State<AppState>,
    Json(request): Json<SetLeaderKeyBody>,
) -> AppResult<Json<ApiResponse<LeaderKeyResponse>>> {
    if !surface_available(&state) {
        return Err(AppError::ServiceUnavailable(
            "the shortcut surface is not available".into(),
        ));
    }
    let key = validate_key(&request.key)?;
    write_leader_key(&state.options.state_dir, key)?;
    Ok(Json(ApiResponse::ok(LeaderKeyResponse {
        key: key.to_owned(),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_validated_not_coerced() {
        assert_eq!(validate_key("ControlRight").unwrap(), "ControlRight");
        assert_eq!(validate_key("  ShiftLeft \n").unwrap(), "ShiftLeft");
        assert_eq!(validate_key("").unwrap(), "");
        assert_eq!(validate_key("   ").unwrap(), "");
        assert!(validate_key("Control-Right").is_err());
        assert!(validate_key("Control Right").is_err());
        assert!(validate_key(&"a".repeat(MAX_KEY_LENGTH + 1)).is_err());
    }

    #[test]
    fn storage_round_trips_and_clears() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(read_leader_key(directory.path()).unwrap(), "");
        write_leader_key(directory.path(), "ControlRight").unwrap();
        assert_eq!(read_leader_key(directory.path()).unwrap(), "ControlRight");
        std::fs::write(leader_key_path(directory.path()), "ShiftRight\n").unwrap();
        assert_eq!(read_leader_key(directory.path()).unwrap(), "ShiftRight");
        write_leader_key(directory.path(), "").unwrap();
        assert_eq!(read_leader_key(directory.path()).unwrap(), "");
        write_leader_key(directory.path(), "").unwrap();
    }

    #[test]
    fn stored_value_is_not_revalidated_on_read() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(leader_key_path(directory.path()), "not-a-key\n").unwrap();
        assert_eq!(read_leader_key(directory.path()).unwrap(), "not-a-key");
    }

    #[test]
    fn response_shape_matches_the_reference() {
        let response = LeaderKeyResponse {
            key: "ControlRight".into(),
        };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json, serde_json::json!({ "key": "ControlRight" }));
    }
}
