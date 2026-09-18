//! Safe-target/focus confirmation for the restricted HID path.
//!
//! The server issues a one-time challenge bound to the authenticated session.
//! The operator confirms a specific target and focus; the server derives opaque
//! session/target/focus bindings and submits them to the denial-by-default
//! control broker as a v2 target attestation. No HID report is accepted here,
//! and the capability stays unavailable until the broker's gates pass.

use std::time::{Duration, Instant};

use axum::{Extension, Json, extract::State};
use bli_kvm_control_focus::{FocusBindings, FocusConfirmation, derive_focus_bindings};
use bli_kvm_control_protocol::{ControlRejectionCode, ControlResponse};
use bli_kvm_core::{HidLeaseRequest, HidReport, PlatformControlRequestV2};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    AppState,
    auth::middleware::CurrentSession,
    error::{ApiResponse, AppError, AppResult},
};

const CHALLENGE_BYTES: usize = 16;
const MAX_CHALLENGES: usize = 32;
const CHALLENGE_TTL: Duration = Duration::from_secs(30);
const ATTESTATION_TTL_SECONDS: u16 = 30;
const OPERATION_ID_LENGTH: usize = 32;

#[derive(Debug)]
struct Challenge {
    nonce: [u8; CHALLENGE_BYTES],
    expires_at: Instant,
}

#[derive(Debug, Default)]
pub(crate) struct FocusChallengeStore {
    entries: Vec<(String, Challenge)>,
    bindings: Vec<(String, FocusBindings, Instant)>,
}

