//! Which set of reconstruction buffers a pass reads and which it writes:
//! the driver's rule (`ve_enc_check_frame` and `ve_enc_sets_done` in
//! `kernel/hardened-blikvm-ve/ve_enc_plan.c`), once more for whoever stands
//! in for the driver. The driver has three sets: one holds the frame the
//! picture refers to, and the passes over the picture are written into the
//! other two in turn.

use crate::abi::{AGAIN, EARLIER, FRAME_FLAGS, KEY, STAY};

pub const SETS: usize = 3;

/// Which set holds what.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sets {
    /// The frame the picture of the last pass refers to.
    reference: Option<usize>,
    /// What the last pass wrote.
    last: Option<usize>,
    /// What the pass before it wrote, if it was over the same picture.
    earlier: Option<usize>,
}

/// The sets of one pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PassSets {
    /// Written by the pass.
    pub own: usize,
    /// Read by the engine: the picture's reference, of a key frame its own.
    pub reference: usize,
    /// The picture's reference as the sets keep it.
    kept: Option<usize>,
}

/// A set that is neither of the two.
fn another(first: Option<usize>, second: Option<usize>) -> usize {
    (0..SETS)
        .find(|set| Some(*set) != first && Some(*set) != second)
        .expect("three sets, two named")
}

impl Sets {
    /// A stream that has no frame yet, or lost its last one.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The sets of the pass that `flags` ask for, or none: flags that are
    /// not the interface's or contradict each other; a picture to be
    /// encoded again while there is none; a frame that is not a key frame
    /// while there is nothing to refer to, or that names a pass that was
    /// not made.
    pub fn check(&self, flags: u32, have_picture: bool) -> Option<PassSets> {
        if flags & !FRAME_FLAGS != 0 {
            return None;
        }
        let mut reference = self.reference;
        let own = if flags & AGAIN != 0 {
            if flags & (EARLIER | STAY) != 0 || !have_picture {
                return None;
            }
            // The last pass stays; the one before it is written over.
            another(reference, Some(self.last?))
        } else {
            if flags & EARLIER != 0 && flags & STAY != 0 {
                return None;
            }
            if flags & EARLIER != 0 {
                reference = Some(self.earlier?);
            } else if flags & STAY == 0 && self.last.is_some() {
                reference = self.last;
            }
            another(reference, None)
        };
        Some(PassSets {
            own,
            // A key frame names its own set as its reference.
            reference: if flags & KEY != 0 { own } else { reference? },
            kept: reference,
        })
    }

    /// The pass was made: what the sets hold now.
    pub fn done(&mut self, flags: u32, pass: &PassSets) {
        self.earlier = if flags & AGAIN != 0 { self.last } else { None };
        self.last = Some(pass.own);
        self.reference = pass.kept;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::WIDE;

    #[test]
    fn a_stream_begins_with_a_key_frame_and_goes_on_from_the_last_pass() {
        let mut sets = Sets::default();
        assert_eq!(sets.check(0, false), None);
        assert_eq!(sets.check(AGAIN | KEY, true), None);
        let key = sets.check(KEY, false).unwrap();
        assert_eq!(key.own, key.reference);
        sets.done(KEY, &key);
        // The picture after it refers to the key frame.
        let first = sets.check(0, true).unwrap();
        assert_eq!(first.reference, key.own);
        assert_ne!(first.own, key.own);
        sets.done(0, &first);
        // A second pass, with the other search: the same reference, a set
        // of its own.
        let second = sets.check(AGAIN | WIDE, true).unwrap();
        assert_eq!(second.reference, key.own);
        assert!(second.own != first.own && second.own != key.own);
        sets.done(AGAIN | WIDE, &second);
        // A third writes over the first and leaves the second.
        let third = sets.check(AGAIN, true).unwrap();
        assert_eq!(third.own, first.own);
        sets.done(AGAIN, &third);
        // The next picture: the last pass, or the one before it.
        assert_eq!(sets.check(0, true).unwrap().reference, third.own);
        assert_eq!(sets.check(EARLIER, true).unwrap().reference, second.own);
        // Or neither: the stream stays where it was.
        let stay = sets.check(STAY, true).unwrap();
        assert_eq!(stay.reference, key.own);
        assert_ne!(stay.own, key.own);
        sets.done(STAY, &stay);
        assert_eq!(sets.check(EARLIER, true), None);
    }

    #[test]
    fn what_contradicts_itself_is_refused() {
        let mut sets = Sets::default();
        let key = sets.check(KEY, false).unwrap();
        sets.done(KEY, &key);
        for flags in [
            EARLIER | STAY,
            AGAIN | EARLIER,
            AGAIN | STAY,
            1 << 5,
            u32::MAX,
        ] {
            assert_eq!(sets.check(flags, true), None, "{flags:#x}");
        }
        // No picture to encode again.
        assert_eq!(sets.check(AGAIN, false), None);
        // After a failure nothing is a frame.
        sets.reset();
        for flags in [0, STAY, EARLIER, AGAIN] {
            assert_eq!(sets.check(flags, true), None, "{flags:#x}");
        }
        assert!(sets.check(KEY | STAY, true).is_some());
    }

    #[test]
    fn a_key_frame_that_is_not_sent_leaves_the_stream_where_it_was() {
        let mut sets = Sets::default();
        let key = sets.check(KEY, false).unwrap();
        sets.done(KEY, &key);
        let frame = sets.check(0, true).unwrap();
        sets.done(0, &frame);
        // A key frame is tried and thrown away.
        let tried = sets.check(KEY, true).unwrap();
        assert_ne!(tried.own, frame.own);
        sets.done(KEY, &tried);
        // The next picture stays with the frame before the key frame.
        let next = sets.check(STAY, true).unwrap();
        assert_eq!(next.reference, frame.own);
    }
}
