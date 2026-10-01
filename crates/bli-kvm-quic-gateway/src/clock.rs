//! The board's clock as the page sees it: the system's monotonic clock, in
//! microseconds. The encoder stamps a frame's capture with the same clock,
//! so the page can tell a frame's age from its pings.

use nix::time::{ClockId, clock_gettime};

pub fn monotonic_micros() -> u64 {
    match clock_gettime(ClockId::CLOCK_MONOTONIC) {
        Ok(time) => {
            (time.tv_sec().max(0) as u64) * 1_000_000 + (time.tv_nsec().max(0) as u64) / 1000
        }
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_moves_forward() {
        let first = monotonic_micros();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let second = monotonic_micros();
        assert!(first > 0 && second >= first + 4000, "{first} then {second}");
    }
}
