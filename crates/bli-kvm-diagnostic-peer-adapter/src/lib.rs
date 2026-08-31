#![forbid(unsafe_code)]

//! Safe fixed-name account resolution and connection-bound peer mapping.
//!
//! This crate has no listener, path, configuration, environment, logging,
//! persistence, or process runtime. It resolves seven compile-time names,
//! validates the current collector UID, reads credentials only through Tokio's
//! safe `UnixStream::peer_cred`, and composes an in-memory frame/window core.

use bli_kvm_diagnostic_peer_policy::{
    DiagnosticAccount, DiagnosticPeerAssignments, DiagnosticPeerConfigError, DiagnosticPeerError,
    DiagnosticSource, KernelPeerCredentials,
};
use bli_kvm_diagnostic_protocol::{DIAGNOSTIC_RESPONSE_FRAME_BYTES, decode_event, encode_response};
use bli_kvm_diagnostic_window_policy::{
    DiagnosticSession, DiagnosticWindowError, DiagnosticWindowManager, DiagnosticWindowSnapshot,
    DisconnectOutcome, WindowTickOutcome,
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

#[derive(PartialEq, Eq)]
pub struct AuthenticatedDiagnosticPeer {
    source: DiagnosticSource,
}

#[derive(PartialEq, Eq)]
pub struct DiagnosticConnection {
    session: Option<DiagnosticSession>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticRuntimeError {
    ConnectionClosed,
    Window(DiagnosticWindowError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticTerminationReason {
    InvalidFrame,
    WindowFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticFrameOutcome {
    Response([u8; DIAGNOSTIC_RESPONSE_FRAME_BYTES]),
    Terminated {
        reason: DiagnosticTerminationReason,
        disconnect: DisconnectOutcome,
    },
}

pub struct DiagnosticRuntimeCore {
    windows: DiagnosticWindowManager,
}

impl DiagnosticRuntimeCore {
    pub fn new(instance_id: u64, now_ms: u64) -> Result<Self, DiagnosticRuntimeError> {
        Ok(Self {
            windows: DiagnosticWindowManager::new(instance_id, now_ms)
                .map_err(DiagnosticRuntimeError::Window)?,
        })
    }

    pub fn accept(
        &mut self,
        peer: AuthenticatedDiagnosticPeer,
        now_ms: u64,
    ) -> Result<DiagnosticConnection, DiagnosticRuntimeError> {
        let session = self
            .windows
            .connect(peer.source, now_ms)
            .map_err(DiagnosticRuntimeError::Window)?;
        Ok(DiagnosticConnection {
            session: Some(session),
        })
    }

    pub fn handle_frame(
        &mut self,
        connection: &mut DiagnosticConnection,
        now_ms: u64,
        frame: &[u8],
    ) -> Result<DiagnosticFrameOutcome, DiagnosticRuntimeError> {
        let session = connection
            .session
            .ok_or(DiagnosticRuntimeError::ConnectionClosed)?;
        let event = match decode_event(frame) {
            Ok(event) => event,
            Err(_) => {
                return self.terminate(
                    connection,
                    session,
                    now_ms,
                    DiagnosticTerminationReason::InvalidFrame,
                );
            }
        };
        match self.windows.submit(session, now_ms, event) {
            Ok(response) => Ok(DiagnosticFrameOutcome::Response(encode_response(response))),
            Err(DiagnosticWindowError::SessionMismatch) => Err(DiagnosticRuntimeError::Window(
                DiagnosticWindowError::SessionMismatch,
            )),
            Err(_) => self.terminate(
                connection,
                session,
                now_ms,
                DiagnosticTerminationReason::WindowFailure,
            ),
        }
    }

    pub fn disconnect(
        &mut self,
        connection: &mut DiagnosticConnection,
        now_ms: u64,
    ) -> Result<DisconnectOutcome, DiagnosticRuntimeError> {
        let session = connection
            .session
            .as_ref()
            .copied()
            .ok_or(DiagnosticRuntimeError::ConnectionClosed)?;
        let outcome = self
            .windows
            .disconnect(session, now_ms)
            .map_err(DiagnosticRuntimeError::Window)?;
        connection.session = None;
        Ok(outcome)
    }

    pub fn tick(&mut self, now_ms: u64) -> Result<WindowTickOutcome, DiagnosticRuntimeError> {
        self.windows
            .tick(now_ms)
            .map_err(DiagnosticRuntimeError::Window)
    }

    pub fn live_snapshot(&self) -> Result<DiagnosticWindowSnapshot, DiagnosticRuntimeError> {
        self.windows
            .live_snapshot()
            .map_err(DiagnosticRuntimeError::Window)
    }

    pub const fn sealed_window(&self) -> Option<&DiagnosticWindowSnapshot> {
        self.windows.sealed_window()
    }

    pub fn acknowledge_sealed(&mut self, window_id: u32) -> Result<(), DiagnosticRuntimeError> {
        self.windows
            .acknowledge_sealed(window_id)
            .map_err(DiagnosticRuntimeError::Window)
    }

    fn terminate(
        &mut self,
        connection: &mut DiagnosticConnection,
        session: DiagnosticSession,
        now_ms: u64,
        reason: DiagnosticTerminationReason,
    ) -> Result<DiagnosticFrameOutcome, DiagnosticRuntimeError> {
        let disconnect = self
            .windows
            .disconnect(session, now_ms)
            .map_err(DiagnosticRuntimeError::Window)?;
        connection.session = None;
        Ok(DiagnosticFrameOutcome::Terminated { reason, disconnect })
    }
}

pub fn resolve_current_assignments() -> Result<DiagnosticPeerAssignments, DiagnosticPeerAdapterError>
{
    resolve_assignments_with(&NixAccountLookup, geteuid().as_raw())
}

pub fn map_tokio_peer(
    assignments: &DiagnosticPeerAssignments,
    stream: &UnixStream,
) -> Result<AuthenticatedDiagnosticPeer, DiagnosticPeerAdapterError> {
    let credentials = stream
        .peer_cred()
        .map_err(|_| DiagnosticPeerAdapterError::PeerCredentialReadFailed)?;
    let source = map_credential_values(
        assignments,
        credentials.pid(),
        credentials.uid(),
        credentials.gid(),
    )?;
    Ok(AuthenticatedDiagnosticPeer { source })
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
    use bli_kvm_diagnostic_protocol::{
        DiagnosticEvent, DiagnosticProducerRejection, DiagnosticProducerResponse, encode_event,
    };
    use bli_kvm_diagnostic_window_policy::LifecycleGapReason;
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
        let authenticated = map_tokio_peer(&assignments, &stream).unwrap();
        assert_eq!(authenticated.source, DiagnosticSource::Server);
    }

    #[test]
    fn runtime_core_accepts_typed_frame_responds_and_records_eof() {
        let mut runtime = DiagnosticRuntimeCore::new(7, 0).unwrap();
        let mut connection = runtime.accept(peer(DiagnosticSource::Server), 0).unwrap();
        let frame = encode_event(DiagnosticEvent::Ready).unwrap();
        assert_eq!(
            runtime.handle_frame(&mut connection, 1, &frame),
            Ok(DiagnosticFrameOutcome::Response(encode_response(
                DiagnosticProducerResponse::Accepted
            )))
        );
        assert_eq!(runtime.live_snapshot().unwrap().bundle().record_count(), 2);
        assert_eq!(
            runtime.disconnect(&mut connection, 2),
            Ok(DisconnectOutcome::Recorded)
        );
        assert_eq!(runtime.live_snapshot().unwrap().bundle().record_count(), 3);
        assert_eq!(
            runtime.handle_frame(&mut connection, 3, &frame),
            Err(DiagnosticRuntimeError::ConnectionClosed)
        );
    }

    #[test]
    fn policy_rejection_returns_one_canonical_response_and_keeps_connection() {
        let mut runtime = DiagnosticRuntimeCore::new(7, 0).unwrap();
        let mut connection = runtime
            .accept(peer(DiagnosticSource::VideoProxy), 0)
            .unwrap();
        let rejected_event = encode_event(DiagnosticEvent::ResourcePressure {
            resource: bli_kvm_diagnostic_protocol::ResourceKind::Temperature,
            level: bli_kvm_diagnostic_protocol::PressureLevel::Critical,
        })
        .unwrap();
        assert_eq!(
            runtime.handle_frame(&mut connection, 1, &rejected_event),
            Ok(DiagnosticFrameOutcome::Response(encode_response(
                DiagnosticProducerResponse::Rejected {
                    reason: DiagnosticProducerRejection::EventNotAllowed,
                }
            )))
        );
        assert_eq!(runtime.live_snapshot().unwrap().bundle().record_count(), 1);
        let ready = encode_event(DiagnosticEvent::Ready).unwrap();
        assert!(matches!(
            runtime.handle_frame(&mut connection, 1, &ready),
            Ok(DiagnosticFrameOutcome::Response(_))
        ));
    }

    #[test]
    fn invalid_frame_has_no_response_and_terminates_with_recorded_stop() {
        let mut runtime = DiagnosticRuntimeCore::new(7, 0).unwrap();
        let mut connection = runtime.accept(peer(DiagnosticSource::Server), 0).unwrap();
        assert_eq!(
            runtime.handle_frame(&mut connection, 1, &[1, 1, 0]),
            Ok(DiagnosticFrameOutcome::Terminated {
                reason: DiagnosticTerminationReason::InvalidFrame,
                disconnect: DisconnectOutcome::Recorded,
            })
        );
        assert_eq!(runtime.live_snapshot().unwrap().bundle().record_count(), 2);
        assert_eq!(
            runtime.disconnect(&mut connection, 2),
            Err(DiagnosticRuntimeError::ConnectionClosed)
        );
    }

    #[test]
    fn clock_failure_terminates_and_marks_visible_incomplete_disconnect() {
        let mut runtime = DiagnosticRuntimeCore::new(7, 10).unwrap();
        let mut connection = runtime.accept(peer(DiagnosticSource::Server), 10).unwrap();
        let frame = encode_event(DiagnosticEvent::Ready).unwrap();
        assert_eq!(
            runtime.handle_frame(&mut connection, 9, &frame),
            Ok(DiagnosticFrameOutcome::Terminated {
                reason: DiagnosticTerminationReason::WindowFailure,
                disconnect: DisconnectOutcome::Incomplete {
                    reason: LifecycleGapReason::ClockMovedBackwards,
                },
            })
        );
        assert_eq!(
            runtime.live_snapshot().unwrap().omitted_lifecycle_events(),
            1
        );
    }

    #[test]
    fn duplicate_authenticated_role_is_rejected_without_reusing_peer_token() {
        let mut runtime = DiagnosticRuntimeCore::new(7, 0).unwrap();
        let _connection = runtime.accept(peer(DiagnosticSource::Server), 0).unwrap();
        assert!(matches!(
            runtime.accept(peer(DiagnosticSource::Server), 0),
            Err(DiagnosticRuntimeError::Window(
                DiagnosticWindowError::SourceAlreadyConnected
            ))
        ));
    }

    #[test]
    fn foreign_core_cannot_consume_or_close_a_valid_connection_handle() {
        let mut owner = DiagnosticRuntimeCore::new(7, 0).unwrap();
        let mut foreign = DiagnosticRuntimeCore::new(8, 0).unwrap();
        let mut connection = owner.accept(peer(DiagnosticSource::Server), 0).unwrap();
        assert_eq!(
            foreign.disconnect(&mut connection, 1),
            Err(DiagnosticRuntimeError::Window(
                DiagnosticWindowError::SessionMismatch
            ))
        );
        let frame = encode_event(DiagnosticEvent::Ready).unwrap();
        assert!(matches!(
            owner.handle_frame(&mut connection, 1, &frame),
            Ok(DiagnosticFrameOutcome::Response(_))
        ));
    }

    #[test]
    fn second_full_window_returns_exact_backpressure_and_keeps_connection_open() {
        let mut runtime = DiagnosticRuntimeCore::new(7, 0).unwrap();
        let mut server = runtime.accept(peer(DiagnosticSource::Server), 0).unwrap();
        let mut other_connections = Vec::new();
        for (index, source) in [
            DiagnosticSource::PlatformBroker,
            DiagnosticSource::VideoProxy,
            DiagnosticSource::ControlBroker,
            DiagnosticSource::UpdateSupervisor,
        ]
        .into_iter()
        .enumerate()
        {
            other_connections.push(runtime.accept(peer(source), index as u64 + 1).unwrap());
        }
        let frame = encode_event(DiagnosticEvent::Ready).unwrap();
        let mut now = 5;
        while runtime.live_snapshot().unwrap().bundle().record_count() < 27 {
            assert!(matches!(
                runtime.handle_frame(&mut server, now, &frame),
                Ok(DiagnosticFrameOutcome::Response(_))
            ));
            now += 1;
        }
        assert_eq!(
            runtime.handle_frame(&mut server, now, &frame),
            Ok(DiagnosticFrameOutcome::Response(encode_response(
                DiagnosticProducerResponse::Accepted
            )))
        );
        now += 1;
        assert_eq!(runtime.sealed_window().unwrap().window_id(), 1);
        while runtime.live_snapshot().unwrap().bundle().record_count() < 27 {
            assert!(matches!(
                runtime.handle_frame(&mut server, now, &frame),
                Ok(DiagnosticFrameOutcome::Response(_))
            ));
            now += 1;
        }
        assert_eq!(
            runtime.handle_frame(&mut server, now, &frame),
            Ok(DiagnosticFrameOutcome::Response(encode_response(
                DiagnosticProducerResponse::Rejected {
                    reason: DiagnosticProducerRejection::Backpressure,
                }
            )))
        );
        assert!(matches!(
            runtime.handle_frame(&mut server, now, &frame),
            Ok(DiagnosticFrameOutcome::Response(_))
        ));
        drop(other_connections);
    }

    #[test]
    fn runtime_exposes_only_bounded_snapshot_seal_and_exact_acknowledgement() {
        const DAY_MS: u64 = 86_400_000;
        let mut runtime = DiagnosticRuntimeCore::new(7, 0).unwrap();
        let mut connection = runtime.accept(peer(DiagnosticSource::Server), 0).unwrap();
        assert_eq!(runtime.tick(DAY_MS + 1), Ok(WindowTickOutcome::Rotated));
        assert_eq!(runtime.sealed_window().unwrap().window_id(), 1);
        assert!(matches!(
            runtime.acknowledge_sealed(2),
            Err(DiagnosticRuntimeError::Window(
                DiagnosticWindowError::AcknowledgementMismatch
            ))
        ));
        assert_eq!(runtime.acknowledge_sealed(1), Ok(()));
        assert!(runtime.sealed_window().is_none());
        assert_eq!(
            runtime.disconnect(&mut connection, DAY_MS + 2),
            Ok(DisconnectOutcome::Recorded)
        );
    }

    fn peer(source: DiagnosticSource) -> AuthenticatedDiagnosticPeer {
        AuthenticatedDiagnosticPeer { source }
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
