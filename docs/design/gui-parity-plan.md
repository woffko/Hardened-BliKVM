# Hardened BLi-KVM vendor replacement plan

Status: canonical plan, rewritten 2026-09-16. **This document supersedes the
2026-09-14 revision of itself and the older `docs/implementation-plan.md`.** It
drives the full replacement of the BliKVM vendor stack on the H616 board with
our hardened port: privileged Rust backend, Nano-derived GUI, our Rust USB
gadget, our Rust board daemon, the vendor `ustreamer` as the video engine and
the retained signed application-update model. Every slice is implemented,
verified and committed in English with a green `scripts/check.sh`.

## Supersedes

- The 2026-09-14 revision, including its phase numbering (0-8) and its framing
  of "removing KVMD". Removal was never the objective: **every vendor subsystem
  is replaced by our equivalent, and the old component is retired only after
  the replacement passes its device gate.**
- The old objective "идти по зафиксированному плану BLi-KVM" and its Phase 3
  restriction "ATX только через KVMD fixed waiting routes, без нативного
  GPIO-перехвата" no longer apply.
- Phase mapping: old Phase 0/1 → E0-E1 recon/API baseline; old Phase 2 → E2
  (board daemon) plus the ATX part already delivered in E0; old Phase 3 → E1
  (storage/media) plus the gadget part already delivered in E0; old Phase 4 →
  E0 (video); old Phase 5 → E3 (network); old Phase 6 → E4 (update); old
  Phase 7 → E5 (extensions, off by default); old Phase 8 → E5 (parity and
  acceptance).

## Principles and locked decisions

1. **Replace, then retire.** A vendor component is stopped or removed only
   after its replacement has a passing device gate and a private receipt.
2. **Functionality stays live.** Video, HID, MSD and ATX work throughout; "no
   picture until later" is not an acceptable state.
3. Architecture A: the hardened BLi-KVM server is the privileged management
   backend and exposes the NanoKVM-compatible API; the Nano-derived web GUI is
   kept.
4. Media and HID run on our own Rust USB gadget. Descriptors are generic and
   driver-safe (`idVendor=0x1d6b`, `idProduct=0x0104`) with unique strings and
   standard HID subclass/protocol/report lengths (keyboard 1/1/8, relative
   mouse 0/0/7, absolute mouse 1/2/5).
5. The vendor C `kvmd-main` (gpio/atx/fan/oled/rtc/switch) is replaced by our
   Rust board daemon; native ATX GPIO is the only ATX path.
6. Video parity is **MJPEG**: the vendor `ustreamer` stays the capture engine
   and our `bli-kvm-video-proxy` serves it. h264/WebRTC (SG2002-native in the
   reference) is not ported and stays hidden.
7. Updates use the retained signed application-update model; `raw` stays off.
8. Exec features (terminal/script/picoclaw) exist but stay off by default.
9. KVMD (PiKVM Python) and the C `kvmd-main` are hard-retired: the units are
   masked, not kept as a fallback. Recovery is only from the recorded unit
   backups.
10. Private evidence, credentials and endpoints stay outside Git; device
    mutations run with explicit operator confirmation and a private receipt.

## Replacement matrix

"Vendor" is what the board shipped; "ours" is the replacement. States are as of
2026-09-16.

| # | Vendor component on the board                              | Our replacement                                       | State                                                        |
| - | ---------------------------------------------------------- | ----------------------------------------------------- | ------------------------------------------------------------ |
| 1 | `kvmd-nginx` HTTPS front                                   | `hardened-blikvm-nginx`                               | deployed, serving 80/443                                     |
| 2 | KVMD API/WebUI/auth                                        | `bli-kvm-server` + Nano-derived GUI (37 routes)       | deployed; some routes capability-hidden                      |
| 3 | KVMD HID (`kvmd-otg`, `hid.usb0/1/2`)                      | our gadget HID in `hardened-blikvm-board-owner`       | deployed and accepted (composite gadget)                     |
| 4 | KVMD MSD + `/var/lib/kvmd/msd` (`mmcblk0p4`)                | our gadget MSD, our own image layout                  | E1: our fstab mount, reference storage API live; remount-after-eject and reconnect still open |
| 5 | KVMD streamer supervision                                  | vendor `ustreamer` + `bli-kvm-video-proxy`            | deployed by E0; MJPEG gates passed on the live surface       |
| 6 | KVMD ATX routes + C `kvmd-main` ATX                        | `bli-kvm-atx-gpio-executor` in the board owner        | deployed and accepted (500/5000/500 ms)                      |
| 7 | C `kvmd-main`: fan, OLED ST7789, display buttons, activity LED, serial switch, RTC, boot config, recovery inputs | Rust board daemon                                    | policy crates and recon snapshots exist; no daemon or units (E2) |
| 8 | KVMD system actions (reboot/session-lock/memory/swap)      | `system-action-*` crates                              | session lock implemented; reboot and shutdown endpoints exist and are refused (capability `unavailable`) until the privileged profile is armed (E3); memory and swap are next |
| 9 | KVMD network/wifi/wol/firewall/ssh/mdns                    | `bli-kvm-server` network module                       | read-only views live; the bounded DNS and IPv6 writes are implemented as capability-gated POST routes that forward to the board owner (`BLI_KVM_NETWORK_WRITES=1`); firewall, ssh/mdns, wol and wifi remain (E3) |
| 10 | Vendor update watchdog + signed slots serving our build    | retained signed-slot pipeline                         | slots and trust intact, serving/watchdog idled by the cutover (E4) |
| 11 | Extras: janus (WebRTC), webterm, vnc, ipmi                  | reference behaviour: hidden/off by default            | not ported; `janus.service` disabled (E5)                    |
| 12 | KVMD diagnostics                                            | our diagnostic collector/store/export stack           | host-side and partially device-verified                      |

