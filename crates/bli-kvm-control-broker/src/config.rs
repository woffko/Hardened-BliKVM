use std::{
    collections::BTreeSet, env, error::Error, ffi::OsString, fmt, path::PathBuf, time::Duration,
};

const DEFAULT_CONTROL_SOCKET: &str = "/run/hardened-blikvm-control/control.sock";

/// The account besides root that may ask the owner anything: the web
/// server, which brokers the operator's requests.
const DEFAULT_CONTROL_PEER_USER: &str = "hardened-blikvm-server";

#[derive(Debug, Clone)]
pub struct ControlBrokerConfig {
    pub socket_path: PathBuf,
    pub request_timeout: Duration,
    pub max_connections: usize,
    /// The peers' user ids the socket serves; anyone else in the socket's
    /// group is closed unanswered (security review of 2026-10-18, MED-01).
    pub admitted_uids: BTreeSet<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlBrokerConfigError {
    InvalidSocketPath,
    InvalidTimeout,
    InvalidConnectionLimit,
}

impl fmt::Display for ControlBrokerConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSocketPath => "control broker socket path must be absolute",
            Self::InvalidTimeout => "control broker timeout must be greater than zero",
            Self::InvalidConnectionLimit => {
                "control broker connection limit must be between 1 and 16"
            }
        })
    }
}

impl Error for ControlBrokerConfigError {}

impl ControlBrokerConfig {
    /// The owner's own configuration: root (the owner's own user) and the
    /// web server's account are admitted. A name that cannot be resolved
    /// leaves root alone, which the log says.
    pub fn from_environment() -> Result<Self, ControlBrokerConfigError> {
        let mut config = Self::new(
            env::var_os("BLI_KVM_CONTROL_SOCKET")
                .unwrap_or_else(|| OsString::from(DEFAULT_CONTROL_SOCKET)),
        )?;
        let user = env::var("BLI_KVM_CONTROL_PEER_USER")
            .unwrap_or_else(|_| DEFAULT_CONTROL_PEER_USER.to_string());
        let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
        let named = admitted_uids(&passwd, &user);
        if named.len() == 1 {
            eprintln!("control: the user {user} is unknown; only root is served");
        }
        // `new` admitted root and the process's own user already.
        config.admitted_uids.extend(named);
        Ok(config)
    }

    pub fn new(socket_path: impl Into<PathBuf>) -> Result<Self, ControlBrokerConfigError> {
        // Root, and the process's own user (the same where the owner runs
        // as root; a test's own user otherwise).
        // SAFETY: geteuid has no preconditions.
        let own = unsafe { libc::geteuid() };
        let config = Self {
            socket_path: socket_path.into(),
            request_timeout: Duration::from_secs(3),
            max_connections: 4,
            admitted_uids: BTreeSet::from([0, own]),
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ControlBrokerConfigError> {
        if !self.socket_path.is_absolute() {
            return Err(ControlBrokerConfigError::InvalidSocketPath);
        }
        if self.request_timeout.is_zero() {
            return Err(ControlBrokerConfigError::InvalidTimeout);
        }
        if !(1..=16).contains(&self.max_connections) {
            return Err(ControlBrokerConfigError::InvalidConnectionLimit);
        }
        Ok(())
    }
}

/// Root and `user`'s id in an `/etc/passwd` style file, when it is there.
pub fn admitted_uids(passwd: &str, user: &str) -> BTreeSet<u32> {
    let mut admitted = BTreeSet::from([0]);
    admitted.extend(passwd.lines().find_map(|line| {
        let mut fields = line.split(':');
        (fields.next()? == user).then_some(())?;
        fields.nth(1)?.parse::<u32>().ok()
    }));
    admitted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_and_the_named_account_are_admitted() {
        let passwd = "root:x:0:0::/root:/bin/sh\nhardened-blikvm-server:x:996:993::/:/usr/sbin/nologin\nhardened-blikvm-video:x:995:993::/:/usr/sbin/nologin\n";
        assert_eq!(
            admitted_uids(passwd, "hardened-blikvm-server"),
            BTreeSet::from([0, 996])
        );
        assert_eq!(admitted_uids(passwd, "absent"), BTreeSet::from([0]));
        assert_eq!(
            admitted_uids("", "hardened-blikvm-server"),
            BTreeSet::from([0])
        );
    }

    #[test]
    fn rejects_relative_unbounded_or_zero_configuration() {
        assert!(matches!(
            ControlBrokerConfig::new("control.sock"),
            Err(ControlBrokerConfigError::InvalidSocketPath)
        ));
        let mut config = ControlBrokerConfig::new("/run/control.sock").unwrap();
        config.max_connections = 17;
        assert_eq!(
            config.validate(),
            Err(ControlBrokerConfigError::InvalidConnectionLimit)
        );
        config.max_connections = 1;
        config.request_timeout = Duration::ZERO;
        assert_eq!(
            config.validate(),
            Err(ControlBrokerConfigError::InvalidTimeout)
        );
    }
}
