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
  `tools/test_plan_route_drift.py`; 37 routes as of 2026-09-15.
- **Phase 4 — video gates (partly).** Two-client probe and soak receipts
  recorded (`video-two-client-probe-20260913-a.json`,
  `video-soak-probe-20260913-a.json`); the frame-timing probe is added and
  host-tested but has no device receipt yet (see below).
- **Phase 5 — network management (read-only part).** Resolver, IPv6 and
  interface views from `/etc/resolv.conf`, the kernel sysctl tree and sysfs;
  the network write paths stay hidden.
- **Phase 6 — update and extensions.** Signed application-update commit-cycle
  and runtime-package receipts recorded; production bundle tools present.

## Open device gates

| Gate | Needed receipt | How it is produced |
| --- | --- | --- |
| Composite gadget acceptance | `board-handoff-gate-*.json` with `acceptanceFacts.gadget_present=yes` | `tools/stage_device_board_profile_gate.py --confirm-handoff --accept-media-hid …` plus the two GUI probes, then `--teardown` |
| Native ATX GPIO bench | same gate receipt with `atxBenchFacts.power_short=ok` | add `--accept-atx-bench --confirm-physical-disconnect` (Power/Reset unplugged) |
| Virtual-media acceptance | `virtual-media-acceptance-*.json` with `result=passed` | `tools/playwright/virtual_media_acceptance.mjs` during the media/HID window |
| Restricted power GUI | `restricted-power-*.json` with `result=passed` | `tools/playwright/restricted_power_probe.mjs` (Power/Reset unplugged) |
| Frame timing | `video-frame-timing-*.json` with `result=passed` | `tools/playwright/video_frame_timing_probe.mjs` against a stand that serves `/api/stream/mjpeg` |
| KVMD removal | `kvmd-removal-*.json` with `removal.kvmd_stopped=yes` | `tools/stage_device_kvmd_removal.py --confirm-removal`, which refuses until the three receipts above exist |

The restricted-control stand (`https://10.0.77.118:8443/`) answers `503` for
`/api/stream/mjpeg` and the main proxy answers `502` while
`hardened-blikvm-server.service` is inactive, so the frame-timing gate needs the
main profile running. A fresh reference GUI capture for capability and marker
comparison needs the reference operator credential.

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
- KVMD retirement inventory and refusal:
  `kvmd-inventory-20260915-a.json`, `kvmd-removal-refusal-20260915-a.json`.
- Signed application-update commit cycle and runtime-package install:
  `persistent-write-*-commit-*.json`, `device-update-runtime-package.json`.
- Pre-migration ATX fixed-route live gate: `atx-live-gate-20260913-a.json`
  (the canonical Phase 2 proof is the native GPIO bench below).
- Board bundle verification: `board-bundle-verify-20260915-b.json`
  (`sha256 39991edf…`, 36 members, includes the ATX GPIO bench gate).
- GUI parity capture and comparison:
  `gui-parity-ours-auth-20260915-g.json`, `gui-parity-compare-20260915-c.json`
  (no unexplained label differences; branding differences are expected).

KVMD and the vendor C daemon remain the fallback until every gate above is
recorded and their receipts exist.
