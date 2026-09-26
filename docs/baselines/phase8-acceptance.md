# Phase 8 acceptance audit (host-side)

Audit of the canonical plan against its real artifacts, produced by
`tools/phase8_acceptance_report.py`. The schema-v2 tool walks an explicit
manifest of host artifacts and device receipts. A device item is only `passed`
when its lexically newest receipt matches the pinned probe, explicit success
marker and exact private release context. A newer failure, malformed receipt or
context mismatch cannot fall back to an older success. Nothing is inferred from
code, commits or intent, and this document is not itself release acceptance.

The report requires a private mode-0600 context file:

```json
{
  "acceptanceRunId": "release-acceptance-unique-id",
  "reviewedSourceCommit": "0123456789abcdef0123456789abcdef01234567"
}
```

Every newly produced device receipt for that run must embed the same object as
`evidenceContext`. Existing schema-v1 receipts remain historical and cannot
complete a new release. Run the audit with
`--release-context evidence/private/<context>.json`; the command exits with 1
while any phase remains incomplete and 2 for malformed input.

The earlier generated reports, including the report that once stated
`overall=complete`, predate this contract and are historical only. A fresh bound
schema-v2 report has not yet completed.

## Complete phases

- **Phase 0 — recon and parity matrix.** Plan `docs/design/gui-parity-plan.md`,
  provenance `docs/baselines/upstream.md`, the read-only topology probe and its
  test; topology capture recorded
  (`evidence/private/device-kvm-topology-20260915-a.json`).
- **Phase 1 — read-only platform API.** Server routes plus the route-drift guard
  `tools/test_plan_route_drift.py`; 111 routes as of 2026-09-26.
- **Phase 4 — video gates (partly).** Two-client probe and soak receipts
  recorded (`video-two-client-probe-20260913-a.json`,
  `video-soak-probe-20260913-a.json`); the frame-timing probe is added and
  host-tested but has no device receipt yet (see below).
- **Phase 5 — network management (read-only part).** Resolver, IPv6 and
  interface views from `/etc/resolv.conf`, the kernel sysctl tree and sysfs;
  the network write paths stay hidden.
- **Phase 6 — update and extensions.** Signed application-update commit-cycle
  and runtime-package receipts recorded; production bundle tools present.

## Phases and receipts (plan E0-E5)

