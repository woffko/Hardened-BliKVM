#![no_std]
#![forbid(unsafe_code)]

//! Pure lifecycle and source policy for typed diagnostic events.
//!
//! The five source variants represent existing Hardened BliKVM process roles.
//! No arbitrary source, message, path, identity, byte payload, or I/O surface
//! exists. A future adapter must bind its trusted peer to one source before
//! constructing records; the source is never parsed from untrusted text.

use bli_kvm_diagnostic_export::{
    DiagnosticBundle, DiagnosticComponent, DiagnosticEvent, DiagnosticExportError,
    DiagnosticExporter, RejectReason, ResourceKind,
};

pub const DIAGNOSTIC_SOURCE_COUNT: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticSource {
    Server,
    PlatformBroker,
    VideoProxy,
    ControlBroker,
    UpdateSupervisor,
}

impl DiagnosticSource {
    pub const fn component(self) -> DiagnosticComponent {
        match self {
            Self::Server => DiagnosticComponent::Server,
            Self::PlatformBroker => DiagnosticComponent::PlatformBroker,
            Self::VideoProxy => DiagnosticComponent::VideoProxy,
            Self::ControlBroker => DiagnosticComponent::ControlBroker,
            Self::UpdateSupervisor => DiagnosticComponent::UpdateSupervisor,
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Server => 0,
            Self::PlatformBroker => 1,
            Self::VideoProxy => 2,
            Self::ControlBroker => 3,
            Self::UpdateSupervisor => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSourceStatus {
    Dormant,
    Active { epoch: u32, next_sequence: u16 },
    Stopped { epoch: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticCollectorError {
    InvalidEpoch,
    SourceNotStarted,
    SourceAlreadyActive,
    SourceStopped,
    EpochMismatch,
    EpochNotAdvanced,
    SequenceMismatch,
    SequenceExhausted,
    EventNotAllowed,
    Export(DiagnosticExportError),
}

impl From<DiagnosticExportError> for DiagnosticCollectorError {
    fn from(error: DiagnosticExportError) -> Self {
        Self::Export(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceRecord {
    source: DiagnosticSource,
    epoch: u32,
    sequence: u16,
    record: bli_kvm_diagnostic_export::DiagnosticRecord,
}

impl SourceRecord {
    pub const fn new(
        source: DiagnosticSource,
        epoch: u32,
        sequence: u16,
        offset_ms: u32,
        event: DiagnosticEvent,
    ) -> Result<Self, DiagnosticCollectorError> {
        if epoch == 0 {
            return Err(DiagnosticCollectorError::InvalidEpoch);
        }
        match bli_kvm_diagnostic_export::DiagnosticRecord::new(offset_ms, source.component(), event)
        {
            Ok(record) => Ok(Self {
                source,
                epoch,
                sequence,
                record,
            }),
            Err(error) => Err(DiagnosticCollectorError::Export(error)),
        }
    }

    pub const fn source(self) -> DiagnosticSource {
        self.source
    }

    pub const fn epoch(self) -> u32 {
        self.epoch
    }

    pub const fn sequence(self) -> u16 {
        self.sequence
    }

    pub const fn offset_ms(self) -> u32 {
        self.record.offset_ms()
    }

    pub const fn event(self) -> DiagnosticEvent {
        self.record.event()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticCollector {
    exporter: DiagnosticExporter,
    sources: [DiagnosticSourceStatus; DIAGNOSTIC_SOURCE_COUNT],
}

impl DiagnosticCollector {
    pub const fn new() -> Self {
        Self {
            exporter: DiagnosticExporter::new(),
            sources: [DiagnosticSourceStatus::Dormant; DIAGNOSTIC_SOURCE_COUNT],
        }
    }

    pub const fn record_count(&self) -> usize {
        self.exporter.record_count()
    }

    pub const fn source_status(&self, source: DiagnosticSource) -> DiagnosticSourceStatus {
        self.sources[source.index()]
    }

    pub fn push(&mut self, record: SourceRecord) -> Result<(), DiagnosticCollectorError> {
        if !event_allowed(record.source, record.event()) {
            return Err(DiagnosticCollectorError::EventNotAllowed);
        }
        let next = next_source_status(self.source_status(record.source), record)?;
        self.exporter.push(record.record)?;
        self.sources[record.source.index()] = next;
        Ok(())
    }

    pub fn export(&self) -> Result<DiagnosticBundle, DiagnosticExportError> {
        self.exporter.export()
    }
}

impl Default for DiagnosticCollector {
    fn default() -> Self {
        Self::new()
    }
}

fn next_source_status(
    current: DiagnosticSourceStatus,
    record: SourceRecord,
) -> Result<DiagnosticSourceStatus, DiagnosticCollectorError> {
    if record.event() == DiagnosticEvent::Started {
        if record.sequence != 0 {
            return Err(DiagnosticCollectorError::SequenceMismatch);
        }
        return match current {
            DiagnosticSourceStatus::Dormant => Ok(DiagnosticSourceStatus::Active {
                epoch: record.epoch,
                next_sequence: 1,
            }),
            DiagnosticSourceStatus::Active { .. } => {
                Err(DiagnosticCollectorError::SourceAlreadyActive)
            }
            DiagnosticSourceStatus::Stopped { epoch } if record.epoch <= epoch => {
                Err(DiagnosticCollectorError::EpochNotAdvanced)
            }
            DiagnosticSourceStatus::Stopped { .. } => Ok(DiagnosticSourceStatus::Active {
                epoch: record.epoch,
                next_sequence: 1,
            }),
        };
    }

    match current {
        DiagnosticSourceStatus::Dormant => Err(DiagnosticCollectorError::SourceNotStarted),
        DiagnosticSourceStatus::Stopped { epoch } if record.epoch <= epoch => {
            Err(DiagnosticCollectorError::SourceStopped)
        }
        DiagnosticSourceStatus::Stopped { .. } => Err(DiagnosticCollectorError::SourceNotStarted),
        DiagnosticSourceStatus::Active {
            epoch,
            next_sequence,
        } => {
            if record.epoch != epoch {
                return Err(DiagnosticCollectorError::EpochMismatch);
            }
            if record.sequence != next_sequence {
                return Err(DiagnosticCollectorError::SequenceMismatch);
            }
            if record.event() == DiagnosticEvent::Stopped {
                return Ok(DiagnosticSourceStatus::Stopped { epoch });
            }
            let next_sequence = record
                .sequence
                .checked_add(1)
                .ok_or(DiagnosticCollectorError::SequenceExhausted)?;
            Ok(DiagnosticSourceStatus::Active {
                epoch,
                next_sequence,
            })
        }
    }
}

const fn event_allowed(source: DiagnosticSource, event: DiagnosticEvent) -> bool {
    match event {
        DiagnosticEvent::Started
        | DiagnosticEvent::Ready
        | DiagnosticEvent::Stopped
        | DiagnosticEvent::Unavailable => true,
        DiagnosticEvent::RequestRejected { reason } => rejection_allowed(source, reason),
        DiagnosticEvent::RetryScheduled { .. } => {
            matches!(source, DiagnosticSource::UpdateSupervisor)
        }
        DiagnosticEvent::OperationFinished { .. } => matches!(
            source,
            DiagnosticSource::ControlBroker | DiagnosticSource::UpdateSupervisor
        ),
        DiagnosticEvent::ResourcePressure { resource, .. } => resource_allowed(source, resource),
        DiagnosticEvent::JournalObserved { .. } => {
            matches!(source, DiagnosticSource::UpdateSupervisor)
        }
    }
}

const fn rejection_allowed(source: DiagnosticSource, reason: RejectReason) -> bool {
    match source {
        DiagnosticSource::Server => true,
        DiagnosticSource::PlatformBroker => matches!(
            reason,
            RejectReason::Unauthorized
                | RejectReason::BackendUnavailable
                | RejectReason::Busy
                | RejectReason::InvalidInput
        ),
        DiagnosticSource::VideoProxy => matches!(
            reason,
            RejectReason::BackendUnavailable | RejectReason::Busy | RejectReason::InvalidInput
        ),
        DiagnosticSource::ControlBroker => matches!(
            reason,
            RejectReason::CapabilityDisabled
                | RejectReason::Busy
                | RejectReason::InvalidInput
                | RejectReason::Conflict
        ),
        DiagnosticSource::UpdateSupervisor => matches!(
            reason,
            RejectReason::BackendUnavailable
                | RejectReason::Busy
                | RejectReason::InvalidInput
                | RejectReason::Conflict
        ),
    }
}

const fn resource_allowed(source: DiagnosticSource, resource: ResourceKind) -> bool {
    match source {
        DiagnosticSource::Server => matches!(
            resource,
            ResourceKind::Memory | ResourceKind::Storage | ResourceKind::Sessions
        ),
        DiagnosticSource::PlatformBroker => matches!(resource, ResourceKind::Temperature),
        DiagnosticSource::VideoProxy | DiagnosticSource::ControlBroker => {
            matches!(resource, ResourceKind::Sessions)
        }
        DiagnosticSource::UpdateSupervisor => matches!(resource, ResourceKind::Storage),
    }
}

#[cfg(test)]
mod tests {
    use bli_kvm_diagnostic_export::{
        JournalHealth, MAX_RECORDS, MAX_RETRY_ATTEMPT, MAX_WINDOW_MS, OperationResult,
        PressureLevel,
    };

    use super::*;

    fn source_record(
        source: DiagnosticSource,
        epoch: u32,
        sequence: u16,
        offset_ms: u32,
        event: DiagnosticEvent,
    ) -> SourceRecord {
        SourceRecord::new(source, epoch, sequence, offset_ms, event).unwrap()
    }

    #[test]
    fn exactly_five_fixed_sources_map_to_distinct_components() {
        let pairs = [
            (DiagnosticSource::Server, DiagnosticComponent::Server),
            (
                DiagnosticSource::PlatformBroker,
                DiagnosticComponent::PlatformBroker,
            ),
            (
                DiagnosticSource::VideoProxy,
                DiagnosticComponent::VideoProxy,
            ),
            (
                DiagnosticSource::ControlBroker,
                DiagnosticComponent::ControlBroker,
            ),
            (
                DiagnosticSource::UpdateSupervisor,
                DiagnosticComponent::UpdateSupervisor,
            ),
        ];
        assert_eq!(pairs.len(), DIAGNOSTIC_SOURCE_COUNT);
        let mut collector = DiagnosticCollector::new();
        for (offset, (source, component)) in pairs.into_iter().enumerate() {
            assert_eq!(source.component(), component);
            collector
                .push(source_record(
                    source,
                    1,
                    0,
                    offset as u32,
                    DiagnosticEvent::Started,
                ))
                .unwrap();
        }
        assert_eq!(collector.record_count(), DIAGNOSTIC_SOURCE_COUNT);
        let bundle = collector.export().unwrap();
        let output = core::str::from_utf8(bundle.as_bytes()).unwrap();
        for component in [
            "component=server",
            "component=platform_broker",
            "component=video_proxy",
            "component=control_broker",
            "component=update_supervisor",
        ] {
            assert!(output.contains(component));
        }
    }

    #[test]
    fn lifecycle_requires_start_exact_sequence_stop_and_new_epoch() {
        let source = DiagnosticSource::Server;
        let mut collector = DiagnosticCollector::new();
        collector
            .push(source_record(source, 7, 0, 0, DiagnosticEvent::Started))
            .unwrap();
        collector
            .push(source_record(source, 7, 1, 1, DiagnosticEvent::Ready))
            .unwrap();
        collector
            .push(source_record(source, 7, 2, 2, DiagnosticEvent::Stopped))
            .unwrap();
        assert_eq!(
            collector.source_status(source),
            DiagnosticSourceStatus::Stopped { epoch: 7 }
        );
        let stopped = collector.clone();
        assert_eq!(
            collector.push(source_record(source, 7, 3, 3, DiagnosticEvent::Ready)),
            Err(DiagnosticCollectorError::SourceStopped)
        );
        assert_eq!(collector, stopped);
        assert_eq!(
            collector.push(source_record(source, 7, 0, 3, DiagnosticEvent::Started)),
            Err(DiagnosticCollectorError::EpochNotAdvanced)
        );
        assert_eq!(collector, stopped);
        assert_eq!(
            collector.push(source_record(source, 8, 0, 3, DiagnosticEvent::Ready)),
            Err(DiagnosticCollectorError::SourceNotStarted)
        );
        assert_eq!(collector, stopped);
        collector
            .push(source_record(source, 8, 0, 3, DiagnosticEvent::Started))
            .unwrap();
        assert_eq!(
            collector.source_status(source),
            DiagnosticSourceStatus::Active {
                epoch: 8,
                next_sequence: 1
            }
        );
    }

    #[test]
    fn prestart_replay_gap_epoch_and_duplicate_start_do_not_mutate() {
        let source = DiagnosticSource::PlatformBroker;
        let mut collector = DiagnosticCollector::new();
        assert_eq!(
            collector.push(source_record(source, 1, 1, 0, DiagnosticEvent::Started)),
            Err(DiagnosticCollectorError::SequenceMismatch)
        );
        assert_eq!(
            collector.push(source_record(source, 1, 1, 0, DiagnosticEvent::Ready)),
            Err(DiagnosticCollectorError::SourceNotStarted)
        );
        collector
            .push(source_record(source, 1, 0, 0, DiagnosticEvent::Started))
            .unwrap();
        let active = collector.clone();
        for (record, error) in [
            (
                source_record(source, 1, 0, 1, DiagnosticEvent::Ready),
                DiagnosticCollectorError::SequenceMismatch,
            ),
            (
                source_record(source, 1, 2, 1, DiagnosticEvent::Ready),
                DiagnosticCollectorError::SequenceMismatch,
            ),
            (
                source_record(source, 2, 1, 1, DiagnosticEvent::Ready),
                DiagnosticCollectorError::EpochMismatch,
            ),
            (
                source_record(source, 2, 0, 1, DiagnosticEvent::Started),
                DiagnosticCollectorError::SourceAlreadyActive,
            ),
        ] {
            assert_eq!(collector.push(record), Err(error));
            assert_eq!(collector, active);
        }
    }

    #[test]
    fn source_event_matrix_is_explicit_and_fail_closed() {
        let cases = [
            (
                DiagnosticSource::Server,
                DiagnosticEvent::ResourcePressure {
                    resource: ResourceKind::Memory,
                    level: PressureLevel::Elevated,
                },
                true,
            ),
            (
                DiagnosticSource::Server,
                DiagnosticEvent::ResourcePressure {
                    resource: ResourceKind::Temperature,
                    level: PressureLevel::Critical,
                },
                false,
            ),
            (
                DiagnosticSource::PlatformBroker,
                DiagnosticEvent::ResourcePressure {
                    resource: ResourceKind::Temperature,
                    level: PressureLevel::Critical,
                },
                true,
            ),
            (
                DiagnosticSource::VideoProxy,
                DiagnosticEvent::RequestRejected {
                    reason: RejectReason::RateLimited,
                },
                false,
            ),
            (
                DiagnosticSource::ControlBroker,
                DiagnosticEvent::RequestRejected {
                    reason: RejectReason::CapabilityDisabled,
                },
                true,
            ),
            (
                DiagnosticSource::UpdateSupervisor,
                DiagnosticEvent::JournalObserved {
                    health: JournalHealth::RecoveryRequired,
                },
                true,
            ),
            (
                DiagnosticSource::PlatformBroker,
                DiagnosticEvent::JournalObserved {
                    health: JournalHealth::Healthy,
                },
                false,
            ),
            (
                DiagnosticSource::UpdateSupervisor,
                DiagnosticEvent::RetryScheduled {
                    attempt: MAX_RETRY_ATTEMPT,
                },
                true,
            ),
            (
                DiagnosticSource::VideoProxy,
                DiagnosticEvent::OperationFinished {
                    result: OperationResult::Applied,
                },
                false,
            ),
        ];
        for (source, event, allowed) in cases {
            let mut collector = DiagnosticCollector::new();
            collector
                .push(source_record(source, 1, 0, 0, DiagnosticEvent::Started))
                .unwrap();
            let before = collector.clone();
            let result = collector.push(source_record(source, 1, 1, 1, event));
            assert_eq!(result.is_ok(), allowed);
            if !allowed {
                assert_eq!(result, Err(DiagnosticCollectorError::EventNotAllowed));
                assert_eq!(collector, before);
            }
        }
    }

    #[test]
    fn complete_source_allowlist_matrix_is_stable() {
        let sources = [
            DiagnosticSource::Server,
            DiagnosticSource::PlatformBroker,
            DiagnosticSource::VideoProxy,
            DiagnosticSource::ControlBroker,
            DiagnosticSource::UpdateSupervisor,
        ];
        for source in sources {
            for event in [
                DiagnosticEvent::Started,
                DiagnosticEvent::Ready,
                DiagnosticEvent::Stopped,
                DiagnosticEvent::Unavailable,
            ] {
                assert!(event_allowed(source, event));
            }
        }

        for (reason, expected) in [
            (
                RejectReason::Unauthorized,
                [true, true, false, false, false],
            ),
            (
                RejectReason::CapabilityDisabled,
                [true, false, false, true, false],
            ),
            (
                RejectReason::BackendUnavailable,
                [true, true, true, false, true],
            ),
            (RejectReason::Busy, [true, true, true, true, true]),
            (RejectReason::InvalidInput, [true, true, true, true, true]),
            (
                RejectReason::RateLimited,
                [true, false, false, false, false],
            ),
            (RejectReason::Conflict, [true, false, false, true, true]),
        ] {
            for (source, expected) in sources.into_iter().zip(expected) {
                assert_eq!(
                    event_allowed(source, DiagnosticEvent::RequestRejected { reason }),
                    expected
                );
            }
        }

        for (resource, expected) in [
            (ResourceKind::Memory, [true, false, false, false, false]),
            (ResourceKind::Storage, [true, false, false, false, true]),
            (
                ResourceKind::Temperature,
                [false, true, false, false, false],
            ),
            (ResourceKind::Sessions, [true, false, true, true, false]),
        ] {
            for (source, expected) in sources.into_iter().zip(expected) {
                assert_eq!(
                    event_allowed(
                        source,
                        DiagnosticEvent::ResourcePressure {
                            resource,
                            level: PressureLevel::Critical,
                        },
                    ),
                    expected
                );
            }
        }

        for (source, retry, operation, journal) in [
            (DiagnosticSource::Server, false, false, false),
            (DiagnosticSource::PlatformBroker, false, false, false),
            (DiagnosticSource::VideoProxy, false, false, false),
            (DiagnosticSource::ControlBroker, false, true, false),
            (DiagnosticSource::UpdateSupervisor, true, true, true),
        ] {
            assert_eq!(
                event_allowed(source, DiagnosticEvent::RetryScheduled { attempt: 1 },),
                retry
            );
            assert_eq!(
                event_allowed(
                    source,
                    DiagnosticEvent::OperationFinished {
                        result: OperationResult::Noop,
                    },
                ),
                operation
            );
            assert_eq!(
                event_allowed(
                    source,
                    DiagnosticEvent::JournalObserved {
                        health: JournalHealth::Healthy,
                    },
                ),
                journal
            );
        }
    }

    #[test]
    fn invalid_epoch_time_event_and_global_clock_rollback_do_not_mutate() {
        assert_eq!(
            SourceRecord::new(DiagnosticSource::Server, 0, 0, 0, DiagnosticEvent::Started,),
            Err(DiagnosticCollectorError::InvalidEpoch)
        );
        assert_eq!(
            SourceRecord::new(
                DiagnosticSource::Server,
                1,
                0,
                MAX_WINDOW_MS + 1,
                DiagnosticEvent::Started,
            ),
            Err(DiagnosticCollectorError::Export(
                DiagnosticExportError::InvalidTime
            ))
        );
        assert_eq!(
            SourceRecord::new(
                DiagnosticSource::UpdateSupervisor,
                1,
                1,
                0,
                DiagnosticEvent::RetryScheduled { attempt: 0 },
            ),
            Err(DiagnosticCollectorError::Export(
                DiagnosticExportError::InvalidEvent
            ))
        );
        let mut collector = DiagnosticCollector::new();
        collector
            .push(source_record(
                DiagnosticSource::Server,
                1,
                0,
                10,
                DiagnosticEvent::Started,
            ))
            .unwrap();
        let before = collector.clone();
        assert_eq!(
            collector.push(source_record(
                DiagnosticSource::VideoProxy,
                1,
                0,
                9,
                DiagnosticEvent::Started,
            )),
            Err(DiagnosticCollectorError::Export(
                DiagnosticExportError::ClockMovedBackwards
            ))
        );
        assert_eq!(collector, before);
    }

    #[test]
    fn hard_capacity_does_not_advance_source_sequence() {
        let source = DiagnosticSource::Server;
        let mut collector = DiagnosticCollector::new();
        collector
            .push(source_record(source, 1, 0, 0, DiagnosticEvent::Started))
            .unwrap();
        for sequence in 1..MAX_RECORDS as u16 {
            collector
                .push(source_record(
                    source,
                    1,
                    sequence,
                    u32::from(sequence),
                    DiagnosticEvent::Ready,
                ))
                .unwrap();
        }
        let full = collector.clone();
        assert_eq!(
            collector.push(source_record(
                source,
                1,
                MAX_RECORDS as u16,
                MAX_RECORDS as u32,
                DiagnosticEvent::Ready,
            )),
            Err(DiagnosticCollectorError::Export(
                DiagnosticExportError::CapacityExceeded
            ))
        );
        assert_eq!(collector, full);
        assert_eq!(collector.record_count(), MAX_RECORDS);
    }

    #[test]
    fn output_is_deterministic_and_contains_no_source_epoch_or_sequence() {
        let mut first = DiagnosticCollector::new();
        let mut second = DiagnosticCollector::new();
        for collector in [&mut first, &mut second] {
            collector
                .push(source_record(
                    DiagnosticSource::ControlBroker,
                    47,
                    0,
                    0,
                    DiagnosticEvent::Started,
                ))
                .unwrap();
            collector
                .push(source_record(
                    DiagnosticSource::ControlBroker,
                    47,
                    1,
                    1,
                    DiagnosticEvent::RequestRejected {
                        reason: RejectReason::CapabilityDisabled,
                    },
                ))
                .unwrap();
        }
        let first = first.export().unwrap();
        let second = second.export().unwrap();
        assert_eq!(first, second);
        let output = core::str::from_utf8(first.as_bytes()).unwrap();
        assert!(!output.contains("epoch"));
        assert!(!output.contains("sequence"));
        assert!(!output.contains("47"));
    }

    #[test]
    fn fresh_collector_has_no_cross_window_source_state() {
        let mut previous = DiagnosticCollector::new();
        previous
            .push(source_record(
                DiagnosticSource::UpdateSupervisor,
                9,
                0,
                100,
                DiagnosticEvent::Started,
            ))
            .unwrap();
        let fresh = DiagnosticCollector::new();
        assert_eq!(fresh.record_count(), 0);
        assert_eq!(
            fresh.source_status(DiagnosticSource::UpdateSupervisor),
            DiagnosticSourceStatus::Dormant
        );
    }
}
