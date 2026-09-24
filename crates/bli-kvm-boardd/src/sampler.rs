//! Owned sampling worker. Cancellation interrupts the interval, not device I/O.

use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub(crate) struct Sampler {
    stopping: Arc<AtomicBool>,
    worker: Option<JoinHandle<io::Result<()>>>,
}

struct StopOnExit(Arc<AtomicBool>);

impl Drop for StopOnExit {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl Sampler {
    pub(crate) fn spawn(
        stopping: Arc<AtomicBool>,
        interval: Duration,
        mut sample: impl FnMut() -> io::Result<()> + Send + 'static,
    ) -> io::Result<Self> {
        let worker_stop = Arc::clone(&stopping);
        let worker = thread::Builder::new()
            .name("boardd-sampler".into())
            .spawn(move || {
                // Also close listener admission if sampling fails or panics.
                let _stop_on_exit = StopOnExit(Arc::clone(&worker_stop));
                while !worker_stop.load(Ordering::SeqCst) {
                    sample()?;
                    let deadline = Instant::now()
                        .checked_add(interval)
                        .ok_or_else(|| io::Error::other("sampler interval overflow"))?;
                    while !worker_stop.load(Ordering::SeqCst) {
                        let Some(remaining) = deadline.checked_duration_since(Instant::now())
                        else {
                            break;
                        };
                        // A spurious unpark must not speed up the sampling cadence.
                        thread::park_timeout(remaining);
                    }
                }
                Ok(())
            })?;
        Ok(Self {
            stopping,
            worker: Some(worker),
        })
    }

    pub(crate) fn finish(mut self) -> io::Result<()> {
        self.join()
    }

    fn join(&mut self) -> io::Result<()> {
        self.stopping.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            worker
                .join()
                .map_err(|_| io::Error::other("boardd sampler panicked"))??;
        }
        Ok(())
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        // Unwind fallback: never detach a worker. Normal returns use finish
        // explicitly so a failed join is propagated to the process result.
        let _ = self.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn finish_interrupts_long_interval_and_joins_before_cleanup() {
        let stop = Arc::new(AtomicBool::new(false));
        let (send, receive) = mpsc::channel();
        let worker = Sampler::spawn(stop.clone(), Duration::from_secs(60), move || {
            send.send(()).unwrap();
            Ok(())
        })
        .unwrap();
        receive.recv_timeout(Duration::from_secs(2)).unwrap();
        let start = Instant::now();
        worker.finish().unwrap();
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(stop.load(Ordering::SeqCst));
        assert_eq!(receive.try_recv(), Err(mpsc::TryRecvError::Disconnected));
    }

    #[test]
    fn failure_and_panic_close_admission_and_fail_join() {
        for panic in [false, true] {
            let stop = Arc::new(AtomicBool::new(false));
            let (send, receive) = mpsc::channel();
            let worker = Sampler::spawn(stop.clone(), Duration::ZERO, move || {
                send.send(()).unwrap();
                assert!(!panic, "fixture sampler panic");
                Err(io::Error::other("fixture sample failure"))
            })
            .unwrap();
            receive.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(worker.finish().is_err());
            assert!(stop.load(Ordering::SeqCst));
        }
    }

    #[test]
    fn already_stopping_never_samples() {
        let stop = Arc::new(AtomicBool::new(true));
        Sampler::spawn(stop, Duration::ZERO, || panic!("sample after stop"))
            .unwrap()
            .finish()
            .unwrap();
    }
}
