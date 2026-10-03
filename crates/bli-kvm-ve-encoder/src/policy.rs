//! A frame's passes: what the rate control and the choice of the search
//! make of one picture, as a sequence of questions and answers. Whoever
//! drives the engine asks how to encode the picture, says how many bits
//! came out, and is told to encode it again otherwise, to send a pass, or
//! to leave the frame out. The lab's long runs and the product's encoder
//! follow the same sequence, so what the lab measured is what the product
//! does.
//!
//! A frame takes at most so many passes (two, unless more are given): the
//! engine's time for a pass does not depend on what the picture shows, so
//! the passes are what decides whether a frame is done within its time.

use crate::rate::{self, RateControl, Search, SearchChoice, Second};

/// How a stream's frames get their QP.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Quantiser {
    /// Every frame at this QP: the lab's.
    Fixed(i32),
    /// From the rate control.
    Rate(RateControl),
}

/// How a stream's passes get their search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Searching {
    /// As whoever drives the engine has it; nothing is said.
    Unsaid,
    /// Every pass with this one.
    Fixed(Search),
    /// Chosen for every frame.
    Chosen(SearchChoice),
}

/// One pass to make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ask {
    pub qp: i32,
    /// None: as whoever drives the engine has it.
    pub search: Option<Search>,
}

/// Which of the passes made over the picture so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    Last,
    /// The pass before the last.
    Earlier,
}

/// What follows a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// The picture once more, against the same reference. `better` is the
    /// pass that the new one must not write over: of those made so far the
    /// one that would be sent.
    Again { ask: Ask, better: Which },
    /// The frame is this pass. `sent` is called when it has gone out.
    Send(Which),
    /// No pass fits and the stream can do without the frame: a frame that
    /// says "nothing changed" goes out in its place, and `left_out` is
    /// told its bits.
    LeaveOut,
}

