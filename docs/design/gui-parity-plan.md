# Hardened BLi-KVM GUI parity plan

Status: approved 2026-09-14. This plan drives the port of the Hardened NanoKVM
operator surface onto the H616 BliKVM while removing KVMD. It is reference-first:
each slice starts from the reference sources and a reference/own screenshot, is
implemented, verified, and committed in English with a green `scripts/check.sh`.

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
   rewritten in Rust; ATX becomes our daemon.
5. Update follows the reference behaviour, adapted to this board and to the
   retained signed application-update model; `raw` stays off by default.
6. Exec features (terminal/script/picoclaw) exist but are off by default in
   settings, matching the reference.
7. KVMD (PiKVM Python) and the C `kvmd-main` are removed completely by the end;
   they remain a fallback until each subsystem is proven.

## Progress (2026-09-14)

Phases 0-1 are done and the KVMD-free host stack for Phases 2-3 is complete.
Everything below is committed with a green `scripts/check.sh`.

- Phase 0: plan, corrected provenance and the read-only topology probe.
- Phase 1 (read-only): `/api/system/time`, `/api/system-log/*`,
  `/api/application/{version,preview}` and a capability-gated read-only
  settings System tab; auth already matched the reference shapes.
- Phase 2-3 (host, no KVMD in the gadget/board builds): backend seams for the
  ATX, media and HID transition engines; the idempotent configfs USB gadget
  state machine; feature-gated gadget media/HID executors; a feature-gated
  native ATX GPIO executor plus the Linux GPIO character-device driver; and the
  single board owner that serves media, HID and ATX from one control socket and
  owns the composite gadget and the reviewed ATX lines. Owner units and
  packaging are verified by `systemd-analyze` and are non-installable until the
  physical gate. `tools/test_gadget_profile_boundary.py` pins that the
  gadget/gpio profiles link no `bli-kvm-kvmd-*` crate.

### Device-gated remainder (needs the H616 stand)

The test endpoint `10.0.77.118` is currently a plain Ubuntu host with no
`/etc/kvmd`, no vendor stack and no UDC, so these cannot proceed until the
board returns:

- capture and pin the real HID report descriptors and function parameters;
- the reviewed physical line handoff that transfers ATX ownership from KVMD,
  then the pulse-timing line/polarity/soak gates;
- media attach/eject/upload, BIOS boot, interruption/reconnect and reboot;
- Phase 4 video gates, Phase 6 image/recovery and Phase 7 acceptance.

## Provenance

- PiKVM KVMD: `github.com/pikvm/kvmd` v3.291 (upstream daemon), installed on the
  device through the Bli-PiKVM line (`RainCat1998 == woffko/Bli-PiKVM`); the
  device carries the `srepac/kvmd-armbian` checkout (`e67a3b4`).
- Original BliKVM H616 hardware/app: `blikvm/blikvm` / `ThomasVon2021/blikvm`
  (C `blikvm_server`/`kvmd-main`, `src/kvmd/blikvm_*`, `package/kvmd-*`,
  `ustreamer.bin`, janus). Source for H616 board behaviour.
- Reference GUI/management: Hardened NanoKVM `server-rust` (axum) and web
  (`10.0.87.133`).

## Current device topology (read-only evidence)

- One OTG gadget `kvmd` (KVMD-owned): `hid.usb0/1/2` + `mass_storage.usb0`,
  config `c.1`, UDC `musb-hdrc.2.auto`; modules `dwc2`, `libcomposite`.
- Descriptor today: `0x1d6b:0x0104`, `manufacturer=PiKVM`,
  `product=Composite KVM Device`, `serialnumber=CAFEBABE`,
  `Config 1: PiKVM device`.
- BliKVM vendor stack present: `/usr/bin/blikvm/{kvmd-main,kvmd-main.sh,
enable-gadget.sh,kvmd-msd.sh,kvmd-video.sh,ustreamer.bin}`, ATX socket
  `/var/blikvm/atx.sock`, state `/dev/shm/blikvm/atx`; `kvmd-main.service`
  runs the C daemon.
- MSD storage `/var/lib/kvmd/msd` on `mmcblk0p4` mounted read-only.
- Video: ustreamer MJPEG.

## Phases

- Phase 0 — recon + this matrix; update `docs/baselines/upstream.md`.
- Phase 1 — Nano-compatible API/GUI parity and hardware-agnostic management
  modules, KVMD still running.
- Phase 2 — Rust board daemon (kvmd-main → Rust) with handoff and gates.
- Phase 3 — Rust USB gadget (HID + MSD) off KVMD; media UX as reference.
- Phase 4 — remove KVMD completely once every subsystem is ours.
- Phase 5 — network management.
- Phase 6 — system update and extensions.
- Phase 7 — exec/agent features, off by default.
- Phase 8 — GUI parity verification harness and report.

