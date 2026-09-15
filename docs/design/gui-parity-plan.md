# Hardened BLi-KVM GUI parity plan

Status: canonical plan, approved 2026-09-14. **This document supersedes the
previous session objective and the phase numbering and ATX constraint in
`docs/implementation-plan.md`.** It drives the port of the Hardened NanoKVM
operator surface onto the H616 BliKVM while removing KVMD. Every slice is
implemented, verified and committed in English with a green `scripts/check.sh`.

## Supersedes

- This plan replaces the old objective "идти по зафиксированному плану BLi-KVM"
  in full: its phase order and its Phase 3 restriction "ATX только через KVMD
  fixed waiting routes, без нативного GPIO-перехвата" no longer apply.
- Decision 4 (the Rust board owner, including native ATX GPIO) is now
  authorized. The default profile still pulses through the KVMD fixed routes
  during migration; the native GPIO path is a non-default, compile-time-gated
  executor that is enabled only after the reviewed physical line handoff.
  `tools/test_phase3_control_boundaries.py` pins that gating.
- Approximate mapping to the pre-existing documents: `implementation-plan.md`
  Phase 4 (Video) maps to canonical Phase 4/8, Phase 5 (Virtual media/board) to
  canonical Phase 3/5, Phase 6 (Hardening/updates) to canonical Phase 6, and
  Phase 7 (Release acceptance) to canonical Phase 8.

## Status and stop rule

The H616 board is available (corrected 2026-09-15): `https://10.0.77.118/`
(SSH root) with KVMD, the vendor stack and the `kvmd` gadget bound. An earlier
"stand unavailable / plain Ubuntu" note came from a check that ran in a local
shell instead of over SSH and is retracted.

- Host-only slices continue without confirmation and keep the plan moving.
- Device mutations (stopping KVMD, binding our gadget, ATX pulses, gadget
  takeover) run only with explicit operator confirmation in the session and
  produce a private receipt.
- The restricted-control test profile is currently the active stand: its own
  HTTPS proxy answers on `8443` with HID and virtual media enabled, while the
  main proxy on `443` answers `502` because the main server unit is inactive.
- The goal is not closed while a required device gate is unrun or unverified.

## Locked decisions

1. Architecture A: the hardened BLi-KVM server becomes a privileged management
   backend and exposes the NanoKVM-compatible API; the Nano-derived web GUI is
   kept.
2. Media and HID move to our own Rust USB gadget (HID + mass storage), fully off
   KVMD. Gadget takeover is a physical gate with rollback; a brief USB/HID
   interruption to the target is accepted.
3. Gadget descriptors: generic and driver-safe `idVendor=0x1d6b`,
   `idProduct=0x0104`; unique `manufacturer`/`product`/`config`/`serialnumber`
   strings (serial derived per device from board serial/MAC/SoC and persisted).
   HID functions keep standard subclass/protocol/report length (keyboard 1/1/8,
   relative mouse 0/0/7, absolute mouse 1/2/5) for BIOS compatibility.
4. Board daemon: the vendor C `kvmd-main` (gpio/atx/fan/oled/rtc/switch) is
   rewritten in Rust; ATX becomes our daemon. This supersedes the earlier
   KVMD-only ATX constraint (see "Supersedes").
5. Update follows the reference behaviour, adapted to this board and to the
   retained signed application-update model; `raw` stays off by default.
6. Exec features (terminal/script/picoclaw) exist but are off by default in
   settings, matching the reference.
7. KVMD (PiKVM Python) and the C `kvmd-main` are removed completely by the end;
   they remain a fallback until each subsystem is proven.

## Progress (2026-09-14)

Phases 0-1 are done for the read-only surface, and the KVMD-free host stack for
Phases 2-3 is complete. Everything below is committed with a green
`scripts/check.sh`.

- Phase 0: plan, corrected provenance and the read-only topology probe.
- Phase 1 (read-only only): `/api/system/time`, `/api/system-log/*`,
  `/api/application/{version,preview}`, `/api/vm/{info,hardware,autostart}`
  and a capability-gated read-only settings System tab; auth already matched
  the reference shapes. Download exposes read-only capability flags; the
  remaining Phase 1 modules (network, firewall) are not implemented and stay
  capability-hidden.