## API parity status

Our hardened server exposes, as of 2026-09-16: 46 routes. The read-only
management surface is implemented and capability-visible: `/api/system/time`
(`time` read-only), `/api/system-log/{config,messages}`
(`config`,`messages` read-only), `/api/application/{version,preview}`
(`version`,`preview` read-only), `/api/vm/{hostname,info,hardware}`
(`hostname`,`info`,`hardware` read-only), `/api/vm/autostart`
(`autostart` list/content read-only) and
`/api/download/image/{enabled,remote/enabled}`
(`image/enabled`,`image/remote/enabled` read-only). The device-wide session
auto-lock (`/api/vm/session-lock`) is implemented: the settings duration and the
forced lock are enforced by the server's own session store. Media
select/attach/eject,
the restricted HID lease, `/api/ws` and `/api/hid/shortcut/leader-key` are
implemented on the restricted surface. The remaining reference routes stay
capability-hidden until their phase lands: HID control modes, storage
reconnect, h264 and detect, vm board/system, ssh and mdns, script and terminal,
firewall, network writes, download upload, tailscale and the application update
UI.

## Current device state (2026-09-16)

`https://10.0.77.118/` serves our board profile: node
`hardened-blikvm-board-owner` (gadget, media, HID, ATX over
`/run/hardened-blikvm-gadget/control.sock`) plus
`hardened-blikvm-board-server` (KVMD-free, `127.0.0.1:18080`) behind
`hardened-blikvm-nginx`. HID, Power and virtual media report `read_write`; the
`hardened` composite gadget is bound with the reviewed image attached. Plain
HTTP is a `308` to HTTPS because session cookies are `Secure`.

KVMD units are stopped, disabled and masked; the KVMD-coupled
`hardened-blikvm-platform-broker`, `hardened-blikvm-video-proxy`,
`hardened-blikvm-server-current` and `hardened-blikvm-update-watchdog.timer`
are stopped and disabled with unit backups under
`/var/lib/hardened-blikvm/backup-20260916/serving/`.

**E0 is done (2026-09-16).** The vendor `ustreamer` runs under
`hardened-blikvm-ustreamer.service` and our `bli-kvm-video-proxy` under
`hardened-blikvm-board-video-proxy.service`, with our own udev rule selecting
the USB capture; the KVMD udev rule was backed up and removed. Frame timing,
two-client and 60 s soak gates passed on the live authenticated surface.
Receipts: `evidence/private/board-video-restore-20260916-a.json`,
`video-frame-timing-20260916-b.json`,
`video-two-client-probe-20260916-a.json`,
`video-soak-probe-20260916-a.json`; baseline
`docs/baselines/board-video-restore.md`.

**E1 is in progress (2026-09-16).** The reference storage surface is
implemented and live (43 routes): list, mounted, cdrom, mount, delete and
usb-reconnect over the control surface, with an atomic mount command, an observe
request and a reconnect command. The MSD partition now mounts through our own
fstab entry at our images directory, and the media paths are provisioned for the
unprivileged server user in tmpfiles. The storage gate now passes end to end (list, mounted, cdrom, mount,
usb/reconnect, eject, re-attach) and so does the media image upload cycle
(upload, mount, eject, delete, re-attach). Six device defects were fixed on the
way: zero-byte configfs writes were silent no-ops, the media swap must unbind
and rebuild the function, the board certificate had no subject alternative
name, the nginx temp buffers needed the unit runtime directory, the server
sandbox blocked the media writes and the upload route bypassed the session and
CSRF middleware. Only the reboot half of the gate is left, and it needs explicit
operator confirmation. Receipts
`evidence/private/board-storage-e1-20260916-{a,b}.json`,
`evidence/private/storage-acceptance-20260916-a.json`; baseline
`docs/baselines/board-storage-e1.md`. Other receipts:
`evidence/private/board-serving-cutover-20260916-a.json`,
`evidence/private/kvmd-removal-20260916-a.json`,
`evidence/private/phase8-acceptance-20260916-d.json`; baselines
`docs/baselines/board-serving-cutover.md`,
`docs/baselines/board-owner-acceptance.md`.