/// The passes a frame of `pixels` (whole macroblocks) may take at `fps`
/// frames a second so that it is done within its time, by what the lab
/// measured on the board (V3.3): a pass takes the engine 3.6 ns a pixel and
/// the loop around it a millisecond, the picture's way in takes 1.8 ns a
/// pixel once a frame, and a process that waits for a busy core wakes up to
/// 4 ms late after every pass. At least one, at most three: two for
/// 1920x1080 at 30 frames a second, three for 1366x768.
pub fn passes_for(pixels: u64, fps: u32) -> u32 {
    let interval = 1_000_000_000 / fps.max(1) as u64;
    let picture = pixels * 18 / 10;
    let pass = pixels * 36 / 10 + 5_000_000;
    (interval.saturating_sub(picture) / pass).clamp(1, 3) as u32
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Between frames.
    Idle,
    /// The first pass is being made.
    First { qp: i32 },
    /// The pass with the other search is being made.
    Other { qp: i32, first_bits: u64 },
    /// The pass at a higher QP is being made.
    Higher { qp: i32 },
    /// The pass with the other search at a QP `steps` higher is being
    /// made: both in one, for a frame that may take only one more pass.
    OtherHigher {
        qp: i32,
        first_bits: u64,
        steps: i32,
    },
    /// A pass is to be sent.
    Sending { bits: u64, qp: i32, key: bool },
    /// The frame is to be left out, after this QP was not enough.
    LeavingOut { qp: i32 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FramePolicy {
    quantiser: Quantiser,
    searching: Searching,
    /// A frame is costly, and worth a pass with the other search, from
    /// this many bits.
    costly: u64,
    state: State,
    /// The passes a frame may take, and those the picture has had.
    passes: u32,
    made: u32,
    /// The search of the pass that would be sent, and of the one that was
    /// sent last.
    pass_search: Option<Search>,
    sent_search: Option<Search>,
    /// The frame that went out last was a key frame.
    after_key: bool,
}

impl FramePolicy {
    /// At a bitrate a frame is costly from three quarters of what a frame
    /// may cost on average; at a fixed QP from 0.03 bits a pixel.
    pub fn costly_bits(quantiser: &Quantiser, kbps: u32, fps: u32, pixels: u64) -> u64 {
        match quantiser {
            Quantiser::Rate(_) => kbps as u64 * 1000 * 3 / 4 / fps.max(1) as u64,
            Quantiser::Fixed(_) => pixels * 3 / 100,
        }
    }

    pub fn new(quantiser: Quantiser, searching: Searching, costly: u64) -> Self {
        Self {
            quantiser,
            searching,
            costly,
            state: State::Idle,
            passes: rate::PASSES,
            made: 0,
            pass_search: None,
            sent_search: None,
            after_key: false,
        }
    }

    /// The same with at most `passes` passes for a frame, 1 to 3. With
    /// three a costly frame that does not fit is encoded with the other
    /// search and then at a higher QP; with two it gets both in its one
    /// more pass, the other search at the higher QP; with one a frame is
    /// what its first pass made of it.
    pub fn with_passes(mut self, passes: u32) -> Self {
        self.passes = passes.clamp(1, 3);
        self
    }

    /// The passes made over the picture of the frame that began last.
    pub fn passes_made(&self) -> u32 {
        self.made
    }

    /// Another rate for a stream that runs. Nothing changes for a stream
    /// at a fixed QP.
    pub fn set_rate(&mut self, kbps: u32, fps: u32) {
        if let Quantiser::Rate(control) = &mut self.quantiser {
            control.set_rate(kbps, fps);
            self.costly = Self::costly_bits(&self.quantiser, kbps, fps, 0);
        }
    }

    pub fn control(&self) -> Option<&RateControl> {
        match &self.quantiser {
            Quantiser::Rate(control) => Some(control),
            Quantiser::Fixed(_) => None,
        }
    }

    pub fn control_mut(&mut self) -> Option<&mut RateControl> {
        match &mut self.quantiser {
            Quantiser::Rate(control) => Some(control),
            Quantiser::Fixed(_) => None,
        }
    }

    pub fn choice(&self) -> Option<&SearchChoice> {
        match &self.searching {
            Searching::Chosen(choice) => Some(choice),
            _ => None,
        }
    }

    /// The search of the next pass.
    fn search(&self) -> Option<Search> {
        match &self.searching {
            Searching::Unsaid => None,
            Searching::Fixed(search) => Some(*search),
            Searching::Chosen(choice) => Some(choice.first()),
        }
    }

    /// A frame begins: its first pass. `key` says that a key frame is
    /// asked for, or that the stream begins.
    pub fn begin(&mut self, key: bool) -> Ask {
        let qp = match &mut self.quantiser {
            Quantiser::Fixed(qp) => *qp,
            Quantiser::Rate(control) => {
                control.tick();
                control.next_qp(key)
            }
        };
        if let Searching::Chosen(choice) = &mut self.searching {
            choice.tick();
        }
        self.state = State::First { qp };
        self.made = 1;
        self.pass_search = self.search();
        Ask {
            qp,
            search: self.search(),
        }
    }

    /// The pass that was asked for made `bits`; `key` says whether it is a
    /// key frame.
    pub fn made(&mut self, bits: u64, key: bool) -> Next {
        match self.state {
            State::First { qp } => {
                // A frame the bucket has no room for needs a pass at a
                // higher QP.
                let higher = match &self.quantiser {
                    Quantiser::Fixed(_) => None,
                    Quantiser::Rate(control) => control.again(bits, qp),
                };
                // A costly frame that is not a key frame is due the other
                // search.
                let other = match &self.searching {
                    Searching::Chosen(choice) if !key => choice.other(bits, self.costly),
                    _ => None,
                };
                let second = rate::second_pass(
                    self.passes,
                    other.is_some(),
                    higher.is_some(),
                    self.after_key,
                );
                match (second, other, higher) {
                    (Some(Second::Other), Some(other), _) => {
                        self.state = State::Other {
                            qp,
                            first_bits: bits,
                        };
                        self.made += 1;
                        Next::Again {
                            ask: Ask {
                                qp,
                                search: Some(other),
                            },
                            better: Which::Last,
                        }
                    }
                    (Some(Second::OtherHigher), Some(other), Some(higher)) => {
                        self.state = State::OtherHigher {
                            qp: higher,
                            first_bits: bits,
                            steps: higher - qp,
                        };
                        self.made += 1;
                        self.pass_search = Some(other);
                        Next::Again {
                            ask: Ask {
                                qp: higher,
                                search: Some(other),
                            },
                            better: Which::Last,
                        }
                    }
                    (Some(Second::Higher), _, Some(higher)) => {
                        self.state = State::Higher { qp: higher };
                        self.made += 1;
                        Next::Again {
                            ask: Ask {
                                qp: higher,
                                search: self.search(),
                            },
                            better: Which::Last,
                        }
                    }
                    _ => self.decide(bits, qp, key, Which::Last),
                }
            }
            State::Other { qp, first_bits } => {
                let Searching::Chosen(choice) = &mut self.searching else {
                    unreachable!("only a chosen search is tried twice");
                };
                if choice.compared(first_bits, bits) {
                    self.pass_search = Some(choice.first());
                    self.fit(bits, qp, key, Which::Last)
                } else {
                    self.fit(first_bits, qp, key, Which::Earlier)
                }
            }
            State::OtherHigher {
                qp,
                first_bits,
                steps,
            } => {
                let Searching::Chosen(choice) = &mut self.searching else {
                    unreachable!("only a chosen search is tried twice");
                };
                // The pass at the QP that was meant to fit is the frame,
                // whichever search the next frame begins with.
                choice.compared_higher(first_bits, steps, bits);
                self.decide(bits, qp, key, Which::Last)
            }
            State::Higher { qp } => self.decide(bits, qp, key, Which::Last),
            State::Idle | State::Sending { .. } | State::LeavingOut { .. } => {
                panic!("a pass was made that was not asked for")
            }
        }
    }

    /// The better pass has `bits`: does it fit, or is one more pass due?
    fn fit(&mut self, bits: u64, qp: i32, key: bool, better: Which) -> Next {
        let higher = match &self.quantiser {
            Quantiser::Rate(control) if self.made < self.passes => control.again(bits, qp),
            _ => None,
        };
        match higher {
            Some(higher) => {
                self.state = State::Higher { qp: higher };
                self.made += 1;
                Next::Again {
                    ask: Ask {
                        qp: higher,
                        search: self.search(),
                    },
                    better,
                }
            }
            None => self.decide(bits, qp, key, better),
        }
    }

    /// A frame that fits at no QP is left out, unless it is a key frame:
    /// the stream needs it to begin with.
    fn decide(&mut self, bits: u64, qp: i32, key: bool, which: Which) -> Next {
        let fits = match &self.quantiser {
            Quantiser::Fixed(_) => true,
            Quantiser::Rate(control) => key || control.fits(bits),
        };
        if fits {
            self.state = State::Sending { bits, qp, key };
            Next::Send(which)
        } else {
            self.state = State::LeavingOut { qp };
            Next::LeaveOut
        }
    }

    /// The pass has gone out. Returns its QP.
    pub fn sent(&mut self) -> i32 {
        let State::Sending { bits, qp, key } = self.state else {
            panic!("nothing was to be sent");
        };
        if let Quantiser::Rate(control) = &mut self.quantiser {
            control.sent(bits, qp, key);
        }
        self.sent_search = self.pass_search;
        self.after_key = key;
        self.state = State::Idle;
        qp
    }

    /// The frame was left out: `bits` of "nothing changed" went out.
    pub fn left_out(&mut self, bits: u64) {
        let State::LeavingOut { qp } = self.state else {
            panic!("nothing was to be left out");
        };
        if let Quantiser::Rate(control) = &mut self.quantiser {
            control.left_out(bits, qp);
        }
        self.after_key = false;
        self.state = State::Idle;
    }

    /// The engine failed in the middle of a frame: nothing went out, and
    /// the next frame begins as if this one had not been tried.
    pub fn failed(&mut self) {
        self.state = State::Idle;
    }

    /// The search the last frame that went out was encoded with, where the
    /// policy says it.
    pub fn sent_search(&self) -> Option<Search> {
        self.sent_search
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chosen(kbps: u32) -> FramePolicy {
        let quantiser = Quantiser::Rate(RateControl::new(kbps, 30));
        let costly = FramePolicy::costly_bits(&quantiser, kbps, 30, 1920 * 1080);
        FramePolicy::new(quantiser, Searching::Chosen(SearchChoice::new()), costly)
    }

    #[test]
    fn a_frame_is_costly_from_three_quarters_of_its_share() {
        let rate = Quantiser::Rate(RateControl::new(8000, 30));
        assert_eq!(
            FramePolicy::costly_bits(&rate, 8000, 30, 1920 * 1080),
            200_000
        );
        assert_eq!(
            FramePolicy::costly_bits(&Quantiser::Fixed(30), 8000, 30, 1920 * 1080),
            62_208
        );
    }

    #[test]
    fn the_passes_a_frame_may_take_end_within_its_time() {
        // Whole macroblocks: 1920x1088, 1376x768, 1280x720, 320x240.
        assert_eq!(passes_for(1920 * 1088, 30), 2);
        assert_eq!(passes_for(1376 * 768, 30), 3);
        assert_eq!(passes_for(1280 * 720, 30), 3);
        assert_eq!(passes_for(320 * 240, 30), 3);
        // Faster than the engine and a second pass allow: one.
        assert_eq!(passes_for(1920 * 1088, 60), 1);
        assert_eq!(passes_for(1920 * 1088, 25), 2);
        assert_eq!(passes_for(1920 * 1088, 15), 3);
        // Never none, whatever is asked.
        assert_eq!(passes_for(1920 * 1088, 1000), 1);
        assert_eq!(passes_for(1920 * 1088, 0), 3);
    }

    #[test]
    fn a_cheap_frame_is_one_pass() {
        let mut policy = chosen(8000);
        let ask = policy.begin(true);
        assert_eq!(ask.search, Some(Search::Exact));
        // A key frame is never tried with the other search.
        assert_eq!(policy.made(900_000, true), Next::Send(Which::Last));
        assert_eq!(policy.sent(), ask.qp);
        let ask = policy.begin(false);
        assert_eq!(policy.made(40_000, false), Next::Send(Which::Last));
        policy.sent();
        assert_eq!(policy.sent_search(), Some(Search::Exact));
        assert_eq!(ask.search, Some(Search::Exact));
    }

    #[test]
    fn a_costly_frame_is_tried_with_the_other_search_and_the_better_pass_is_sent() {
        let mut policy = chosen(8000);
        policy.begin(true);
        policy.made(900_000, true);
        policy.sent();

        // The other search makes a fifth of the bits: it is the frame, and
        // the next frame begins with it.
        let ask = policy.begin(false);
        let Next::Again { ask: again, better } = policy.made(1_000_000, false) else {
            panic!("the other search was not tried");
        };
        assert_eq!((again.qp, again.search), (ask.qp, Some(Search::Wide)));
        assert_eq!(better, Which::Last);
        assert_eq!(policy.made(200_000, false), Next::Send(Which::Last));
        policy.sent();
        assert_eq!(policy.sent_search(), Some(Search::Wide));
        assert_eq!(policy.begin(false).search, Some(Search::Wide));
        assert_eq!(policy.made(100_000, false), Next::Send(Which::Last));
        policy.sent();

        // Three frames of rest, then a costly frame is tried again; the
        // other search is no better, and the first pass is the frame.
        for _ in 0..3 {
            policy.begin(false);
            assert_eq!(policy.made(190_000, false), Next::Send(Which::Last));
            policy.sent();
        }
        policy.begin(false);
        let Next::Again { ask: again, .. } = policy.made(250_000, false) else {
            panic!("the other search was not tried again");
        };
        assert_eq!(again.search, Some(Search::Exact));
        assert_eq!(policy.made(240_000, false), Next::Send(Which::Earlier));
        policy.sent();
        assert_eq!(policy.sent_search(), Some(Search::Wide));
        assert_eq!(policy.choice().unwrap().tried, 2);
        assert_eq!(policy.choice().unwrap().changed, 1);
    }

    /// A key frame and one frame after it, so that what follows is not
    /// the frame after a key frame.
    fn begun(kbps: u32) -> FramePolicy {
        let mut policy = chosen(kbps);
        policy.begin(true);
        policy.made(600_000, true);
        policy.sent();
        policy.begin(false);
        assert_eq!(policy.made(20_000, false), Next::Send(Which::Last));
        policy.sent();
        policy
    }

    #[test]
    fn of_two_passes_a_frame_that_is_too_large_and_costly_gets_both_in_one() {
        // More than the bucket has room for, and due the other search:
        // the one more pass is the other search at a higher QP.
        let mut policy = begun(2500);
        let ask = policy.begin(false);
        let Next::Again {
            ask: higher,
            better,
        } = policy.made(4_000_000, false)
        else {
            panic!("no second pass");
        };
        assert!(higher.qp > ask.qp);
        assert_eq!((higher.search, better), (Some(Search::Wide), Which::Last));
        // A tenth of what the first search would have made there: the
        // pass is the frame and its search the next frame's.
        assert_eq!(policy.made(150_000, false), Next::Send(Which::Last));
        assert_eq!(policy.sent(), higher.qp);
        assert_eq!(policy.passes_made(), 2);
        assert_eq!(policy.sent_search(), Some(Search::Wide));
        assert_eq!(policy.choice().unwrap().changed, 1);
        assert_eq!(policy.begin(false).search, Some(Search::Wide));
        assert_eq!(policy.made(40_000, false), Next::Send(Which::Last));
        policy.sent();

        // The same where the other search is no better: its pass is the
        // frame all the same, and the next frame begins with the search
        // this one began with.
        let mut policy = begun(2500);
        let first = policy.begin(false);
        let Next::Again { ask: higher, .. } = policy.made(4_000_000, false) else {
            panic!("no second pass");
        };
        let steps = higher.qp - first.qp;
        let expected = (4_000_000.0 / 2f64.powf(steps as f64 / 6.0)) as u64;
        assert_eq!(policy.made(expected, false), Next::Send(Which::Last));
        policy.sent();
        assert_eq!(policy.sent_search(), Some(Search::Wide));
        assert_eq!(policy.choice().unwrap().changed, 0);
        assert_eq!(policy.begin(false).search, Some(Search::Exact));
        // While the other search rests, a frame that does not fit is
        // encoded again with the same search.
        let Next::Again { ask: again, .. } = policy.made(2_600_000, false) else {
            panic!("no pass at a higher QP");
        };
        assert_eq!(again.search, Some(Search::Exact));
        // A frame that does not fit after its second pass is left out.
        assert_eq!(policy.made(9_000_000, false), Next::LeaveOut);
        policy.left_out(80);
        assert_eq!(policy.passes_made(), 2);

        // A frame that fits and is costly: the other search at the same
        // QP, and no third pass whatever comes of it.
        let mut policy = begun(2500);
        let ask = policy.begin(false);
        let Next::Again { ask: other, .. } = policy.made(700_000, false) else {
            panic!("the other search was not tried");
        };
        assert_eq!((other.qp, other.search), (ask.qp, Some(Search::Wide)));
        assert_eq!(policy.made(150_000, false), Next::Send(Which::Last));
        policy.sent();
        assert_eq!(policy.passes_made(), 2);
        assert_eq!(policy.sent_search(), Some(Search::Wide));

        // One pass: a frame is what its first pass made of it.
        let mut policy = chosen(2500).with_passes(1);
        policy.begin(true);
        policy.made(600_000, true);
        policy.sent();
        policy.begin(false);
        assert_eq!(policy.made(700_000, false), Next::Send(Which::Last));
        policy.sent();
        policy.begin(false);
        assert_eq!(policy.made(9_000_000, false), Next::LeaveOut);
        policy.left_out(80);
        assert_eq!(policy.passes_made(), 1);
    }

    #[test]
    fn the_frame_after_a_key_frame_gets_the_higher_qp_with_its_own_search() {
        let mut policy = chosen(2500);
        policy.begin(true);
        policy.made(600_000, true);
        policy.sent();
        // Too large, and the other search has never been tried: right
        // after a key frame the one more pass is the rate's all the same.
        let ask = policy.begin(false);
        let Next::Again { ask: higher, .. } = policy.made(4_000_000, false) else {
            panic!("no pass at a higher QP");
        };
        assert!(higher.qp > ask.qp);
        assert_eq!(higher.search, Some(Search::Exact));
        assert_eq!(policy.made(150_000, false), Next::Send(Which::Last));
        policy.sent();
        assert_eq!(policy.choice().unwrap().tried, 0);
        // The frame after that one is judged as every frame.
        policy.begin(false);
        let Next::Again { ask: both, .. } = policy.made(4_000_000, false) else {
            panic!("no second pass");
        };
        assert_eq!(both.search, Some(Search::Wide));
        assert_eq!(policy.made(100_000, false), Next::Send(Which::Last));
        policy.sent();
        // A frame that fits is tried with the other search after a key
        // frame too: nothing can be lost by that.
        let mut policy = chosen(8000);
        policy.begin(true);
        policy.made(900_000, true);
        policy.sent();
        let ask = policy.begin(false);
        let Next::Again { ask: other, .. } = policy.made(1_000_000, false) else {
            panic!("the other search was not tried");
        };
        assert_eq!((other.qp, other.search), (ask.qp, Some(Search::Wide)));
        // A frame that is left out is not a key frame: what follows it is
        // judged as every frame.
        let mut policy = chosen(2500);
        policy.begin(true);
        policy.made(600_000, true);
        policy.sent();
        policy.begin(false);
        let mut next = policy.made(9_000_000, false);
        while let Next::Again { .. } = next {
            next = policy.made(9_000_000, false);
        }
        assert_eq!(next, Next::LeaveOut);
        policy.left_out(80);
        policy.begin(false);
        let Next::Again { ask: both, .. } = policy.made(4_000_000, false) else {
            panic!("no second pass");
        };
        assert_eq!(both.search, Some(Search::Wide));
    }

    #[test]
    fn with_three_passes_a_frame_that_does_not_fit_gets_both() {
        let mut policy = chosen(2500).with_passes(3);
        policy.begin(true);
        policy.made(600_000, true);
        policy.sent();

        // Both searches make more than the bucket has room for: a pass at
        // a higher QP with the search that made less, which fits.
        let ask = policy.begin(false);
        let Next::Again { ask: other, .. } = policy.made(4_000_000, false) else {
            panic!("the other search was not tried");
        };
        assert_eq!(other.qp, ask.qp);
        let Next::Again {
            ask: higher,
            better,
        } = policy.made(3_000_000, false)
        else {
            panic!("no pass at a higher QP");
        };
        assert!(higher.qp > ask.qp);
        assert_eq!((higher.search, better), (Some(Search::Wide), Which::Last));
        assert_eq!(policy.made(900_000, false), Next::Send(Which::Last));
        assert_eq!(policy.sent(), higher.qp);
        assert_eq!(policy.passes_made(), 3);

        // A frame that fits at no QP is left out, and the control knows.
        let before = policy.control().unwrap().skipped;
        policy.begin(false);
        let mut next = policy.made(9_000_000, false);
        while let Next::Again { .. } = next {
            next = policy.made(9_000_000, false);
        }
        assert_eq!(next, Next::LeaveOut);
        policy.left_out(80);
        assert_eq!(policy.control().unwrap().skipped, before + 1);

        // A key frame goes out whatever it costs.
        policy.begin(true);
        let mut next = policy.made(9_000_000, true);
        while let Next::Again { .. } = next {
            next = policy.made(9_000_000, true);
        }
        assert_eq!(next, Next::Send(Which::Last));
        policy.sent();
    }

    #[test]
    fn a_fixed_qp_and_a_fixed_search_ask_for_one_pass() {
        let mut policy =
            FramePolicy::new(Quantiser::Fixed(30), Searching::Fixed(Search::Wide), 62_208);
        for bits in [5_000_000, 100, 700_000] {
            let ask = policy.begin(false);
            assert_eq!(
                ask,
                Ask {
                    qp: 30,
                    search: Some(Search::Wide)
                }
            );
            assert_eq!(policy.made(bits, false), Next::Send(Which::Last));
            assert_eq!(policy.sent(), 30);
        }
        // Nothing said about the search: nothing asked.
        let mut policy = FramePolicy::new(Quantiser::Fixed(12), Searching::Unsaid, 62_208);
        assert_eq!(
            policy.begin(true),
            Ask {
                qp: 12,
                search: None
            }
        );
        assert_eq!(policy.made(1_000_000, true), Next::Send(Which::Last));
        policy.sent();
        assert_eq!(policy.sent_search(), None);
    }

    #[test]
    fn a_failure_in_the_middle_of_a_frame_leaves_the_policy_ready() {
        let mut policy = chosen(8000);
        policy.begin(true);
        policy.failed();
        let ask = policy.begin(true);
        assert_eq!(policy.made(900_000, true), Next::Send(Which::Last));
        assert_eq!(policy.sent(), ask.qp);
    }
}
