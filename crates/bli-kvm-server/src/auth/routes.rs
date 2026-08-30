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

    let source = address.ip().to_string();
    let mut limiter = state.login_limiter.lock().await;
    if limiter.is_locked(&source, &request.username) {
        return Err(AppError::RateLimited(
            "account locked after repeated failures".into(),
        ));
    }
    let verified = state
        .accounts
        .verify(&request.username, &request.password)?;
    if !verified {
        let _locked = limiter.record_failure(&source, &request.username);
        return Err(AppError::InvalidCredentials);
    }
    limiter.record_success(&source, &request.username);
    drop(limiter);

    let session = state
        .sessions
        .issue(&request.username, state.options.session_ttl_secs)
        .await;
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        header::SET_COOKIE,
        session_cookie(
            &session.token,
            state.options.session_ttl_secs,
            state.options.secure_cookies,
        )?,
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
    state
        .accounts
        .replace(&request.username, &request.password)?;
    state.sessions.revoke_user(&request.username).await;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::SET_COOKIE,
        expired_session_cookie(state.options.secure_cookies)?,
    );
    Ok((headers, Json(ApiResponse::ok_empty())))
}
