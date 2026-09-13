# Objective audit — Phase 0 and Phases 3-7

Audit of the fixed BLi-KVM plan after the 2026-09-13 work session. Evidence
references public commits and documents only; private receipts are named but
never committed. Nothing below is a release acceptance.

## Step 0 — retained update path: complete

- Signed seq11 archive built twice byte-identically, SHA-256
  `e113e4bc…`, slot `app-…11-d6283a7b…`; host verification receipt
  (`persistent-write-seq11-host-verification-20260913-a.json`).
- First activation rejected by the eight-slot staging quota; the reviewed
  retained-GC deleted only seq3 (`slot-reclamation-retained-seq3-20260913-a.json`),
  leaving slots seq4-seq11 and the retained guard/evidence store intact.
- Retained activation passed (`persistent-write-seq11-activation-20260913-b.json`,
  HTTP 202, `published`); confirmation observed floor 11, generation 49, no
  pending (`seq11-status-poll-20260913-1.json`).
- Baseline `docs/baselines/persistent-write-seq11-retained-update.md`, commit
  `f3e202c`; the legacy closed GUI probe does not apply under retained
  admission (runbook updated).

## Phase 3 — HID: host complete, one device gate outstanding

- Control protocol v2 with broker-minted lease grants, typed leases, frames and
  fixed rejection codes (`433d368`, `b4dd14c`, `1f8709b`, `5ad96b0`).
- Broker target attestation registry and denial-by-default v2 router
  (`cf2814b`, `b4dd14c`); HID backend behind `hid-transition-profile`
  (`5ad96b0`); eligibility-gated HID policy (`6fe1a01`).
- Dedicated credential loader, sandbox unit and reversible H616 delivery gate
  (`cb084fe`, `9c97967`); gate passed (`hid-credential-delivery-gate-20260913-a.json`).
- Focus binding derivation (`90aa7f5`), server focus/lease endpoints
  (`4bfb337`, `aedd58c`), lease TTL contract (`dad749d`), aligned GUI key set
  (`f66ee7a`), compile-time capability advertise (`8999dfe`), GUI power,
  keyboard and mouse wiring with explicit operator confirmation (`f6e0289`,
  `92d9e1c`, `a03cb82`, `06eea34`).
- Deployable profile selectable from env and a verified test bundle
  (`bb4db0b`, `d8caa54`); device gate passed (`hid-profile-gate-20260913-a.json`).
- Isolated-backend H616 gates passed for startup replay, apply/release, expiry,
  mismatch, failure release-all, mouse modes, rollover, 10,000-event soak,
  backend reconnect and stuck-input recovery (`hid-isolated-gate-20260913-a..d`).
- OUTSTANDING: the BliKVM reboot gate is built, hardened and has a
  `--verify-only` mode; the first software reboot left the device unreachable
  until manual intervention, and the log configuration was repaired so the
  next boot is diagnosable. One explicitly confirmed reboot remains to observe
  a changed boot identity together with a preserved previous-boot journal, per
  `docs/design/phase3-5-device-gate-runbook.md`.
- Checkpoint `docs/baselines/phase3-hid-atx-host-checkpoint.md`; plan checkpoint
  `docs/implementation-plan.md` (`93ced66`).

## Phase 3 — ATX: complete for the initial code

- Reviewed feature-gated eligibility constructor (`97a186b`); persisted
  Pending -> fixed KVMD waiting route -> completion state machine (`c64550e`,
  `d14b29d`).
- Fixed-route boundary pinned (`93fe775`); reconciliation now requires
  non-Unknown read-only power-LED evidence (`59d3fb7`).
- Server `POST /api/application-power` returns the fixed broker rejection for
  indeterminate/cooldown (`44b0bc3`).
- Live H616 gates passed with Power/Reset physically disconnected
  (`atx-live-gate-20260913-a.json`): `power`, `power_long`, `reset`, duplicate,
  cooldown, restart-in-Pending and reconciliation.
- Capability decision: stays unavailable; enablement is a later release
  decision after the remaining gates.

## Phase 4 — video: host baseline done, device gates outstanding

- Fail-closed baseline (loopback MJPEG only, Janus disabled) `83bc87f`,
  `a65c78a`; capability-selected transport helper `bb7b829`.
- Host coverage: 2,000-frame soak `c8d2dd5`, two-client isolation `f2176ca`,
  upstream-disconnect resilience `a2a0ba6`, first-frame streaming `1abe696`.
- OUTSTANDING (device): hotplug, resolution, latency measurement and the
  24-hour soak; WebRTC integration only after stable parity evidence.

## Phase 5 — virtual media: host and isolated-device done, real-target gates outstanding

- Persisted transition engine over the fixed client and opaque catalog
  (`9e911bf`), with host tests for apply/replay, catalog route, busy/
  must-disconnect/noop, backend failure, restart-in-Pending and conflict/cooldown
  (`6037d89`); fixed-route boundary pinned (`bf6608f`); bounded timeout
  (`a7ece96`).
- Isolated H616 gate passed (`media-isolated-gate-20260913-a.json`): attach,
  eject, catalog selection, duplicate replay, backend failure with reconnect and
  restart with Pending plus reconciliation.
- OUTSTANDING (device/release): real-target attach/eject, upload interruption,
  read-only/writable modes, BIOS boot, reconnect, reboot and storage exhaustion;
  visible indeterminate/cooldown feedback.

## Phase 6 — system image/recovery: groundwork only

- License inventory and grouped report (`8773bcb`, `2ce4cdd`); workspace
  license inheritance pinned (`aa5a93e`).
- OUTSTANDING and release-blocking per `docs/licensing.md`: accepted A/B layout
  (the current live layout is rejected), writer/readback, boot-control,
  power-cut recovery, cleared inputs, key ceremony and the SPDX 3.0.1 SBOM.

## Phase 7 — release acceptance: not started

- Blocked: requires two devices and two recovery cards; the user has postponed
  the additional recovery card.

## Required external inputs to continue

1. Explicit confirmation to run the BliKVM reboot gate.
2. A safe, visible, authorized target for HID keyboard/mouse and real media
   gates.
3. A capability-enable/release decision.
4. An accepted A/B system-image layout and the second recovery card.

Until then the capabilities stay unavailable and no device-mutating step is
executed without confirmation.