impl FocusChallengeStore {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            bindings: Vec::new(),
        }
    }

    fn remember_bindings(&mut self, session_token: &str, bindings: FocusBindings, now: Instant) {
        self.sweep_bindings(now);
        self.bindings.retain(|(token, _, _)| token != session_token);
        if self.bindings.len() >= MAX_CHALLENGES {
            self.bindings.remove(0);
        }
        self.bindings
            .push((session_token.to_owned(), bindings, now + CHALLENGE_TTL));
    }

    pub(crate) fn bindings(&mut self, session_token: &str, now: Instant) -> Option<FocusBindings> {
        self.sweep_bindings(now);
        self.bindings
            .iter()
            .find(|(token, _, _)| token == session_token)
            .map(|(_, bindings, _)| *bindings)
    }

    fn sweep_bindings(&mut self, now: Instant) {
        self.bindings.retain(|(_, _, expires_at)| now < *expires_at);
    }

    fn issue(&mut self, session_token: &str, now: Instant) -> [u8; CHALLENGE_BYTES] {
        self.sweep(now);
        let mut nonce = [0_u8; CHALLENGE_BYTES];
        OsRng.fill_bytes(&mut nonce);
        self.entries.retain(|(token, _)| token != session_token);
        if self.entries.len() >= MAX_CHALLENGES {
            self.entries.remove(0);
        }
        self.entries.push((
            session_token.to_owned(),
            Challenge {
                nonce,
                expires_at: now + CHALLENGE_TTL,
            },
        ));
        nonce
    }

    fn consume(
        &mut self,
        session_token: &str,
        nonce: &[u8; CHALLENGE_BYTES],
        now: Instant,
    ) -> bool {
        self.sweep(now);
        let found = self.entries.iter().position(|(token, challenge)| {
            token == session_token && constant_time_eq(&challenge.nonce, nonce)
        });
        match found {
            Some(index) => {
                self.entries.remove(index);
                true
            }
            None => false,
        }
    }

    fn sweep(&mut self, now: Instant) {
        self.entries
            .retain(|(_, challenge)| now < challenge.expires_at);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FocusChallengeResponse {
    pub challenge: String,
    pub expires_in_seconds: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FocusConfirmRequest {
    pub operation_id: String,
    pub challenge: String,
    pub target_id: String,
    pub operator_confirmed: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FocusConfirmResponse {
    pub attested: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection: Option<&'static str>,
}

pub(crate) async fn issue_focus_challenge(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<FocusChallengeResponse>>> {
    if state.control.is_none() {
        return Err(AppError::ServiceUnavailable(
            "HID control channel is not configured".into(),
        ));
    }
    let nonce = state
        .focus
        .lock()
        .await
        .issue(&session.token, Instant::now());
    Ok(Json(ApiResponse::ok(FocusChallengeResponse {
        challenge: to_hex(&nonce),
        expires_in_seconds: CHALLENGE_TTL.as_secs(),
    })))
}

pub(crate) async fn confirm_focus(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(request): Json<FocusConfirmRequest>,
) -> AppResult<Json<ApiResponse<FocusConfirmResponse>>> {
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "HID control channel is not configured".into(),
        ));
    };
    if !valid_operation_id(&request.operation_id) {
        return Err(AppError::BadRequest("focus operation ID is invalid".into()));
    }
    let nonce = parse_hex::<CHALLENGE_BYTES>(&request.challenge)
        .ok_or_else(|| AppError::BadRequest("focus challenge is invalid".into()))?;
    let target_id = parse_hex::<16>(&request.target_id)
        .ok_or_else(|| AppError::BadRequest("focus target is invalid".into()))?;
    let now = Instant::now();
    if !state
        .focus
        .lock()
        .await
        .consume(&session.token, &nonce, now)
    {
        return Err(AppError::BadRequest(
            "focus challenge is invalid or expired".into(),
        ));
    }
    let confirmed_at_unix_ms = unix_millis();
    if confirmed_at_unix_ms == 0 {
        return Err(AppError::Internal("focus clock is invalid".into()));
    }
    let confirmation = FocusConfirmation {
        operator_confirmed: request.operator_confirmed,
        session_id: session_identity(&session.token),
        target_id,
        focus_nonce: nonce,
        confirmed_at_unix_ms,
    };
    let bindings = derive_focus_bindings(&confirmation, confirmed_at_unix_ms)
        .map_err(|_| AppError::BadRequest("focus confirmation is invalid".into()))?;
    let response = control
        .execute_v2(
            request.operation_id,
            PlatformControlRequestV2::TargetAttest {
                attestation: bindings.attestation(ATTESTATION_TTL_SECONDS),
            },
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("HID control channel is unavailable".into()))?;
    let attested = match &response {
        ControlResponse::Applied
        | ControlResponse::LeaseGranted { .. }
        | ControlResponse::MediaState { .. } => true,
        ControlResponse::Rejected { .. }
        | ControlResponse::BoardStatus { .. }
        | ControlResponse::VideoState { .. }
        | ControlResponse::MdnsState { .. }
        | ControlResponse::MouseJigglerState { .. }
        | ControlResponse::HidOnlyState { .. } => false,
    };
    if attested {
        state
            .focus
            .lock()
            .await
            .remember_bindings(&session.token, bindings, now);
    }
    Ok(Json(ApiResponse::ok(match response {
        ControlResponse::Applied
        | ControlResponse::LeaseGranted { .. }
        | ControlResponse::MediaState { .. } => FocusConfirmResponse {
            attested: true,
            rejection: None,
        },
        ControlResponse::BoardStatus { .. }
        | ControlResponse::VideoState { .. }
        | ControlResponse::MdnsState { .. }
        | ControlResponse::MouseJigglerState { .. }
        | ControlResponse::HidOnlyState { .. } => FocusConfirmResponse {
            attested: false,
            rejection: Some("internal"),
        },
        ControlResponse::Rejected { code } => FocusConfirmResponse {
            attested: false,
            rejection: Some(rejection_name(code)),
        },
    })))
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum HidLeaseBody {
    Acquire {
        operation_id: String,
        ttl_seconds: u16,
    },
    Renew {
        operation_id: String,
        lease_id: String,
        ttl_seconds: u16,
    },
    Report {
        operation_id: String,
        lease_id: String,
        report: HidReport,
    },
    Release {
        operation_id: String,
        lease_id: String,
    },
}

/// A restricted HID lease that a websocket connection may report through.
///
/// The websocket binary frame carries no lease identity (the reference GUI
/// protocol is a bare report), so the lease is bound to the authenticated
/// session that acquired it over REST and expires on its own TTL.
#[derive(Debug, Clone, Copy)]
pub(crate) struct WsLease {
    lease_id: [u8; 16],
    bindings: FocusBindings,
    expires_at: Instant,
}

#[derive(Debug, Default)]
pub(crate) struct WsLeaseStore {
    entries: Vec<(String, WsLease)>,
}

impl WsLeaseStore {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn remember(
        &mut self,
        session_token: &str,
        lease_id: [u8; 16],
        bindings: FocusBindings,
        expires_at: Instant,
    ) {
        self.entries.retain(|(token, _)| token != session_token);
        self.entries.push((
            session_token.to_owned(),
            WsLease {
                lease_id,
                bindings,
                expires_at,
            },
        ));
    }

    pub fn extend(&mut self, session_token: &str, expires_at: Instant) {
        if let Some((_, lease)) = self
            .entries
            .iter_mut()
            .find(|(token, _)| token == session_token)
        {
            lease.expires_at = expires_at;
        }
    }

    pub fn clear(&mut self, session_token: &str) {
        self.entries.retain(|(token, _)| token != session_token);
    }

    pub fn live(&mut self, session_token: &str, now: Instant) -> Option<([u8; 16], FocusBindings)> {
        self.entries.retain(|(_, lease)| now < lease.expires_at);
        self.entries
            .iter()
            .find(|(token, _)| token == session_token)
            .map(|(_, lease)| (lease.lease_id, lease.bindings))
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HidLeaseResponse {
    pub applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection: Option<&'static str>,
}

pub(crate) async fn hid_lease(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(request): Json<HidLeaseBody>,
) -> AppResult<Json<ApiResponse<HidLeaseResponse>>> {
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "HID control channel is not configured".into(),
        ));
    };
    let operation_id = lease_operation_id(&request);
    if !valid_operation_id(operation_id) {
        return Err(AppError::BadRequest("lease operation ID is invalid".into()));
    }
    let now = Instant::now();
    let bindings = state
        .focus
        .lock()
        .await
        .bindings(&session.token, now)
        .ok_or_else(|| AppError::BadRequest("a current focus confirmation is required".into()))?;
    let lease = build_lease_request(&request, &bindings)?;
    let requested_ttl = lease_ttl(&request);
    let response = control
        .execute_v2(operation_id, PlatformControlRequestV2::HidLease { lease })
        .await
        .map_err(|_| AppError::ServiceUnavailable("HID control channel is unavailable".into()))?;
    let now = Instant::now();
    match &response {
        ControlResponse::LeaseGranted { lease_id } => {
            if let Some(ttl) = requested_ttl {
                state.hid_ws_leases.lock().await.remember(
                    &session.token,
                    *lease_id,
                    bindings,
                    now + Duration::from_secs(u64::from(ttl)),
                );
            }
        }
        ControlResponse::Applied | ControlResponse::MediaState { .. } => {
            if matches!(request, HidLeaseBody::Release { .. }) {
                state.hid_ws_leases.lock().await.clear(&session.token);
            } else if let Some(ttl) = requested_ttl {
                state
                    .hid_ws_leases
                    .lock()
                    .await
                    .extend(&session.token, now + Duration::from_secs(u64::from(ttl)));
            }
        }
        ControlResponse::Rejected { .. }
        | ControlResponse::BoardStatus { .. }
        | ControlResponse::VideoState { .. }
        | ControlResponse::MdnsState { .. }
        | ControlResponse::MouseJigglerState { .. }
        | ControlResponse::HidOnlyState { .. } => {}
    }
    Ok(Json(ApiResponse::ok(match response {
        ControlResponse::Applied => HidLeaseResponse {
            applied: true,
            lease_id: None,
            ttl_seconds: requested_ttl,
            rejection: None,
        },
        ControlResponse::LeaseGranted { lease_id } => HidLeaseResponse {
            applied: true,
            lease_id: Some(to_hex(&lease_id)),
            ttl_seconds: requested_ttl,
            rejection: None,
        },
        ControlResponse::Rejected { code } => HidLeaseResponse {
            applied: false,
            lease_id: None,
            ttl_seconds: None,
            rejection: Some(rejection_name(code)),
        },
        ControlResponse::BoardStatus { .. }
        | ControlResponse::VideoState { .. }
        | ControlResponse::MdnsState { .. }
        | ControlResponse::MouseJigglerState { .. }
        | ControlResponse::HidOnlyState { .. } => HidLeaseResponse {
            applied: false,
            lease_id: None,
            ttl_seconds: None,
            rejection: Some("internal"),
        },
        ControlResponse::MediaState { .. } => HidLeaseResponse {
            applied: false,
            lease_id: None,
            ttl_seconds: None,
            rejection: Some("invalid_state"),
        },
    })))
}

fn lease_ttl(request: &HidLeaseBody) -> Option<u16> {
    match request {
        HidLeaseBody::Acquire { ttl_seconds, .. } | HidLeaseBody::Renew { ttl_seconds, .. } => {
            Some(*ttl_seconds)
        }
        HidLeaseBody::Report { .. } | HidLeaseBody::Release { .. } => None,
    }
}

fn lease_operation_id(request: &HidLeaseBody) -> &str {
    match request {
        HidLeaseBody::Acquire { operation_id, .. }
        | HidLeaseBody::Renew { operation_id, .. }
        | HidLeaseBody::Report { operation_id, .. }
        | HidLeaseBody::Release { operation_id, .. } => operation_id,
    }
}

fn build_lease_request(
    request: &HidLeaseBody,
    bindings: &FocusBindings,
) -> AppResult<HidLeaseRequest> {
    let lease_id = |value: &str| {
        parse_hex::<16>(value).ok_or_else(|| AppError::BadRequest("lease ID is invalid".into()))
    };
    Ok(match request {
        HidLeaseBody::Acquire { ttl_seconds, .. } => HidLeaseRequest::Acquire {
            session_binding: bindings.session_binding,
            target_binding: bindings.target_binding,
            ttl_seconds: *ttl_seconds,
        },
        HidLeaseBody::Renew {
            lease_id: id,
            ttl_seconds,
            ..
        } => HidLeaseRequest::Renew {
            lease_id: lease_id(id)?,
            session_binding: bindings.session_binding,
            target_binding: bindings.target_binding,
            ttl_seconds: *ttl_seconds,
        },
        HidLeaseBody::Report {
            lease_id: id,
            report,
            ..
        } => HidLeaseRequest::Report {
            lease_id: lease_id(id)?,
            session_binding: bindings.session_binding,
            target_binding: bindings.target_binding,
            report: report.clone(),
        },
        HidLeaseBody::Release { lease_id: id, .. } => HidLeaseRequest::Release {
            lease_id: lease_id(id)?,
            session_binding: bindings.session_binding,
            target_binding: bindings.target_binding,
        },
    })
}

pub(crate) fn session_identity(token: &str) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(b"hardened-blikvm-session-binding-v1");
    hasher.update(token.as_bytes());
    let digest = hasher.finalize();
    let identity = u64::from_be_bytes(digest[..8].try_into().expect("SHA-256 is 32 bytes"));
    identity.max(1)
}

