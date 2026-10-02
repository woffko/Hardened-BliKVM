//! What is decided for every frame in software: its QP, from the bytes of
//! the frames before, and which of the engine's two searches it is given.
//!
//! Rate control in software: a QP for every frame from the bytes of the
//! frames before. The engine is given a fixed QP for a frame and nothing
//! else; what the stream costs is steered here.
//!
//! The bound is the one the product's x264 has today: a leaky bucket of one
//! second at the target rate. A frame that would not fit is encoded once
//! more at a higher QP, with the reference left as it was. If it does not
//! fit then either, it is not sent: the stream gets a frame that says
//! "nothing changed" in its place, which costs a few bytes, and the picture
//! stands still for that frame. That is what keeps the rate when the
//! coarsest quantiser is not enough, as on a picture that changes
//! everywhere at 1.5 Mbit/s.

use crate::regs::Search;

/// The QPs the controller uses: all the syntax has, as the product's x264
/// may. The engine encodes at every one of them (V3.4).
pub const QP_MIN: i32 = 0;
pub const QP_MAX: i32 = 51;
/// Where a stream begins, before it knows what its pictures cost.
pub const QP_START: i32 = 30;

/// Bits halve with six steps of QP, near enough for a controller.
const STEPS_PER_HALVING: f64 = 6.0;
/// How much of a frame's deviation is taken into the next QP.
const GAIN: f64 = 0.5;
/// A QP falls slowly and rises fast.
const FALL: f64 = 1.0;
const RISE: f64 = 3.0;
/// After a frame that cost next to nothing it may fall faster: the picture
/// rests, and what it lost while it moved should come back as fast as the
/// rate pays for it. A step too many is caught by the second pass.
const FALL_AT_REST: f64 = 3.0;
/// A frame costs next to nothing below this part of what it was meant to.
const AT_REST: f64 = 0.25;
/// The part of the bucket's room a frame is planned into.
const PLANNED: f64 = 0.8;
/// How full the bucket is wanted: room above it for a change of scene, and
/// little more than the rate while it fills at a stream's beginning.
const WANTED: f64 = 0.3;
/// The least a frame is meant to cost, as a part of the average, while the
/// bucket is fuller than wanted. The bucket is full after motion the search
/// did not follow, which is when the picture has the most to win back: it
/// is emptied slowly, with nine tenths of the rate still spent.
const LEAST: f64 = 0.9;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateControl {
    /// Bits a frame may cost on average.
    per_frame: f64,
    /// The bucket: one second at the rate.
    capacity: f64,
    fullness: f64,
    /// The QP of frames that are not key frames, with its fraction.
    qp: f64,
    /// The last key frame's bits and QP: what the next one is planned by.
    last_key: Option<(f64, i32)>,
    /// Frames that went out although the bucket had no room for them: key
    /// frames, which cannot be left out.
    pub overflows: u32,
    /// Frames that were left out.
    pub skipped: u32,
}

fn steps(ratio: f64) -> f64 {
    STEPS_PER_HALVING * ratio.log2()
}

impl RateControl {
    pub fn new(kbps: u32, fps: u32) -> Self {
        let rate = kbps as f64 * 1000.0;
        Self {
            per_frame: rate / fps.max(1) as f64,
            capacity: rate,
            fullness: 0.0,
            qp: QP_START as f64,
            last_key: None,
            overflows: 0,
            skipped: 0,
        }
    }

    /// Whether a frame of `bits` may go out now.
    pub fn fits(&self, bits: u64) -> bool {
        bits as f64 <= self.room()
    }

    /// What the bucket still takes.
    fn room(&self) -> f64 {
        (self.capacity - self.fullness).max(0.0)
    }

    /// A frame's time passes: the bucket drains. Called once before every
    /// frame, the first included.
    pub fn tick(&mut self) {
        self.fullness = (self.fullness - self.per_frame).max(0.0);
    }

    /// The QP to try for the next frame.
    pub fn next_qp(&self, key: bool) -> i32 {
        let usual = self.qp.round() as i32;
        if !key {
            return usual.clamp(QP_MIN, QP_MAX);
        }
        // A key frame costs many times a frame after it: plan it by the
        // last one, into what the bucket has room for.
        let Some((bits, qp)) = self.last_key else {
            return usual.clamp(QP_MIN, QP_MAX);
        };
        let allowed = (self.room() * PLANNED).max(1.0);
        let needed = qp + steps(bits / allowed).ceil() as i32;
        needed.max(usual - 3).clamp(QP_MIN, QP_MAX)
    }