| Phase | Device receipts (glob) | Status |
| --- | --- | --- |
| P0 stability | `gadget-lifecycle-gate-*.json`, `boot-gate-*.json` | the lifecycle gate tool restarts the owner and requires the startup bind plus an attach/eject cycle that leaves HID bound (`result=passed`); the boot gate tool reboots once and requires the changed boot identity, six active product units, `443` and `/api/health` at `200`, an empty failed-unit list and surviving first-boot journal records (`result=passed`) |
| P1 video menu | `video-settings-gate-*.json` | the gate applies the reviewed frame rate and quality through the board API, requires the API read-back and the engine's own `/state` to agree with the frame rate, then restores the values it found (`result=passed`) |
| P2 settings | `device-settings-gate-*.json` | the gate switches mDNS on and verifies the resolver's own link state, has HID-only refuse a media attach, sends the system-log test line and finds it in the journal, records the read-only IPv6/WiFi/TLS views and restores every switch (`result=passed`) |
| E0 video | `video-frame-timing-*.json`, `video-two-client-probe-*.json`, `video-soak-probe-*.json` | recorded 2026-09-16 (`result=passed`) |
| E1 storage and media | `storage-acceptance-*.json`, `storage-upload-*.json`, `virtual-media-acceptance-*.json` | recorded 2026-09-16 (`result=passed`) |
| E2 board daemon | `boardd-fan-preflight-*.json`, `boardd-fan-gate-*.json`, `boardd-panel-gate-*.json`, `boardd-led-gate-*.json`, `boardd-button-gate-*.json`, `boardd-atx-leds-gate-*.json`, `kvmd-removal-*.json` | preflight and KVMD retirement recorded; the fan, LED and panel handoffs are recorded on 2026-09-17 as `observed-pending` (the daemon owns the lines and answered every request; the operator's physical observation is still open), the button gate is left to the operator, who runs the ATX check personally, the ATX host LED inputs read through the owner to `inventory.powerLed` (`backend=native`), and the panel is bench-verified: the vendor 240x240 initialization with the active-low reset brought the image up and the re-run panel gate passed (`boardd-panel-gate-20260917-d.json`, with the visible pattern required and a driver failure failing the gate) |
| E3 network and system actions | `device-system-action-store-*.json`, `device-system-action-maintenance.json`, `system-action-gate-*.json`, `network-write-gate-*.json` | the bounded network write gate is recorded on 2026-09-17 (`result=passed`: the resolver write and its restore applied; the IPv6 step is an explicit skip because the owner has no host network namespace); the privileged system-action store and maintenance dispatcher are recorded (`result=passed`); the end-to-end reboot gate is staged (`tools/stage_device_system_action_gate.py`) and awaits the operator's reboot approval. The unprivileged session auto-lock (`/api/vm/session-lock`) is implemented and covered by server tests |
| E4 retained update path | `persistent-write-*-commit-*.json`, `update-slot-serving-*.json` | commit cycle recorded; the host chain is verified end to end (shipped bundle `7e0ab131…`, board-profile features, `libexec/bli-kvm-boardd` and our board units staged; runtime package `0743cc85…` maps the daemon and the units); the slot-serving cutover is implemented on the host (the shipped build compiles the board profile, the runtime package and the production bundle carry `bli-kvm-boardd` and our board units, and `server-current`/`server-candidate` serve the slot through the board owner socket), while the device candidate/confirm cycle is open |
| E5 GUI parity and acceptance | `gui-parity-ours-auth-*.json`, `gui-parity-compare-*.json`, `vendor-replacement-*.json` | recorded: both parity captures and the read-only vendor replacement summary (`tools/capture_vendor_replacement_state.py`, `result=passed`) |

Recorded detail:

- E0: `video-frame-timing-20260916-b.json` (277 frames, 27.9 fps, p95 48.7 ms),
  `video-two-client-probe-20260916-a.json` (two clients),
  `video-soak-probe-20260916-a.json` (60 s, 12 samples, 330 MB).
- E1: `storage-acceptance-20260916-b.json` (list, mounted, cdrom, mount,
  reconnect, eject, re-attach), `storage-upload-20260916-b.json` and
  `storage-upload-cycle-20260916-a.json` (upload, mount, eject, delete,
  re-attach).
- E2 ATX: the owner unit enables the reviewed `BLI_KVM_GPIO_ATX_PROFILE`, so
  the capability advertises the power and reset actions again, and the daemon
  reads the host power (234) and HDD (233) LED inputs. `POST /api/network/wol`
  matches the captured reference contract. The board profile serves the power
  state from its own arrangement: the owner proxies one read-only board status
  and `/api/inventory` reports `backend=native` with `powerLed` (observed
  `false` with the host LED off).
- E2: `boardd-fan-preflight-20260916-a.json` (vendor sysfs lines 259, 260, 261,
  262 and 269 still claimed, line 257 unused, thermal about 51.5 C) and
  `kvmd-removal-20260916-a.json` (`removal.kvmd_stopped=yes`). The gate tool's
  read-only plan mode was also run per subsystem before any handoff:
  `boardd-fan-preflight-20260916-b.json` (line 269 present, value 0, output,
  active-high, daemon absent, thermal 53.5 C), `boardd-panel-preflight-...-a`
  (lines 259, 262 and 260 present, value 1, output, active-high),
  `boardd-led-preflight-...-a` (line 261 present, value 1) and
  `boardd-button-preflight-...-a` (line 257 absent, as the read-only capture
  already showed for an unused line). Each plan receipt is `result=plan`, never a
  gate pass.
- E3: `device-system-action-store-native.json` and
  `device-system-action-maintenance.json` (`result=passed`): the privileged
  system-action store and its maintenance dispatcher are verified on the board,
  so reboot and shutdown have a journaled, policy-guarded path. The owner now
  persists that journal across restarts (`crates/bli-kvm-control-broker`, with a
  broker test that restores the recorded phase from the store), the reviewed
  acceptance step is `bli-kvm-system-action-maintenance accept-post-boot` behind
  `hardened-blikvm-system-action-accept.service` (not installable: starting it
  is the operator's guard attestation), and
  `tools/stage_device_system_action_gate.py` stages the end-to-end gate with a
  reboot-free persistence mode. What remains is the armed device run
  (`system-action-gate-*.json`) plus the network writes
  (`network-write-gate-*.json`). The bounded DNS and IPv6 writes are
  implemented as capability-gated API routes that forward to the owner, and
  `tools/stage_device_network_write_gate.py` stages them with a write-and-restore
  sequence (plan-only by default).
- E4: the signed application update commit cycle
  (`persistent-write-*-commit-*.json`). On 2026-09-17 the production bundle for
  the board profile was built and signed with the production key: runtime
  package `589a9eaa…` from the shipped bundle `2883ca79…`, signed artifact
  `hardened-blikvm-application-0.1.0-dev2-seq7.tar` (sha256 `1b8281cd…`, 28
  payload files, source commit `3cd6b07`). Publishing it as a candidate, running
  candidate health, confirming it into `current` and the slot-serving cutover
  are the remaining device steps. On 2026-09-19 the same package
  (`589a9eaa…`) was re-signed with the rotated key
  `app-production-2026-09-18-01` using the same source/current commit bindings:
  the re-signed artifact is byte-identical across two independent builds
  (sha256 `d80ef86d0e8d4ad58b39d21173d04ec69a6fd8f37bad4581a02449b513837ffa`,
  28 payload files) and passed the host supervisor verification
  (`evidence/private/production-application-newkey-seq7-host-verification.json`:
  `result=passed`, `independentBuildsIdentical=true`, `wrongKeyRejected=true`,
  `payloadExecuted=false`). The old-key seq7 bundle remains as history.
- E5 parity (2026-09-17): the reference stand answers with a NanoKVM-branded
  build (`Remote Desktop - NanoKVM`, label `Hardened NanoKVM`) and our build
  answers with `Remote Desktop - Hardened BliKVM` (label `Hardened BliKVM`).
  Both captures authenticate through the JSON login (`apiLogin=accepted`), and
  `tools/compare_gui_parity.py` reports that the only captured label difference
  is the product name, with no unexplained labels, no capability differences, no
  marker differences, no failed requests on either side and an unchanged
  console-error count. The probe finds our settings surface (`About BliKVM`,
  `Diagnostics`, `Appearance`, `System`, `Account`) but not the reference's:
  that stand serves a NanoKVM-branded build whose settings UI the reviewed probe
  does not reach, so the settings-tab and capability surfaces stay uncaptured on
  the reference side.
- E5 parity refresh (2026-09-19): the authenticated capture was re-run against
  the deployed board profile (`gui-parity-ours-auth-20260919-a.json`,
  `apiLogin=accepted`, `mutationsPerformed=false`, `consoleErrors=0`, markers
  `mobileKeyboardInput=1`, `videoElements=2`) and compared with the latest
  reference receipt (`gui-parity-compare-20260919-a.json`): the only captured
  label difference is still the product name, with no capability differences, no
  marker differences, no failed requests and an unchanged console-error count.
  The reference-side settings tabs remain uncaptured as recorded above. The
  reference was re-captured on 2026-09-19 with the operator's credentials
  (`gui-parity-reference-auth-20260919-a.json`) and compared with our fresh
  capture (`gui-parity-compare-20260919-b.json`): the only label difference is
  still the product name, with no capability, marker or request differences.
- E2 firewall baseline (2026-09-19): `nftables` was installed. Moving the
  owner to the host network namespace made the baseline apply but raised its
  `systemd-analyze security` exposure from 1.7 to 2.1, above the reviewed
  threshold of 2.0, so that change was rejected. The owner now asks PID1 for a
  short-lived unit to run each `nft` command (`systemd-run --wait`, host
  network namespace, only `CAP_NET_ADMIN`); the owner's sandbox is untouched.
  The reviewed baseline then applied and restored on the device
  (`firewall-gate-20260919-c.json`: `backendAvailable=true`, `applied=true`,
  `confirmed=true`, mode restored).
- E5: `gui-parity-ours-auth-20260916-a.json`,
  `gui-parity-compare-20260916-a.json` and
  `vendor-replacement-20260916-b.json`: our five units active, every KVMD unit
  inactive and masked, the composite gadget bound with its four functions, the
  MSD partition mounted at our images directory with the reviewed image, both
  video sockets present, and the public health endpoint answering with the Rust
  backend and our platform identity.

The storage reboot gate is staged: `tools/stage_device_storage_reboot_gate.py`
ran plan-only on 2026-09-17 and recorded the pre-reboot state the reboot must
preserve (fstab entry, mount source/target/type/options, tmpfiles entries,
directory owners and modes, the catalogued image, no failed units), all intact.
Its `--confirm-reboot` step needs the operator's separate reboot approval.

Open device gates need an operator confirmation and their own stage tool: the
stability gates run through `tools/stage_device_gadget_gate.py` (probe
`hardened-blikvm-gadget-lifecycle-gate`: `--confirm-restart` proves the owner's
startup bind and `--image` with `--confirm-media-cycle` proves that an attach
and eject leave the pinned HID functions bound) and
`tools/stage_device_boot_gate.py` (probe `hardened-blikvm-boot-gate`:
`--confirm-reboot` requests the reviewed reboot once and only records `passed`
when the board returns with every product unit active, `443` and
`/api/health` at `200`, no failed unit and surviving first-boot journal
records). The
board daemon gates (fan, panel, LED and button) run through
`tools/stage_device_boardd_gate.py --subsystem <name> --confirm-handoff`, which
records the vendor sysfs exports, installs the daemon from the verified bundle,
drives the subsystem over the control socket and only records `passed` with
`--confirm-observed`; the network and system action gates run through their
stage tools, the slot serving cutover through
`tools/switch_signed_slot_server.py`, and the vendor replacement summary through
the final acceptance run. KVMD retirement
came from `tools/stage_device_kvmd_removal.py --confirm-removal`.

## Regression verification after the board-profile deployments

On 2026-09-17, after the owner, daemon and server binaries were reinstalled and
the ATX profile, the reviewed sandbox grants and the network writers changed,
the live surface was re-verified: the video frame-timing probe measured 221
frames at 27.93 fps (mean 35.8 ms, p95 46.6 ms) and the storage acceptance gate
passed all nine steps (list, mounted, cdrom, mount, reconnect, eject and
re-attach), so neither the stream nor the media engine regressed.

## Read-only evidence already in place

- Preflight, topology and HID descriptors:
  `handoff-preflight-20260915-a.json`, `device-kvm-topology-20260915-a.json`,
  `device-hid-descriptors-20260915-a.json`.
- Reversible handoff gate and teardown:
  `board-handoff-gate-20260915-e.json`,
  `board-handoff-teardown-20260915-a.json`.
- KVMD control-source and vendor timings:
  `device-kvmd-control-source-20260915-b.json`,
  `device-vendor-atx-timings-20260915-a.json`.
- Signed application-update commit cycle and runtime-package install:
  `persistent-write-*-commit-*.json`, `device-update-runtime-package.json`.
- Board bundle verification and stage:
  `board-bundle-verify-20260916-q.json` (bundle `748bafc7`, 43 members).
- Board serving cutover and video restore:
  `board-serving-cutover-20260916-a.json`,
  `board-video-restore-20260916-a.json`.

KVMD and the vendor C daemon are retired (stopped, disabled and masked); the
recorded unit backups are the only recovery path.
