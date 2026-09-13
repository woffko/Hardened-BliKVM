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
  (`9c97967`). The device gate passed. The broker can additionally select the
  HID transition backend from fixed environment keys and systemd's
  `CREDENTIALS_DIRECTORY` (`bb4db0b`), but only when built with the
  `hid-transition-profile` feature. A dedicated
  `hardened-blikvm-hid-transition.service` unit and ledger tmpfiles entry are
  packaged for that test build and pass the systemd static verifier, separate
  from the network server and from the credential-gate unit.
  `scripts/build-hid-transition-profile.sh` builds that test bundle (feature
  opt-in through `scripts/build-aarch64.sh`) with `releaseAccepted: false`.
  A test-profile bundle was produced (`7b3b6658…`) and its device gate passed
  on the test H616 (`hid-profile-gate-20260913-a.json`): the feature-built
  broker refuses to start without a systemd `CREDENTIALS_DIRECTORY`, starts and
  binds its control socket with a temporary credential directory, and leaves no
  process, account, unit or staging path behind (`persistentChangeScope: none`);
  it is not a release artifact and only temporary files were used.
- Safe-target/focus bindings: domain-separated SHA-256 derivation in
  `bli-kvm-control-focus` (`90aa7f5`) and the session-bound, one-time
  `GET/POST /api/hid/focus` server endpoint that submits only a v2
  `target_attest` (`4bfb337`, `ea0f6ec`). `POST /api/hid/lease` now carries
  acquire/renew/report/release using only the bindings remembered from a
  successful confirmation; without a current confirmation it is rejected
  before any broker call.
- GUI key set aligned to the pinned adapter table (`f66ee7a`): only usages
  4..=102 and 135..=139 remain, enforced by
  `tools/test_hid_keymap_alignment.py`.
- ATX: reviewed feature-gated eligibility constructor (`97a186b`) and the
  persisted Pending -> fixed KVMD waiting route -> completion state machine
  with duplicate/conflict replay and explicit reconciliation
  (`bli-kvm-atx-transition`, `c64550e`). The control broker can attach it only
  under the `atx-transition-profile` feature, using the reviewed eligibility
  constructor; a host test drives a mock KVMD socket and proves one real call
  plus duplicate replay (`d14b29d`). The server adds
  `POST /api/application-power`, which submits one typed action from a current
  focus confirmation and returns the fixed broker rejection, so cooldown and
  indeterminate states are representable before any GUI panel exists. The only
  representable routes are the
  three fixed `wait=1` KVMD clicks; no native GPIO or device path exists in the
  ATX path, pinned by `tools/test_phase3_control_boundaries.py`.
- Capability advertisement is compile-time gated: only a
  `restricted-control-surface` server build with an explicit opt-in and a
  configured control socket advertises HID and power `read_write`; default and
  shipped builds keep them unavailable, pinned by server tests.
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
  receipts `hid-isolated-gate-20260913-a.json` through `-c.json`):
  startup-ledger replay, lease apply/release, expiry release, identity mismatch
  without backend I/O, backend-failure release-all, absolute and relative mouse
  modes with buttons and wheel, six-key rollover with release-before-press
  ordering, a 10,000-event soak, backend-outage reconnect recovery and a
  stuck-input crash/restart recovery all passed against local mock KVMD
  sockets, with `physical_actions_invoked=no`, `persistentChangeScope=none`
  and confirmed staging cleanup;
- still pending a safe visible target and a reboot confirmation: BliKVM
  reboot;
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
  rejected without retry. `run_with_atx_profile` applies reviewed
  reconciliations before the broker starts serving, proven end-to-end on host;
  a packaged ATX service and GUI feedback remain;
- GUI indeterminate/cooldown feedback: the power menu and the keyboard/desktop
  and mobile mouse surfaces route through `/api/hid/focus`, `/api/hid/lease`
  and `/api/application-power` only when the capability is advertised, and keep
  the legacy path otherwise.
- Explicit operator confirmation: `focusConfirmedAtom` starts false and the
  HID/power hooks require an antd `Modal.confirm` operator acknowledgement
  before any restricted action; cancelling keeps controls inactive. This
  resolves the earlier blocking gap, though the capability still stays
  unavailable until the device and release gates pass.

## Phase 4 baseline

