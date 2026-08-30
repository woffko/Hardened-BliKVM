use http::{HeaderMap, HeaderName, HeaderValue};
use thiserror::Error;

const MAX_CREDENTIAL_BYTES: usize = 256;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CredentialError {
    #[error("KVMD username must be between 1 and 256 bytes")]
    InvalidUsernameLength,
    #[error("KVMD password must be between 1 and 256 bytes")]
    InvalidPasswordLength,
    #[error("KVMD credential is not a valid HTTP header value")]
    InvalidHeaderValue,
}

/// In-memory KVMD credential with deliberately restricted observability.
///
/// The type implements neither `Debug` nor `Clone`. Its backing buffers are
/// overwritten on drop. Production construction belongs in the broker's
/// systemd-credential loader; test callers may construct a fake credential.
pub struct KvmdCredentials {
    username: Box<[u8]>,
    password: Box<[u8]>,
}

impl KvmdCredentials {
    pub fn new(
        username: impl AsRef<[u8]>,
        password: impl AsRef<[u8]>,
    ) -> Result<Self, CredentialError> {
        let username = username.as_ref();
        let password = password.as_ref();
        if !(1..=MAX_CREDENTIAL_BYTES).contains(&username.len()) {
            return Err(CredentialError::InvalidUsernameLength);
        }
        if !(1..=MAX_CREDENTIAL_BYTES).contains(&password.len()) {
            return Err(CredentialError::InvalidPasswordLength);
        }
        HeaderValue::from_bytes(username).map_err(|_| CredentialError::InvalidHeaderValue)?;
        HeaderValue::from_bytes(password).map_err(|_| CredentialError::InvalidHeaderValue)?;
        Ok(Self {
            username: username.into(),
            password: password.into(),
        })
    }

    pub fn apply_request_headers(&self, headers: &mut HeaderMap) {
        headers.insert(
            HeaderName::from_static("x-kvmd-user"),
            self.username_header(),
        );
        headers.insert(
            HeaderName::from_static("x-kvmd-passwd"),
            self.password_header(),
        );
    }

    fn username_header(&self) -> HeaderValue {
        HeaderValue::from_bytes(&self.username).expect("credential was validated at construction")
    }

    fn password_header(&self) -> HeaderValue {
        HeaderValue::from_bytes(&self.password).expect("credential was validated at construction")
    }
}

impl Drop for KvmdCredentials {
    fn drop(&mut self) {
        self.username.fill(0);
        self.password.fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_and_header_injection_values() {
        assert_eq!(
            KvmdCredentials::new([], b"password").err(),
            Some(CredentialError::InvalidUsernameLength)
        );
        assert_eq!(
            KvmdCredentials::new(b"operator", []).err(),
            Some(CredentialError::InvalidPasswordLength)
        );
        assert_eq!(
            KvmdCredentials::new(b"operator\r\ninjected", b"password").err(),
            Some(CredentialError::InvalidHeaderValue)
        );
        assert_eq!(
            KvmdCredentials::new(b"operator", b"password\nnext").err(),
            Some(CredentialError::InvalidHeaderValue)
        );
    }
}