- Phases 2-3 (host, no KVMD in the gadget/board builds): backend seams for the
  ATX, media and HID transition engines; the idempotent configfs USB gadget
  state machine; feature-gated gadget media/HID executors; a feature-gated
  native ATX GPIO executor plus the Linux GPIO character-device driver; and the
  single board owner that serves media, HID and ATX from one control socket.
  The host toolchain builds, verifies and stages the board owner reversibly
  (`scripts/build-board-profile.sh`, `tools/verify_board_profile_bundle.py`,
  `tools/plan_board_profile_handoff.py`, `tools/stage_device_board_profile_gate.py`);
  the real aarch64 build also fixed the GPIO ioctl request cast for
  aarch64-musl, and `tools/test_gadget_profile_boundary.py` pins that the
  gadget/gpio profiles link no `bli-kvm-kvmd-*` crate.
- Device (2026-09-15): preflight `handoffReady=true` on the board, read-only
  topology and HID descriptor captures, and the **reversible board-owner
  handoff gate passed** (KVMD stopped, the Rust owner served its control socket,
  teardown restored KVMD and removed only our state) - see
  `docs/baselines/board-handoff-gate.md`. The captured HID report
  descriptors are pinned in `crates/bli-kvm-usb-gadget` (verified against the
  recorded SHA-256), and the board gadget is now a composite device: MSD plus
  the three pinned HID functions. The pointer interfaces are pinned exactly as
  captured: `hid.usb1` is the absolute mouse (`<BHHbb`, 16-bit X/Y in
  `0..=32767`) and `hid.usb2` is the relative mouse (`<Bbbbb`, signed 8-bit
  deltas); the executor composes both, follows KVMD's move-range remap for
  absolute moves, and clears the interface left behind when the pointer mode
  switches. Media/HID and ATX pulse gates remain.

  Read-only KVMD sourcing (`tools/capture_device_kvmd_control_source.py`,
  receipts `evidence/private/device-kvmd-control-source-20260915-{a,b}.json`)
  confirmed the
  report layouts and the remap formula, and recorded the board's real ATX lines
  from `/etc/kvmd/override.yaml`: gpiochip0 with power switch 228 and reset
  switch 272 (LEDs 234/233). Read-only disassembly of the vendor `/usr/bin/atx`
  (`evidence/private/device-vendor-atx-timings-20260915-a.json`) then pinned the
  intended pulse durations: `power_on` 500 ms, `power_off` (force off) 5000 ms
  and `power_reset` 500 ms. They are now the board defaults in
  `crates/bli-kvm-platform-bli` and in the handoff tools, still overridable and
  still awaiting a bench confirmation. The bench itself is now a bundled,
  argument-free gate binary (`bli-kvm-atx-gpio-bench-gate`) that the handoff gate
  can run with `--accept-atx-bench --confirm-physical-disconnect`; it records the
  measured hold per action and fails closed on drift. The rebuilt board bundle
  (`sha256 39991edf…`, 36 members, receipt
  `evidence/private/board-bundle-verify-20260915-b.json`) carries it.

  The restricted surface also accepts one bounded image upload
  (`POST /api/download/file`, `.img`/`.iso`, 4 GiB) and records it in the media
  catalog atomically. The board owner re-reads the catalog from disk before
  every applied operation (keeping the last good copy if the file is briefly
  unreadable), so a newly uploaded image is selectable without a restart.
  `bli-kvm-usb-gadget::reconnect` performs the reviewed unbind/rebind primitive
  for the reference's USB-reconnect action; the protocol, policy and API wiring
  for `/api/storage/usb/reconnect` remain a separate reviewed slice. The
  GUI places its upload control in the download menu, which is gated by the
  remote-download capability and therefore stays hidden today: the upload is an
  operator/API capability until the menu gating is revisited in Phase 6 (the
  remote `POST /api/download/image` stays unimplemented).

## Device-bound remainder (board available)

The H616 board is present and ready (`https://10.0.77.118/`, SSH root;
preflight `handoffReady=true`: KVMD and the vendor daemon present, the `kvmd`
gadget bound, configfs mounted). An earlier note wrongly reported the endpoint
as a plain Ubuntu host; that check ran in a local shell instead of over SSH and
is retracted. The active stand is the restricted-control profile on
`https://10.0.77.118:8443/`; the main proxy on `443` returns `502` while
`hardened-blikvm-server.service` is inactive.

The following now run on the board, mutation steps only with explicit operator
confirmation, each producing a private receipt:

- HID report descriptors and function parameters (captured read-only:
  `evidence/private/device-hid-descriptors-20260915-a.json`);
- the reversible board-owner handoff gate
  (`tools/stage_device_board_profile_gate.py --confirm-handoff`): stop KVMD,
  start the Rust owner, verify the owner socket and the `hardened` gadget,
  then `--teardown` restores KVMD;
- media attach/eject/upload, BIOS boot, interruption/reconnect and reboot;
- Phase 4 video gates, Phase 6 image/recovery and Phase 8 acceptance.

