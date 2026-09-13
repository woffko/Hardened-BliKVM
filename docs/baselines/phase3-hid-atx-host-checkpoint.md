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
  (`crates/bli-kvm-control-broker/src/v2.rs`, `b4dd14c`). An acquire is
  answered with `lease_granted` carrying a broker-minted non-zero lease ID
  (`5ad96b0`). The production broker constructs exactly
  `ControlV2State::disabled()`; the `hid-transition-profile` feature can attach
  a `HidControlPolicy`, and even then acquire/renew/report require a current
  target attestation while release stays unconditional. Without the feature the
  HID path still fails closed with `backend_unavailable`.
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
  (`bli-kvm-atx-transition`, `c64550e`). The control broker can attach it only
  under the `atx-transition-profile` feature, using the reviewed eligibility
  constructor; a host test drives a mock KVMD socket and proves one real call
  plus duplicate replay (`d14b29d`). The only representable routes are the
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

- isolated-backend device gate passed on the test H616 on 2026-09-13 (private
  receipts `hid-isolated-gate-20260913-a.json` and `-b.json`):
  startup-ledger replay, lease apply/release, expiry release, identity mismatch
  without backend I/O, backend-failure release-all, absolute and relative mouse
  modes with buttons and wheel, six-key rollover with release-before-press
  ordering, and a 10,000-event soak all passed against local mock KVMD sockets,
  with `physical_actions_invoked=no`, `persistentChangeScope=none` and
  confirmed staging cleanup;
- still pending a safe visible target: reconnect after a real backend outage,
  BliKVM reboot and stuck-input recovery;
- reversible credential-delivery gate passed on the test H616 on 2026-09-13
  (private receipt `hid-credential-delivery-gate-20260913-a.json`):
  `deliveryToken=ok`, `sourceReadDenied=true`, `cleanupConfirmed=true`,
  `persistentChangeScope=none`, with the temporary account, transient unit and
  staging independently confirmed absent afterwards;
- GUI capability advertisement and lease TTL UX after the above.

ATX:

- live waiting-route gate passed on the test H616 on 2026-09-13 (private
  receipt `atx-live-gate-20260913-a.json`) with Power and Reset physically
  disconnected: three real KVMD `wait=1` clicks (`power`, `power_long`,
  `reset`), duplicate replay, same-ID conflict rejection, five-second cooldown,
  restart with a Pending journal and explicit reconciliation. `power_short`
  returned in 1126 ms, `physical_actions_invoked=yes`,
  `persistentChangeScope=none` and the transient unit and staging were
  removed;
- reconciliation now enforces non-Unknown read-only power-LED evidence before
  recording a Pending outcome, and a busy/failed KVMD response is persisted as
  rejected without retry (`48f06d4` baseline, this checkpoint adds the gate);
  the daemon wiring and GUI feedback remain;
- GUI indeterminate/cooldown feedback.

All device-mutating steps require explicit confirmation before execution.
