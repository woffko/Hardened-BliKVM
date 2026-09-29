use std::{
    fs::{self, File},
    io::Write,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use serde::{Deserialize, Serialize};
use tempfile::Builder;

use crate::error::{AppError, AppResult};

// Argon2 verifications in this thread, for the equal-cost test.
#[cfg(test)]
thread_local! {
    static VERIFICATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A hash with the parameters of the account's own, of a password nobody knows.
fn dummy_hash() -> &'static str {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DUMMY.get_or_init(|| {
        hash_password(&crate::auth::token::random_token(32))
            .unwrap_or_else(|_| String::new())
    })
}

/// At most this many hashes run at once: each takes memory and a core.
const CONCURRENT_VERIFICATIONS: usize = 2;

/// Runs [`AccountStore::verify`] on the blocking pool.
pub async fn verify_blocking(
    store: std::sync::Arc<AccountStore>,
    username: String,
    password: String,
) -> AppResult<bool> {
    tokio::task::spawn_blocking(move || store.verify(&username, &password))
        .await
        .map_err(|_| AppError::Internal("password verification failed".into()))?
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub username: String,
    pub password_hash: String,
}

#[derive(Debug)]
pub struct AccountStore {
    path: PathBuf,
    permits: tokio::sync::Semaphore,
}

impl AccountStore {
    pub fn new(path: PathBuf) -> Self {
        // Build the dummy hash now, so the first wrong name costs no more.
        #[cfg(not(test))]
        std::thread::spawn(|| {
            let _ = dummy_hash();
        });
        Self {
            path,
            permits: tokio::sync::Semaphore::new(CONCURRENT_VERIFICATIONS),
        }
    }

    /// Waits for a place to verify a password.
    pub async fn verification_permit(&self) -> tokio::sync::SemaphorePermit<'_> {
        self.permits
            .acquire()
            .await
            .expect("the verification semaphore is never closed")
    }

    pub fn exists(&self) -> bool {
        self.path.symlink_metadata().is_ok()
    }

    pub fn load(&self) -> AppResult<Option<Account>> {
        let metadata = match self.path.symlink_metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(AppError::Config(
                "account path is not a regular file".into(),
            ));
        }
        if metadata.mode() & 0o077 != 0 {
            return Err(AppError::Config(
                "account file permissions are broader than 0600".into(),
            ));
        }
        let account = serde_json::from_slice::<Account>(&fs::read(&self.path)?)
            .map_err(|_| AppError::Config("invalid account file".into()))?;
        Ok(Some(account))
    }

    pub fn create_initial(&self, username: &str, password: &str) -> AppResult<()> {
        if self.exists() {
            return Err(AppError::Conflict("account already initialized".into()));
        }
        validate_credentials(username, password)?;
        let account = Account {
            username: username.into(),
            password_hash: hash_password(password)?,
        };
        self.write_account(&account, true)
    }

    pub fn replace(&self, username: &str, password: &str) -> AppResult<()> {
        validate_credentials(username, password)?;
        if self.load()?.is_none() {
            return Err(AppError::NotFound("account not initialized".into()));
        }
        let account = Account {
            username: username.into(),
            password_hash: hash_password(password)?,
        };
        self.write_account(&account, false)
    }

    /// Checks a login. An unknown or absent account is verified against a
    /// dummy hash of the same cost, so the time does not tell a right user name
    /// from a wrong one.
    pub fn verify(&self, username: &str, password: &str) -> AppResult<bool> {
        let account = self.load()?.filter(|account| account.username == username);
        let stored = match &account {
            Some(account) => account.password_hash.as_str(),
            None => dummy_hash(),
        };
        let parsed = PasswordHash::new(stored)
            .map_err(|_| AppError::Config("invalid account password hash".into()))?;
        #[cfg(test)]
        VERIFICATIONS.with(|count| count.set(count.get() + 1));
        let matched = Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok();
        Ok(matched && account.is_some())
    }

    fn write_account(&self, account: &Account, no_clobber: bool) -> AppResult<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| AppError::Config("account path has no parent".into()))?;
        prepare_private_directory(parent)?;
        let data = serde_json::to_vec(account)
            .map_err(|_| AppError::Internal("account serialization failed".into()))?;
        let mut temporary = Builder::new().prefix(".account-").tempfile_in(parent)?;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
        temporary.write_all(&data)?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        if no_clobber {
            temporary.persist_noclobber(&self.path).map_err(|error| {
                if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                    AppError::Conflict("account already initialized".into())
                } else {
                    AppError::Io(error.error)
                }
            })?;
        } else {
            temporary
                .persist(&self.path)
                .map_err(|error| AppError::Io(error.error))?;
        }
        fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))?;
        File::open(parent)?.sync_all()?;
        Ok(())
    }
}