Video starts from a fail-closed state pinned by
`tools/test_phase4_video_boundary.py`: the manifest advertises only the
loopback Unix-socket MJPEG transport with audio unavailable, the KVMD Janus
inventory stays `janus_enabled: false`, and no WebRTC/Janus transport is
enabled in the server or web dependencies. WebRTC, hotplug, resolution,
two-client, latency and 24-hour soak gates remain ahead. A host soak test also
streams 2,000 MJPEG frames through the loopback proxy without truncation or
reordering, a capability-selected transport helper chooses only advertised
transports with MJPEG as the sole default, two concurrent clients receive
independent streams without cross-contamination, an upstream mid-stream
disconnect ends downstream cleanly, and the first frame is forwarded without
buffering the whole response.

## Phase 5 host baseline

Virtual media gains a persisted transition engine
(`bli-kvm-media-transition`, `9e911bf`): a permit persists Pending before the
single fixed KVMD route, the completion is persisted after, duplicates and
conflicts replay without another call, `Noop` never touches the journal, and
busy/`must_disconnect` fail without backend I/O. The catalog stays the only
image-ID authority and no upload, download or device path exists. Host tests
drive a mock media socket and cover duplicate replay, backend-failure
persistence without retry, restart with a Pending journal plus explicit
reconciliation, and operation-ID conflict and cooldown rejection.
Interruption, reboot and BIOS-boot gates remain device-bound. The isolated
media gate also passed on the test H616 (private receipt
`media-isolated-gate-20260913-a.json`): attach, eject, catalog selection,
duplicate replay, backend failure with reconnect recovery and restart with a
Pending journal plus reconciliation all passed against local mock media
sockets, with `physical_actions_invoked=no` and `persistentChangeScope=none`.

## Phase 6 groundwork

`tools/license_inventory.py` builds a deterministic Rust license inventory from
`cargo metadata --locked` (package, version, declared license or license file,
workspace membership) and fails closed when any package has no declared
license. It is an input to the later SPDX 3.0.1 release SBOM, not the SBOM
itself; the accepted A/B layout, writer/readback, boot-control and power-cut
gates remain device/recovery-card bound.

## Device maintenance and reboot gate

- The BliKVM reboot gate (`tools/stage_device_hid_reboot_gate.py`) was
  authorized and executed once. It requested a single software reboot; the
  device stayed unreachable longer than the gate window and recovered only
  after manual intervention, so the receipt records the failure
  (`hid-reboot-gate-20260913-a.json`). A later `--verify-only` run confirmed
  the post-reboot state: `kvmd` active, retained slots and guard/evidence
  intact, no staging or test account, and a changed boot identity.
- The field image had a volatile journal and corrupted log configuration:
  `/etc/logrotate.conf` and three `/etc/logrotate.d/` files were filled with
  NUL bytes, and `logrotate.service` failed. They were restored from standard
  content with on-device backups, `journald` was set to `Storage=persistent`,
  and `/var/log/journal` is now bind-mounted onto the empty `/mnt` partition
  (`/mnt/hardened-blikvm-logs/journal`) ordered after `mnt.mount`. Result:
  `failedUnits=0`, `logrotate` succeeds, and shutdown logs will survive future
  reboots (private receipt `device-log-fix-20260913-a.json`).
- Pre-existing and untouched: a `findmnt --verify` warning on the MSD fstab
  line, a broken `rsyslog` rotation example, and kernel noise for HDMI audio,
  `spi-nor`, and `axp2101`.

- The reboot gate was hardened: it distinguishes an unreachable device from an
  unchanged boot identity, records `pollAttempts`, and has a `--verify-only`
  mode that records the post-reboot state without another reboot. Because the
  journal is now persistent on `/mnt`, its postflight also records whether the
  previous boot journal is present. `tools/check_device_log_health.py` provides
  a read-only health receipt (`device-log-health-20260913-a.json`,
  `logrotateValid=true`, `journaldStorage=persistent`, `failedUnits=0`); a
  stacked duplicate bind mount found during that check was reduced to a single
  mount.
- Reboot gate result: the device rebooted for real; the post-reboot receipt
  (`hid-reboot-gate-20260913-c.json`) confirms `kvmd` active, retained slots and
  guard/evidence intact, no staging or test account, and
  `previousBootJournalPresent=true`, so log persistence across boot is proven.
  Caveat: this board's software reboot stalls for roughly half an hour between
  journal stop and the next boot (no `stop job` line), so the gate's default
  window was raised to 1800 s. A pre-existing `kvmd-main.sh` error stream
  ("Error reading from GPIO 0") was observed and left untouched.

All device-mutating steps require explicit confirmation before execution.