## Parity matrix

Reference routes are from `server-rust/src/http/routes.rs`. "Our status" is the
hardened BLi-KVM server today (20 routes).

| Subsystem                                 | Reference endpoints                                                                                 | Our status                                                      | Target phase               |
| ----------------------------------------- | --------------------------------------------------------------------------------------------------- | --------------------------------------------------------------- | -------------------------- |
| auth                                      | `/api/auth/{logout,account,password}` (+`setup`,`login`)                                            | implemented (`setup`,`login`,`logout`,`account`,`password`)     | 1 (Nano response shape)    |
| capabilities/health/inventory/diagnostics | `compatibility_routes`, `/api/health`                                                               | implemented (`capabilities`,`health`,`inventory`,`diagnostics`) | 1                          |
| HID live                                  | `/api/hid/{focus,lease}`                                                                            | implemented (restricted profile)                                | 3 (move to gadget)         |
| HID control                               | `/api/hid/{mode,usb-wakeup,reset,paste,shortcuts,shortcut,shortcut/leader-key}`                     | hidden                                                          | 3                          |
| websocket                                 | `/api/ws`                                                                                           | not implemented                                                 | 3 (HID) / 7 (terminal)     |
| storage/media                             | `/api/storage/image`, `/image/mounted`, `/image/mount`, `/image/delete`, `/cdrom`, `/usb/reconnect` | partial (`/api/virtual-media`,`/api/virtual-media/catalog`)     | 3                          |
| stream                                    | `/api/stream/mjpeg`, `/h264`, `/h264/direct`, `/mjpeg/detect`, `/mjpeg/detect/stop`                 | `mjpeg` implemented; h264/detect hidden                         | 4 / 8                      |
| vm read-only                              | `/api/vm/{info,hardware,hostname,gpio,web-title,device/virtual}`                                    | `hostname` implemented                                          | 1 / 2                      |
| vm board                                  | `/api/vm/{oled,hdmi,hdmi/reset,hdmi/enable,hdmi/disable,gpio}`                                      | hidden                                                          | 2                          |
| vm system                                 | `/api/vm/{system/reboot,session-lock,memory/limit,swap,mouse-jiggler,tls}`                          | hidden                                                          | 2 / 6                      |
| vm ssh/mdns                               | `/api/vm/{ssh,ssh/enable,ssh/disable,mdns,mdns/enable,mdns/disable}`                                | hidden                                                          | 5                          |
| vm autostart/script                       | `/api/vm/autostart{,/{name}}`, `/api/vm/script{,/upload,/run}`                                      | hidden                                                          | 6 (autostart) / 7 (script) |
| terminal                                  | `/api/vm/terminal{,/unlock,/enabled}`                                                               | hidden                                                          | 7 (off by default)         |
| application                               | `/api/application/{version,update,update/offline,preview}`                                          | `current-version`; app-update (`status`,`upload`,`activate`)    | 6 (adapt to retained)      |
| system update                             | `/api/system-update/{version,raw-enabled,check,status,download,install,rollback,confirm}`           | hidden                                                          | 6 (adapt; raw off)         |
| system log                                | `/api/system-log/{config,messages,test}`                                                            | hidden                                                          | 1 (read) / 6               |
| system time                               | `/api/system/time`, `/sync`                                                                         | hidden                                                          | 1 (read) / 3               |
| firewall                                  | `/api/system/firewall`, `/confirm`                                                                  | hidden                                                          | 5                          |
| network                                   | `/api/network/{wol,wol/mac,wol/mac/name,dns,ipv6,wifi,wifi/connect,wifi/disconnect}`                | hidden                                                          | 5                          |
| download                                  | `/api/download/{image,image/status,image/enabled,image/remote/enabled,file}`                        | hidden                                                          | 6                          |
| tailscale                                 | `/api/extensions/tailscale/*`                                                                       | hidden                                                          | 6                          |
| power/ATX                                 | `/api/application-power` (ours)                                                                     | implemented (restricted profile)                                | 2 (our Rust ATX)           |

## Verification

- `tools/capture_device_kvm_topology.py` records a sanitized, read-only snapshot
  of the gadget functions, a descriptor-block hash, the KVMD hid/msd mode, the
  vendor daemon presence, the ATX socket, the MSD mount mode and active
  services (receipt `evidence/private/device-kvm-topology-20260914-a.json`).
  Run it before and after every gadget/board ownership handoff.
- `tools/playwright/gui_parity_probe.mjs` captures screenshots and DOM structure
  of every visible menu on our build and the reference `.133`, writing receipts
  under `evidence/private` (never committed).
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