    /// Whether a frame of `bits` at `qp` must be encoded again, and at
    /// which QP. Asked once for a frame: its second pass goes out.
    pub fn again(&self, bits: u64, qp: i32) -> Option<i32> {
        let bits = bits as f64;
        if bits <= self.room() || qp >= QP_MAX {
            return None;
        }
        let allowed = (self.room() * PLANNED).max(1.0);
        let more = steps(bits / allowed).ceil().clamp(1.0, 24.0) as i32;
        Some((qp + more).min(QP_MAX))
    }

    /// The frame was left out: `bits` of "nothing changed" went out in its
    /// place, after `qp` was not enough.
    pub fn left_out(&mut self, bits: u64, qp: i32) {
        self.skipped += 1;
        self.fullness = (self.fullness + bits as f64).min(self.capacity);
        // The next frame starts from where this one failed.
        self.qp = self.qp.max(qp as f64);
    }

    /// The frame went out with `bits` at `qp`.
    pub fn sent(&mut self, bits: u64, qp: i32, key: bool) {
        let bits = bits as f64;
        if bits > self.room() {
            self.overflows += 1;
        }
        self.fullness = (self.fullness + bits).min(self.capacity);
        if key {
            self.last_key = Some((bits, qp));
            return;
        }
        // A second pass moved the QP: go on from where the frame was sent.
        if qp != self.qp.round() as i32 {
            self.qp = qp as f64;
        }
        // Above the level the bucket is wanted at a frame should cost less
        // than the average, below it more.
        let level = self.fullness / self.capacity;
        let target = self.per_frame * (1.0 + WANTED - level).clamp(LEAST, 1.5);
        let fall = if bits < AT_REST * target {
            FALL_AT_REST
        } else {
            FALL
        };
        let change = (GAIN * steps(bits.max(1.0) / target)).clamp(-fall, RISE);
        self.qp = (self.qp + change).clamp(QP_MIN as f64, QP_MAX as f64);
    }
}

/// The passes a frame may take when nothing else is said: its first and
/// one more. A pass over a 1920x1080 picture takes the engine 7 ms and the
/// loop around it 3 more; three of them and the picture's way in are a
/// frame's whole time at 30 frames a second, and longer when the
/// processors are busy.
pub const PASSES: u32 = 2;

/// What a frame's pass after the first is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Second {
    /// The other search at the same QP: the smaller of the two passes is
    /// the frame.
    Other,
    /// The same search at a higher QP: the first pass did not fit.
    Higher,
    /// Both in one pass, where a frame may take only one more: the other
    /// search at the higher QP.
    OtherHigher,
}

/// What follows the first pass of a frame that may take `passes` passes:
/// `other` says that the other search is due, `too_large` that the bucket
/// has no room for the first pass. With three passes the other search
/// comes first and a higher QP may follow it. With two, a frame that is
/// due the other search and too large gets both at once: such a frame is
/// where a wrong search shows first, and a control that only raised the QP
/// would hide the cost the other search is chosen by.
pub fn second_pass(passes: u32, other: bool, too_large: bool) -> Option<Second> {
    match (passes, other, too_large) {
        (0 | 1, _, _) | (_, false, false) => None,
        (_, false, true) => Some(Second::Higher),
        (_, true, false) => Some(Second::Other),
        (2, true, true) => Some(Second::OtherHigher),
        (_, true, true) => Some(Second::Other),
    }
}

/// Frames for which the other search is left alone after it was tried.
const SEARCH_REST: u32 = 3;
/// The other search is better when its frame is this much smaller.
const SEARCH_BETTER: f64 = 0.9;
/// A frame this many times the size of the last one that was tried with
/// both is tried with both at once: the picture began to move otherwise.
const SEARCH_AGAIN: u64 = 2;

/// Which of the engine's two searches a frame is given. Neither is the
/// better one everywhere: the exact one loses a picture that moves down,
/// the wide one makes text that moves a few pixels cost several times as
/// much, and loses a frame now and then. A frame is encoded with the
/// search the frame before it was sent with; when it comes out costly, it
/// is encoded again with the other one, and when that pass is smaller by a
/// tenth it is the frame and its search the next frame's. After such a try
/// the other search rests for three frames, unless a frame costs twice
/// what the tried one did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchChoice {
    current: Search,
    rest: u32,
    /// The bits of the smaller pass of the last frame tried with both.
    tried_bits: u64,
    /// Frames that were encoded with both.
    pub tried: u32,
    /// Frames after which the search changed.
    pub changed: u32,
}

impl Default for SearchChoice {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchChoice {
    pub fn new() -> Self {
        Self {
            current: Search::Exact,
            rest: 0,
            tried_bits: 0,
            tried: 0,
            changed: 0,
        }
    }