KVMD retirement is prepared but not executed: `tools/stage_device_kvmd_removal.py`
records a read-only inventory (receipt
`evidence/private/kvmd-inventory-20260915-a.json`) and refuses to stop anything
until the private receipts prove the replacement (composite-gadget acceptance,
media attach/eject, native ATX bench). The refusal is a real receipt
(`kvmd-removal-refusal-20260915-a.json`) and the accepted path copies the unit
files into a timestamped backup before disabling the four KVMD units and
verifying our owner still serves.

Phase 4 video host harness: `tools/playwright/video_frame_timing_probe.mjs`
consumes the MJPEG stream and records complete-frame count, mean/p95/max frame
interval, FPS and average frame bytes (host-tested against a local MJPEG
fixture). A device receipt requires the main profile to serve video: the
restricted-control stand answers `503` for `/api/stream/mjpeg` (its video proxy
is inactive) and the main proxy answers `502` while its server unit is down, so
the timing/hotplug/resolution gates stay with the operator.

The human-readable audit lives in `docs/baselines/phase8-acceptance.md`: it
repeats the generated report, names every open gate and the exact tool that
produces its receipt, and states that KVMD stays the fallback.

Phase 8 acceptance report (host-only, read-only): `tools/phase8_acceptance_report.py`
walks an explicit manifest of host artifacts and device receipts and writes a
private report; a device item without a matching receipt stays `unrun` and the
overall status stays `incomplete`. Current report
(`evidence/private/phase8-acceptance-20260915-a.json`): phases 0, 1, 4 and 6
complete; phase 2 awaits the native ATX GPIO bench confirmation, phase 3 the
composite-gadget acceptance and phase 8 the virtual-media acceptance receipt.

`tools/playwright/app_harness.mjs` serves the built `web/dist` with a mock
restricted API. `tools/test_restricted_power_probe.mjs`, `tools/test_restricted_keyboard_probe.mjs`
and `tools/test_virtual_media_probe_app.mjs` run the power, on-screen-keyboard and
virtual-media probes against it, so the three GUI control paths are verified
locally before any device run. The media probe reads the wrapped
`{code,data}` response, the shape the real server returns.
`tools/test_gui_parity_app.mjs` serves the built `web/dist` with a mock API and
runs the probe against the real application: it proves the desktop still
renders, that the leader-key load now hits the implemented endpoint, and that
the removed legacy calls (`/api/vm/gpio`, `/api/hid/mode`, `/api/hid/reset`)
are no longer requested.

Phase 8 parity harness (host-only, no mutation): `gui_parity_probe.mjs` now
captures menu labels, tag counts, the capability manifest, surface markers
(mobile keyboard input, on-screen keys, video elements), the settings tab labels
(best effort: it needs the menu to expand) and bounded console error samples; `compare_gui_parity.py` separates label differences that a
differing capability explains or that are intentional branding from
`unexplainedLabelsOnlyInReference/InOurs`, the real gaps, and only compares
capabilities, markers and the recorded failed GUI requests when both receipts
carry them, so a one-sided capture never reads as a gap. Latest read-only
capture (ours-f, compare-b) has no unexplained differences in labels. The
observed console errors on the rc stand name one real gap to track: the GUI
opens `wss://…/api/ws`, which our server does not implement yet (parity matrix
target 3/7); the two `503`s come from the inactive platform broker in that
profile and the `401`/`404` are pre-login and asset requests.

## Provenance

- PiKVM KVMD: `github.com/pikvm/kvmd` v3.291 (upstream daemon), installed on the
  device through the Bli-PiKVM line (`RainCat1998 == woffko/Bli-PiKVM`); the
  device carries the `srepac/kvmd-armbian` checkout (`e67a3b4`).
- Original BliKVM H616 hardware/app: `blikvm/blikvm` / `ThomasVon2021/blikvm`
  (C `blikvm_server`/`kvmd-main`, `src/kvmd/blikvm_*`, `package/kvmd-*`,
  `ustreamer.bin`, janus). Source for H616 board behaviour.
- Reference GUI/management: Hardened NanoKVM `server-rust` (axum) and web
  (`10.0.87.133`).

## Device topology (historical capture 2026-09-14)

This section is a historical read-only capture of the BliKVM H616 stand. It is
**not** the current state of `10.0.77.118` (see "Device-gated remainder").

- One OTG gadget `kvmd` (KVMD-owned): `hid.usb0/1/2` + `mass_storage.usb0`,
  config `c.1`, UDC `musb-hdrc.2.auto`; modules `dwc2`, `libcomposite`.
