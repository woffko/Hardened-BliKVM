#![no_std]
#![forbid(unsafe_code)]

//! Pure typed diagnostics export with no raw log-message surface.
//!
//! Callers can record only fixed semantic components, events, reasons, results,
//! and bounded counters. The crate accepts no path, unit name, hostname,
//! address, endpoint, username, request data, arbitrary key, or free-form text.
//! It performs no filesystem, process, clock, network, or device I/O.

pub const EXPORT_SCHEMA_VERSION: u16 = 1;
pub const MAX_RECORDS: usize = 32;
pub const MAX_EXPORT_BYTES: usize = 8 * 1024;
pub const MAX_WINDOW_MS: u32 = 24 * 60 * 60 * 1000;
pub const MAX_RETRY_ATTEMPT: u8 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticComponent {
    Server,
    PlatformBroker,
    VideoProxy,
    ControlBroker,
    UpdateSupervisor,
    SystemActionStore,
    NetworkPolicy,
    RecoveryVerifier,
}

impl DiagnosticComponent {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::PlatformBroker => "platform_broker",
            Self::VideoProxy => "video_proxy",
            Self::ControlBroker => "control_broker",
            Self::UpdateSupervisor => "update_supervisor",
            Self::SystemActionStore => "system_action_store",
            Self::NetworkPolicy => "network_policy",
            Self::RecoveryVerifier => "recovery_verifier",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RejectReason {
    Unauthorized,
    CapabilityDisabled,
    BackendUnavailable,
    Busy,
    InvalidInput,
    RateLimited,
    Conflict,
}

impl RejectReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::CapabilityDisabled => "capability_disabled",
            Self::BackendUnavailable => "backend_unavailable",
            Self::Busy => "busy",
            Self::InvalidInput => "invalid_input",
            Self::RateLimited => "rate_limited",
            Self::Conflict => "conflict",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperationResult {
    Applied,
    Rejected,
    Noop,
    RolledBack,
}

impl OperationResult {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Rejected => "rejected",
            Self::Noop => "noop",
            Self::RolledBack => "rolled_back",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    Memory,
    Storage,
    Temperature,
    Sessions,
}

impl ResourceKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Storage => "storage",
            Self::Temperature => "temperature",
            Self::Sessions => "sessions",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PressureLevel {
    Elevated,
    Critical,
}

impl PressureLevel {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Elevated => "elevated",
            Self::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JournalHealth {
    Healthy,
    Degraded,
    RecoveryRequired,
}

impl JournalHealth {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::RecoveryRequired => "recovery_required",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticEvent {
    Started,
    Ready,
    Stopped,
    Unavailable,
    RequestRejected {
        reason: RejectReason,
    },
    RetryScheduled {
        attempt: u8,
    },
    OperationFinished {
        result: OperationResult,
    },
    ResourcePressure {
        resource: ResourceKind,
        level: PressureLevel,
    },
    JournalObserved {
        health: JournalHealth,
    },
}

impl DiagnosticEvent {
    const fn validate(self) -> Result<(), DiagnosticExportError> {
        match self {
            Self::RetryScheduled { attempt } if attempt == 0 || attempt > MAX_RETRY_ATTEMPT => {
                Err(DiagnosticExportError::InvalidEvent)
            }
            _ => Ok(()),
        }
    }

