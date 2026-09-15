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

The H616 test stand is currently unavailable (see "Device-gated remainder"), so
the device-bound phases cannot proceed. Mode B applies:

- the goal stays active and host-only slices continue;
- every device-bound phase is explicitly marked blocked until the board
  returns;
- when only device-bound work remains, stop and report "waiting for the H616
  stand" instead of inventing work; do not close the goal prematurely.

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
  `/api/application/{version,preview}`, `/api/vm/{info,hardware}` and a
  capability-gated read-only settings System tab; auth already matched the
  reference shapes. The remaining Phase 1 modules (autostart, download,
  network, firewall) are not implemented and stay capability-hidden.
- Phases 2-3 (host, no KVMD in the gadget/board builds): backend seams for the
  ATX, media and HID transition engines; the idempotent configfs USB gadget
  state machine; feature-gated gadget media/HID executors; a feature-gated
  native ATX GPIO executor plus the Linux GPIO character-device driver; and the
  single board owner that serves media, HID and ATX from one control socket and
  owns the composite gadget and the reviewed ATX lines. Owner units and
  packaging are verified by `systemd-analyze` and are non-installable until the
  physical gate. `tools/test_gadget_profile_boundary.py` pins that the
  gadget/gpio profiles link no `bli-kvm-kvmd-*` crate.

## Device-gated remainder (blocked until the H616 stand returns)

The test endpoint `10.0.77.118` is currently a plain Ubuntu 24.04 host with no
`/etc/kvmd`, no `/usr/bin/blikvm` vendor stack and no `/sys/class/udc`, so these
cannot proceed until the board returns:

- capture and pin the real HID report descriptors and function parameters;
- the reviewed physical line handoff that transfers ATX ownership from KVMD,
  then the pulse-timing line/polarity/soak gates;
- media attach/eject/upload, BIOS boot, interruption/reconnect and reboot;
- Phase 4 video gates, Phase 6 image/recovery and Phase 8 acceptance.

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
hardened BLi-KVM server as of 2026-09-14: 27 routes.

| Subsystem                                 | Reference endpoints                                                                                 | Our status                                                                                  | Target phase            |
| ----------------------------------------- | --------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------- | ----------------------- |
| auth                                      | `/api/auth/{logout,account,password}` (+`setup`,`login`)                                            | implemented (`setup`,`login`,`logout`,`account`,`password`)                                 | 1 (Nano response shape) |
| capabilities/health/inventory/diagnostics | `compatibility_routes`, `/api/health`                                                               | implemented (`capabilities`,`health`,`inventory`,`diagnostics`)                             | 1                       |
| HID live                                  | `/api/hid/{focus,lease}`                                                                            | implemented (restricted profile); gadget executor exists host-side behind a feature         | 3                       |
| HID control                               | `/api/hid/{mode,usb-wakeup,reset,paste,shortcuts,shortcut,shortcut/leader-key}`                     | hidden                                                                                      | 3                       |
| websocket                                 | `/api/ws`                                                                                           | not implemented                                                                             | 3 (HID) / 7 (terminal)  |
| storage/media                             | `/api/storage/image`, `/image/mounted`, `/image/mount`, `/image/delete`, `/cdrom`, `/usb/reconnect` | partial (`/api/virtual-media`,`/api/virtual-media/catalog`)                                 | 3                       |
| stream                                    | `/api/stream/mjpeg`, `/h264`, `/h264/direct`, `/mjpeg/detect`, `/mjpeg/detect/stop`                 | `mjpeg` implemented; h264/detect hidden                                                     | 4 / 8                   |
| vm read-only                              | `/api/vm/{info,hardware,hostname,gpio,web-title,device/virtual}`                                    | `hostname`,`info`,`hardware` read-only (`ips`/`deviceKey` intentionally empty)              | 1 (done) / 2            |
| vm board                                  | `/api/vm/{oled,hdmi,hdmi/reset,hdmi/enable,hdmi/disable,gpio}`                                      | hidden                                                                                      | 2                       |
| vm system                                 | `/api/vm/{system/reboot,session-lock,memory/limit,swap,mouse-jiggler,tls}`                          | hidden                                                                                      | 2 / 6                   |
| vm ssh/mdns                               | `/api/vm/{ssh,ssh/enable,ssh/disable,mdns,mdns/enable,mdns/disable}`                                | hidden                                                                                      | 5                       |
| vm autostart/script                       | `/api/vm/autostart{,/{name}}`, `/api/vm/script{,/upload,/run}`                                      | hidden                                                                                      | 6 (autostart) / 7       |
| terminal                                  | `/api/vm/terminal{,/unlock,/enabled}`                                                               | hidden                                                                                      | 7 (off by default)      |
| application                               | `/api/application/{version,update,update/offline,preview}`                                          | `version`,`preview` read-only; `current-version`; app-update (`status`,`upload`,`activate`) | 1 (read) / 6            |
| system update                             | `/api/system-update/{version,raw-enabled,check,status,download,install,rollback,confirm}`           | hidden                                                                                      | 6 (adapt; raw off)      |
| system log                                | `/api/system-log/{config,messages,test}`                                                            | `config`,`messages` read-only                                                               | 1 (done) / 6            |
| system time                               | `/api/system/time`, `/sync`                                                                         | `time` read-only                                                                            | 1 (done) / 3            |
| firewall                                  | `/api/system/firewall`, `/confirm`                                                                  | hidden                                                                                      | 5                       |
| network                                   | `/api/network/{wol,wol/mac,wol/mac/name,dns,ipv6,wifi,wifi/connect,wifi/disconnect}`                | hidden                                                                                      | 5                       |
| download                                  | `/api/download/{image,image/status,image/enabled,image/remote/enabled,file}`                        | hidden                                                                                      | 6                       |
| tailscale                                 | `/api/extensions/tailscale/*`                                                                       | hidden                                                                                      | 6                       |
| power/ATX                                 | `/api/application-power` (ours)                                                                     | implemented (restricted profile); KVMD routes default, native GPIO behind a feature         | 2 (board owner)         |

## Verification

- `tools/capture_device_kvm_topology.py` records a sanitized, read-only snapshot
  of the gadget functions, a descriptor-block hash, the KVMD hid/msd mode, the
  vendor daemon presence, the ATX socket, the MSD mount mode and active
  services (receipt `evidence/private/device-kvm-topology-20260914-a.json`).
  Run it before and after every gadget/board ownership handoff.
- `tools/playwright/gui_parity_probe.mjs` captures the title, route, bounded
  visible menu labels, tag counts and console-error count for one page and
  writes a sanitized receipt under `evidence/private` (never committed). It is
  self-tested against a local `file://` fixture
  (`tools/test_gui_parity_probe.mjs`), so the harness itself is verified
  host-side; running it against our deployed build and the reference `.133` is
  device-gated.
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