- Descriptor at capture: `0x1d6b:0x0104`, `manufacturer=PiKVM`,
  `product=Composite KVM Device`, `serialnumber=CAFEBABE`,
  `Config 1: PiKVM device`.
- BliKVM vendor stack present: `/usr/bin/blikvm/{kvmd-main,kvmd-main.sh,
enable-gadget.sh,kvmd-msd.sh,kvmd-video.sh,ustreamer.bin}`, ATX socket
  `/var/blikvm/atx.sock`, state `/dev/shm/blikvm/atx`; `kvmd-main.service`
  runs the C daemon.
- MSD storage `/var/lib/kvmd/msd` on `mmcblk0p4` mounted read-only.
- Video: ustreamer MJPEG.

## Phases (canonical)

- Phase 0 — recon + this matrix; update `docs/baselines/upstream.md`.
- Phase 1 — Nano-compatible API/GUI parity and hardware-agnostic management
  modules, KVMD still running. Read-only part done; the rest is capability
  hidden.
- Phase 2 — Rust board daemon (kvmd-main → Rust) with handoff and gates.
- Phase 3 — Rust USB gadget (HID + MSD) off KVMD; media UX as reference.
- Phase 4 — video gates (hotplug, resolution, two clients, latency, soak) and
  removal of KVMD once every subsystem is ours.
- Phase 5 — network management.
- Phase 6 — system update and extensions.
- Phase 7 — exec/agent features, off by default.
- Phase 8 — GUI parity verification harness and acceptance report.

## Parity matrix

Reference routes are from `server-rust/src/http/routes.rs`. "Our status" is the
hardened BLi-KVM server as of 2026-09-15: 37 routes.

| Subsystem                                                                                                                                                            | Reference endpoints                                                                                 | Our status                                                                                                                                                          | Target phase            |
| -------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------- |
| auth                                                                                                                                                                 | `/api/auth/{logout,account,password}` (+`setup`,`login`)                                            | implemented (`setup`,`login`,`logout`,`account`,`password`)                                                                                                         | 1 (Nano response shape) |
| capabilities/health/inventory/diagnostics                                                                                                                            | `compatibility_routes`, `/api/health`                                                               | implemented (`capabilities`,`health`,`inventory`,`diagnostics`)                                                                                                     | 1                       |
| HID live                                                                                                                                                             | `/api/hid/{focus,lease}`                                                                            | implemented (restricted profile); gadget executor exists host-side behind a feature                                                                                 | 3                       |
| HID control                                                                                                                                                          | `/api/hid/{mode,usb-wakeup,reset,paste,shortcuts,shortcut,shortcut/leader-key}`                     | hidden; `shortcut/leader-key` implemented on the restricted surface (state-dir preference)                                                                          | 3                       |
| websocket                                                                                                                                                            | `/api/ws`                                                                                           | restricted HID live over `/api/ws` (session lease, same-origin); terminal target 7                                                                                  | 3 (HID) / 7 (terminal)  |
| storage/media                                                                                                                                                        | `/api/storage/image`, `/image/mounted`, `/image/mount`, `/image/delete`, `/cdrom`, `/usb/reconnect` | partial: catalog, select/attach/eject, bounded upload (`/api/download/file`); the gadget rebind primitive exists, the `/usb/reconnect` API surface is not wired yet | 3                       |
| stream                                                                                                                                                               | `/api/stream/mjpeg`, `/h264`, `/h264/direct`, `/mjpeg/detect`, `/mjpeg/detect/stop`                 | `mjpeg` implemented; h264/detect hidden                                                                                                                             | 4 / 8                   |
| vm read-only                                                                                                                                                         | `/api/vm/{info,hardware,hostname,gpio,web-title,device/virtual}`                                    | `hostname`,`info`,`hardware` read-only (`ips`/`deviceKey` intentionally empty)                                                                                      | 1 (done) / 2            |
| vm board                                                                                                                                                             | `/api/vm/{oled,hdmi,hdmi/reset,hdmi/enable,hdmi/disable,gpio}`                                      | hidden; the GUI no longer falls back to `/api/vm/gpio` on this platform                                                                                             | 2                       |
| vm system                                                                                                                                                            | `/api/vm/{system/reboot,session-lock,memory/limit,swap,mouse-jiggler,tls}`                          | hidden                                                                                                                                                              | 2 / 6                   |
| vm ssh/mdns                                                                                                                                                          | `/api/vm/{ssh,ssh/enable,ssh/disable,mdns,mdns/enable,mdns/disable}`                                | hidden                                                                                                                                                              | 5                       |
| vm autostart/script                                                                                                                                                  | `/api/vm/autostart{,/{name}}`, `/api/vm/script{,/upload,/run}`                                      | `autostart` list/content read-only; script hidden                                                                                                                   | 1 (read) / 7            |
| terminal                                                                                                                                                             | `/api/vm/terminal{,/unlock,/enabled}`                                                               | hidden                                                                                                                                                              | 7 (off by default)      |
| application                                                                                                                                                          | `/api/application/{version,update,update/offline,preview}`                                          | `version`,`preview` read-only; `current-version`; app-update (`status`,`upload`,`activate`)                                                                         | 1 (read) / 6            |
| system update, firewall, network, download, tailscale: hidden/read-only flags; `/api/network/dns`, `/api/network/ipv6` and `/api/network/interfaces` read-only views | targets 5/6                                                                                         |
| system log                                                                                                                                                           | `/api/system-log/{config,messages,test}`                                                            | `config`,`messages` read-only                                                                                                                                       | 1 (done) / 6            |
| system time                                                                                                                                                          | `/api/system/time`, `/sync`                                                                         | `time` read-only                                                                                                                                                    | 1 (done) / 3            |
| firewall                                                                                                                                                             | `/api/system/firewall`, `/confirm`                                                                  | hidden                                                                                                                                                              | 5                       |
| network                                                                                                                                                              | `/api/network/{wol,wol/mac,wol/mac/name,dns,ipv6,wifi,wifi/connect,wifi/disconnect}`                | hidden                                                                                                                                                              | 5                       |
| download                                                                                                                                                             | `/api/download/{image,image/status,image/enabled,image/remote/enabled,file}`                        | `image/enabled`,`image/remote/enabled` read-only (remote disabled); upload hidden                                                                                   | 1 (read) / 6            |
| tailscale                                                                                                                                                            | `/api/extensions/tailscale/*`                                                                       | hidden                                                                                                                                                              | 6                       |
| power/ATX                                                                                                                                                            | `/api/application-power` (ours)                                                                     | implemented (restricted profile); KVMD routes default, native GPIO behind a feature                                                                                 | 2 (board owner)         |

