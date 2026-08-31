#![forbid(unsafe_code)]

//! Safe fixed-name account resolution and connection-bound peer mapping.
//!
//! This crate has no listener, path, configuration, environment, logging,
//! persistence, or collector runtime. It resolves seven compile-time names,
//! validates the current collector UID, reads credentials only through Tokio's
//! safe `UnixStream::peer_cred`, maps them once, and returns only the source.

use bli_kvm_diagnostic_peer_policy::{
    DiagnosticAccount, DiagnosticPeerAssignments, DiagnosticPeerConfigError, DiagnosticPeerError,
    DiagnosticSource, KernelPeerCredentials,
};
use nix::unistd::{Group, User, geteuid};
use tokio::net::UnixStream;

pub const DIAGNOSTIC_COLLECTOR_ACCOUNT: &str = "hardened-blikvm-diagnostics";
pub const DIAGNOSTIC_SERVER_ACCOUNT: &str = "hardened-blikvm-server";
pub const DIAGNOSTIC_PLATFORM_BROKER_ACCOUNT: &str = "hardened-blikvm-broker";
pub const DIAGNOSTIC_VIDEO_PROXY_ACCOUNT: &str = "hardened-blikvm-video";
pub const DIAGNOSTIC_CONTROL_BROKER_ACCOUNT: &str = "hardened-blikvm-control";
pub const DIAGNOSTIC_UPDATE_SUPERVISOR_ACCOUNT: &str = "hardened-blikvm-update";
pub const DIAGNOSTIC_SHARED_GROUP: &str = "hardened-blikvm";

