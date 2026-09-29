use std::net::SocketAddr;

use axum::{
    Extension, Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, header},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    auth::{
        cookie::{expired_session_cookie, session_cookie},
        middleware::{CurrentSession, require_request_origin},
        password::validate_credentials,
    },
    error::{ApiResponse, AppError, AppResult},
};

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct AccountChangeRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginResponse {
    pub csrf_token: String,
    pub expires_at: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountResponse {
    pub username: String,
    pub csrf_token: String,
    pub expires_at: u64,
}

#[derive(Debug, Serialize)]
pub struct SetupStateResponse {
    pub required: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordStateResponse {
    pub is_updated: bool,
}

/// As in the reference, the root password follows the web account's: the
/// first setup and every password change set it through the administration
/// helper before the account is stored, and nothing is stored if it fails.
/// A server without the helper (development and tests) keeps the account
/// alone.
async fn set_root_password(state: &AppState, password: &str) -> AppResult<()> {
    let Some(socket) = state.options.admin_socket.as_ref() else {
        return Ok(());
    };
    let request = bli_kvm_admin_helper::AdminRequest::RootPassword {
        password: password.to_string(),
    };
    match crate::scripts::call_with_timeout(
        socket,
        &request,
        None,
        std::time::Duration::from_secs(15),
    )
    .await?
    {
        bli_kvm_admin_helper::AdminReply::Done => Ok(()),
        _ => Err(AppError::Internal(
            "the root password could not be changed".into(),
        )),
    }
}

pub async fn setup_state(State(state): State<AppState>) -> Json<ApiResponse<SetupStateResponse>> {
    Json(ApiResponse::ok(SetupStateResponse {
        required: !state.accounts.exists(),
    }))
}

pub async fn setup_first_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<AccountChangeRequest>,
) -> AppResult<Json<ApiResponse<()>>> {
    require_request_origin(&headers, state.options.allow_missing_origin)?;
    validate_credentials(&request.username, &request.password)?;
    let _guard = state.setup_lock.lock().await;
    if state.accounts.exists() {
        return Err(AppError::Conflict("account already initialized".into()));
    }
    set_root_password(&state, &request.password).await?;
    state
        .accounts
        .create_initial(&request.username, &request.password)?;
    Ok(Json(ApiResponse::ok_empty()))
}

pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<LoginRequest>,
) -> AppResult<impl IntoResponse> {
    require_request_origin(&headers, state.options.allow_missing_origin)?;
    if !state.accounts.exists() {
        return Err(AppError::Conflict("first-account setup required".into()));
    }

    // Bounds as First Setup enforces them; refused before the limiter or the
    // hash see the request, so junk names cost nothing and are not remembered.
    if request.username.len() > 64 || request.password.len() > 256 {
        return Err(AppError::BadRequest("invalid username or password".into()));
    }
    let source = crate::auth::rate_limit::client_address(address, &headers);
    let locked = || {
        AppError::RateLimited("account locked after repeated failures".into())
    };
    if state
        .login_limiter
        .lock()
        .await
        .is_locked(&source, &request.username)
    {
        return Err(locked());
    }
    // The hash runs on the blocking pool, a few at a time and outside the
    // limiter's mutex; the lockout is checked again once a place is free, so
    // queued guesses cannot outrun it.
    let permit = crate::auth::password::verification_permit().await;
    if state
        .login_limiter
        .lock()
        .await
        .is_locked(&source, &request.username)
    {
        return Err(locked());
    }
    let verified = crate::auth::password::verify_blocking(
        state.accounts.clone(),
        request.username.clone(),
        request.password.clone(),
    )
    .await?;
    drop(permit);
    let mut limiter = state.login_limiter.lock().await;
    if !verified {
        let _locked = limiter.record_failure(&source, &request.username);
        return Err(AppError::InvalidCredentials);
    }
    limiter.record_success(&source, &request.username);
    drop(limiter);

    // The session lives as long as Session lock after says, as in the
    // reference; the server's default applies while the lock is off.
    let lifetime = state
        .sessions
        .lifetime_seconds(state.options.session_ttl_secs)
        .await;
    let session = state.sessions.issue(&request.username, lifetime).await;
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::SET_COOKIE,
        session_cookie(&session.token, lifetime, state.options.secure_cookies)?,
    );
    Ok((
        response_headers,
        Json(ApiResponse::ok(LoginResponse {
            csrf_token: session.csrf_token,
            expires_at: session.expires_at_unix,
        })),
    ))
}

pub async fn account(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<AccountResponse>>> {
    let account = state
        .accounts
        .load()?
        .ok_or_else(|| AppError::NotFound("account not initialized".into()))?;
    if account.username != session.username {
        return Err(AppError::Forbidden("session/account mismatch".into()));
    }
    Ok(Json(ApiResponse::ok(AccountResponse {
        username: account.username,
        csrf_token: session.csrf_token,
        expires_at: session.expires_at_unix,
    })))
}

pub async fn logout(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<impl IntoResponse> {
    state.sessions.revoke(&session.token).await;
    crate::hid::release_session_hid(&state, &session.token).await;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        expired_session_cookie(state.options.secure_cookies)?,
    );
    Ok((headers, Json(ApiResponse::ok_empty())))
}

pub async fn password_state(
    State(state): State<AppState>,
) -> Json<ApiResponse<PasswordStateResponse>> {
    Json(ApiResponse::ok(PasswordStateResponse {
        is_updated: state.accounts.exists(),
    }))
}

pub async fn change_password(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(request): Json<AccountChangeRequest>,
) -> AppResult<impl IntoResponse> {
    if request.username != session.username {
        return Err(AppError::Forbidden(
            "cannot change another account password".into(),
        ));
    }
    validate_credentials(&request.username, &request.password)?;
    set_root_password(&state, &request.password).await?;
    state
        .accounts
        .replace(&request.username, &request.password)?;
    let revoked = state.sessions.revoke_user(&request.username).await;
    for token in revoked {
        crate::hid::release_session_hid(&state, &token).await;
    }
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        expired_session_cookie(state.options.secure_cookies)?,
    );
    Ok((headers, Json(ApiResponse::ok_empty())))
}
