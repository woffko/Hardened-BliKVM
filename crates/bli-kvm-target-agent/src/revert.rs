//! The undoing of a monitors' change, as Windows' own "Keep changes" does:
//! a change is put back after `DISPLAYS_REVERT_SECONDS` unless it is kept,
//! and a second change before the first is kept puts back what was there
//! before the first. One lock covers a change from its read of the
//! configuration to its claim of the undo, the timer from its check to its
//! putting back, and a keep, so that none of them runs inside another: a
//! timer that fired while a second change was made put back the first
//! change's undo point and then left the second to put back the first
//! change's result, saved. What calls Windows is the binary's
//! (`actions.rs`); this part is tested everywhere.

use std::sync::{Mutex, MutexGuard, PoisonError};

/// What waits to be put back: the change it waits for, and the
/// configuration from before the first change not yet kept.
struct Pending<C> {
    change: u64,
    before: C,
}

struct State<C> {
    changes: u64,
    pending: Option<Pending<C>>,
}

/// The monitors' changes that wait to be kept or put back, of
/// configurations `C`.
pub struct Revert<C> {
    state: Mutex<State<C>>,
}

impl<C> Default for Revert<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C> Revert<C> {
    pub const fn new() -> Self {
        Self {
            state: Mutex::new(State {
                changes: 0,
                pending: None,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State<C>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Makes a change: `change` reads the configuration, changes it and
    /// gives the configuration it read, all under the lock. Returns the
    /// change's number, which its timer gives to [`Self::due`]. A change
    /// that fails leaves what waited as it was.
    pub fn change(&self, change: impl FnOnce() -> Result<C, String>) -> Result<u64, String> {
        let mut state = self.lock();
        let before = change()?;
        state.changes += 1;
        let number = state.changes;
        let before = match state.pending.take() {
            Some(pending) => pending.before,
            None => before,
        };
        state.pending = Some(Pending {
            change: number,
            before,
        });
        Ok(number)
    }

    /// The timer of change `number`: when that change still waits,
    /// `put_back` gets the configuration to put back, under the lock.
    pub fn due(&self, number: u64, put_back: impl FnOnce(C)) {
        let mut state = self.lock();
        if state
            .pending
            .as_ref()
            .is_some_and(|pending| pending.change == number)
            && let Some(pending) = state.pending.take()
        {
            put_back(pending.before);
        }
    }

    /// Keeps the change that waits: `save` saves the configuration as it
    /// is, under the lock, and only a saved change stops waiting, so that
    /// one whose saving failed is still put back in time and may be kept
    /// again meanwhile. With nothing waiting (put back already, or kept)
    /// it fails, so that the page does not take it for kept.
    pub fn keep(&self, save: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
        let mut state = self.lock();
        if state.pending.is_none() {
            return Err("nothing is waiting to be kept".into());
        }
        save()?;
        state.pending = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn a_timer_that_fires_while_a_second_change_is_made_puts_back_the_first_undo_point() {
        // The configurations: 0 at first, 1 after the first change.
        let revert = Arc::new(Revert::new());
        let first = revert.change(|| Ok(0)).unwrap();
        let (applying, applied) = std::sync::mpsc::channel();
        let second = std::thread::spawn({
            let revert = revert.clone();
            move || {
                revert
                    .change(|| {
                        applying.send(()).unwrap();
                        // Windows takes its time over the second change.
                        std::thread::sleep(Duration::from_millis(200));
                        Ok(1)
                    })
                    .unwrap()
            }
        });
        // The first change's timer fires meanwhile.
        applied.recv().unwrap();
        let put_back = Mutex::new(Vec::new());
        revert.due(first, |config| put_back.lock().unwrap().push(config));
        let second = second.join().unwrap();
        revert.due(second, |config| put_back.lock().unwrap().push(config));
        // Only the second change's timer puts anything back: what was there
        // before the first.
        assert_eq!(*put_back.lock().unwrap(), vec![0]);
    }

    #[test]
    fn only_a_change_that_waits_is_kept_and_one_not_saved_is_still_put_back() {
        let revert = Revert::new();
        let saved = Mutex::new(0);
        let save = || {
            *saved.lock().unwrap() += 1;
            Ok(())
        };
        // Nothing waits: there is nothing to keep, and nothing is saved.
        assert_eq!(
            revert.keep(save),
            Err("nothing is waiting to be kept".to_string())
        );
        assert_eq!(*saved.lock().unwrap(), 0);
        // A keep whose saving fails leaves the change to be put back in time.
        let change = revert.change(|| Ok(0)).unwrap();
        assert_eq!(
            revert.keep(|| Err("Windows refused".into())),
            Err("Windows refused".to_string())
        );
        let mut put_back = Vec::new();
        revert.due(change, |config| put_back.push(config));
        assert_eq!(put_back, vec![0]);
        // A change kept is not put back, and kept once.
        let change = revert.change(|| Ok(0)).unwrap();
        assert_eq!(revert.keep(save), Ok(()));
        revert.due(change, |config| put_back.push(config));
        assert_eq!(put_back, vec![0]);
        assert!(revert.keep(save).is_err());
        assert_eq!(*saved.lock().unwrap(), 1);
    }

    #[test]
    fn a_second_change_puts_back_what_was_there_before_the_first() {
        let revert = Revert::new();
        let first = revert.change(|| Ok(0)).unwrap();
        let second = revert.change(|| Ok(1)).unwrap();
        let mut put_back = Vec::new();
        revert.due(first, |config| put_back.push(config));
        revert.due(second, |config| put_back.push(config));
        assert_eq!(put_back, vec![0]);
        // A change that fails leaves nothing waiting of its own.
        assert!(revert.change(|| Err("refused".into())).is_err());
        revert.due(second + 1, |config| put_back.push(config));
        assert_eq!(put_back, vec![0]);
    }
}
