use axum::http::HeaderValue;

use crate::error::{AppError, AppResult};

pub const SESSION_COOKIE_NAME: &str = "hardened-blikvm-session";

pub fn session_cookie(token: &str, max_age_secs: u64, secure: bool) -> AppResult<HeaderValue> {
    build_cookie(Some(token), max_age_secs, secure)
}

pub fn expired_session_cookie(secure: bool) -> AppResult<HeaderValue> {
    build_cookie(None, 0, secure)
}

fn build_cookie(token: Option<&str>, max_age_secs: u64, secure: bool) -> AppResult<HeaderValue> {
    let mut value = format!(
        "{SESSION_COOKIE_NAME}={}; Path=/; Max-Age={max_age_secs}; HttpOnly; SameSite=Strict",
        token.unwrap_or_default()
    );
    if secure {
        value.push_str("; Secure");
    }
    HeaderValue::from_str(&value)
        .map_err(|_| AppError::Internal("failed to build session cookie".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_is_httponly_and_strict() {
        let value = session_cookie("token", 900, true).unwrap();
        let value = value.to_str().unwrap();
        assert!(value.contains("HttpOnly"));
        assert!(value.contains("SameSite=Strict"));
        assert!(value.contains("Secure"));
    }
}
