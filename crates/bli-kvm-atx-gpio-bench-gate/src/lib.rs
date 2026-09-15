//! Native ATX GPIO pulse bench.
//!
//! Power and Reset must be physically disconnected. The bench drives the two
//! reviewed actuator lines through the real Linux GPIO character device with the
//! board's sourced timings, measures how long each pulse actually took, and
//! reports whether the measured hold stayed within tolerance of the configured
//! value. It never runs without an explicit physical-disconnect confirmation and
//! it always releases the lines afterwards.

use std::path::Path;
use std::time::Instant;

use bli_kvm_atx_gpio_executor::{AtxTiming, chardev_atx_executor};
use bli_kvm_atx_transition::AtxPulseRunner;
use bli_kvm_core::AtxControlAction;

/// Allowed scheduling overhead over the configured hold, in milliseconds.
pub const TOLERANCE_MS: u64 = 400;

#[derive(Debug, PartialEq, Eq)]
pub enum BenchError {
    NotConfirmed,
    Backend(String),
}

impl std::fmt::Display for BenchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfirmed => write!(formatter, "physical disconnect was not confirmed"),
            Self::Backend(message) => write!(formatter, "{message}"),
        }
    }
}

fn actions(timing: AtxTiming) -> [(AtxControlAction, &'static str, u64); 3] {
    [
        (
            AtxControlAction::PowerShort,
            "power_short",
            timing.power_short_ms,
        ),
        (
            AtxControlAction::PowerLong,
            "power_long",
            timing.power_long_ms,
        ),
        (AtxControlAction::Reset, "reset", timing.reset_ms),
    ]
}

/// Run the three pulses through any reviewed runner and measure each one.
pub fn run_bench_with_runner(
    runner: &mut dyn AtxPulseRunner,
    timing: AtxTiming,
    confirmed: bool,
) -> Result<Vec<(String, String)>, BenchError> {
    if !confirmed {
        return Err(BenchError::NotConfirmed);
    }
    let mut facts = Vec::new();
    for (action, name, expected) in actions(timing) {
        let start = Instant::now();
        runner.execute(action).map_err(BenchError::Backend)?;
        let elapsed = start.elapsed().as_millis() as u64;
        let within = elapsed >= expected && elapsed <= expected + TOLERANCE_MS;
        facts.push((format!("{name}_elapsed_ms"), elapsed.to_string()));
        facts.push((format!("{name}_expected_ms"), expected.to_string()));
        facts.push((
            format!("{name}_within_tolerance"),
            if within { "yes" } else { "no" }.to_owned(),
        ));
        facts.push((
            name.to_owned(),
            if within { "ok" } else { "drift" }.to_owned(),
        ));
    }
    facts.push(("run_rc".to_owned(), "0".to_owned()));
    Ok(facts)
}

/// Open the real GPIO character device and run the bench.
pub fn run_bench(
    chip: &Path,
    timing: AtxTiming,
    confirmed: bool,
) -> Result<Vec<(String, String)>, BenchError> {
    if !confirmed {
        return Err(BenchError::NotConfirmed);
    }
    let mut executor = chardev_atx_executor(chip, timing)
        .map_err(|error| BenchError::Backend(error.to_string()))?;
    run_bench_with_runner(&mut executor, timing, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Sleeper {
        run: bool,
    }

    impl AtxPulseRunner for Sleeper {
        fn execute(&mut self, _action: AtxControlAction) -> Result<(), String> {
            if self.run {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Ok(())
        }
    }

    fn timing() -> AtxTiming {
        AtxTiming {
            power_short_ms: 5,
            power_long_ms: 5,
            reset_ms: 5,
        }
    }

    #[test]
    fn the_bench_refuses_without_the_physical_disconnect_confirmation() {
        let mut runner = Sleeper { run: true };
        assert_eq!(
            run_bench_with_runner(&mut runner, timing(), false),
            Err(BenchError::NotConfirmed)
        );
    }

    #[test]
    fn a_matching_hold_is_reported_ok_and_a_fast_return_drifts() {
        let mut exact = Sleeper { run: true };
        let facts = run_bench_with_runner(&mut exact, timing(), true).unwrap();
        assert_eq!(
            facts
                .iter()
                .find(|(key, _)| key == "power_short")
                .map(|(_, value)| value.as_str()),
            Some("ok")
        );
        assert_eq!(
            facts
                .iter()
                .find(|(key, _)| key == "reset_within_tolerance")
                .map(|(_, value)| value.as_str()),
            Some("yes")
        );

        let mut fast = Sleeper { run: false };
        let facts = run_bench_with_runner(&mut fast, timing(), true).unwrap();
        assert_eq!(
            facts
                .iter()
                .find(|(key, _)| key == "power_long")
                .map(|(_, value)| value.as_str()),
            Some("drift")
        );
        assert_eq!(
            facts
                .iter()
                .find(|(key, _)| key == "reset_within_tolerance")
                .map(|(_, value)| value.as_str()),
            Some("no")
        );
    }

    #[test]
    fn a_backend_failure_is_reported_not_swallowed() {
        struct Failing;
        impl AtxPulseRunner for Failing {
            fn execute(&mut self, _action: AtxControlAction) -> Result<(), String> {
                Err("open failed".to_owned())
            }
        }
        let mut runner = Failing;
        assert_eq!(
            run_bench_with_runner(&mut runner, timing(), true),
            Err(BenchError::Backend("open failed".to_owned()))
        );
    }
}
