use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, Method, Request, Uri, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::{
    AppState,
    auth::{cookie::SESSION_COOKIE_NAME, session::Session},
    error::AppError,
};

#[derive(Debug, Clone)]
pub struct CurrentSession(pub Session);

pub async fn protected(
    State(state): State<AppState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let Some(token) = session_cookie(request.headers()) else {
        return AppError::Unauthorized.into_response();
    };
    let Some(session) = state.sessions.validate(&token).await else {
        return AppError::Unauthorized.into_response();
    };

    if is_state_changing(request.method()) {
        let csrf = request
            .headers()
            .get("x-csrf-token")
            .and_then(|value| value.to_str().ok());
        if csrf != Some(session.csrf_token.as_str()) {
            return AppError::Forbidden("missing or invalid CSRF token".into()).into_response();
        }
        if !valid_request_origin(request.headers(), state.options.allow_missing_origin) {
            return AppError::Forbidden("invalid request origin".into()).into_response();
        }
    }

    request.extensions_mut().insert(CurrentSession(session));
    next.run(request).await
}

pub fn require_request_origin(headers: &HeaderMap, allow_missing: bool) -> Result<(), AppError> {
    if valid_request_origin(headers, allow_missing) {
        Ok(())
    } else {
        Err(AppError::Forbidden("invalid request origin".into()))
    }
}

fn is_state_changing(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

fn valid_request_origin(headers: &HeaderMap, allow_missing: bool) -> bool {
    if let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        return same_host_origin(origin, headers);
    }
    if let Some(referer) = headers
        .get(header::REFERER)
        .and_then(|value| value.to_str().ok())
    {
        return normalize_origin(referer).is_some_and(|origin| same_host_origin(&origin, headers));
    }
    allow_missing
}

fn same_host_origin(origin: &str, headers: &HeaderMap) -> bool {
    let Some(origin) = normalize_origin(origin) else {
        return false;
    };
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    origin == format!("http://{host}") || origin == format!("https://{host}")
}

fn normalize_origin(value: &str) -> Option<String> {
    let uri = value.parse::<Uri>().ok()?;
    let scheme = uri.scheme_str()?;
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    let authority = uri.authority()?.as_str();
    Some(format!("{scheme}://{authority}"))
}

fn session_cookie(headers: &HeaderMap) -> Option<String> {
    let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
    cookie.split(';').find_map(|item| {
        item.trim()
            .strip_prefix(&format!("{SESSION_COOKIE_NAME}="))
            .filter(|token| !token.is_empty())
            .map(str::to_owned)
    })
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn same_origin_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("kvm.test"));
        headers.insert(header::ORIGIN, HeaderValue::from_static("https://kvm.test"));
        headers
    }

    #[test]
    fn same_host_origin_is_allowed() {
        assert!(valid_request_origin(&same_origin_headers(), false));
    }

    #[test]
    fn cross_host_and_missing_origins_are_rejected() {
        let mut headers = same_origin_headers();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.test"),
        );
        assert!(!valid_request_origin(&headers, false));
        assert!(!valid_request_origin(&HeaderMap::new(), false));
        assert!(valid_request_origin(&HeaderMap::new(), true));
    }

    #[test]
    fn extracts_only_the_named_nonempty_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("other=x; hardened-blikvm-session=abc; ignored=y"),
        );
        assert_eq!(session_cookie(&headers).as_deref(), Some("abc"));
    }
}
