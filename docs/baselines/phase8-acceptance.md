# Phase 8 acceptance audit (host-side)

Audit of the canonical plan against its real artifacts, produced by
`tools/phase8_acceptance_report.py`. The tool walks an explicit manifest of host
artifacts and device receipts: a device item is only `recorded` when a receipt
matches the pinned probe and marker, everything else stays `unrun`, and the
overall status stays `incomplete` while any gate is open. Nothing here is
inferred from code, commits or intent, and this document is not a release
acceptance.

Latest generated report: `evidence/private/phase8-acceptance-20260915-h.json`
(`overall=incomplete`, `hostMissing=0`, `deviceUnrun=6`).

## Complete phases

- **Phase 0 — recon and parity matrix.** Plan `docs/design/gui-parity-plan.md`,
  provenance `docs/baselines/upstream.md`, the read-only topology probe and its
  test; topology capture recorded
  (`evidence/private/device-kvm-topology-20260915-a.json`).
- **Phase 1 — read-only platform API.** Server routes plus the route-drift guard
  `tools/test_plan_route_drift.py`; 47 routes as of 2026-09-17.
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
| E0 video | `video-frame-timing-*.json`, `video-two-client-probe-*.json`, `video-soak-probe-*.json` | recorded 2026-09-16 (`result=passed`) |
| E1 storage and media | `storage-acceptance-*.json`, `storage-upload-*.json`, `virtual-media-acceptance-*.json` | recorded 2026-09-16 (`result=passed`) |
| E2 board daemon | `boardd-fan-preflight-*.json`, `boardd-fan-gate-*.json`, `boardd-panel-gate-*.json`, `boardd-led-gate-*.json`, `boardd-button-gate-*.json`, `boardd-atx-leds-gate-*.json`, `kvmd-removal-*.json` | preflight and KVMD retirement recorded; the fan, LED and panel handoffs are recorded on 2026-09-17 as `observed-pending` (the daemon owns the lines and answered every request; the operator's physical observation is still open), the button gate is left to the operator, who runs the ATX check personally, and the ATX host LED inputs read through the owner to `inventory.powerLed` (`backend=native`) |
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
  (`persistent-write-*-commit-*.json`).
- E5 parity (2026-09-17): the reference stand answers with a NanoKVM-branded
  build (`Remote Desktop - NanoKVM`, label `Hardened NanoKVM`) and our build
  answers with `Remote Desktop - Hardened BliKVM` (label `Hardened BliKVM`).
  `tools/compare_gui_parity.py` reports that the only captured label difference
  is the product name, with no unexplained labels, no capability differences, no
  marker differences and an unchanged console-error count; the settings-tab and
  capability surfaces still need an authenticated capture on both sides.
- E5: `gui-parity-ours-auth-20260916-a.json`,
  `gui-parity-compare-20260916-a.json` and
  `vendor-replacement-20260916-b.json`: our five units active, every KVMD unit
  inactive and masked, the composite gadget bound with its four functions, the
  MSD partition mounted at our images directory with the reviewed image, both
  video sockets present, and the public health endpoint answering with the Rust
  backend and our platform identity.

Open device gates need an operator confirmation and their own stage tool: the
board daemon gates (fan, panel, LED and button) run through
`tools/stage_device_boardd_gate.py --subsystem <name> --confirm-handoff`, which
records the vendor sysfs exports, installs the daemon from the verified bundle,
drives the subsystem over the control socket and only records `passed` with
`--confirm-observed`; the network and system action gates run through their
stage tools, the slot serving cutover through
`tools/switch_signed_slot_server.py`, and the vendor replacement summary through
the final acceptance run. KVMD retirement
came from `tools/stage_device_kvmd_removal.py --confirm-removal`.

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