pub fn validate_credentials(username: &str, password: &str) -> AppResult<()> {
    if username.is_empty()
        || username.len() > 64
        || !username
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-@".contains(character))
    {
        return Err(AppError::BadRequest("invalid username".into()));
    }
    if !(8..=256).contains(&password.len()) {
        return Err(AppError::BadRequest(
            "password must be between 8 and 256 bytes".into(),
        ));
    }
    // Root's password follows it, and chpasswd reads one line.
    if password
        .bytes()
        .any(|byte| matches!(byte, b'\n' | b'\r' | 0))
    {
        return Err(AppError::BadRequest(
            "password must not contain line breaks".into(),
        ));
    }
    Ok(())
}

fn hash_password(password: &str) -> AppResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| AppError::Internal("password hashing failed".into()))
}

fn prepare_private_directory(path: &Path) -> AppResult<()> {
    fs::create_dir_all(path)?;
    let metadata = path.symlink_metadata()?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(AppError::Config(
            "account directory is not a directory".into(),
        ));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_wrong_user_name_costs_one_verification_like_a_right_one() {
        let dir = tempfile::tempdir().unwrap();
        let store = AccountStore::new(dir.path().join("account.json"));
        store.create_initial("operator", "initial password").unwrap();
        let count = || VERIFICATIONS.with(|count| count.get());
        let before = count();
        assert!(store.verify("operator", "initial password").unwrap());
        assert_eq!(count() - before, 1);
        assert!(!store.verify("someone-else", "initial password").unwrap());
        assert_eq!(count() - before, 2);
        assert!(!store.verify("operator", "wrong password").unwrap());
        assert_eq!(count() - before, 3);
    }

    use super::*;

    #[test]
    fn initial_account_is_argon2_and_private() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state/account.json");
        let store = AccountStore::new(path.clone());
        store
            .create_initial("operator", "correct horse battery staple")
            .unwrap();

        let account = store.load().unwrap().unwrap();
        assert_eq!(account.username, "operator");
        assert!(account.password_hash.starts_with("$argon2"));
        assert!(
            store
                .verify("operator", "correct horse battery staple")
                .unwrap()
        );
        assert!(!store.verify("operator", "wrong password").unwrap());
        assert_eq!(path.metadata().unwrap().mode() & 0o777, 0o600);
        assert!(
            !String::from_utf8_lossy(&fs::read(path).unwrap())
                .contains("correct horse battery staple")
        );
    }

    #[test]
    fn initial_account_cannot_be_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let store = AccountStore::new(directory.path().join("account.json"));
        store.create_initial("first", "first password").unwrap();
        assert!(matches!(
            store.create_initial("second", "second password"),
            Err(AppError::Conflict(_))
        ));
        assert!(store.verify("first", "first password").unwrap());
    }

    #[test]
    fn broad_account_permissions_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("account.json");
        let store = AccountStore::new(path.clone());
        store
            .create_initial("operator", "initial password")
            .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(store.load(), Err(AppError::Config(_))));
    }
}
