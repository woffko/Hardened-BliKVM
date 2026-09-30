//! A single-frame hand-off between capture reading and encoding.
//!
//! The reader never blocks on a slow encoder: a newer frame replaces the
//! pending one, so latency stays bounded by one frame under overload.

use std::sync::{Condvar, Mutex};
use std::time::Duration;

#[derive(Debug, Default)]
pub struct LatestFrame<T> {
    state: Mutex<State<T>>,
    ready: Condvar,
}

#[derive(Debug)]
struct State<T> {
    frame: Option<T>,
    replaced: u64,
    closed: bool,
}

impl<T> Default for State<T> {
    fn default() -> Self {
        Self {
            frame: None,
            replaced: 0,
            closed: false,
        }
    }
}

impl<T> LatestFrame<T> {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State::default()),
            ready: Condvar::new(),
        }
    }

    /// Store a frame, dropping an unconsumed older one. Returns false after close.
    pub fn publish(&self, frame: T) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if state.closed {
            return false;
        }
        if state.frame.replace(frame).is_some() {
            state.replaced = state.replaced.saturating_add(1);
        }
        self.ready.notify_one();
        true
    }

    /// Wait up to `timeout` for a frame. `None` on timeout or after close
    /// with nothing pending.
    pub fn take(&self, timeout: Duration) -> Option<T> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let (mut state, _) = self
            .ready
            .wait_timeout_while(state, timeout, |state| {
                state.frame.is_none() && !state.closed
            })
            .unwrap_or_else(|poison| poison.into_inner());
        state.frame.take()
    }

    /// Frames dropped because a newer one arrived first.
    pub fn replaced(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .replaced
    }

    pub fn close(&self) {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .closed = true;
        self.ready.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, thread, time::Instant};

    use super::*;

    #[test]
    fn newest_frame_wins_and_close_releases_waiters() {
        let slot = LatestFrame::new();
        assert!(slot.publish(1));
        assert!(slot.publish(2));
        assert_eq!(slot.replaced(), 1);
        assert_eq!(slot.take(Duration::from_millis(1)), Some(2));
        assert_eq!(slot.take(Duration::from_millis(10)), None);

        let slot = Arc::new(LatestFrame::<u32>::new());
        let waiter = {
            let slot = Arc::clone(&slot);
            thread::spawn(move || {
                let start = Instant::now();
                (slot.take(Duration::from_secs(5)), start.elapsed())
            })
        };
        thread::sleep(Duration::from_millis(20));
        slot.close();
        let (frame, waited) = waiter.join().unwrap();
        assert_eq!(frame, None);
        assert!(waited < Duration::from_secs(5));
        assert!(!slot.publish(3));
    }
}