## Phases

### E0 - video replacement (immediate)

Goal: restore the picture with the vendor streamer under our own units, keeping
KVMD retired.

- Host: `packaging/systemd/hardened-blikvm-ustreamer.service` (vendor
  `ustreamer`, `/dev/kvmd-video`, MJPG, 1280x720, 40 fps, same-frame drop and
  `--slowdown`, socket under `/run/hardened-blikvm-ustreamer/`) and
  `packaging/systemd/hardened-blikvm-board-video-proxy.service` (our proxy,
  socket `/run/hardened-blikvm-video/mjpeg.sock`). A udev rule replaces the
  KVMD rule that maps the UVC capture to `/dev/kvmd-video`.
- Device gate: frames flow through the authenticated `/api/stream/mjpeg`;
  `video_frame_timing_probe.mjs` passes; two clients stream simultaneously;
  the soak probe passes.
- Receipts: `video-two-client-probe-*.json`, `video-frame-timing-*.json`,
  `video-soak-probe-*.json`. Rollback: stop both units.

### E1 - storage and media

Goal: own the media path end to end.

- Move images into our own layout (own directory and mount handling instead of
  the symlink into `/var/lib/kvmd/msd`), keep the catalog reload writer.
- Wire the missing `/api/storage/usb/reconnect` surface.
- Device gates: attach, eject, reconnect, upload and a reboot with the image
  re-attached. Receipts: `virtual-media-acceptance-*.json` plus a reconnect
  receipt.

### E2 - board daemon instead of C `kvmd-main`

Goal: the vendor C daemon is replaced by one Rust daemon. Design:
`docs/design/board-daemon-v1.md` (hardware facts, transports, policy reuse,
control surface, ownership handoff, per-subsystem gates).

- Deliver a `bli-kvm-boardd` binary and unit covering fan/PWM and thermal,
  OLED ST7789 and display buttons, activity LED, RTC and boot config (the
  serial switch has no transport on this revision and stays hidden), exposed
  over its own control socket and mapped by the server.
- Device gates: one per subsystem, starting from the existing recon receipts
  (`device-oled-topology`, `device-fan-gpio`, `device-serial-topology`,
  `device-rtc`).
- Retire `kvmd-main`, its `/var/blikvm/*.sock` sockets and
  `/dev/shm/blikvm/*` state only after every subsystem gate passes;
  `kvmd-fix.service` is handled in the same slice.

### E3 - system and network writes

Goal: close the remaining KVMD management surfaces. Design:
`docs/design/system-actions-v1.md` for the system actions and
`docs/design/network-writes-v1.md` for the bounded network writes.

- System actions: the session auto-lock is implemented; reboot and shutdown go
  through the board owner's journaled policy path (the store and maintenance
  dispatcher already passed on the device) and are advertised only by a new
  `systemActions` capability; memory and swap writes are bounded owner actions
  with read-back. The capability manifest carries a `network` section
  (`status`, `dns`, `ipv6`), and the two network writes are advertised as
  `read_write` only when the board owner serves the reviewed network-write
  surface.
- Network writes: DNS and IPv6 first, then firewall, SSH/mDNS, Wake-on-LAN and
  wifi where the hardware supports it (the board has `wlan0` and
  NetworkManager); interactive wifi stays out of scope until a reviewed helper
  exists.
- Device gates: capability-gated safe writes plus a reboot-persistence proof;
  receipt `system-action-gate-*.json` and `network-write-gate-*.json`.

### E4 - update path

Goal: ship our build through the retained signed update model.

- Build the production bundle from the current source, publish it as a
  candidate, run candidate health, confirm it into `current`, and re-enable the
  activation/watchdog units. Our units and binaries are delivered by the slot.
- Serving shape (decided by the operator 2026-09-16): the signed model serves
  from the slot (`server-current` runs the slot binary with the web root inside
  the slot) while the units come from the runtime package. Option (a) applies:
  package our board-profile units (board owner, board server, board daemon,
  ustreamer, video proxy, nginx) and the board daemon binary into the runtime
  package, and switch the slot-serving units to our arrangement (board owner
  plus control socket) so the confirmed slot is served by the board profile.
  The production bundle then carries the board daemon and the web, and the
  bundle verifier and its tests pin both. Implemented on the host: the shipped
  build compiles the board profile, the prototype bundle verifies and stages
  `bli-kvm-boardd` plus our board units, the Debian runtime package maps the
  daemon and the units into the system, the production update bundle maps and
  requires the daemon payload, and `server-current`/`server-candidate` now
  run the slot payload against the board owner control socket, the board video
  proxy and the media catalog while the activation-flow fixture models the
  board placeholders. The device cycle (candidate, health, confirm, reboot) is
  the remaining step.
