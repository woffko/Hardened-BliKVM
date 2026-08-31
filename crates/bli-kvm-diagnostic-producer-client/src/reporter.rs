use std::{
    error::Error,
    fmt,
    future::Future,
    sync::{
        Arc, Mutex as StdMutex, TryLockError,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use bli_kvm_diagnostic_protocol::{DiagnosticEvent, DiagnosticProducerRejection, encode_event};
use tokio::{runtime::Handle, sync::Notify, task::JoinHandle, time::timeout};

use crate::{DiagnosticProducerClient, DiagnosticSendError, DiagnosticSendOutcome};

pub const DIAGNOSTIC_REPORT_QUEUE_CAPACITY: usize = 8;
pub const DIAGNOSTIC_REPORTER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticReportDropReason {
    InvalidEvent,
    Busy,
    Full,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticReportReceipt {
    Queued,
    Dropped { reason: DiagnosticReportDropReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticReporterStartError {
    NoRuntime,
}

impl fmt::Display for DiagnosticReporterStartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("diagnostic reporter requires a Tokio runtime")
    }
}

impl Error for DiagnosticReporterStartError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DiagnosticReporterSnapshot {
    pub queued: u64,
    pub dropped_invalid: u64,
    pub dropped_busy: u64,
    pub dropped_full: u64,
    pub dropped_closed: u64,
    pub accepted: u64,
    pub rejected_event: u64,
    pub rejected_backpressure: u64,
    pub rejected_unavailable: u64,
    pub not_sent: u64,
    pub delivery_ambiguous: u64,
    pub worker_failed: u64,
    pub shutdown_aborted: u64,
}

#[derive(Clone)]
pub struct DiagnosticReporterHandle {
    shared: Arc<ReporterShared>,
}

impl DiagnosticReporterHandle {
    pub fn try_report(&self, event: DiagnosticEvent) -> DiagnosticReportReceipt {
        if encode_event(event).is_err() {
            increment(&self.shared.counters.dropped_invalid);
            return DiagnosticReportReceipt::Dropped {
                reason: DiagnosticReportDropReason::InvalidEvent,
            };
        }
        let mut queue = match self.shared.queue.try_lock() {
            Ok(queue) => queue,
            Err(TryLockError::WouldBlock) => {
                increment(&self.shared.counters.dropped_busy);
                return DiagnosticReportReceipt::Dropped {
                    reason: DiagnosticReportDropReason::Busy,
                };
            }
            Err(TryLockError::Poisoned(_)) => {
                increment(&self.shared.counters.dropped_closed);
                return DiagnosticReportReceipt::Dropped {
                    reason: DiagnosticReportDropReason::Closed,
                };
            }
        };
        if !queue.accepting {
            increment(&self.shared.counters.dropped_closed);
            return DiagnosticReportReceipt::Dropped {
                reason: DiagnosticReportDropReason::Closed,
            };
        }
        if !queue.push(event) {
            increment(&self.shared.counters.dropped_full);
            return DiagnosticReportReceipt::Dropped {
                reason: DiagnosticReportDropReason::Full,
            };
        }
        increment(&self.shared.counters.queued);
        drop(queue);
        self.shared.notify.notify_one();
        DiagnosticReportReceipt::Queued
    }

    pub fn snapshot(&self) -> DiagnosticReporterSnapshot {
        self.shared.counters.snapshot()
    }
}

pub struct DiagnosticReporter {
    shared: Arc<ReporterShared>,
    worker: Option<JoinHandle<()>>,
    shutdown_timeout: Duration,
}

impl DiagnosticReporter {
    pub fn start() -> Result<Self, DiagnosticReporterStartError> {
        Self::start_with_sink(
            ClientSink {
                client: DiagnosticProducerClient::new(),
            },
            DIAGNOSTIC_REPORTER_SHUTDOWN_TIMEOUT,
        )
    }

    pub fn handle(&self) -> DiagnosticReporterHandle {
        DiagnosticReporterHandle {
            shared: Arc::clone(&self.shared),
        }
    }

    pub fn snapshot(&self) -> DiagnosticReporterSnapshot {
        self.shared.counters.snapshot()
    }

    pub async fn shutdown(mut self) -> DiagnosticReporterSnapshot {
        close_queue(&self.shared);
        if let Some(mut worker) = self.worker.take() {
            match timeout(self.shutdown_timeout, &mut worker).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => increment(&self.shared.counters.worker_failed),
                Err(_) => {
                    increment(&self.shared.counters.shutdown_aborted);
                    worker.abort();
                    let _ = worker.await;
                }
            }
        }
        self.shared.counters.snapshot()
    }

    fn start_with_sink<S>(
        sink: S,
        shutdown_timeout: Duration,
    ) -> Result<Self, DiagnosticReporterStartError>
    where
        S: ReporterSink,
    {
        let runtime = Handle::try_current().map_err(|_| DiagnosticReporterStartError::NoRuntime)?;
        let shared = Arc::new(ReporterShared::new());
        let worker_shared = Arc::clone(&shared);
        let worker = runtime.spawn(async move { run_worker(worker_shared, sink).await });
        Ok(Self {
            shared,
            worker: Some(worker),
            shutdown_timeout,
        })
    }
}

impl Drop for DiagnosticReporter {
    fn drop(&mut self) {
        close_queue(&self.shared);
        if let Some(worker) = self.worker.take() {
            increment(&self.shared.counters.shutdown_aborted);
            worker.abort();
        }
    }
}

trait ReporterSink: Send + 'static {
    fn send(
        &mut self,
        event: DiagnosticEvent,
    ) -> impl Future<Output = Result<DiagnosticSendOutcome, DiagnosticSendError>> + Send;
}

struct ClientSink {
    client: DiagnosticProducerClient,
}

impl ReporterSink for ClientSink {
    async fn send(
        &mut self,
        event: DiagnosticEvent,
    ) -> Result<DiagnosticSendOutcome, DiagnosticSendError> {
        self.client.send(event).await
    }
}

async fn run_worker<S>(shared: Arc<ReporterShared>, mut sink: S)
where
    S: ReporterSink,
{
    loop {
        match next_event(&shared) {
            NextEvent::Event(event) => {
                let result = sink.send(event).await;
                record_result(&shared.counters, result);
            }
            NextEvent::Empty => shared.notify.notified().await,
            NextEvent::Closed => return,
            NextEvent::Poisoned => {
                increment(&shared.counters.worker_failed);
                return;
            }
        }
    }
}

fn record_result(
    counters: &ReporterCounters,
    result: Result<DiagnosticSendOutcome, DiagnosticSendError>,
) {
    match result {
        Ok(DiagnosticSendOutcome::Accepted) => increment(&counters.accepted),
        Ok(DiagnosticSendOutcome::Rejected {
            reason: DiagnosticProducerRejection::EventNotAllowed,
        }) => increment(&counters.rejected_event),
        Ok(DiagnosticSendOutcome::Rejected {
            reason: DiagnosticProducerRejection::Backpressure,
        }) => increment(&counters.rejected_backpressure),
        Ok(DiagnosticSendOutcome::Rejected {
            reason: DiagnosticProducerRejection::CollectorUnavailable,
        }) => increment(&counters.rejected_unavailable),
        Err(DiagnosticSendError::NotSent { .. }) => increment(&counters.not_sent),
        Err(DiagnosticSendError::DeliveryAmbiguous { .. }) => {
            increment(&counters.delivery_ambiguous)
        }
    }
}

fn next_event(shared: &ReporterShared) -> NextEvent {
    match shared.queue.lock() {
        Ok(mut queue) => match queue.pop() {
            Some(event) => NextEvent::Event(event),
            None if queue.accepting => NextEvent::Empty,
            None => NextEvent::Closed,
        },
        Err(_) => NextEvent::Poisoned,
    }
}

fn close_queue(shared: &ReporterShared) {
    match shared.queue.lock() {
        Ok(mut queue) => queue.accepting = false,
        Err(_) => increment(&shared.counters.worker_failed),
    }
    shared.notify.notify_one();
}

enum NextEvent {
    Event(DiagnosticEvent),
    Empty,
    Closed,
    Poisoned,
}

struct ReporterShared {
    queue: StdMutex<ReporterQueue>,
    notify: Notify,
    counters: ReporterCounters,
}

impl ReporterShared {
    fn new() -> Self {
        Self {
            queue: StdMutex::new(ReporterQueue::new()),
            notify: Notify::new(),
            counters: ReporterCounters::new(),
        }
    }
}

struct ReporterQueue {
    slots: [Option<DiagnosticEvent>; DIAGNOSTIC_REPORT_QUEUE_CAPACITY],
    head: usize,
    len: usize,
    accepting: bool,
}

impl ReporterQueue {
    const fn new() -> Self {
        Self {
            slots: [None; DIAGNOSTIC_REPORT_QUEUE_CAPACITY],
            head: 0,
            len: 0,
            accepting: true,
        }
    }

    fn push(&mut self, event: DiagnosticEvent) -> bool {
        if self.len == self.slots.len() {
            return false;
        }
        let tail = (self.head + self.len) % self.slots.len();
        self.slots[tail] = Some(event);
        self.len += 1;
        true
    }

    fn pop(&mut self) -> Option<DiagnosticEvent> {
        if self.len == 0 {
            return None;
        }
        let event = self.slots[self.head].take();
        self.head = (self.head + 1) % self.slots.len();
        self.len -= 1;
        event
    }
}

struct ReporterCounters {
    queued: AtomicU64,
    dropped_invalid: AtomicU64,
    dropped_busy: AtomicU64,
    dropped_full: AtomicU64,
    dropped_closed: AtomicU64,
    accepted: AtomicU64,
    rejected_event: AtomicU64,
    rejected_backpressure: AtomicU64,
    rejected_unavailable: AtomicU64,
    not_sent: AtomicU64,
    delivery_ambiguous: AtomicU64,
    worker_failed: AtomicU64,
    shutdown_aborted: AtomicU64,
}

impl ReporterCounters {
    const fn new() -> Self {
        Self {
            queued: AtomicU64::new(0),
            dropped_invalid: AtomicU64::new(0),
            dropped_busy: AtomicU64::new(0),
            dropped_full: AtomicU64::new(0),
            dropped_closed: AtomicU64::new(0),
            accepted: AtomicU64::new(0),
            rejected_event: AtomicU64::new(0),
            rejected_backpressure: AtomicU64::new(0),
            rejected_unavailable: AtomicU64::new(0),
            not_sent: AtomicU64::new(0),
            delivery_ambiguous: AtomicU64::new(0),
            worker_failed: AtomicU64::new(0),
            shutdown_aborted: AtomicU64::new(0),
        }
    }

    fn snapshot(&self) -> DiagnosticReporterSnapshot {
        DiagnosticReporterSnapshot {
            queued: self.queued.load(Ordering::Relaxed),
            dropped_invalid: self.dropped_invalid.load(Ordering::Relaxed),
            dropped_busy: self.dropped_busy.load(Ordering::Relaxed),
            dropped_full: self.dropped_full.load(Ordering::Relaxed),
            dropped_closed: self.dropped_closed.load(Ordering::Relaxed),
            accepted: self.accepted.load(Ordering::Relaxed),
            rejected_event: self.rejected_event.load(Ordering::Relaxed),
            rejected_backpressure: self.rejected_backpressure.load(Ordering::Relaxed),
            rejected_unavailable: self.rejected_unavailable.load(Ordering::Relaxed),
            not_sent: self.not_sent.load(Ordering::Relaxed),
            delivery_ambiguous: self.delivery_ambiguous.load(Ordering::Relaxed),
            worker_failed: self.worker_failed.load(Ordering::Relaxed),
            shutdown_aborted: self.shutdown_aborted.load(Ordering::Relaxed),
        }
    }
}

fn increment(counter: &AtomicU64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(1))
    });
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        future::pending,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering as AtomicOrdering},
        },
    };

    use bli_kvm_diagnostic_protocol::DiagnosticProducerRejection;
    use tokio::{sync::oneshot, time::sleep};

    use super::*;

    const TEST_SHUTDOWN: Duration = Duration::from_millis(40);
    const TEST_WAIT: Duration = Duration::from_millis(500);

    #[test]
    fn start_requires_a_tokio_runtime() {
        assert!(matches!(
            DiagnosticReporter::start(),
            Err(DiagnosticReporterStartError::NoRuntime)
        ));
    }

    #[tokio::test]
    async fn records_every_semantic_worker_outcome_without_retry() {
        let results = Arc::new(Mutex::new(VecDeque::from([
            Ok(DiagnosticSendOutcome::Accepted),
            Ok(DiagnosticSendOutcome::Rejected {
                reason: DiagnosticProducerRejection::EventNotAllowed,
            }),
            Ok(DiagnosticSendOutcome::Rejected {
                reason: DiagnosticProducerRejection::Backpressure,
            }),
            Ok(DiagnosticSendOutcome::Rejected {
                reason: DiagnosticProducerRejection::CollectorUnavailable,
            }),
            Err(DiagnosticSendError::NotSent {
                reason: crate::DiagnosticNotSentReason::ConnectFailed,
            }),
            Err(DiagnosticSendError::DeliveryAmbiguous {
                stage: crate::DiagnosticAmbiguousStage::ResponseTimeout,
            }),
        ])));
        let calls = Arc::new(AtomicUsize::new(0));
        let reporter = DiagnosticReporter::start_with_sink(
            SequenceSink {
                results,
                calls: Arc::clone(&calls),
            },
            TEST_SHUTDOWN,
        )
        .unwrap();
        let handle = reporter.handle();
        for event in [
            DiagnosticEvent::Ready,
            DiagnosticEvent::Unavailable,
            DiagnosticEvent::Ready,
            DiagnosticEvent::Unavailable,
            DiagnosticEvent::Ready,
            DiagnosticEvent::Unavailable,
        ] {
            assert_eq!(handle.try_report(event), DiagnosticReportReceipt::Queued);
        }
        wait_for(&handle, |snapshot| snapshot.delivery_ambiguous == 1).await;
        let snapshot = reporter.shutdown().await;
        assert_eq!(calls.load(AtomicOrdering::Relaxed), 6);
        assert_eq!(snapshot.queued, 6);
        assert_eq!(snapshot.accepted, 1);
        assert_eq!(snapshot.rejected_event, 1);
        assert_eq!(snapshot.rejected_backpressure, 1);
        assert_eq!(snapshot.rejected_unavailable, 1);
        assert_eq!(snapshot.not_sent, 1);
        assert_eq!(snapshot.delivery_ambiguous, 1);
    }

    #[tokio::test]
    async fn real_client_collector_outage_is_counted_once_without_retry() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing.sock");
        let reporter = DiagnosticReporter::start_with_sink(
            ClientSink {
                client: DiagnosticProducerClient::with_parts(
                    missing,
                    crate::ClientLimits {
                        connect_timeout: TEST_SHUTDOWN,
                        write_timeout: TEST_SHUTDOWN,
                        response_timeout: TEST_SHUTDOWN,
                    },
                ),
            },
            TEST_SHUTDOWN,
        )
        .unwrap();
        let handle = reporter.handle();
        assert_eq!(
            handle.try_report(DiagnosticEvent::Ready),
            DiagnosticReportReceipt::Queued
        );
        wait_for(&handle, |snapshot| snapshot.not_sent == 1).await;
        sleep(TEST_SHUTDOWN).await;
        assert_eq!(handle.snapshot().not_sent, 1);
        let snapshot = reporter.shutdown().await;
        assert_eq!(snapshot.not_sent, 1);
        assert_eq!(snapshot.shutdown_aborted, 0);
    }

    #[tokio::test]
    async fn saturation_busy_invalid_and_closed_are_nonblocking_receipts() {
        let (started_sender, started) = oneshot::channel();
        let reporter = DiagnosticReporter::start_with_sink(
            BlockingSink {
                started: Some(started_sender),
            },
            TEST_SHUTDOWN,
        )
        .unwrap();
        let handle = reporter.handle();
        assert_eq!(
            handle.try_report(DiagnosticEvent::Started),
            DiagnosticReportReceipt::Dropped {
                reason: DiagnosticReportDropReason::InvalidEvent
            }
        );
        assert_eq!(
            handle.try_report(DiagnosticEvent::Ready),
            DiagnosticReportReceipt::Queued
        );
        started.await.unwrap();
        for _ in 0..DIAGNOSTIC_REPORT_QUEUE_CAPACITY {
            assert_eq!(
                handle.try_report(DiagnosticEvent::Ready),
                DiagnosticReportReceipt::Queued
            );
        }
        assert_eq!(
            handle.try_report(DiagnosticEvent::Ready),
            DiagnosticReportReceipt::Dropped {
                reason: DiagnosticReportDropReason::Full
            }
        );
        {
            let _guard = reporter.shared.queue.lock().unwrap();
            assert_eq!(
                handle.try_report(DiagnosticEvent::Ready),
                DiagnosticReportReceipt::Dropped {
                    reason: DiagnosticReportDropReason::Busy
                }
            );
        }
        let snapshot = reporter.shutdown().await;
        assert_eq!(snapshot.dropped_invalid, 1);
        assert_eq!(snapshot.dropped_full, 1);
        assert_eq!(snapshot.dropped_busy, 1);
        assert_eq!(snapshot.shutdown_aborted, 1);
        assert_eq!(
            handle.try_report(DiagnosticEvent::Ready),
            DiagnosticReportReceipt::Dropped {
                reason: DiagnosticReportDropReason::Closed
            }
        );
        assert_eq!(handle.snapshot().dropped_closed, 1);
    }

    #[tokio::test]
    async fn fixed_ring_preserves_order_and_drains_on_shutdown() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let reporter = DiagnosticReporter::start_with_sink(
            RecordingSink {
                seen: Arc::clone(&seen),
            },
            TEST_WAIT,
        )
        .unwrap();
        let handle = reporter.handle();
        let events = [
            DiagnosticEvent::Ready,
            DiagnosticEvent::Unavailable,
            DiagnosticEvent::RequestRejected {
                reason: bli_kvm_diagnostic_protocol::RejectReason::Busy,
            },
        ];
        for event in events {
            assert_eq!(handle.try_report(event), DiagnosticReportReceipt::Queued);
        }
        let snapshot = reporter.shutdown().await;
        assert_eq!(snapshot.accepted, 3);
        assert_eq!(*seen.lock().unwrap(), events);
    }

    #[tokio::test]
    async fn drop_aborts_worker_and_closes_all_handles() {
        let (started_sender, started) = oneshot::channel();
        let reporter = DiagnosticReporter::start_with_sink(
            BlockingSink {
                started: Some(started_sender),
            },
            TEST_WAIT,
        )
        .unwrap();
        let handle = reporter.handle();
        assert_eq!(
            handle.try_report(DiagnosticEvent::Ready),
            DiagnosticReportReceipt::Queued
        );
        started.await.unwrap();
        drop(reporter);
        assert_eq!(handle.snapshot().shutdown_aborted, 1);
        assert_eq!(
            handle.try_report(DiagnosticEvent::Ready),
            DiagnosticReportReceipt::Dropped {
                reason: DiagnosticReportDropReason::Closed
            }
        );
    }

    #[tokio::test]
    async fn worker_panic_is_a_bounded_semantic_counter() {
        let reporter = DiagnosticReporter::start_with_sink(PanicSink, TEST_SHUTDOWN).unwrap();
        let handle = reporter.handle();
        assert_eq!(
            handle.try_report(DiagnosticEvent::Ready),
            DiagnosticReportReceipt::Queued
        );
        sleep(Duration::from_millis(10)).await;
        let snapshot = reporter.shutdown().await;
        assert_eq!(snapshot.worker_failed, 1);
    }

    #[test]
    fn counters_saturate_without_wrapping() {
        let counters = ReporterCounters::new();
        counters.accepted.store(u64::MAX, Ordering::Relaxed);
        increment(&counters.accepted);
        assert_eq!(counters.accepted.load(Ordering::Relaxed), u64::MAX);
    }

    async fn wait_for(
        handle: &DiagnosticReporterHandle,
        predicate: impl Fn(DiagnosticReporterSnapshot) -> bool,
    ) {
        timeout(TEST_WAIT, async {
            loop {
                if predicate(handle.snapshot()) {
                    return;
                }
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("reporter did not reach expected state");
    }

    struct SequenceSink {
        results: Arc<Mutex<VecDeque<Result<DiagnosticSendOutcome, DiagnosticSendError>>>>,
        calls: Arc<AtomicUsize>,
    }

    impl ReporterSink for SequenceSink {
        async fn send(
            &mut self,
            _event: DiagnosticEvent,
        ) -> Result<DiagnosticSendOutcome, DiagnosticSendError> {
            self.calls.fetch_add(1, AtomicOrdering::Relaxed);
            self.results.lock().unwrap().pop_front().unwrap()
        }
    }

    struct RecordingSink {
        seen: Arc<Mutex<Vec<DiagnosticEvent>>>,
    }

    impl ReporterSink for RecordingSink {
        async fn send(
            &mut self,
            event: DiagnosticEvent,
        ) -> Result<DiagnosticSendOutcome, DiagnosticSendError> {
            self.seen.lock().unwrap().push(event);
            Ok(DiagnosticSendOutcome::Accepted)
        }
    }

    struct BlockingSink {
        started: Option<oneshot::Sender<()>>,
    }

    impl ReporterSink for BlockingSink {
        async fn send(
            &mut self,
            _event: DiagnosticEvent,
        ) -> Result<DiagnosticSendOutcome, DiagnosticSendError> {
            if let Some(started) = self.started.take() {
                let _ = started.send(());
            }
            pending().await
        }
    }

    struct PanicSink;

    impl ReporterSink for PanicSink {
        async fn send(
            &mut self,
            _event: DiagnosticEvent,
        ) -> Result<DiagnosticSendOutcome, DiagnosticSendError> {
            panic!("test reporter worker panic")
        }
    }
}