const ACCOUNTS: [DiagnosticAccount; 6] = [
    DiagnosticAccount::Collector,
    DiagnosticAccount::Server,
    DiagnosticAccount::PlatformBroker,
    DiagnosticAccount::VideoProxy,
    DiagnosticAccount::ControlBroker,
    DiagnosticAccount::UpdateSupervisor,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticPeerAdapterError {
    UserLookupFailed(DiagnosticAccount),
    UserMissing(DiagnosticAccount),
    UserNameMismatch(DiagnosticAccount),
    GroupLookupFailed,
    GroupMissing,
    GroupNameMismatch,
    WrongCollectorUid,
    InvalidAssignments(DiagnosticPeerConfigError),
    PeerCredentialReadFailed,
    PeerRejected(DiagnosticPeerError),
}

pub fn resolve_current_assignments() -> Result<DiagnosticPeerAssignments, DiagnosticPeerAdapterError>
{
    resolve_assignments_with(&NixAccountLookup, geteuid().as_raw())
}

pub fn map_tokio_peer(
    assignments: &DiagnosticPeerAssignments,
    stream: &UnixStream,
) -> Result<DiagnosticSource, DiagnosticPeerAdapterError> {
    let credentials = stream
        .peer_cred()
        .map_err(|_| DiagnosticPeerAdapterError::PeerCredentialReadFailed)?;
    map_credential_values(
        assignments,
        credentials.pid(),
        credentials.uid(),
        credentials.gid(),
    )
}

fn map_credential_values(
    assignments: &DiagnosticPeerAssignments,
    pid: Option<i32>,
    uid: u32,
    gid: u32,
) -> Result<DiagnosticSource, DiagnosticPeerAdapterError> {
    let pid = pid.map(|pid| u32::try_from(pid).unwrap_or(0));
    assignments
        .map_peer(KernelPeerCredentials::new(pid, uid, gid))
        .map_err(DiagnosticPeerAdapterError::PeerRejected)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResolvedUser {
    exact_name: bool,
    uid: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResolvedGroup {
    exact_name: bool,
    gid: u32,
}

trait AccountLookup {
    fn user(&self, account: DiagnosticAccount) -> Result<Option<ResolvedUser>, ()>;
    fn shared_group(&self) -> Result<Option<ResolvedGroup>, ()>;
}

struct NixAccountLookup;

impl AccountLookup for NixAccountLookup {
    fn user(&self, account: DiagnosticAccount) -> Result<Option<ResolvedUser>, ()> {
        let expected = account_name(account);
        User::from_name(expected).map_err(|_| ()).map(|user| {
            user.map(|user| ResolvedUser {
                exact_name: user.name == expected,
                uid: user.uid.as_raw(),
            })
        })
    }

    fn shared_group(&self) -> Result<Option<ResolvedGroup>, ()> {
        Group::from_name(DIAGNOSTIC_SHARED_GROUP)
            .map_err(|_| ())
            .map(|group| {
                group.map(|group| ResolvedGroup {
                    exact_name: group.name == DIAGNOSTIC_SHARED_GROUP,
                    gid: group.gid.as_raw(),
                })
            })
    }
}

fn resolve_assignments_with(
    lookup: &impl AccountLookup,
    current_euid: u32,
) -> Result<DiagnosticPeerAssignments, DiagnosticPeerAdapterError> {
    let mut uids = [0_u32; 6];
    for (index, account) in ACCOUNTS.into_iter().enumerate() {
        let user = lookup
            .user(account)
            .map_err(|_| DiagnosticPeerAdapterError::UserLookupFailed(account))?
            .ok_or(DiagnosticPeerAdapterError::UserMissing(account))?;
        if !user.exact_name {
            return Err(DiagnosticPeerAdapterError::UserNameMismatch(account));
        }
        uids[index] = user.uid;
    }
    let group = lookup
        .shared_group()
        .map_err(|_| DiagnosticPeerAdapterError::GroupLookupFailed)?
        .ok_or(DiagnosticPeerAdapterError::GroupMissing)?;
    if !group.exact_name {
        return Err(DiagnosticPeerAdapterError::GroupNameMismatch);
    }
    if current_euid != uids[0] {
        return Err(DiagnosticPeerAdapterError::WrongCollectorUid);
    }
    DiagnosticPeerAssignments::new(
        uids[0], uids[1], uids[2], uids[3], uids[4], uids[5], group.gid,
    )
    .map_err(DiagnosticPeerAdapterError::InvalidAssignments)
}

const fn account_name(account: DiagnosticAccount) -> &'static str {
    match account {
        DiagnosticAccount::Collector => DIAGNOSTIC_COLLECTOR_ACCOUNT,
        DiagnosticAccount::Server => DIAGNOSTIC_SERVER_ACCOUNT,
        DiagnosticAccount::PlatformBroker => DIAGNOSTIC_PLATFORM_BROKER_ACCOUNT,
        DiagnosticAccount::VideoProxy => DIAGNOSTIC_VIDEO_PROXY_ACCOUNT,
        DiagnosticAccount::ControlBroker => DIAGNOSTIC_CONTROL_BROKER_ACCOUNT,
        DiagnosticAccount::UpdateSupervisor => DIAGNOSTIC_UPDATE_SUPERVISOR_ACCOUNT,
    }
}

#[cfg(test)]
mod tests {
    use nix::unistd::{getegid, geteuid};

    use super::*;

    const UIDS: [u32; 6] = [100, 101, 102, 103, 104, 105];
    const SHARED_GID: u32 = 200;

    #[derive(Clone)]
    struct MockLookup {
        users: [Option<ResolvedUser>; 6],
        group: Option<ResolvedGroup>,
        failed_user: Option<DiagnosticAccount>,
        failed_group: bool,
    }

    impl MockLookup {
        fn valid() -> Self {
            Self {
                users: [
                    Some(ResolvedUser {
                        exact_name: true,
                        uid: UIDS[0],
                    }),
                    Some(ResolvedUser {
                        exact_name: true,
                        uid: UIDS[1],
                    }),
                    Some(ResolvedUser {
                        exact_name: true,
                        uid: UIDS[2],
                    }),
                    Some(ResolvedUser {
                        exact_name: true,
                        uid: UIDS[3],
                    }),
                    Some(ResolvedUser {
                        exact_name: true,
                        uid: UIDS[4],
                    }),
                    Some(ResolvedUser {
                        exact_name: true,
                        uid: UIDS[5],
                    }),
                ],
                group: Some(ResolvedGroup {
                    exact_name: true,
                    gid: SHARED_GID,
                }),
                failed_user: None,
                failed_group: false,
            }
        }
    }

    impl AccountLookup for MockLookup {
        fn user(&self, account: DiagnosticAccount) -> Result<Option<ResolvedUser>, ()> {
            if self.failed_user == Some(account) {
                Err(())
            } else {
                Ok(self.users[account_index(account)])
            }
        }

        fn shared_group(&self) -> Result<Option<ResolvedGroup>, ()> {
            if self.failed_group {
                Err(())
            } else {
                Ok(self.group)
            }
        }
    }

    #[test]
    fn fixed_names_and_valid_resolution_are_exact() {
        assert_eq!(
            account_name(DiagnosticAccount::Collector),
            DIAGNOSTIC_COLLECTOR_ACCOUNT
        );
        assert_eq!(
            account_name(DiagnosticAccount::Server),
            DIAGNOSTIC_SERVER_ACCOUNT
        );
        assert_eq!(
            account_name(DiagnosticAccount::PlatformBroker),
            DIAGNOSTIC_PLATFORM_BROKER_ACCOUNT
        );
        assert_eq!(
            account_name(DiagnosticAccount::VideoProxy),
            DIAGNOSTIC_VIDEO_PROXY_ACCOUNT
        );
        assert_eq!(
            account_name(DiagnosticAccount::ControlBroker),
            DIAGNOSTIC_CONTROL_BROKER_ACCOUNT
        );
        assert_eq!(
            account_name(DiagnosticAccount::UpdateSupervisor),
            DIAGNOSTIC_UPDATE_SUPERVISOR_ACCOUNT
        );
        let assignments = resolve_assignments_with(&MockLookup::valid(), UIDS[0]).unwrap();
        assert_eq!(
            assignments.map_peer(KernelPeerCredentials::new(Some(1), UIDS[1], SHARED_GID,)),
            Ok(DiagnosticSource::Server)
        );
    }

    #[test]
    fn every_missing_mismatched_and_failed_user_is_typed() {
        for account in ACCOUNTS {
            let index = account_index(account);
            let mut missing = MockLookup::valid();
            missing.users[index] = None;
            assert!(matches!(
                resolve_assignments_with(&missing, UIDS[0]),
                Err(DiagnosticPeerAdapterError::UserMissing(found)) if found == account
            ));

            let mut mismatch = MockLookup::valid();
            mismatch.users[index] = Some(ResolvedUser {
                exact_name: false,
                uid: UIDS[index],
            });
            assert!(matches!(
                resolve_assignments_with(&mismatch, UIDS[0]),
                Err(DiagnosticPeerAdapterError::UserNameMismatch(found)) if found == account
            ));

            let mut failed = MockLookup::valid();
            failed.failed_user = Some(account);
            assert!(matches!(
                resolve_assignments_with(&failed, UIDS[0]),
                Err(DiagnosticPeerAdapterError::UserLookupFailed(found)) if found == account
            ));
        }
    }

    #[test]
    fn group_failure_missing_and_name_mismatch_are_distinct() {
        let mut failed = MockLookup::valid();
        failed.failed_group = true;
        assert!(matches!(
            resolve_assignments_with(&failed, UIDS[0]),
            Err(DiagnosticPeerAdapterError::GroupLookupFailed)
        ));

        let mut missing = MockLookup::valid();
        missing.group = None;
        assert!(matches!(
            resolve_assignments_with(&missing, UIDS[0]),
            Err(DiagnosticPeerAdapterError::GroupMissing)
        ));

        let mut mismatch = MockLookup::valid();
        mismatch.group = Some(ResolvedGroup {
            exact_name: false,
            gid: SHARED_GID,
        });
        assert!(matches!(
            resolve_assignments_with(&mismatch, UIDS[0]),
            Err(DiagnosticPeerAdapterError::GroupNameMismatch)
        ));
    }

    #[test]
    fn current_collector_uid_and_assignment_validation_fail_closed() {
        assert!(matches!(
            resolve_assignments_with(&MockLookup::valid(), UIDS[1]),
            Err(DiagnosticPeerAdapterError::WrongCollectorUid)
        ));
        let mut duplicate = MockLookup::valid();
        duplicate.users[2] = Some(ResolvedUser {
            exact_name: true,
            uid: UIDS[1],
        });
        assert!(matches!(
            resolve_assignments_with(&duplicate, UIDS[0]),
            Err(DiagnosticPeerAdapterError::InvalidAssignments(
                DiagnosticPeerConfigError::DuplicateUid { .. }
            ))
        ));
    }

    #[test]
    fn credential_conversion_rejects_missing_negative_and_wrong_values() {
        let assignments = resolve_assignments_with(&MockLookup::valid(), UIDS[0]).unwrap();
        for (pid, uid, gid, expected) in [
            (
                None,
                UIDS[1],
                SHARED_GID,
                DiagnosticPeerError::MissingPeerPid,
            ),
            (
                Some(-1),
                UIDS[1],
                SHARED_GID,
                DiagnosticPeerError::InvalidPeerPid,
            ),
            (Some(1), 999, SHARED_GID, DiagnosticPeerError::UnknownPeer),
            (
                Some(1),
                UIDS[1],
                SHARED_GID + 1,
                DiagnosticPeerError::UnexpectedPeerGroup,
            ),
        ] {
            assert_eq!(
                map_credential_values(&assignments, pid, uid, gid),
                Err(DiagnosticPeerAdapterError::PeerRejected(expected))
            );
        }
    }

    #[tokio::test]
    async fn tokio_peer_cred_is_connection_bound_and_numeric_facts_are_discarded() {
        let (stream, _peer) = UnixStream::pair().unwrap();
        let uid = geteuid().as_raw();
        let gid = getegid().as_raw();
        let mut values = [0_u32; 6];
        let mut candidate = 1_u32;
        for index in 0..values.len() {
            while candidate == uid || values[..index].contains(&candidate) {
                candidate += 1;
            }
            values[index] = candidate;
            candidate += 1;
        }
        if uid == 0 || gid == 0 {
            let assignments = DiagnosticPeerAssignments::new(
                values[0], values[1], values[2], values[3], values[4], values[5], 1,
            )
            .unwrap();
            assert!(matches!(
                map_tokio_peer(&assignments, &stream),
                Err(DiagnosticPeerAdapterError::PeerRejected(_))
            ));
            return;
        }
        values[1] = uid;
        let assignments = DiagnosticPeerAssignments::new(
            values[0], values[1], values[2], values[3], values[4], values[5], gid,
        )
        .unwrap();
        assert_eq!(
            map_tokio_peer(&assignments, &stream),
            Ok(DiagnosticSource::Server)
        );
    }

    const fn account_index(account: DiagnosticAccount) -> usize {
        match account {
            DiagnosticAccount::Collector => 0,
            DiagnosticAccount::Server => 1,
            DiagnosticAccount::PlatformBroker => 2,
            DiagnosticAccount::VideoProxy => 3,
            DiagnosticAccount::ControlBroker => 4,
            DiagnosticAccount::UpdateSupervisor => 5,
        }
    }
}
