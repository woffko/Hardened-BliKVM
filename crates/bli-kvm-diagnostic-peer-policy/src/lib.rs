#![no_std]
#![forbid(unsafe_code)]

//! Pure mapping from connection-bound kernel credentials to fixed sources.
//!
//! User/group lookup and `SO_PEERCRED` retrieval are outside this crate. The
//! policy accepts only numeric facts supplied by a future safe adapter, requires
//! six distinct non-root UIDs and one exact non-root shared GID, and returns
//! only the semantic source. PID, UID, and GID never enter diagnostic output.

use bli_kvm_diagnostic_collector_policy::DiagnosticSource;

pub const MAX_KERNEL_PID: u32 = 2_147_483_647;

const ACCOUNTS: [DiagnosticAccount; 6] = [
    DiagnosticAccount::Collector,
    DiagnosticAccount::Server,
    DiagnosticAccount::PlatformBroker,
    DiagnosticAccount::VideoProxy,
    DiagnosticAccount::ControlBroker,
    DiagnosticAccount::UpdateSupervisor,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticAccount {
    Collector,
    Server,
    PlatformBroker,
    VideoProxy,
    ControlBroker,
    UpdateSupervisor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticPeerConfigError {
    ZeroUid(DiagnosticAccount),
    DuplicateUid {
        first: DiagnosticAccount,
        second: DiagnosticAccount,
    },
    ZeroSharedGroup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticPeerError {
    MissingPeerPid,
    InvalidPeerPid,
    RootPeer,
    CollectorPeer,
    UnknownPeer,
    UnexpectedPeerGroup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KernelPeerCredentials {
    pid: Option<u32>,
    uid: u32,
    gid: u32,
}

impl KernelPeerCredentials {
    pub const fn new(pid: Option<u32>, uid: u32, gid: u32) -> Self {
        Self { pid, uid, gid }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DiagnosticPeerAssignments {
    collector_uid: u32,
    producer_uids: [u32; 5],
    shared_group_gid: u32,
}

impl DiagnosticPeerAssignments {
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        collector_uid: u32,
        server_uid: u32,
        platform_broker_uid: u32,
        video_proxy_uid: u32,
        control_broker_uid: u32,
        update_supervisor_uid: u32,
        shared_group_gid: u32,
    ) -> Result<Self, DiagnosticPeerConfigError> {
        let uids = [
            collector_uid,
            server_uid,
            platform_broker_uid,
            video_proxy_uid,
            control_broker_uid,
            update_supervisor_uid,
        ];
        let mut index = 0;
        while index < uids.len() {
            if uids[index] == 0 {
                return Err(DiagnosticPeerConfigError::ZeroUid(ACCOUNTS[index]));
            }
            let mut other = index + 1;
            while other < uids.len() {
                if uids[index] == uids[other] {
                    return Err(DiagnosticPeerConfigError::DuplicateUid {
                        first: ACCOUNTS[index],
                        second: ACCOUNTS[other],
                    });
                }
                other += 1;
            }
            index += 1;
        }
        if shared_group_gid == 0 {
            return Err(DiagnosticPeerConfigError::ZeroSharedGroup);
        }
        Ok(Self {
            collector_uid,
            producer_uids: [
                server_uid,
                platform_broker_uid,
                video_proxy_uid,
                control_broker_uid,
                update_supervisor_uid,
            ],
            shared_group_gid,
        })
    }

    pub const fn map_peer(
        &self,
        credentials: KernelPeerCredentials,
    ) -> Result<DiagnosticSource, DiagnosticPeerError> {
        let pid = match credentials.pid {
            Some(pid) => pid,
            None => return Err(DiagnosticPeerError::MissingPeerPid),
        };
        if pid == 0 || pid > MAX_KERNEL_PID {
            return Err(DiagnosticPeerError::InvalidPeerPid);
        }
        if credentials.uid == 0 {
            return Err(DiagnosticPeerError::RootPeer);
        }
        if credentials.uid == self.collector_uid {
            return Err(DiagnosticPeerError::CollectorPeer);
        }
        let source = if credentials.uid == self.producer_uids[0] {
            DiagnosticSource::Server
        } else if credentials.uid == self.producer_uids[1] {
            DiagnosticSource::PlatformBroker
        } else if credentials.uid == self.producer_uids[2] {
            DiagnosticSource::VideoProxy
        } else if credentials.uid == self.producer_uids[3] {
            DiagnosticSource::ControlBroker
        } else if credentials.uid == self.producer_uids[4] {
            DiagnosticSource::UpdateSupervisor
        } else {
            return Err(DiagnosticPeerError::UnknownPeer);
        };
        if credentials.gid != self.shared_group_gid {
            return Err(DiagnosticPeerError::UnexpectedPeerGroup);
        }
        Ok(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UIDS: [u32; 6] = [100, 101, 102, 103, 104, 105];
    const SHARED_GID: u32 = 200;

    fn assignments_from(
        values: [u32; 7],
    ) -> Result<DiagnosticPeerAssignments, DiagnosticPeerConfigError> {
        DiagnosticPeerAssignments::new(
            values[0], values[1], values[2], values[3], values[4], values[5], values[6],
        )
    }

    fn assignments() -> DiagnosticPeerAssignments {
        assignments_from([100, 101, 102, 103, 104, 105, SHARED_GID]).unwrap()
    }

    #[test]
    fn every_exact_uid_maps_to_one_fixed_source() {
        let assignments = assignments();
        for (index, expected) in [
            DiagnosticSource::Server,
            DiagnosticSource::PlatformBroker,
            DiagnosticSource::VideoProxy,
            DiagnosticSource::ControlBroker,
            DiagnosticSource::UpdateSupervisor,
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(
                assignments.map_peer(KernelPeerCredentials::new(
                    Some((index + 1) as u32),
                    UIDS[index + 1],
                    SHARED_GID,
                )),
                Ok(expected)
            );
        }
    }

    #[test]
    fn every_zero_uid_and_zero_shared_group_are_rejected() {
        for index in 0..UIDS.len() {
            let mut values = [100, 101, 102, 103, 104, 105, SHARED_GID];
            values[index] = 0;
            assert_eq!(
                assignments_from(values),
                Err(DiagnosticPeerConfigError::ZeroUid(ACCOUNTS[index]))
            );
        }
        assert_eq!(
            assignments_from([100, 101, 102, 103, 104, 105, 0]),
            Err(DiagnosticPeerConfigError::ZeroSharedGroup)
        );
    }

    #[test]
    fn every_pair_of_duplicate_uids_is_rejected() {
        for first in 0..UIDS.len() {
            for second in (first + 1)..UIDS.len() {
                let mut values = [100, 101, 102, 103, 104, 105, SHARED_GID];
                values[second] = values[first];
                assert_eq!(
                    assignments_from(values),
                    Err(DiagnosticPeerConfigError::DuplicateUid {
                        first: ACCOUNTS[first],
                        second: ACCOUNTS[second],
                    })
                );
            }
        }
    }

    #[test]
    fn missing_zero_and_out_of_range_pids_fail_closed() {
        let assignments = assignments();
        for (pid, expected) in [
            (None, DiagnosticPeerError::MissingPeerPid),
            (Some(0), DiagnosticPeerError::InvalidPeerPid),
            (
                Some(MAX_KERNEL_PID + 1),
                DiagnosticPeerError::InvalidPeerPid,
            ),
        ] {
            assert_eq!(
                assignments.map_peer(KernelPeerCredentials::new(pid, UIDS[1], SHARED_GID)),
                Err(expected)
            );
        }
        assert_eq!(
            assignments.map_peer(KernelPeerCredentials::new(
                Some(MAX_KERNEL_PID),
                UIDS[1],
                SHARED_GID,
            )),
            Ok(DiagnosticSource::Server)
        );
    }

    #[test]
    fn root_collector_and_unknown_uids_are_distinct_failures() {
        let assignments = assignments();
        for (uid, expected) in [
            (0, DiagnosticPeerError::RootPeer),
            (UIDS[0], DiagnosticPeerError::CollectorPeer),
            (999, DiagnosticPeerError::UnknownPeer),
        ] {
            assert_eq!(
                assignments.map_peer(KernelPeerCredentials::new(Some(1), uid, SHARED_GID)),
                Err(expected)
            );
        }
    }

    #[test]
    fn shared_group_is_required_but_never_selects_a_role() {
        let assignments = assignments();
        for gid in [0, SHARED_GID - 1, SHARED_GID + 1] {
            assert_eq!(
                assignments.map_peer(KernelPeerCredentials::new(Some(1), UIDS[1], gid)),
                Err(DiagnosticPeerError::UnexpectedPeerGroup)
            );
        }
        assert_eq!(
            assignments.map_peer(KernelPeerCredentials::new(Some(1), 999, SHARED_GID)),
            Err(DiagnosticPeerError::UnknownPeer)
        );
    }

    #[test]
    fn pid_changes_do_not_change_connection_bound_role_mapping() {
        let assignments = assignments();
        for pid in [1, 2, 65_535, MAX_KERNEL_PID] {
            assert_eq!(
                assignments.map_peer(KernelPeerCredentials::new(Some(pid), UIDS[4], SHARED_GID,)),
                Ok(DiagnosticSource::ControlBroker)
            );
        }
    }

    #[test]
    fn mapping_is_deterministic_and_returns_only_the_source_type() {
        let assignments = assignments();
        let credentials = KernelPeerCredentials::new(Some(77), UIDS[5], SHARED_GID);
        assert_eq!(
            assignments.map_peer(credentials),
            Ok(DiagnosticSource::UpdateSupervisor)
        );
        assert_eq!(
            assignments.map_peer(credentials),
            Ok(DiagnosticSource::UpdateSupervisor)
        );
    }
}
