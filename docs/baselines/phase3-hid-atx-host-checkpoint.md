# Phase 3 HID and ATX host checkpoint

Host-side checkpoint for the restricted HID and ATX paths. No capability is
enabled by this checkpoint: the H616 manifest still advertises
`hid.status = not_exposed` with the three modes, `power.status = not_exposed`
and only `power.led_state = read_only`. Every mutation path is compile-time
disabled and the denial broker is unchanged in shipped builds.

## Delivered

- Control protocol v2 (`docs/api/control-v2.md`): typed target attestation,
  HID lease acquire/renew/report/release, ATX confirmation fields, explicit
  version rejection and the extended fixed rejection codes; core types and
  bounds in `crates/bli-kvm-core/src/control_v2.rs`; framing and corrrelation
  in `crates/bli-kvm-control-protocol/src/lib.rs` (commits `433d368`,
  `b4dd14c`, `1f8709b`).
- Broker-side target attestation: bounded, expiring, replace-on-session and
  idempotent release registry (`crates/bli-kvm-control-broker/src/attestation.rs`,
  `cf2814b`) plus the denial-by-default v2 router
  (`crates/bli-kvm-control-broker/src/v2.rs`, `b4dd14c`). The production broker
  constructs exactly `ControlV2State::disabled()`; the enabled path exists only
  under `#[cfg(test)]` and never attaches a backend.
- Eligibility-gated HID control policy (`bli-kvm-hid-policy`, `6fe1a01`):
  acquire/renew/report gated by capability, backend and attestation; release,
  session revocation, expiry and disconnect always allowed; fixed rejection to
  protocol code mapping; reporting cannot change a decision.
- Dedicated HID credential and sandbox channel: fixed-path loader and
  argument-free gate binary (`bli-kvm-hid-credential`, `cb084fe`), the
  `hardened-blikvm-hid-credential.service` oneshot unit with `LoadCredential`
  from `/etc/hardened-blikvm/hid/`, a dedicated `hardened-blikvm-hid` account,
  and the reversible H616 delivery gate `tools/stage_device_hid_credential_gate.py`
  (`9c97967`). The device gate has not been executed.
- Safe-target/focus bindings: domain-separated SHA-256 derivation in
  `bli-kvm-control-focus` (`90aa7f5`) and the session-bound, one-time
  `GET/POST /api/hid/focus` server endpoint that submits only a v2
  `target_attest` (`4bfb337`, `ea0f6ec`).
- GUI key set aligned to the pinned adapter table (`f66ee7a`): only usages
  4..=102 and 135..=139 remain, enforced by
  `tools/test_hid_keymap_alignment.py`.
- ATX: reviewed feature-gated eligibility constructor (`97a186b`) and the
  persisted Pending -> fixed KVMD waiting route -> completion state machine
  with duplicate/conflict replay and explicit reconciliation
  (`bli-kvm-atx-transition`, `c64550e`). The only representable routes are the
  three fixed `wait=1` KVMD clicks; no native GPIO or device path exists in the
  ATX path, pinned by `tools/test_phase3_control_boundaries.py`.
- Fail-closed invariants are pinned by
  `tools/test_phase3_control_boundaries.py`.

## Decision on capability enablement

Both capabilities stay unavailable in shipped builds. Enabling HID or ATX is a
later, gate-driven change: the ATX eligibility constructor and HID eligibility
are feature/test-profile only, and the broker has no attached HID or ATX
backend.

## Remaining gates

HID:

- device gates on H616: startup-ledger replay with an isolated backend,
  disconnect/expiry/mismatch, then keyboard and mouse modes, rollover,
  10,000 events, reconnect, BliKVM reboot and stuck-input recovery;
- reversible credential-delivery gate passed on the test H616 on 2026-09-13
  (private receipt `hid-credential-delivery-gate-20260913-a.json`):
  `deliveryToken=ok`, `sourceReadDenied=true`, `cleanupConfirmed=true`,
  `persistentChangeScope=none`, with the temporary account, transient unit and
  staging independently confirmed absent afterwards;
- GUI capability advertisement and lease TTL UX after the above.

ATX:

- wire the transition state machine to the broker/server with a startup
  reconciliation that compares read-only LED/state evidence;
- live gates with Power and Reset physically disconnected: pulse timing, busy,
  duplicate, cooldown, restart in Pending, reconciliation;
- GUI indeterminate/cooldown feedback.

All device-mutating steps require explicit confirmation before execution.