    /// The search of a frame's first pass.
    pub fn first(&self) -> Search {
        self.current
    }

    /// A frame begins. Called once before every frame.
    pub fn tick(&mut self) {
        self.rest = self.rest.saturating_sub(1);
    }

    /// The search to encode the frame with once more, after its first pass
    /// made `bits`; none when the frame is not costly (`costly` bits or
    /// more), or the other search was tried a moment ago on a frame that
    /// was not much smaller.
    pub fn other(&self, bits: u64, costly: u64) -> Option<Search> {
        let due = self.rest == 0 || bits >= SEARCH_AGAIN * self.tried_bits;
        (bits >= costly && due).then(|| self.current.other())
    }

    /// The second pass was made with the other search at a QP `steps`
    /// higher than the first. What the first search would have made there
    /// is taken from the control's rule of thumb, bits halving with six
    /// steps; text shrinks less than that, so the rule errs towards the
    /// search the frame came with. True when the other search's pass is a
    /// tenth smaller than that estimate: it is then the next frame's.
    pub fn compared_higher(&mut self, first_bits: u64, steps: i32, second_bits: u64) -> bool {
        let expected = first_bits as f64 / 2f64.powf(steps as f64 / STEPS_PER_HALVING);
        self.tried += 1;
        self.rest = SEARCH_REST + 1;
        self.tried_bits = (expected as u64).min(second_bits);
        let second = (second_bits as f64) < SEARCH_BETTER * expected;
        if second {
            self.current = self.current.other();
            self.changed += 1;
        }
        second
    }

