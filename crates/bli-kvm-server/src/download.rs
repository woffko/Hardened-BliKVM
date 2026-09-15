//! Read-only image-download capability flags.
//!
//! Ported from the Hardened NanoKVM `ImageEnabledRsp` shape. The hardened
//! server exposes only the read-only flags: the local catalog may be present,
//! while remote image download is not a shipped capability and stays disabled.

use axum::{Json, extract::State};
use serde::Serialize;

use crate::AppState;
use crate::error::ApiResponse;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageEnabledResponse {
    enabled: bool,
    remote_enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct RemoteEnabledResponse {
    enabled: bool,
}

pub async fn image_enabled(
    State(state): State<AppState>,
) -> Json<ApiResponse<ImageEnabledResponse>> {
    Json(ApiResponse::ok(ImageEnabledResponse {
        enabled: state.options.media_catalog.is_some(),
        remote_enabled: false,
    }))
}

pub async fn remote_enabled() -> Json<ApiResponse<RemoteEnabledResponse>> {
    Json(ApiResponse::ok(RemoteEnabledResponse { enabled: false }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_download_stays_disabled() {
        let response = RemoteEnabledResponse { enabled: false };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["enabled"], false);
    }

    #[test]
    fn image_enabled_response_is_camel_case() {
        let json = serde_json::to_value(ImageEnabledResponse {
            enabled: true,
            remote_enabled: false,
        })
        .unwrap();
        assert_eq!(json["enabled"], true);
        assert_eq!(json["remoteEnabled"], false);
    }
}
