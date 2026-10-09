//! The share of the chosen bitrate that a session's path carries
//! (`docs/design/quic-transport-v1.md`, "The bitrate"). The board has one
//! H.264 encoder for every viewer, and the server runs it at the chosen
//! bitrate times the least share of the sessions that watch. A session's
//! share falls when a queue builds in front of its page, the path carrying
//! less than the encoder makes, and climbs back while the page keeps up.
//!
//! The page's lag is how long the oldest frame it has not said it received
//! has been on its way (`video.rs`). The least lag of each second stands for
//! the path in that second: the page says what it received four times a
//! second, and the least lag is the one just after it said. The least of
//! those over half a minute is the path without a queue: its round trip and
//! the page's own delay in saying.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use bli_kvm_quic_protocol::link::LEAST_VIDEO_SHARE;

/// The whole chosen bitrate.
pub const FULL: u8 = 100;

const SECOND: Duration = Duration::from_secs(1);
/// The span whose least lag is the path's without a queue.
const BASE_SPAN: Duration = Duration::from_secs(30);
/// The first seconds of a picture change nothing: its first key frame and
/// the path's first round trips tell little of the queue.
const WARM_UP: Duration = Duration::from_secs(3);
/// A second whose least lag is this far above the base, and no shorter than
/// the second's before: a queue builds, and the share falls by a quarter...
const QUEUED: Duration = Duration::from_millis(300);
/// ... at most once in this time, in which the lower bitrate shows.
const DOWN_SPACING: Duration = Duration::from_secs(2);
/// A second whose least lag is no further than this above the base: the
/// page keeps up.
const CLEAR: Duration = Duration::from_millis(100);
/// The share climbs by a fifth once the page has kept up this long, and
/// again after as long.
const UP_AFTER: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub struct PathShare {
    percent: u8,
    started: Instant,
    /// The second under way: when it began, and its least lag so far.
    second: Option<(Instant, Duration)>,
    /// The least lag of each second over the base span, oldest first.
    seconds: VecDeque<(Instant, Duration)>,
    /// The last change of the share.
    changed: Option<Instant>,
    /// The end of the last second in which the page did not keep up.
    troubled: Instant,
    /// The last second ended with a queue in front of the page.
    queued: bool,
}

impl PathShare {
    /// A picture that starts with the share its session has.
    pub fn new(percent: u8, now: Instant) -> Self {
        Self {
            percent: percent.clamp(LEAST_VIDEO_SHARE, FULL),
            started: now,
            second: None,
            seconds: VecDeque::new(),
            changed: None,
            troubled: now,
            queued: false,
        }
    }

    pub fn percent(&self) -> u8 {
        self.percent
    }

    /// Whether the last second ended with a queue in front of the page:
    /// what is still on its way is late.
    pub fn queued(&self) -> bool {
        self.queued
    }

    /// The page's lag as a frame is sent; the new share, if it changed.
    pub fn lag(&mut self, now: Instant, lag: Duration) -> Option<u8> {
        let mut changed = None;
        if let Some((began, least)) = self.second
            && now.duration_since(began) >= SECOND
        {
            self.second = None;
            changed = self.judge(began, least, now);
        }
        match &mut self.second {
            Some((_, least)) => *least = (*least).min(lag),
            None => self.second = Some((now, lag)),
        }
        changed
    }

    /// The page fell so far behind that its group of pictures was given up:
    /// the share falls at once.
    pub fn gave_up(&mut self, now: Instant) -> Option<u8> {
        self.troubled = now;
        self.fall(now)
    }