- Device gate: the signed update commit cycle and a reboot that keeps the new
  `current`. Receipts: `persistent-write-*-commit-*.json` and the update
  runtime/status receipts.
- Blocked on operator material: producing the signed production bundle needs the
  operator-held signing key and the current enrollment request file (the last
  recorded enrollment request digest is
  `0c7b995accf0edffe9a463b6b9822a74c079465e663ba93da4e824c170909208`, while the
  private key is never stored with the bundle). The host chain is verified up to
  and including the runtime package, so once the key and request are provided the
  bundle can be built and published as a candidate.

### E5 - extensions, parity and acceptance

Goal: prove the replacement is complete.

- Extensions: keep janus/webterm/vnc/ipmi hidden or off by default, matching
  the reference; terminal and script stay behind the exec decision.
- Update `tools/phase8_acceptance_report.py` so the manifest reflects the new
  phases (video in E0, storage in E1, board daemon in E2, network in E3, update
  in E4) and keeps "KVMD removal" after the video gates.
- Run the GUI parity probe against `https://10.0.87.133/` and compare; produce
  the final report, reboot/soak acceptance and the baselines.

## Gates, order and rollback

- Order per subsystem: replacement deployed, gate passed, receipt recorded,
  then the vendor component is retired. No bulk removals.
- One owner per gadget, video and ATX line; every switch is reversible with a
  unit backup and documented restore steps.
- Every slice keeps `scripts/check.sh` green and adds focused host tests; the
  board bundle and `scripts/verify-systemd-units.sh` carry the new units.

## Verification tooling

- `tools/capture_device_handoff_preflight.py` and
  `tools/capture_device_kvm_topology.py`: read-only board gates before and after
  every ownership change.
- `tools/capture_device_hid_descriptors.py`: HID function/descriptor capture.
- `tools/stage_device_board_profile_gate.py --confirm-handoff`: reversible
  board-owner handoff gate (owner socket and `hardened` gadget).
- `tools/stage_device_kvmd_removal.py`: evidence-gated KVMD retirement with unit
  backups and `--restore`.
- `tools/playwright/video_frame_timing_probe.mjs`,
  `tools/playwright/video_two_client_probe.mjs`,
  `tools/playwright/video_soak_probe.mjs`: MJPEG gates for E0.
- `tools/playwright/gui_parity_probe.mjs` and `tools/compare_gui_parity.py`:
  parity capture and comparison for E5.
- `tools/phase8_acceptance_report.py`: audited acceptance report over the
  manifest.
- `tools/build-board-profile.sh`, `tools/verify_board_profile_bundle.py`,
  `tools/stage_device_board_bundle.py`: board build, verification and staging.

## Invariants

- No dead legacy calls: every visible control is implemented or hidden by
  capability. Shipped builds keep restricted surfaces compiled out.
- Private evidence, credentials and endpoints stay outside Git; secrets only
  via `project-memory-exec`.
- Builds and large artifacts under `/mnt/d/debug/blikvm/hardened-blikvm/`.
- A single owner per gadget line; takeover only through an isolated gate with
  handoff, verification and rollback.
- Reference WebRTC is not ported; video stays our ustreamer MJPEG. Hardware
  behaviour is sourced from BliKVM (H616), not Nano (SG2002).

## Provenance and historical topology

- PiKVM KVMD: `github.com/pikvm/kvmd` v3.291 through the Bli-PiKVM line; the
  device carries the `srepac/kvmd-armbian` checkout (`e67a3b4`).
- Original BliKVM H616 hardware/app: `blikvm/blikvm` /
  `ThomasVon2021/blikvm` (C `blikvm_server`/`kvmd-main`, `src/kvmd/blikvm_*`,
  `package/kvmd-*`, `ustreamer.bin`, janus).
- Reference GUI/management: Hardened NanoKVM `server-rust` (axum) and web
  (`10.0.87.133`).
- Historical device capture and file inventory: `docs/baselines/upstream.md`,
  `docs/baselines/hardware.md`, `docs/baselines/device-video.md`,
  `docs/baselines/device-fan-gpio.md`, `docs/baselines/device-oled-topology.md`,
  `docs/baselines/device-serial-topology.md`, `docs/baselines/device-rtc.md`.
- One OTG gadget per owner; module set `dwc2` and `libcomposite`; the single UDC
  `musb-hdrc.2.auto` is the serialisation point for every handoff.
