//! Read-only application compatibility surface.
//!
//! Ported from the Hardened NanoKVM `VersionRsp`/`PreviewRsp` shapes. BLi-KVM
//! ships its releases through the retained signed application-update path, so
//! this module exposes the read-only view only: no update trigger, no preview
//! flag mutation. `latest` stays empty because the hardened update channel
//! reports availability through `/api/application-update/status`.

use std::path::Path;

use axum::Json;
use serde::Serialize;

use crate::error::ApiResponse;

const PREVIEW_UPDATES_FLAG: &str = "/etc/kvm/preview_updates";

#[derive(Debug, Serialize)]
pub struct VersionResponse {
    pub current: String,
    pub latest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PreviewResponse {
    enabled: bool,
}

pub async fn version() -> Json<ApiResponse<VersionResponse>> {
    Json(ApiResponse::ok(VersionResponse {
        current: env!("CARGO_PKG_VERSION").to_string(),
        latest: String::new(),
        blocked: None,
        error: None,
    }))
}

pub async fn preview() -> Json<ApiResponse<PreviewResponse>> {
    Json(ApiResponse::ok(PreviewResponse {
        enabled: preview_enabled_at(Path::new(PREVIEW_UPDATES_FLAG)),
    }))
}

fn preview_enabled_at(path: &Path) -> bool {
    path.exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_reports_the_bundled_application_version() {
        let response = VersionResponse {
            current: env!("CARGO_PKG_VERSION").to_string(),
            latest: String::new(),
            blocked: None,
            error: None,
        };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["current"], env!("CARGO_PKG_VERSION"));
        assert_eq!(json["latest"], "");
        assert!(json.get("blocked").is_none());
        assert!(json.get("error").is_none());
    }

    #[test]
    fn preview_flag_is_read_from_the_filesystem() {
        let directory = tempfile::tempdir().unwrap();
        let flag = directory.path().join("preview_updates");
        assert!(!preview_enabled_at(&flag));
        std::fs::write(&flag, b"1").unwrap();
        assert!(preview_enabled_at(&flag));
        std::fs::remove_file(&flag).unwrap();
        assert!(!preview_enabled_at(&flag));
    }
}
