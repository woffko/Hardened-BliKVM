#![no_std]
#![forbid(unsafe_code)]

//! Pure two-window diagnostic lifecycle and handoff policy.
//!
//! The manager owns source sessions, epochs, per-window sequences, relative
//! monotonic offsets, one live collector, and at most one sealed bundle waiting
//! for exact acknowledgement. It performs no identity lookup, socket, clock,
//! filesystem, process, network, device, path, text, or arbitrary-byte I/O.

use bli_kvm_diagnostic_collector_policy::{
    DIAGNOSTIC_SOURCE_COUNT, DiagnosticCollector, DiagnosticCollectorError, DiagnosticSource,
    DiagnosticSourceStatus, SourceRecord,
};
use bli_kvm_diagnostic_export::{
    DiagnosticBundle, DiagnosticEvent, DiagnosticExportError, MAX_RECORDS, MAX_WINDOW_MS,
};
use bli_kvm_diagnostic_protocol::{DiagnosticProducerRejection, DiagnosticProducerResponse};

const SOURCES: [DiagnosticSource; DIAGNOSTIC_SOURCE_COUNT] = [
    DiagnosticSource::Server,
    DiagnosticSource::PlatformBroker,
    DiagnosticSource::VideoProxy,
    DiagnosticSource::ControlBroker,
    DiagnosticSource::UpdateSupervisor,
];

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DiagnosticSession {
    instance_id: u64,
    source: DiagnosticSource,
    epoch: u32,
}