## Verification

- `tools/capture_device_kvm_topology.py` records a sanitized, read-only snapshot
  of the gadget functions, a descriptor-block hash, the KVMD hid/msd mode, the
  vendor daemon presence, the ATX socket, the MSD mount mode and active
  services (receipt `evidence/private/device-kvm-topology-20260914-a.json`).
  Run it before and after every gadget/board ownership handoff.
- `tools/capture_device_handoff_preflight.py` is the read-only gate check that
  the target really is the BliKVM H616 stack (KVMD, vendor daemon, gadget, UDC,
  configfs) and returns `handoffReady`; it must pass before any handoff step.
- `tools/capture_device_hid_descriptors.py` captures the gadget HID function
  parameters and report descriptors read-only into a sanitized private receipt,
  with a host-tested bounded parser (name, range, hex and hash checks). Running
  it against the board is part of the device-gated handoff.
- `tools/playwright/gui_parity_probe.mjs` captures the title, route, bounded
  visible menu labels (text, `aria-label` or `title`), tag counts and
  console-error count for one page and writes a sanitized receipt under
  `evidence/private` (never committed). It optionally logs in (username file
  plus password on stdin) so authenticated desktop menus can be captured, and is
  self-tested against a local `file://` fixture
  (`tools/test_gui_parity_probe.mjs`). The live stand is reachable at
  `https://10.0.77.118/` and the reference at `https://10.0.87.133/`, so Phase 8
  runs over HTTP and is not hardware-gated; only the gadget/board phases are.
- `tools/compare_gui_parity.py` compares our build receipt with the reference
  receipt (menu-label and tag-count differences, console-error delta) into a
  sanitized private receipt, so the Phase 8 acceptance report can be produced
  from two probe runs once both endpoints are reachable.
- Each slice keeps `scripts/check.sh` green and adds focused host tests.

## Invariants

- No dead legacy calls: every visible control is either implemented or hidden by
  capability. Shipped builds keep restricted surfaces compiled out.
- Private evidence, credentials and endpoints stay outside Git; secrets only via
  `project-memory-exec`.
- Builds and large artifacts under `/mnt/d/debug/blikvm/hardened-blikvm/`.
- A single owner per gadget line: takeover only through an isolated gate with
  handoff, verification and rollback.
- Reference video/WebRTC (SG2002 native) is not ported; video stays our
  ustreamer MJPEG. Hardware behaviour is sourced from BliKVM (H616), not Nano
  (SG2002).
