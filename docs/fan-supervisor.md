# Unlinked fan supervisor boundary

`bli-kvm-fan-supervisor` composes the pure policy with an abstract
`FanActuator` trait. No concrete actuator exists, the crate has no device or
async-runtime dependency, and it is not linked into the platform broker,
server, or any shipped process.

The lifecycle is explicit:

1. `initialize()` must confirm the 100-percent startup command before samples
   are accepted.
2. Policy commands are sent only when they differ from the last confirmed
   command.
3. Any actuator error triggers an immediate 100-percent fallback attempt.
4. A confirmed fallback enters a nonrecovering failsafe latch; samples are no
   longer evaluated.
5. If command and fallback both fail, state becomes `ActuationUncertain` and
   only an explicit 100-percent retry is allowed.
6. A successful retry remains latched; automatic recovery is intentionally
   absent.
7. `prepare_shutdown()` always re-confirms 100 percent and latches. Failure to
   confirm it marks actuation uncertain.
8. Policy clock errors never reach the actuator, and a newly created supervisor
   always starts from maximum cooling again.

Mock tests cover startup gating, no-op suppression, normal application,
single/double actuator failures, repeated failsafe retry, startup failure,
monotonic-clock rejection, restart, and controlled-shutdown success/failure.

This contract does not prove crash safety. A sudden process exit, SIGKILL,
kernel failure, watchdog reset, or power loss may leave an unknown hardware
state. A future concrete driver still needs verified polarity/frequency/duty,
exclusive ownership, readback or stall evidence where possible, and a
kernel/hardware maximum-cooling failsafe before any runtime linkage.

Host-test and AArch64 codegen evidence is recorded in
`baselines/fan-supervisor.md`.