impl DiagnosticSession {
    pub const fn source(self) -> DiagnosticSource {
        self.source
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticRuntimeSourceStatus {
    Disconnected { last_epoch: u32 },
    Connected { epoch: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceConnectionState {
    Disconnected { last_epoch: u32 },
    Connected { epoch: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticWindowError {
    InvalidInstanceId,
    ClockMovedBackwards,
    SourceAlreadyConnected,
    SessionMismatch,
    Backpressure,
    CollectorUnavailable,
    NoSealedWindow,
    AcknowledgementMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleGapReason {
    Backpressure,
    ClockMovedBackwards,
    CollectorUnavailable,
    PolicyFailure,
    CounterSaturated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisconnectOutcome {
    Recorded,
    Incomplete { reason: LifecycleGapReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowTickOutcome {
    Unchanged,
    Rotated,
    Backpressured,
    CollectorUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticWindowSnapshot {
    window_id: u32,
    bundle: DiagnosticBundle,
    omitted_lifecycle_events: u8,
}

impl DiagnosticWindowSnapshot {
    pub const fn window_id(&self) -> u32 {
        self.window_id
    }

    pub const fn omitted_lifecycle_events(&self) -> u8 {
        self.omitted_lifecycle_events
    }

    pub const fn is_complete(&self) -> bool {
        self.omitted_lifecycle_events == 0
    }

    pub const fn bundle(&self) -> &DiagnosticBundle {
        &self.bundle
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticWindowManager {
    instance_id: u64,
    live_window_id: u32,
    live_origin_ms: u64,
    last_now_ms: u64,
    collector: DiagnosticCollector,
    sealed: Option<DiagnosticWindowSnapshot>,
    sources: [SourceConnectionState; DIAGNOSTIC_SOURCE_COUNT],
    omitted_lifecycle_events: u8,
}

impl DiagnosticWindowManager {
    pub const fn new(instance_id: u64, now_ms: u64) -> Result<Self, DiagnosticWindowError> {
        if instance_id == 0 {
            return Err(DiagnosticWindowError::InvalidInstanceId);
        }
        Ok(Self {
            instance_id,
            live_window_id: 1,
            live_origin_ms: now_ms,
            last_now_ms: now_ms,
            collector: DiagnosticCollector::new(),
            sealed: None,
            sources: [SourceConnectionState::Disconnected { last_epoch: 0 };
                DIAGNOSTIC_SOURCE_COUNT],
            omitted_lifecycle_events: 0,
        })
    }

    pub const fn live_window_id(&self) -> u32 {
        self.live_window_id
    }

    pub const fn live_record_count(&self) -> usize {
        self.collector.record_count()
    }

    pub fn active_source_count(&self) -> usize {
        self.sources
            .iter()
            .filter(|state| matches!(state, SourceConnectionState::Connected { .. }))
            .count()
    }

    pub const fn live_omitted_lifecycle_events(&self) -> u8 {
        self.omitted_lifecycle_events
    }

    pub const fn sealed_window(&self) -> Option<&DiagnosticWindowSnapshot> {
        self.sealed.as_ref()
    }

    pub const fn source_status(&self, source: DiagnosticSource) -> DiagnosticRuntimeSourceStatus {
        match self.sources[source_index(source)] {
            SourceConnectionState::Disconnected { last_epoch } => {
                DiagnosticRuntimeSourceStatus::Disconnected { last_epoch }
            }
            SourceConnectionState::Connected { epoch } => {
                DiagnosticRuntimeSourceStatus::Connected { epoch }
            }
        }
    }

    pub fn live_snapshot(&self) -> Result<DiagnosticWindowSnapshot, DiagnosticWindowError> {
        Ok(DiagnosticWindowSnapshot {
            window_id: self.live_window_id,
            bundle: self
                .collector
                .export()
                .map_err(|_| DiagnosticWindowError::CollectorUnavailable)?,
            omitted_lifecycle_events: self.omitted_lifecycle_events,
        })
    }

    pub fn connect(
        &mut self,
        source: DiagnosticSource,
        now_ms: u64,
    ) -> Result<DiagnosticSession, DiagnosticWindowError> {
        self.validate_now(now_ms)?;
        if matches!(
            self.sources[source_index(source)],
            SourceConnectionState::Connected { .. }
        ) {
            return Err(DiagnosticWindowError::SourceAlreadyConnected);
        }
        let mut candidate = self.clone();
        let session = candidate.connect_inner(source, now_ms)?;
        *self = candidate;
        Ok(session)
    }

    pub fn submit(
        &mut self,
        session: DiagnosticSession,
        now_ms: u64,
        event: DiagnosticEvent,
    ) -> Result<DiagnosticProducerResponse, DiagnosticWindowError> {
        self.validate_session(session)?;
        self.validate_now(now_ms)?;
        if matches!(event, DiagnosticEvent::Started | DiagnosticEvent::Stopped) {
            return Ok(rejected(DiagnosticProducerRejection::EventNotAllowed));
        }
        let mut candidate = self.clone();
        match candidate.submit_inner(session, now_ms, event) {
            Ok(()) => {
                *self = candidate;
                Ok(DiagnosticProducerResponse::Accepted)
            }
            Err(SubmitFailure::EventNotAllowed) => {
                Ok(rejected(DiagnosticProducerRejection::EventNotAllowed))
            }
            Err(SubmitFailure::Backpressure) => {
                Ok(rejected(DiagnosticProducerRejection::Backpressure))
            }
            Err(SubmitFailure::CollectorUnavailable) => {
                Ok(rejected(DiagnosticProducerRejection::CollectorUnavailable))
            }
        }
    }

    pub fn disconnect(
        &mut self,
        session: DiagnosticSession,
        now_ms: u64,
    ) -> Result<DisconnectOutcome, DiagnosticWindowError> {
        self.validate_session(session)?;
        if now_ms < self.last_now_ms {
            return Ok(self.mark_incomplete_disconnect(
                session,
                now_ms,
                LifecycleGapReason::ClockMovedBackwards,
            ));
        }
        let mut candidate = self.clone();
        match candidate.disconnect_inner(session, now_ms) {
            Ok(()) => {
                *self = candidate;
                Ok(DisconnectOutcome::Recorded)
            }
            Err(DiagnosticWindowError::Backpressure) => Ok(self.mark_incomplete_disconnect(
                session,
                now_ms,
                LifecycleGapReason::Backpressure,
            )),
            Err(DiagnosticWindowError::CollectorUnavailable) => Ok(self
                .mark_incomplete_disconnect(
                    session,
                    now_ms,
                    LifecycleGapReason::CollectorUnavailable,
                )),
            Err(_) => Ok(self.mark_incomplete_disconnect(
                session,
                now_ms,
                LifecycleGapReason::PolicyFailure,
            )),
        }
    }

    pub fn tick(&mut self, now_ms: u64) -> Result<WindowTickOutcome, DiagnosticWindowError> {
        self.validate_now(now_ms)?;
        let needs_rotation = self.needs_rotation(now_ms);
        if !needs_rotation {
            self.last_now_ms = now_ms;
            return Ok(WindowTickOutcome::Unchanged);
        }
        let mut candidate = self.clone();
        match candidate.rotate(now_ms) {
            Ok(rotated) => {
                candidate.last_now_ms = now_ms;
                *self = candidate;
                Ok(if rotated {
                    WindowTickOutcome::Rotated
                } else {
                    WindowTickOutcome::Unchanged
                })
            }
            Err(DiagnosticWindowError::Backpressure) => Ok(WindowTickOutcome::Backpressured),
            Err(DiagnosticWindowError::CollectorUnavailable) => {
                Ok(WindowTickOutcome::CollectorUnavailable)
            }
            Err(error) => Err(error),
        }
    }

    pub fn acknowledge_sealed(&mut self, window_id: u32) -> Result<(), DiagnosticWindowError> {
        let sealed = self
            .sealed
            .as_ref()
            .ok_or(DiagnosticWindowError::NoSealedWindow)?;
        if sealed.window_id != window_id {
            return Err(DiagnosticWindowError::AcknowledgementMismatch);
        }
        self.sealed = None;
        Ok(())
    }

    fn connect_inner(
        &mut self,
        source: DiagnosticSource,
        now_ms: u64,
    ) -> Result<DiagnosticSession, DiagnosticWindowError> {
        self.prepare_for_write(now_ms)?;
        let index = source_index(source);
        let last_epoch = match self.sources[index] {
            SourceConnectionState::Disconnected { last_epoch } => last_epoch,
            SourceConnectionState::Connected { .. } => {
                return Err(DiagnosticWindowError::SourceAlreadyConnected);
            }
        };
        let epoch = last_epoch
            .checked_add(1)
            .ok_or(DiagnosticWindowError::CollectorUnavailable)?;
        let active_after = self
            .active_source_count()
            .checked_add(1)
            .ok_or(DiagnosticWindowError::CollectorUnavailable)?;
        self.ensure_room(1, active_after, now_ms)?;
        let offset_ms = self.offset_ms(now_ms)?;
        let record = SourceRecord::new(source, epoch, 0, offset_ms, DiagnosticEvent::Started)
            .map_err(|_| DiagnosticWindowError::CollectorUnavailable)?;
        self.collector
            .push(record)
            .map_err(|_| DiagnosticWindowError::CollectorUnavailable)?;
        self.sources[index] = SourceConnectionState::Connected { epoch };
        self.last_now_ms = now_ms;
        Ok(DiagnosticSession {
            instance_id: self.instance_id,
            source,
            epoch,
        })
    }

    fn submit_inner(
        &mut self,
        session: DiagnosticSession,
        now_ms: u64,
        event: DiagnosticEvent,
    ) -> Result<(), SubmitFailure> {
        self.prepare_for_write(now_ms)
            .map_err(SubmitFailure::from_window_error)?;
        self.ensure_room(1, self.active_source_count(), now_ms)
            .map_err(SubmitFailure::from_window_error)?;
        let next_sequence = match self.collector.source_status(session.source) {
            DiagnosticSourceStatus::Active {
                epoch,
                next_sequence,
            } if epoch == session.epoch => next_sequence,
            _ => return Err(SubmitFailure::CollectorUnavailable),
        };
        let offset_ms = self
            .offset_ms(now_ms)
            .map_err(SubmitFailure::from_window_error)?;
        let record = SourceRecord::new(
            session.source,
            session.epoch,
            next_sequence,
            offset_ms,
            event,
        )
        .map_err(SubmitFailure::from_collector_error)?;
        self.collector
            .push(record)
            .map_err(SubmitFailure::from_collector_error)?;
        self.last_now_ms = now_ms;
        Ok(())
    }

    fn disconnect_inner(
        &mut self,
        session: DiagnosticSession,
        now_ms: u64,
    ) -> Result<(), DiagnosticWindowError> {
        self.prepare_for_write(now_ms)?;
        let next_sequence = match self.collector.source_status(session.source) {
            DiagnosticSourceStatus::Active {
                epoch,
                next_sequence,
            } if epoch == session.epoch => next_sequence,
            _ => return Err(DiagnosticWindowError::CollectorUnavailable),
        };
        let offset_ms = self.offset_ms(now_ms)?;
        let record = SourceRecord::new(
            session.source,
            session.epoch,
            next_sequence,
            offset_ms,
            DiagnosticEvent::Stopped,
        )
        .map_err(|_| DiagnosticWindowError::CollectorUnavailable)?;
        self.collector
            .push(record)
            .map_err(|_| DiagnosticWindowError::CollectorUnavailable)?;
        self.sources[source_index(session.source)] = SourceConnectionState::Disconnected {
            last_epoch: session.epoch,
        };
        self.last_now_ms = now_ms;
        Ok(())
    }

    fn mark_incomplete_disconnect(
        &mut self,
        session: DiagnosticSession,
        now_ms: u64,
        reason: LifecycleGapReason,
    ) -> DisconnectOutcome {
        self.sources[source_index(session.source)] = SourceConnectionState::Disconnected {
            last_epoch: session.epoch,
        };
        let reason = match self.omitted_lifecycle_events.checked_add(1) {
            Some(count) => {
                self.omitted_lifecycle_events = count;
                reason
            }
            None => {
                self.omitted_lifecycle_events = u8::MAX;
                LifecycleGapReason::CounterSaturated
            }
        };
        if now_ms >= self.last_now_ms {
            self.last_now_ms = now_ms;
        }
        DisconnectOutcome::Incomplete { reason }
    }

    fn prepare_for_write(&mut self, now_ms: u64) -> Result<(), DiagnosticWindowError> {
        if self.needs_rotation(now_ms) {
            self.rotate(now_ms)?;
        }
        Ok(())
    }

    fn ensure_room(
        &mut self,
        additional_records: usize,
        active_after: usize,
        now_ms: u64,
    ) -> Result<(), DiagnosticWindowError> {
        if self.has_room(additional_records, active_after) {
            return Ok(());
        }
        self.rotate(now_ms)?;
        if self.has_room(additional_records, active_after) {
            Ok(())
        } else {
            Err(DiagnosticWindowError::CollectorUnavailable)
        }
    }

    fn has_room(&self, additional_records: usize, active_after: usize) -> bool {
        self.collector
            .record_count()
            .checked_add(additional_records)
            .and_then(|count| count.checked_add(active_after))
            .is_some_and(|count| count <= MAX_RECORDS)
    }

    fn needs_rotation(&self, now_ms: u64) -> bool {
        self.omitted_lifecycle_events != 0
            || now_ms.saturating_sub(self.live_origin_ms) > u64::from(MAX_WINDOW_MS)
    }

    fn rotate(&mut self, now_ms: u64) -> Result<bool, DiagnosticWindowError> {
        if self.collector.record_count() == 0 {
            self.live_origin_ms = now_ms;
            self.omitted_lifecycle_events = 0;
            return Ok(false);
        }
        if self.sealed.is_some() {
            return Err(DiagnosticWindowError::Backpressure);
        }
        let next_window_id = self
            .live_window_id
            .checked_add(1)
            .ok_or(DiagnosticWindowError::CollectorUnavailable)?;
        let bundle = self
            .collector
            .export()
            .map_err(|_| DiagnosticWindowError::CollectorUnavailable)?;
        let sealed = DiagnosticWindowSnapshot {
            window_id: self.live_window_id,
            bundle,
            omitted_lifecycle_events: self.omitted_lifecycle_events,
        };
        let mut collector = DiagnosticCollector::new();
        for source in SOURCES {
            if let SourceConnectionState::Connected { epoch } = self.sources[source_index(source)] {
                let record = SourceRecord::new(source, epoch, 0, 0, DiagnosticEvent::Started)
                    .map_err(|_| DiagnosticWindowError::CollectorUnavailable)?;
                collector
                    .push(record)
                    .map_err(|_| DiagnosticWindowError::CollectorUnavailable)?;
            }
        }
        self.sealed = Some(sealed);
        self.live_window_id = next_window_id;
        self.live_origin_ms = now_ms;
        self.collector = collector;
        self.omitted_lifecycle_events = 0;
        Ok(true)
    }

    fn offset_ms(&self, now_ms: u64) -> Result<u32, DiagnosticWindowError> {
        let elapsed = now_ms
            .checked_sub(self.live_origin_ms)
            .ok_or(DiagnosticWindowError::ClockMovedBackwards)?;
        if elapsed > u64::from(MAX_WINDOW_MS) {
            return Err(DiagnosticWindowError::CollectorUnavailable);
        }
        u32::try_from(elapsed).map_err(|_| DiagnosticWindowError::CollectorUnavailable)
    }

    const fn validate_now(&self, now_ms: u64) -> Result<(), DiagnosticWindowError> {
        if now_ms < self.last_now_ms {
            Err(DiagnosticWindowError::ClockMovedBackwards)
        } else {
            Ok(())
        }
    }

    const fn validate_session(
        &self,
        session: DiagnosticSession,
    ) -> Result<(), DiagnosticWindowError> {
        if session.instance_id != self.instance_id {
            return Err(DiagnosticWindowError::SessionMismatch);
        }
        match self.sources[source_index(session.source)] {
            SourceConnectionState::Connected { epoch } if epoch == session.epoch => Ok(()),
            _ => Err(DiagnosticWindowError::SessionMismatch),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubmitFailure {
    EventNotAllowed,
    Backpressure,
    CollectorUnavailable,
}

impl SubmitFailure {
    const fn from_window_error(error: DiagnosticWindowError) -> Self {
        match error {
            DiagnosticWindowError::Backpressure => Self::Backpressure,
            _ => Self::CollectorUnavailable,
        }
    }

    const fn from_collector_error(error: DiagnosticCollectorError) -> Self {
        match error {
            DiagnosticCollectorError::EventNotAllowed
            | DiagnosticCollectorError::Export(DiagnosticExportError::InvalidEvent) => {
                Self::EventNotAllowed
            }
            _ => Self::CollectorUnavailable,
        }
    }
}

const fn rejected(reason: DiagnosticProducerRejection) -> DiagnosticProducerResponse {
    DiagnosticProducerResponse::Rejected { reason }
}

const fn source_index(source: DiagnosticSource) -> usize {
    match source {
        DiagnosticSource::Server => 0,
        DiagnosticSource::PlatformBroker => 1,
        DiagnosticSource::VideoProxy => 2,
        DiagnosticSource::ControlBroker => 3,
        DiagnosticSource::UpdateSupervisor => 4,
    }
}

#[cfg(test)]
mod tests {
    use bli_kvm_diagnostic_export::{PressureLevel, RejectReason, ResourceKind};

    use super::*;

    fn manager() -> DiagnosticWindowManager {
        DiagnosticWindowManager::new(7, 0).unwrap()
    }

    fn accepted() -> DiagnosticProducerResponse {
        DiagnosticProducerResponse::Accepted
    }

    #[test]
    fn initial_state_and_instance_binding_are_exact() {
        assert_eq!(
            DiagnosticWindowManager::new(0, 0),
            Err(DiagnosticWindowError::InvalidInstanceId)
        );
        let manager = manager();
        assert_eq!(manager.live_window_id(), 1);
        assert_eq!(manager.live_record_count(), 0);
        assert_eq!(manager.active_source_count(), 0);
        assert!(manager.sealed_window().is_none());
        for source in SOURCES {
            assert_eq!(
                manager.source_status(source),
                DiagnosticRuntimeSourceStatus::Disconnected { last_epoch: 0 }
            );
        }
    }

    #[test]
    fn connect_disconnect_reconnect_and_stale_session_are_fail_closed() {
        let mut manager = manager();
        let first = manager.connect(DiagnosticSource::Server, 0).unwrap();
        assert_eq!(
            manager.source_status(DiagnosticSource::Server),
            DiagnosticRuntimeSourceStatus::Connected { epoch: 1 }
        );
        assert_eq!(manager.live_record_count(), 1);
        assert!(matches!(
            manager.connect(DiagnosticSource::Server, 0),
            Err(DiagnosticWindowError::SourceAlreadyConnected)
        ));
        assert_eq!(
            manager.disconnect(first, 1),
            Ok(DisconnectOutcome::Recorded)
        );
        assert_eq!(
            manager.source_status(DiagnosticSource::Server),
            DiagnosticRuntimeSourceStatus::Disconnected { last_epoch: 1 }
        );
        let second = manager.connect(DiagnosticSource::Server, 2).unwrap();
        assert_eq!(second.epoch, 2);
        assert_eq!(
            manager.submit(first, 3, DiagnosticEvent::Ready),
            Err(DiagnosticWindowError::SessionMismatch)
        );
        assert_eq!(
            manager.submit(second, 3, DiagnosticEvent::Ready),
            Ok(accepted())
        );
    }

    #[test]
    fn accepted_events_hide_session_guards_and_rejections_do_not_mutate() {
        let mut manager = manager();
        let session = manager.connect(DiagnosticSource::VideoProxy, 0).unwrap();
        assert_eq!(
            manager.submit(session, 1, DiagnosticEvent::Ready),
            Ok(accepted())
        );
        let before = manager.clone();
        assert_eq!(
            manager.submit(
                session,
                2,
                DiagnosticEvent::ResourcePressure {
                    resource: ResourceKind::Temperature,
                    level: PressureLevel::Critical,
                },
            ),
            Ok(rejected(DiagnosticProducerRejection::EventNotAllowed))
        );
        assert_eq!(manager, before);
        assert_eq!(
            manager.submit(session, 2, DiagnosticEvent::Started),
            Ok(rejected(DiagnosticProducerRejection::EventNotAllowed))
        );
        assert_eq!(manager, before);
        let snapshot = manager.live_snapshot().unwrap();
        let output = core::str::from_utf8(snapshot.bundle().as_bytes()).unwrap();
        for forbidden in ["epoch", "sequence", "instance", "source", "uid", "pid"] {
            assert!(!output.contains(forbidden));
        }
    }

    #[test]
    fn clock_rollback_is_fatal_for_events_but_disconnect_is_visible_and_releases_peer() {
        let mut manager = DiagnosticWindowManager::new(7, 10).unwrap();
        let session = manager.connect(DiagnosticSource::Server, 10).unwrap();
        let before = manager.clone();
        assert_eq!(
            manager.submit(session, 9, DiagnosticEvent::Ready),
            Err(DiagnosticWindowError::ClockMovedBackwards)
        );
        assert_eq!(manager, before);
        assert_eq!(
            manager.disconnect(session, 9),
            Ok(DisconnectOutcome::Incomplete {
                reason: LifecycleGapReason::ClockMovedBackwards
            })
        );
        assert_eq!(manager.live_omitted_lifecycle_events(), 1);
        assert_eq!(
            manager.source_status(DiagnosticSource::Server),
            DiagnosticRuntimeSourceStatus::Disconnected { last_epoch: 1 }
        );
        assert_eq!(manager.tick(10), Ok(WindowTickOutcome::Rotated));
        assert_eq!(
            manager.sealed_window().unwrap().omitted_lifecycle_events(),
            1
        );
        assert!(!manager.sealed_window().unwrap().is_complete());
    }

    #[test]
    fn five_active_sources_reserve_stops_and_rotate_atomically_at_capacity() {
        let mut manager = manager();
        let mut sessions = [None; DIAGNOSTIC_SOURCE_COUNT];
        for (index, source) in SOURCES.into_iter().enumerate() {
            sessions[index] = Some(manager.connect(source, index as u64).unwrap());
        }
        assert_eq!(manager.live_record_count(), 5);
        assert_eq!(manager.active_source_count(), 5);
        let server = sessions[0].unwrap();
        let mut now = 5;
        while manager.live_record_count() < MAX_RECORDS - manager.active_source_count() {
            assert_eq!(
                manager.submit(server, now, DiagnosticEvent::Ready),
                Ok(accepted())
            );
            now += 1;
        }
        assert_eq!(manager.live_record_count(), 27);
        assert_eq!(
            manager.submit(server, now, DiagnosticEvent::Ready),
            Ok(accepted())
        );
        now += 1;
        assert_eq!(manager.sealed_window().unwrap().window_id(), 1);
        assert_eq!(manager.sealed_window().unwrap().bundle().record_count(), 27);
        assert_eq!(manager.live_window_id(), 2);
        assert_eq!(manager.live_record_count(), 6);
        while manager.live_record_count() < MAX_RECORDS - manager.active_source_count() {
            assert_eq!(
                manager.submit(server, now, DiagnosticEvent::Ready),
                Ok(accepted())
            );
            now += 1;
        }
        let full = manager.clone();
        assert_eq!(
            manager.submit(server, now, DiagnosticEvent::Ready),
            Ok(rejected(DiagnosticProducerRejection::Backpressure))
        );
        assert_eq!(manager, full);
        for session in sessions.into_iter().flatten() {
            assert_eq!(
                manager.disconnect(session, now),
                Ok(DisconnectOutcome::Recorded)
            );
        }
        assert_eq!(manager.live_record_count(), MAX_RECORDS);
        assert_eq!(manager.active_source_count(), 0);
    }

    #[test]
    fn sealed_acknowledgement_is_exact_and_frees_one_future_rotation() {
        let mut manager = manager();
        let session = manager.connect(DiagnosticSource::Server, 0).unwrap();
        assert_eq!(
            manager.tick(u64::from(MAX_WINDOW_MS) + 1),
            Ok(WindowTickOutcome::Rotated)
        );
        assert_eq!(manager.sealed_window().unwrap().window_id(), 1);
        let before = manager.clone();
        assert_eq!(
            manager.acknowledge_sealed(2),
            Err(DiagnosticWindowError::AcknowledgementMismatch)
        );
        assert_eq!(manager, before);
        assert_eq!(manager.acknowledge_sealed(1), Ok(()));
        assert!(manager.sealed_window().is_none());
        assert_eq!(
            manager.acknowledge_sealed(1),
            Err(DiagnosticWindowError::NoSealedWindow)
        );
        assert_eq!(
            manager.submit(
                session,
                u64::from(MAX_WINDOW_MS) + 2,
                DiagnosticEvent::Ready
            ),
            Ok(accepted())
        );
    }

    #[test]
    fn exact_maximum_age_is_valid_and_next_tick_backpressures_with_pending_seal() {
        let mut manager = manager();
        manager.connect(DiagnosticSource::Server, 0).unwrap();
        assert_eq!(
            manager.tick(u64::from(MAX_WINDOW_MS)),
            Ok(WindowTickOutcome::Unchanged)
        );
        assert_eq!(
            manager.tick(u64::from(MAX_WINDOW_MS) + 1),
            Ok(WindowTickOutcome::Rotated)
        );
        assert_eq!(
            manager.tick(2 * u64::from(MAX_WINDOW_MS) + 2),
            Ok(WindowTickOutcome::Backpressured)
        );
    }

    #[test]
    fn overdue_disconnect_with_pending_seal_marks_gap_and_never_strands_source() {
        let mut manager = manager();
        let session = manager.connect(DiagnosticSource::Server, 0).unwrap();
        assert_eq!(
            manager.tick(u64::from(MAX_WINDOW_MS) + 1),
            Ok(WindowTickOutcome::Rotated)
        );
        let now = 2 * u64::from(MAX_WINDOW_MS) + 2;
        assert_eq!(
            manager.disconnect(session, now),
            Ok(DisconnectOutcome::Incomplete {
                reason: LifecycleGapReason::Backpressure
            })
        );
        assert_eq!(manager.active_source_count(), 0);
        assert_eq!(manager.live_omitted_lifecycle_events(), 1);
        assert!(matches!(
            manager.connect(DiagnosticSource::Server, now),
            Err(DiagnosticWindowError::Backpressure)
        ));
        assert_eq!(manager.acknowledge_sealed(1), Ok(()));
        assert_eq!(manager.tick(now), Ok(WindowTickOutcome::Rotated));
        let sealed = manager.sealed_window().unwrap();
        assert_eq!(sealed.window_id(), 2);
        assert_eq!(sealed.omitted_lifecycle_events(), 1);
        assert!(!sealed.is_complete());
        assert_eq!(manager.live_window_id(), 3);
    }

    #[test]
    fn window_and_source_counter_exhaustion_are_explicit() {
        let mut window_exhausted = manager();
        let session = window_exhausted
            .connect(DiagnosticSource::Server, 0)
            .unwrap();
        window_exhausted.live_window_id = u32::MAX;
        let before = window_exhausted.clone();
        assert_eq!(
            window_exhausted.submit(
                session,
                u64::from(MAX_WINDOW_MS) + 1,
                DiagnosticEvent::Ready,
            ),
            Ok(rejected(DiagnosticProducerRejection::CollectorUnavailable))
        );
        assert_eq!(window_exhausted, before);

        let mut source_exhausted = manager();
        source_exhausted.sources[source_index(DiagnosticSource::Server)] =
            SourceConnectionState::Disconnected {
                last_epoch: u32::MAX,
            };
        assert!(matches!(
            source_exhausted.connect(DiagnosticSource::Server, 0),
            Err(DiagnosticWindowError::CollectorUnavailable)
        ));
    }

    #[test]
    fn manager_restart_and_instance_tokens_never_cross() {
        let mut first = DiagnosticWindowManager::new(1, 0).unwrap();
        let token = first.connect(DiagnosticSource::Server, 0).unwrap();
        let mut second = DiagnosticWindowManager::new(2, 0).unwrap();
        second.connect(DiagnosticSource::Server, 0).unwrap();
        assert_eq!(
            second.submit(token, 1, DiagnosticEvent::Ready),
            Err(DiagnosticWindowError::SessionMismatch)
        );
        assert_eq!(second.live_window_id(), 1);
        assert!(second.sealed_window().is_none());
    }

    #[test]
    fn snapshots_are_deterministic_for_identical_collector_owned_inputs() {
        let mut first = manager();
        let mut second = manager();
        for manager in [&mut first, &mut second] {
            let session = manager.connect(DiagnosticSource::Server, 0).unwrap();
            assert_eq!(
                manager.submit(
                    session,
                    1,
                    DiagnosticEvent::RequestRejected {
                        reason: RejectReason::RateLimited,
                    },
                ),
                Ok(accepted())
            );
        }
        assert_eq!(first.live_snapshot(), second.live_snapshot());
    }
}