pub(crate) fn rejection_name(code: ControlRejectionCode) -> &'static str {
    match code {
        ControlRejectionCode::CapabilityDisabled => "capability_disabled",
        ControlRejectionCode::NotArmed => "not_armed",
        ControlRejectionCode::NotAttested => "not_attested",
        ControlRejectionCode::LeaseConflict => "lease_conflict",
        ControlRejectionCode::LeaseExpired => "lease_expired",
        ControlRejectionCode::CooldownActive => "cooldown_active",
        ControlRejectionCode::ConfirmationStale => "confirmation_stale",
        ControlRejectionCode::Duplicate => "duplicate",
        ControlRejectionCode::PendingUnreconciled => "pending_unreconciled",
        ControlRejectionCode::InvalidState => "invalid_state",
        ControlRejectionCode::BackendUnavailable => "backend_unavailable",
        ControlRejectionCode::BackendRejected => "backend_rejected",
        ControlRejectionCode::Internal => "internal",
    }
}

pub(crate) fn valid_operation_id(value: &str) -> bool {
    value.len() == OPERATION_ID_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn to_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn parse_hex<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2 {
        return None;
    }
    let mut output = [0_u8; N];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(chunk).ok()?;
        output[index] = u8::from_str_radix(text, 16).ok()?;
    }
    Some(output)
}

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_is_one_time_session_bound_and_expiring() {
        let now = Instant::now();
        let mut store = FocusChallengeStore::new();
        let nonce = store.issue("session-a", now);
        assert_eq!(store.len(), 1);

        assert!(!store.consume("session-b", &nonce, now));
        assert!(!store.consume("session-a", &[0; CHALLENGE_BYTES], now));
        assert!(store.consume("session-a", &nonce, now));
        assert!(!store.consume("session-a", &nonce, now));

        let expired = store.issue("session-a", now);
        assert!(!store.consume("session-a", &expired, now + CHALLENGE_TTL));
        store.sweep(now + CHALLENGE_TTL);
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn challenge_store_is_bounded_and_replaces_per_session() {
        let now = Instant::now();
        let mut store = FocusChallengeStore::new();
        for index in 0..(MAX_CHALLENGES + 5) {
            let _ = store.issue(&format!("session-{index}"), now);
        }
        assert_eq!(store.len(), MAX_CHALLENGES);
        let first = store.issue("session-x", now);
        let second = store.issue("session-x", now);
        assert!(!store.consume("session-x", &first, now));
        assert!(store.consume("session-x", &second, now));
    }

    #[test]
    fn session_identity_is_stable_and_nonzero() {
        let identity = session_identity("token-a");
        assert_eq!(identity, session_identity("token-a"));
        assert_ne!(identity, session_identity("token-b"));
        assert_ne!(identity, 0);
    }

    #[test]
    fn hex_helpers_roundtrip_and_reject_wrong_lengths() {
        let bytes = [0x11_u8; 16];
        let text = to_hex(&bytes);
        assert_eq!(text.len(), 32);
        assert_eq!(parse_hex::<16>(&text), Some(bytes));
        assert_eq!(parse_hex::<16>("00"), None);
        assert_eq!(parse_hex::<16>(&"zz".repeat(16)), None);
        assert!(valid_operation_id("0123456789abcdef0123456789abcdef"));
        assert!(!valid_operation_id("0123456789ABCDEF0123456789ABCDEF"));
        assert!(!valid_operation_id("short"));
    }

    #[test]
    fn websocket_leases_expire_are_session_scoped_and_clearable() {
        let now = Instant::now();
        let bindings = FocusBindings {
            session_binding: 7,
            target_binding: [3; 32],
            focus_binding: [4; 16],
        };
        let mut store = WsLeaseStore::new();
        store.remember("token-a", [1; 16], bindings, now + Duration::from_secs(5));
        store.remember("token-b", [2; 16], bindings, now + Duration::from_secs(5));
        assert_eq!(store.live("token-a", now), Some(([1; 16], bindings)));
        assert_eq!(store.live("missing", now), None);
        store.extend("token-b", now + Duration::from_secs(30));
        assert_eq!(
            store.live("token-b", now + Duration::from_secs(10)),
            Some(([2; 16], bindings))
        );
        assert_eq!(
            store.live("token-a", now + Duration::from_secs(6)),
            None,
            "an expired lease must not be usable"
        );
        store.remember("token-a", [9; 16], bindings, now + Duration::from_secs(5));
        assert_eq!(store.live("token-a", now), Some(([9; 16], bindings)));
        store.clear("token-a");
        assert_eq!(store.live("token-a", now), None);
    }
}
