# Host-only fan policy boundary

`bli-kvm-fan-policy` is a pure deterministic state machine. It has no GPIO,
PWM, filesystem, device, clock, async-runtime, platform-broker, or server
dependency and is not linked into a shipped process. Callers supply sanitized
temperature observations and monotonic milliseconds; the crate returns only a
proposed `Off` or bounded `DutyPercent` command.

The crate deliberately provides no production default. A configuration is
accepted only when:

- off, on, and critical temperatures are ordered and within -40 through 150
  degrees Celsius;
- minimum on/off dwell and sensor timeout are nonzero and bounded;
- minimum duty is 1 through 100 percent;
- one through eight curve points have strictly increasing temperatures and
  non-decreasing bounded duties.

Every engine starts in 100-percent failsafe. Explicit sensor failure, an
out-of-range sample, missing samples at the exact timeout, and critical
temperature all demand 100 percent immediately and override minimum-off dwell.
Valid noncritical samples may recover to the configured curve. Turning power
off or on still respects minimum dwell and off/on hysteresis. Monotonic time
rollback is rejected without changing state.

This is not a fan implementation or permission to actuate hardware. The H616
fan capability remains unavailable. A runtime backend needs, at minimum:

- verified fan-pin identity and active/inactive polarity;
- verified PWM frequency, duty semantics, minimum spinning duty, and maximum
  cooling direction;
- exactly one GPIO/PWM owner and a fail-safe startup/crash/stop mechanism;
- bounded configuration provenance and persisted last-known policy;
- sensor-disconnect injection, stall detection where hardware permits, and
  temperature/PWM logging without sensitive data;
- boot, restart, suspend, load, ambient, obstruction, and power-loss tests;
- a physical thermal-soak matrix before capability promotion.

The pinned vendor code is compatibility evidence only. It creates
world-writable state, starts PWM before validating the sensor, leaves the last
duty on read failure, relies on a sleep loop instead of monotonic dwell, and
mixes configurable hysteresis with a fixed curve. None of those behaviors is
carried into the policy contract.

Host-test and AArch64 codegen evidence is recorded in
`baselines/fan-policy.md`.

The next unlinked lifecycle layer is specified in `fan-supervisor.md`.