    /// Both passes are made. True when the second is the frame; the search
    /// that made the smaller one is the next frame's.
    pub fn compared(&mut self, first_bits: u64, second_bits: u64) -> bool {
        self.tried += 1;
        // The frame that is tried counts as the first of the rest.
        self.rest = SEARCH_REST + 1;
        self.tried_bits = first_bits.min(second_bits);
        let second = (second_bits as f64) < SEARCH_BETTER * first_bits as f64;
        if second {
            self.current = self.current.other();
            self.changed += 1;
        }
        second
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A model of an encoder: a picture's cost at QP 30, halving with six
    /// steps of QP; a key frame costs eight times a frame after it.
    fn cost(complexity: f64, qp: i32, key: bool) -> u64 {
        let bits = complexity * 2f64.powf((30 - qp) as f64 / 6.0);
        (if key { bits * 8.0 } else { bits }) as u64
    }

    /// Runs `frames` through the controller; returns every frame's bits.
    fn run(
        control: &mut RateControl,
        frames: usize,
        key_every: usize,
        complexity: impl Fn(usize) -> f64,
    ) -> Vec<u64> {
        let mut sent = Vec::new();
        for frame in 0..frames {
            let key = frame % key_every == 0;
            control.tick();
            let mut qp = control.next_qp(key);
            let mut bits = cost(complexity(frame), qp, key);
            if let Some(higher) = control.again(bits, qp) {
                assert!(higher > qp);
                qp = higher;
                bits = cost(complexity(frame), qp, key);
            }
            if key || control.fits(bits) {
                control.sent(bits, qp, key);
                sent.push(bits);
            } else {
                // A frame of skipped macroblocks: 80 bits.
                control.left_out(80, qp);
                sent.push(80);
            }
        }
        sent
    }

    fn worst_window(sent: &[u64], frames: usize, target: f64) -> (f64, f64) {
        let mut low = f64::INFINITY;
        let mut high: f64 = 0.0;
        for window in sent.windows(frames) {
            let ratio = window.iter().sum::<u64>() as f64 / target;
            low = low.min(ratio);
            high = high.max(ratio);
        }
        (low, high)
    }

    #[test]
    fn a_frame_that_is_too_large_and_due_the_other_search_gets_both_in_one_pass() {
        // One pass: the first is all there is.
        for (other, too_large) in [(false, false), (true, false), (false, true), (true, true)] {
            assert_eq!(second_pass(1, other, too_large), None);
        }
        for passes in [2, 3] {
            assert_eq!(second_pass(passes, false, false), None);
            assert_eq!(second_pass(passes, true, false), Some(Second::Other));
            assert_eq!(second_pass(passes, false, true), Some(Second::Higher));
        }
        // Two: both at once. Three: the other search, and a higher QP after it.
        assert_eq!(second_pass(2, true, true), Some(Second::OtherHigher));
        assert_eq!(second_pass(3, true, true), Some(Second::Other));
        assert_eq!(PASSES, 2);
    }

    #[test]
    fn the_other_search_at_a_higher_qp_is_judged_by_what_the_first_would_have_made_there() {
        // 800 kbit at QP 30 are 400 at QP 36 by the rule of thumb.
        let mut choice = SearchChoice::new();
        assert_eq!(choice.first(), Search::Exact);
        // 380 kbit: not a tenth less. The search stays, and rests.
        assert!(!choice.compared_higher(800_000, 6, 380_000));
        assert_eq!(
            (choice.first(), choice.tried, choice.changed),
            (Search::Exact, 1, 0)
        );
        assert_eq!(choice.other(500_000, 1), None);
        // Unless a frame costs twice what the tried one did.
        assert_eq!(choice.other(1_000_000, 1), Some(Search::Wide));
        // 100 kbit: the other search found what the first did not.
        let mut choice = SearchChoice::new();
        assert!(choice.compared_higher(800_000, 6, 100_000));
        assert_eq!(
            (choice.first(), choice.tried, choice.changed),
            (Search::Wide, 1, 1)
        );
        // No step of QP between them: the comparison of two passes at one QP.
        let mut choice = SearchChoice::new();
        assert!(choice.compared_higher(800_000, 0, 700_000));
        let mut choice = SearchChoice::new();
        assert!(!choice.compared_higher(800_000, 0, 730_000));
    }

    #[test]
    fn a_steady_picture_is_held_at_the_rate() {
        for kbps in [1500, 2500, 5000, 8000] {
            let mut control = RateControl::new(kbps, 30);
            // A frame costs 200 kbit at QP 30: far too much at every rate.
            let sent = run(&mut control, 1200, 100_000, |_| 200_000.0);
            // After the first two seconds every ten seconds are within 5%.
            let (low, high) = worst_window(&sent[60..], 300, kbps as f64 * 10_000.0);
            assert!(low > 0.95 && high < 1.05, "{kbps}: {low} to {high}");
            assert_eq!(control.overflows, 0, "{kbps}");
        }
    }

    #[test]
    fn a_change_of_scene_is_caught_by_a_second_pass() {
        let mut control = RateControl::new(2500, 30);
        // Easy for ten seconds, then ten times as costly.
        let sent = run(&mut control, 900, 100_000, |frame| {
            if frame < 300 { 60_000.0 } else { 600_000.0 }
        });
        assert_eq!(control.overflows, 0);
        // No frame after the change is larger than the bucket.
        assert!(sent[300..].iter().all(|bits| *bits <= 2_500_000));
        let (_, high) = worst_window(&sent[1..], 300, 25_000_000.0);
        assert!(high < 1.15, "{high}");
        // And the stream is back at its rate.
        let (low, high) = worst_window(&sent[450..], 300, 25_000_000.0);
        assert!(low > 0.9 && high < 1.1, "{low} to {high}");
    }

    #[test]
    fn key_frames_are_planned_into_the_bucket() {
        let mut control = RateControl::new(2500, 30);
        // A key frame every second.
        let sent = run(&mut control, 900, 30, |_| 150_000.0);
        assert_eq!(control.overflows, 0);
        let (low, high) = worst_window(&sent[90..], 300, 25_000_000.0);
        assert!(low > 0.85 && high < 1.15, "{low} to {high}");
        // From the third key frame on no second pass is needed: its QP is
        // planned from the one before.
        control.tick();
        let qp = control.next_qp(true);
        assert!(control.again(cost(150_000.0, qp, true), qp).is_none());
    }

    #[test]
    fn a_still_picture_gets_the_lowest_qp_and_stays_below_the_rate() {
        let mut control = RateControl::new(8000, 30);
        // Half a kilobit a frame at QP 30: a pointer on a still desktop.
        let sent = run(&mut control, 600, 100_000, |_| 500.0);
        assert_eq!(control.next_qp(false), QP_MIN);
        let (_, high) = worst_window(&sent, 300, 80_000_000.0);
        assert!(high < 0.1, "{high}");
    }

    #[test]
    fn a_picture_too_costly_at_any_qp_is_held_by_leaving_frames_out() {
        for kbps in [1500, 2500] {
            let mut control = RateControl::new(kbps, 30);
            // 100 kbit a frame at the coarsest QP: twice what 1.5 Mbit/s has.
            let sent = run(&mut control, 1200, 100_000, |_| 1_130_000.0);
            assert_eq!(control.overflows, 0, "{kbps}");
            assert!(control.skipped > 100, "{kbps}: {}", control.skipped);
            // Every ten seconds are within 12% of the rate, the first
            // included, and the bucket is never overrun.
            let (low, high) = worst_window(&sent, 300, kbps as f64 * 10_000.0);
            assert!(low > 0.88 && high < 1.12, "{kbps}: {low} to {high}");
            // Frames that go out are at the coarsest QP.
            assert_eq!(control.next_qp(false), QP_MAX);
        }
    }

    #[test]
    fn a_picture_that_rests_after_motion_comes_back_at_the_rates_pace() {
        // A picture that rests costs a frame only what makes it finer:
        // about a frame's bits at 8 Mbit/s for every step of QP below the
        // finest it has had (a page of text at 1920x1080, V3.4).
        let step = |qp: i32| (120_000.0 + (48 - qp).max(-3) as f64 * 12_000.0) as u64;
        let frame = |qp: i32, finest: i32| 2_400 + (qp..finest).map(step).sum::<u64>();

        let mut control = RateControl::new(8000, 30);
        // A key frame, and a second of a picture that costs 1.2 Mbit a
        // frame at any QP: the bucket is full and the QP at its coarsest.
        control.tick();
        control.sent(1_000_000, control.next_qp(true), true);
        for _ in 0..30 {
            control.tick();
            let qp = control.next_qp(false);
            let qp = control.again(1_200_000, qp).unwrap_or(qp);
            if control.fits(1_200_000) {
                control.sent(1_200_000, qp, false);
            } else {
                control.left_out(80, qp);
            }
        }
        assert_eq!(control.next_qp(false), QP_MAX);
        assert!(!control.fits(1_200_000));

        let mut finest = QP_MAX;
        let mut reached = Vec::new();
        for _ in 0..60 {
            control.tick();
            let mut qp = control.next_qp(false);
            let mut bits = frame(qp, finest);
            if let Some(higher) = control.again(bits, qp) {
                qp = higher;
                bits = frame(qp, finest);
            }
            assert!(control.fits(bits));
            control.sent(bits, qp, false);
            finest = finest.min(qp);
            reached.push(finest);
        }
        assert_eq!(control.overflows, 0);
        // The rate pays for about a step a frame: 15 steps to a readable
        // page, 21 to a good one, and that is what it takes. A controller
        // that let the QP fall by one step after a cheap frame, and
        // emptied the bucket first, needed 28 and 41 frames.
        let frames_to = |qp: i32| reached.iter().position(|finest| *finest <= qp).unwrap() + 1;
        assert!(frames_to(36) <= 20, "{}", frames_to(36));
        assert!(frames_to(30) <= 28, "{}", frames_to(30));
    }

    #[test]
    fn a_costly_frame_is_tried_with_the_other_search() {
        let mut choice = SearchChoice::new();
        assert_eq!(choice.first(), Search::Exact);
        // A cheap frame is not tried twice.
        choice.tick();
        assert_eq!(choice.other(9_999, 10_000), None);
        // A costly one is, and the smaller pass is the frame.
        assert_eq!(choice.other(400_000, 10_000), Some(Search::Wide));
        assert!(choice.compared(400_000, 60_000));
        assert_eq!(choice.first(), Search::Wide);
        assert_eq!((choice.tried, choice.changed), (1, 1));
        // The other search rests for three frames of that size.
        for _ in 0..3 {
            choice.tick();
            assert_eq!(choice.other(100_000, 10_000), None);
        }
        choice.tick();
        assert_eq!(choice.other(100_000, 10_000), Some(Search::Exact));
        // A pass that is not a tenth smaller changes nothing.
        assert!(!choice.compared(100_000, 95_000));
        assert_eq!(choice.first(), Search::Wide);
        assert_eq!((choice.tried, choice.changed), (2, 1));
        // A frame of twice the size does not wait: the picture moves
        // otherwise now.
        choice.tick();
        assert_eq!(choice.other(189_999, 10_000), None);
        assert_eq!(choice.other(190_000, 10_000), Some(Search::Exact));
        assert!(choice.compared(190_000, 30_000));
        assert_eq!(choice.first(), Search::Exact);
        // And a cheap frame is never tried, whatever came before.
        for _ in 0..10 {
            choice.tick();
        }
        assert_eq!(choice.other(9_999, 10_000), None);
    }

    #[test]
    fn the_first_frame_of_a_costly_stream_does_not_overflow() {
        let mut control = RateControl::new(1500, 30);
        // The first frame is a key frame of 4 Mbit at the starting QP.
        control.tick();
        let qp = control.next_qp(true);
        assert_eq!(qp, QP_START);
        let bits = cost(500_000.0, qp, true);
        let higher = control.again(bits, qp).unwrap();
        let second = cost(500_000.0, higher, true);
        assert!(second <= 1_500_000, "{second}");
        control.sent(second, higher, true);
        assert_eq!(control.overflows, 0);
    }
}