    fn judge(&mut self, began: Instant, least: Duration, now: Instant) -> Option<u8> {
        let before = self.seconds.back().map(|(_, least)| *least);
        self.seconds.push_back((began, least));
        while self
            .seconds
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) > BASE_SPAN)
        {
            self.seconds.pop_front();
        }
        let base = self
            .seconds
            .iter()
            .map(|(_, least)| *least)
            .min()
            .unwrap_or(least);
        let queue = least.saturating_sub(base);
        self.queued = queue > QUEUED;
        if queue > CLEAR {
            self.troubled = now;
        }
        // A queue that drains already needs no lower bitrate.
        let draining = before.is_some_and(|before| least < before);
        if queue > QUEUED && !draining && now.duration_since(self.started) >= WARM_UP {
            return self.fall(now);
        }
        if queue <= CLEAR
            && self.percent < FULL
            && now.duration_since(self.troubled) >= UP_AFTER
            && self
                .changed
                .is_none_or(|at| now.duration_since(at) >= UP_AFTER)
        {
            self.percent = (u16::from(self.percent) * 6 / 5).min(u16::from(FULL)) as u8;
            self.changed = Some(now);
            return Some(self.percent);
        }
        None
    }

    fn fall(&mut self, now: Instant) -> Option<u8> {
        if self.percent == LEAST_VIDEO_SHARE
            || self
                .changed
                .is_some_and(|at| now.duration_since(at) < DOWN_SPACING)
        {
            return None;
        }
        self.percent = (u16::from(self.percent) * 3 / 4).max(u16::from(LEAST_VIDEO_SHARE)) as u8;
        self.changed = Some(now);
        Some(self.percent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Duration = Duration::from_millis(33);

    /// Frames every 33 ms from `from` for `span`, each with the lag `lag`
    /// gives for its time since `from`; the shares the controller asked for.
    fn frames(
        share: &mut PathShare,
        from: Instant,
        span: Duration,
        lag: impl Fn(Duration) -> Duration,
    ) -> Vec<(Duration, u8)> {
        let mut asked = Vec::new();
        let mut at = Duration::ZERO;
        while at < span {
            if let Some(percent) = share.lag(from + at, lag(at)) {
                asked.push((at, percent));
            }
            at += FRAME;
        }
        asked
    }

    /// A path that keeps up: the page reports four times a second, so the
    /// lag rises from its round trip to a quarter second more and drops.
    fn keeping_up(at: Duration) -> Duration {
        Duration::from_millis(40) + Duration::from_millis((at.as_millis() % 250) as u64)
    }

    #[test]
    fn a_path_that_keeps_up_keeps_the_whole_bitrate() {
        let start = Instant::now();
        let mut share = PathShare::new(FULL, start);
        assert!(frames(&mut share, start, Duration::from_secs(60), keeping_up).is_empty());
        assert_eq!(share.percent(), FULL);
    }

    #[test]
    fn a_growing_queue_lowers_the_share_a_quarter_at_a_time() {
        let start = Instant::now();
        let mut share = PathShare::new(FULL, start);
        // Ten seconds of a path that keeps up, then a queue that grows by
        // half a second every second.
        let asked = frames(&mut share, start, Duration::from_secs(20), |at| {
            let queued = at.saturating_sub(Duration::from_secs(10)) / 2;
            keeping_up(at) + queued
        });
        let shares: Vec<u8> = asked.iter().map(|(_, percent)| *percent).collect();
        assert_eq!(shares[..3], [75, 56, 42], "{asked:?}");
        // The queue passes 300 ms 0.6 s into it: the first second that
        // begins after that ends with the first fall, and the next falls
        // come no sooner than two seconds apart.
        assert!(
            asked[0].0 >= Duration::from_millis(10_600)
                && asked[0].0 <= Duration::from_millis(12_700),
            "{asked:?}"
        );
        for pair in asked.windows(2) {
            assert!(pair[1].0 - pair[0].0 >= DOWN_SPACING, "{asked:?}");
        }
        // It never falls below the least share.
        let asked = frames(
            &mut share,
            start + Duration::from_secs(20),
            Duration::from_secs(60),
            |at| keeping_up(at) + Duration::from_secs(5) + at,
        );
        assert_eq!(
            asked.last().map(|(_, percent)| *percent),
            Some(LEAST_VIDEO_SHARE)
        );
        assert_eq!(share.percent(), LEAST_VIDEO_SHARE);
    }

    #[test]
    fn a_queue_that_drains_lowers_nothing_more() {
        let start = Instant::now();
        let mut share = PathShare::new(FULL, start);
        // A queue grows to a second over five seconds, then drains by 200 ms
        // a second.
        let asked = frames(&mut share, start, Duration::from_secs(20), |at| {
            let ms = at.as_millis() as u64;
            let queued = if ms < 10_000 {
                ms.saturating_sub(5000) / 5
            } else {
                1000u64.saturating_sub((ms - 10_000) / 5)
            };
            keeping_up(at) + Duration::from_millis(queued)
        });
        assert!(
            asked
                .iter()
                .all(|(at, _)| *at < Duration::from_millis(11_000)),
            "fell while the queue drained: {asked:?}"
        );
    }

    #[test]
    fn the_first_seconds_change_nothing_but_a_group_given_up_does() {
        let start = Instant::now();
        let mut share = PathShare::new(FULL, start);
        // A first key frame that takes two seconds.
        let asked = frames(&mut share, start, Duration::from_millis(2900), |at| {
            Duration::from_millis(2000)
                .saturating_sub(at)
                .max(Duration::from_millis(40))
                + Duration::from_millis(400)
        });
        assert!(asked.is_empty(), "{asked:?}");
        assert_eq!(share.gave_up(start + Duration::from_secs(1)), Some(75));
        // Not again within two seconds of the last fall.
        assert_eq!(share.gave_up(start + Duration::from_millis(2500)), None);
        assert_eq!(share.gave_up(start + Duration::from_millis(3100)), Some(56));
    }

    #[test]
    fn the_share_climbs_back_while_the_page_keeps_up() {
        let start = Instant::now();
        let mut share = PathShare::new(LEAST_VIDEO_SHARE, start);
        let asked = frames(&mut share, start, Duration::from_secs(100), keeping_up);
        let shares: Vec<u8> = asked.iter().map(|(_, percent)| *percent).collect();
        assert_eq!(shares, [30, 36, 43, 51, 61, 73, 87, 100]);
        // A fifth at a time, ten seconds apart: the first after ten seconds
        // of keeping up.
        assert!(
            asked[0].0 >= UP_AFTER && asked[0].0 <= UP_AFTER + SECOND,
            "{asked:?}"
        );
        for pair in asked.windows(2) {
            assert!(pair[1].0 - pair[0].0 >= UP_AFTER, "{asked:?}");
        }
        // A second in which the page lags, if not enough to lower the
        // share, puts the climb off by ten seconds.
        let mut share = PathShare::new(50, start);
        let asked = frames(&mut share, start, Duration::from_secs(25), |at| {
            if (8000..10_500).contains(&at.as_millis()) {
                Duration::from_millis(250)
            } else {
                keeping_up(at)
            }
        });
        assert_eq!(asked.len(), 1, "{asked:?}");
        assert_eq!(asked[0].1, 60, "{asked:?}");
        assert!(asked[0].0 >= Duration::from_secs(20), "{asked:?}");
    }

    #[test]
    fn a_still_screen_is_no_queue() {
        // Frames now and then, the page having said it received each before
        // the next: the lag is nothing.
        let start = Instant::now();
        let mut share = PathShare::new(FULL, start);
        for second in 0..60 {
            let at = start + Duration::from_millis(second * 1900);
            assert_eq!(share.lag(at, Duration::ZERO), None);
        }
    }
}