    const fn severity(self) -> DiagnosticSeverity {
        match self {
            Self::Started | Self::Ready => DiagnosticSeverity::Info,
            Self::Stopped
            | Self::RequestRejected { .. }
            | Self::RetryScheduled { .. }
            | Self::OperationFinished {
                result: OperationResult::Rejected | OperationResult::RolledBack,
            }
            | Self::ResourcePressure {
                level: PressureLevel::Elevated,
                ..
            }
            | Self::JournalObserved {
                health: JournalHealth::Degraded,
            } => DiagnosticSeverity::Warning,
            Self::Unavailable
            | Self::ResourcePressure {
                level: PressureLevel::Critical,
                ..
            }
            | Self::JournalObserved {
                health: JournalHealth::RecoveryRequired,
            } => DiagnosticSeverity::Error,
            Self::OperationFinished {
                result: OperationResult::Applied | OperationResult::Noop,
            }
            | Self::JournalObserved {
                health: JournalHealth::Healthy,
            } => DiagnosticSeverity::Info,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

impl DiagnosticSeverity {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DiagnosticRecord {
    offset_ms: u32,
    component: DiagnosticComponent,
    event: DiagnosticEvent,
}

impl DiagnosticRecord {
    pub const fn new(
        offset_ms: u32,
        component: DiagnosticComponent,
        event: DiagnosticEvent,
    ) -> Result<Self, DiagnosticExportError> {
        if offset_ms > MAX_WINDOW_MS {
            return Err(DiagnosticExportError::InvalidTime);
        }
        match event.validate() {
            Ok(()) => Ok(Self {
                offset_ms,
                component,
                event,
            }),
            Err(error) => Err(error),
        }
    }

    pub const fn offset_ms(self) -> u32 {
        self.offset_ms
    }

    pub const fn component(self) -> DiagnosticComponent {
        self.component
    }

    pub const fn event(self) -> DiagnosticEvent {
        self.event
    }

    pub const fn severity(self) -> DiagnosticSeverity {
        self.event.severity()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticExportError {
    InvalidTime,
    InvalidEvent,
    InvalidBundle,
    ClockMovedBackwards,
    CapacityExceeded,
    BufferExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticExporter {
    records: [Option<DiagnosticRecord>; MAX_RECORDS],
    count: usize,
    last_offset_ms: Option<u32>,
}

impl DiagnosticExporter {
    pub const fn new() -> Self {
        Self {
            records: [None; MAX_RECORDS],
            count: 0,
            last_offset_ms: None,
        }
    }

    pub const fn record_count(&self) -> usize {
        self.count
    }

    pub const fn last_offset_ms(&self) -> Option<u32> {
        self.last_offset_ms
    }

    pub fn push(&mut self, record: DiagnosticRecord) -> Result<(), DiagnosticExportError> {
        if self
            .last_offset_ms
            .is_some_and(|previous| record.offset_ms < previous)
        {
            return Err(DiagnosticExportError::ClockMovedBackwards);
        }
        if self.count == MAX_RECORDS {
            return Err(DiagnosticExportError::CapacityExceeded);
        }
        self.records[self.count] = Some(record);
        self.count += 1;
        self.last_offset_ms = Some(record.offset_ms);
        Ok(())
    }

    pub fn export(&self) -> Result<DiagnosticBundle, DiagnosticExportError> {
        let mut writer = ExportWriter::new();
        writer.write_str("schema=")?;
        writer.write_u32(u32::from(EXPORT_SCHEMA_VERSION))?;
        writer.write_str(" record_count=")?;
        writer.write_u32(self.count as u32)?;
        writer.write_byte(b'\n')?;
        for record in self.records.iter().take(self.count).flatten() {
            encode_record(&mut writer, *record)?;
        }
        Ok(writer.finish(self.count))
    }
}

impl Default for DiagnosticExporter {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticBundle {
    bytes: [u8; MAX_EXPORT_BYTES],
    length: usize,
    record_count: usize,
}

impl DiagnosticBundle {
    /// Decodes and re-encodes one exact canonical typed bundle.
    ///
    /// Parsing allocates nothing. Every field is converted back into the
    /// closed semantic enums and the resulting bundle must be byte-identical
    /// to the input, which rejects alternate numeric, whitespace, ordering,
    /// severity, and trailing-data representations.
    pub fn decode(bytes: &[u8]) -> Result<Self, DiagnosticExportError> {
        if bytes.len() > MAX_EXPORT_BYTES || !bytes.ends_with(b"\n") {
            return Err(DiagnosticExportError::InvalidBundle);
        }
        let text = core::str::from_utf8(bytes).map_err(|_| DiagnosticExportError::InvalidBundle)?;
        let mut lines = text.split_terminator('\n');
        let declared_count = lines
            .next()
            .and_then(|header| header.strip_prefix("schema=1 record_count="))
            .and_then(parse_u32)
            .and_then(|count| usize::try_from(count).ok())
            .filter(|count| *count <= MAX_RECORDS)
            .ok_or(DiagnosticExportError::InvalidBundle)?;

        let mut exporter = DiagnosticExporter::new();
        for line in lines {
            let record = parse_record(line)?;
            exporter
                .push(record)
                .map_err(|_| DiagnosticExportError::InvalidBundle)?;
        }
        if exporter.record_count() != declared_count {
            return Err(DiagnosticExportError::InvalidBundle);
        }
        let bundle = exporter
            .export()
            .map_err(|_| DiagnosticExportError::InvalidBundle)?;
        if bundle.as_bytes() != bytes {
            return Err(DiagnosticExportError::InvalidBundle);
        }
        Ok(bundle)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }

    pub const fn length(&self) -> usize {
        self.length
    }

    pub const fn record_count(&self) -> usize {
        self.record_count
    }
}

fn parse_record(line: &str) -> Result<DiagnosticRecord, DiagnosticExportError> {
    let mut fields = line.split(' ');
    let offset_ms = parse_field(&mut fields, "offset_ms=")
        .and_then(parse_u32)
        .ok_or(DiagnosticExportError::InvalidBundle)?;
    let severity = parse_field(&mut fields, "severity=")
        .and_then(parse_severity)
        .ok_or(DiagnosticExportError::InvalidBundle)?;
    let component = parse_field(&mut fields, "component=")
        .and_then(parse_component)
        .ok_or(DiagnosticExportError::InvalidBundle)?;
    let event_name =
        parse_field(&mut fields, "event=").ok_or(DiagnosticExportError::InvalidBundle)?;
    let event = match event_name {
        "started" => DiagnosticEvent::Started,
        "ready" => DiagnosticEvent::Ready,
        "stopped" => DiagnosticEvent::Stopped,
        "unavailable" => DiagnosticEvent::Unavailable,
        "request_rejected" => DiagnosticEvent::RequestRejected {
            reason: parse_field(&mut fields, "reason=")
                .and_then(parse_reject_reason)
                .ok_or(DiagnosticExportError::InvalidBundle)?,
        },
        "retry_scheduled" => DiagnosticEvent::RetryScheduled {
            attempt: parse_field(&mut fields, "attempt=")
                .and_then(parse_u8)
                .ok_or(DiagnosticExportError::InvalidBundle)?,
        },
        "operation_finished" => DiagnosticEvent::OperationFinished {
            result: parse_field(&mut fields, "result=")
                .and_then(parse_operation_result)
                .ok_or(DiagnosticExportError::InvalidBundle)?,
        },
        "resource_pressure" => DiagnosticEvent::ResourcePressure {
            resource: parse_field(&mut fields, "resource=")
                .and_then(parse_resource_kind)
                .ok_or(DiagnosticExportError::InvalidBundle)?,
            level: parse_field(&mut fields, "level=")
                .and_then(parse_pressure_level)
                .ok_or(DiagnosticExportError::InvalidBundle)?,
        },
        "journal_observed" => DiagnosticEvent::JournalObserved {
            health: parse_field(&mut fields, "health=")
                .and_then(parse_journal_health)
                .ok_or(DiagnosticExportError::InvalidBundle)?,
        },
        _ => return Err(DiagnosticExportError::InvalidBundle),
    };
    if fields.next().is_some() {
        return Err(DiagnosticExportError::InvalidBundle);
    }
    let record = DiagnosticRecord::new(offset_ms, component, event)
        .map_err(|_| DiagnosticExportError::InvalidBundle)?;
    if record.severity() != severity {
        return Err(DiagnosticExportError::InvalidBundle);
    }
    Ok(record)
}

fn parse_field<'a>(fields: &mut impl Iterator<Item = &'a str>, prefix: &str) -> Option<&'a str> {
    fields
        .next()?
        .strip_prefix(prefix)
        .filter(|value| !value.is_empty())
}

fn parse_u32(value: &str) -> Option<u32> {
    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        value.parse().ok()
    } else {
        None
    }
}

fn parse_u8(value: &str) -> Option<u8> {
    parse_u32(value).and_then(|value| u8::try_from(value).ok())
}

fn parse_component(value: &str) -> Option<DiagnosticComponent> {
    Some(match value {
        "server" => DiagnosticComponent::Server,
        "platform_broker" => DiagnosticComponent::PlatformBroker,
        "video_proxy" => DiagnosticComponent::VideoProxy,
        "control_broker" => DiagnosticComponent::ControlBroker,
        "update_supervisor" => DiagnosticComponent::UpdateSupervisor,
        "system_action_store" => DiagnosticComponent::SystemActionStore,
        "network_policy" => DiagnosticComponent::NetworkPolicy,
        "recovery_verifier" => DiagnosticComponent::RecoveryVerifier,
        _ => return None,
    })
}

fn parse_severity(value: &str) -> Option<DiagnosticSeverity> {
    Some(match value {
        "info" => DiagnosticSeverity::Info,
        "warning" => DiagnosticSeverity::Warning,
        "error" => DiagnosticSeverity::Error,
        _ => return None,
    })
}

fn parse_reject_reason(value: &str) -> Option<RejectReason> {
    Some(match value {
        "unauthorized" => RejectReason::Unauthorized,
        "capability_disabled" => RejectReason::CapabilityDisabled,
        "backend_unavailable" => RejectReason::BackendUnavailable,
        "busy" => RejectReason::Busy,
        "invalid_input" => RejectReason::InvalidInput,
        "rate_limited" => RejectReason::RateLimited,
        "conflict" => RejectReason::Conflict,
        _ => return None,
    })
}

fn parse_operation_result(value: &str) -> Option<OperationResult> {
    Some(match value {
        "applied" => OperationResult::Applied,
        "rejected" => OperationResult::Rejected,
        "noop" => OperationResult::Noop,
        "rolled_back" => OperationResult::RolledBack,
        _ => return None,
    })
}

fn parse_resource_kind(value: &str) -> Option<ResourceKind> {
    Some(match value {
        "memory" => ResourceKind::Memory,
        "storage" => ResourceKind::Storage,
        "temperature" => ResourceKind::Temperature,
        "sessions" => ResourceKind::Sessions,
        _ => return None,
    })
}

fn parse_pressure_level(value: &str) -> Option<PressureLevel> {
    Some(match value {
        "elevated" => PressureLevel::Elevated,
        "critical" => PressureLevel::Critical,
        _ => return None,
    })
}

fn parse_journal_health(value: &str) -> Option<JournalHealth> {
    Some(match value {
        "healthy" => JournalHealth::Healthy,
        "degraded" => JournalHealth::Degraded,
        "recovery_required" => JournalHealth::RecoveryRequired,
        _ => return None,
    })
}

fn encode_record(
    writer: &mut ExportWriter,
    record: DiagnosticRecord,
) -> Result<(), DiagnosticExportError> {
    writer.write_str("offset_ms=")?;
    writer.write_u32(record.offset_ms)?;
    writer.write_str(" severity=")?;
    writer.write_str(record.severity().as_str())?;
    writer.write_str(" component=")?;
    writer.write_str(record.component.as_str())?;
    match record.event {
        DiagnosticEvent::Started => writer.write_str(" event=started")?,
        DiagnosticEvent::Ready => writer.write_str(" event=ready")?,
        DiagnosticEvent::Stopped => writer.write_str(" event=stopped")?,
        DiagnosticEvent::Unavailable => writer.write_str(" event=unavailable")?,
        DiagnosticEvent::RequestRejected { reason } => {
            writer.write_str(" event=request_rejected reason=")?;
            writer.write_str(reason.as_str())?;
        }
        DiagnosticEvent::RetryScheduled { attempt } => {
            writer.write_str(" event=retry_scheduled attempt=")?;
            writer.write_u32(u32::from(attempt))?;
        }
        DiagnosticEvent::OperationFinished { result } => {
            writer.write_str(" event=operation_finished result=")?;
            writer.write_str(result.as_str())?;
        }
        DiagnosticEvent::ResourcePressure { resource, level } => {
            writer.write_str(" event=resource_pressure resource=")?;
            writer.write_str(resource.as_str())?;
            writer.write_str(" level=")?;
            writer.write_str(level.as_str())?;
        }
        DiagnosticEvent::JournalObserved { health } => {
            writer.write_str(" event=journal_observed health=")?;
            writer.write_str(health.as_str())?;
        }
    }
    writer.write_byte(b'\n')
}

struct ExportWriter {
    bytes: [u8; MAX_EXPORT_BYTES],
    length: usize,
}

impl ExportWriter {
    const fn new() -> Self {
        Self {
            bytes: [0; MAX_EXPORT_BYTES],
            length: 0,
        }
    }

    fn write_byte(&mut self, byte: u8) -> Result<(), DiagnosticExportError> {
        if self.length == self.bytes.len() {
            return Err(DiagnosticExportError::BufferExceeded);
        }
        self.bytes[self.length] = byte;
        self.length += 1;
        Ok(())
    }

    fn write_str(&mut self, value: &str) -> Result<(), DiagnosticExportError> {
        for byte in value.bytes() {
            self.write_byte(byte)?;
        }
        Ok(())
    }

    fn write_u32(&mut self, mut value: u32) -> Result<(), DiagnosticExportError> {
        let mut digits = [0u8; 10];
        let mut position = digits.len();
        loop {
            position -= 1;
            digits[position] = b'0' + (value % 10) as u8;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        for digit in &digits[position..] {
            self.write_byte(*digit)?;
        }
        Ok(())
    }

    fn finish(self, record_count: usize) -> DiagnosticBundle {
        DiagnosticBundle {
            bytes: self.bytes,
            length: self.length,
            record_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(offset_ms: u32, event: DiagnosticEvent) -> DiagnosticRecord {
        DiagnosticRecord::new(offset_ms, DiagnosticComponent::Server, event).unwrap()
    }

    #[test]
    fn exact_export_contains_only_typed_fields() {
        let mut exporter = DiagnosticExporter::new();
        exporter.push(record(0, DiagnosticEvent::Started)).unwrap();
        exporter
            .push(record(
                25,
                DiagnosticEvent::RequestRejected {
                    reason: RejectReason::CapabilityDisabled,
                },
            ))
            .unwrap();
        exporter
            .push(record(
                30,
                DiagnosticEvent::OperationFinished {
                    result: OperationResult::RolledBack,
                },
            ))
            .unwrap();
        assert_eq!(
            core::str::from_utf8(exporter.export().unwrap().as_bytes()).unwrap(),
            "schema=1 record_count=3\n\
             offset_ms=0 severity=info component=server event=started\n\
             offset_ms=25 severity=warning component=server event=request_rejected reason=capability_disabled\n\
             offset_ms=30 severity=warning component=server event=operation_finished result=rolled_back\n"
        );
    }

    #[test]
    fn canonical_bundle_round_trips_without_allocation_or_loss() {
        let mut exporter = DiagnosticExporter::new();
        let events = [
            DiagnosticEvent::Started,
            DiagnosticEvent::Ready,
            DiagnosticEvent::Stopped,
            DiagnosticEvent::Unavailable,
            DiagnosticEvent::RequestRejected {
                reason: RejectReason::Unauthorized,
            },
            DiagnosticEvent::RetryScheduled { attempt: 4 },
            DiagnosticEvent::OperationFinished {
                result: OperationResult::Applied,
            },
            DiagnosticEvent::ResourcePressure {
                resource: ResourceKind::Storage,
                level: PressureLevel::Critical,
            },
            DiagnosticEvent::JournalObserved {
                health: JournalHealth::RecoveryRequired,
            },
        ];
        for (offset, event) in events.into_iter().enumerate() {
            exporter
                .push(
                    DiagnosticRecord::new(
                        offset as u32,
                        DiagnosticComponent::PlatformBroker,
                        event,
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        let expected = exporter.export().unwrap();
        assert_eq!(DiagnosticBundle::decode(expected.as_bytes()), Ok(expected));

        let empty = DiagnosticExporter::new().export().unwrap();
        assert_eq!(DiagnosticBundle::decode(empty.as_bytes()), Ok(empty));
    }

    #[test]
    fn bundle_decoder_rejects_every_noncanonical_dimension() {
        let valid = b"schema=1 record_count=1\n\
offset_ms=7 severity=warning component=control_broker event=request_rejected reason=capability_disabled\n";
        assert_eq!(DiagnosticBundle::decode(valid).unwrap().record_count(), 1);
        for invalid in [
            &b"schema=1 record_count=2\n\
offset_ms=7 severity=warning component=control_broker event=request_rejected reason=capability_disabled\n"[..],
            &b"schema=1 record_count=1\n\
offset_ms=07 severity=warning component=control_broker event=request_rejected reason=capability_disabled\n"[..],
            &b"schema=1 record_count=1\n\
offset_ms=7 severity=info component=control_broker event=request_rejected reason=capability_disabled\n"[..],
            &b"schema=1 record_count=1\n\
offset_ms=7 severity=warning component=control_broker event=request_rejected reason=capability_disabled extra=value\n"[..],
            &b"schema=1 record_count=1\n\
offset_ms=7 severity=warning component=control_broker event=unknown\n"[..],
            &valid[..valid.len() - 1],
            &b"schema=1 record_count=0\n\n"[..],
            &b"schema=1 record_count=0\n\xff"[..],
        ] {
            assert_eq!(
                DiagnosticBundle::decode(invalid),
                Err(DiagnosticExportError::InvalidBundle)
            );
        }
        let oversized = [b'x'; MAX_EXPORT_BYTES + 1];
        assert_eq!(
            DiagnosticBundle::decode(&oversized),
            Err(DiagnosticExportError::InvalidBundle)
        );
    }

    #[test]
    fn invalid_time_attempt_and_clock_rollback_do_not_mutate() {
        assert_eq!(
            DiagnosticRecord::new(
                MAX_WINDOW_MS + 1,
                DiagnosticComponent::Server,
                DiagnosticEvent::Ready,
            ),
            Err(DiagnosticExportError::InvalidTime)
        );
        assert_eq!(
            DiagnosticRecord::new(
                0,
                DiagnosticComponent::Server,
                DiagnosticEvent::RetryScheduled { attempt: 0 },
            ),
            Err(DiagnosticExportError::InvalidEvent)
        );
        let mut exporter = DiagnosticExporter::new();
        exporter.push(record(10, DiagnosticEvent::Ready)).unwrap();
        let before = exporter.clone();
        assert_eq!(
            exporter.push(record(9, DiagnosticEvent::Stopped)),
            Err(DiagnosticExportError::ClockMovedBackwards)
        );
        assert_eq!(exporter, before);
    }

    #[test]
    fn capacity_is_hard_and_never_silently_truncates() {
        let mut exporter = DiagnosticExporter::new();
        for offset in 0..MAX_RECORDS as u32 {
            exporter
                .push(record(offset, DiagnosticEvent::Ready))
                .unwrap();
        }
        let before = exporter.clone();
        assert_eq!(
            exporter.push(record(MAX_RECORDS as u32, DiagnosticEvent::Stopped)),
            Err(DiagnosticExportError::CapacityExceeded)
        );
        assert_eq!(exporter, before);
        assert_eq!(exporter.export().unwrap().record_count(), MAX_RECORDS);
    }

    #[test]
    fn every_enum_variant_encodes_as_ascii_without_identity_fields() {
        let components = [
            DiagnosticComponent::Server,
            DiagnosticComponent::PlatformBroker,
            DiagnosticComponent::VideoProxy,
            DiagnosticComponent::ControlBroker,
            DiagnosticComponent::UpdateSupervisor,
            DiagnosticComponent::SystemActionStore,
            DiagnosticComponent::NetworkPolicy,
            DiagnosticComponent::RecoveryVerifier,
        ];
        let mut exporter = DiagnosticExporter::new();
        let mut offset = 0;
        for component in components {
            exporter
                .push(DiagnosticRecord::new(offset, component, DiagnosticEvent::Started).unwrap())
                .unwrap();
            offset += 1;
        }
        for event in [
            DiagnosticEvent::Ready,
            DiagnosticEvent::Stopped,
            DiagnosticEvent::Unavailable,
            DiagnosticEvent::RetryScheduled { attempt: 1 },
            DiagnosticEvent::RetryScheduled {
                attempt: MAX_RETRY_ATTEMPT,
            },
        ] {
            exporter.push(record(offset, event)).unwrap();
            offset += 1;
        }
        for reason in [
            RejectReason::Unauthorized,
            RejectReason::CapabilityDisabled,
            RejectReason::BackendUnavailable,
            RejectReason::Busy,
            RejectReason::InvalidInput,
            RejectReason::RateLimited,
            RejectReason::Conflict,
        ] {
            exporter
                .push(record(offset, DiagnosticEvent::RequestRejected { reason }))
                .unwrap();
            offset += 1;
        }
        for result in [
            OperationResult::Applied,
            OperationResult::Rejected,
            OperationResult::Noop,
            OperationResult::RolledBack,
        ] {
            exporter
                .push(record(
                    offset,
                    DiagnosticEvent::OperationFinished { result },
                ))
                .unwrap();
            offset += 1;
        }
        for (resource, level) in [
            (ResourceKind::Memory, PressureLevel::Elevated),
            (ResourceKind::Storage, PressureLevel::Critical),
            (ResourceKind::Temperature, PressureLevel::Elevated),
            (ResourceKind::Sessions, PressureLevel::Critical),
        ] {
            exporter
                .push(record(
                    offset,
                    DiagnosticEvent::ResourcePressure { resource, level },
                ))
                .unwrap();
            offset += 1;
        }
        for health in [
            JournalHealth::Healthy,
            JournalHealth::Degraded,
            JournalHealth::RecoveryRequired,
        ] {
            exporter
                .push(record(offset, DiagnosticEvent::JournalObserved { health }))
                .unwrap();
            offset += 1;
        }
        let bundle = exporter.export().unwrap();
        assert_eq!(
            DiagnosticBundle::decode(bundle.as_bytes()),
            Ok(bundle.clone())
        );
        let text = core::str::from_utf8(bundle.as_bytes()).unwrap();
        assert_eq!(bundle.record_count(), 31);
        assert!(text.is_ascii());
        for forbidden in [
            "hostname",
            "username",
            "address",
            "endpoint",
            "token",
            "cookie",
            "authorization",
            "request_data",
            "path=",
        ] {
            assert!(!text.contains(forbidden));
        }
    }

    #[test]
    fn maximum_valid_window_and_long_records_fit_without_truncation() {
        let mut exporter = DiagnosticExporter::new();
        for _ in 0..MAX_RECORDS {
            exporter
                .push(
                    DiagnosticRecord::new(
                        MAX_WINDOW_MS,
                        DiagnosticComponent::SystemActionStore,
                        DiagnosticEvent::RequestRejected {
                            reason: RejectReason::CapabilityDisabled,
                        },
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        let bundle = exporter.export().unwrap();
        assert_eq!(bundle.record_count(), MAX_RECORDS);
        assert!(bundle.length() < MAX_EXPORT_BYTES);
        assert!(bundle.as_bytes().ends_with(b"\n"));
    }

    #[test]
    fn output_is_deterministic_and_unused_capacity_stays_private() {
        let mut first = DiagnosticExporter::new();
        let mut second = DiagnosticExporter::new();
        for exporter in [&mut first, &mut second] {
            exporter
                .push(
                    DiagnosticRecord::new(
                        5,
                        DiagnosticComponent::SystemActionStore,
                        DiagnosticEvent::JournalObserved {
                            health: JournalHealth::Healthy,
                        },
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        let first = first.export().unwrap();
        let second = second.export().unwrap();
        assert_eq!(first, second);
        assert!(first.length() < MAX_EXPORT_BYTES);
        assert_eq!(first.record_count(), 1);
        assert!(first.bytes[first.length()..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn fresh_exporter_has_no_cross_window_state() {
        let mut previous = DiagnosticExporter::new();
        previous
            .push(record(100, DiagnosticEvent::Stopped))
            .unwrap();
        let mut fresh = DiagnosticExporter::new();
        fresh.push(record(0, DiagnosticEvent::Started)).unwrap();
        assert_eq!(fresh.last_offset_ms(), Some(0));
        assert_eq!(fresh.record_count(), 1);
    }
}
