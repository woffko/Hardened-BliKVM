# Vendor replacement status

Status: every vendor component is retired or hidden on the board and the
replacement is served by our own arrangement; three device items are still open
because they need operator material or approval (the button gate, the armed
reboot gate and the signed slot-serving cutover).

## What replaced what

| Vendor component | Replacement | Evidence |
| --- | --- | --- |
| `kvmd-nginx` | `hardened-blikvm-nginx` | vendor units inactive, disabled and masked, including the `kvmd-fix` and `kvmd-bootconfig` helpers; `kvmd-removal-20260916-a.json` |
| KVMD API/WebUI/auth | `bli-kvm-server` (56 routes) | route-drift guard; `gui-parity-compare-20260917-c.json` |
| KVMD HID | our composite gadget in the board owner | gadget `hardened` bound to `musb-hdrc.2.auto` with `hid.usb0/1/2` and `mass_storage.usb0` |
| KVMD MSD | our gadget MSD on `mmcblk0p4` | `storage-acceptance-20260916-b.json`, `storage-upload-cycle-20260916-a.json` |
| KVMD streamer supervision | vendor `ustreamer` under our unit plus `bli-kvm-video-proxy` | `video-frame-timing-20260916-b.json`, `video-two-client-probe-20260916-a.json`, `video-soak-probe-20260916-a.json` |
| KVMD ATX and C `kvmd-main` ATX | `bli-kvm-atx-gpio-executor` in the board owner | the owner unit sets `BLI_KVM_GPIO_ATX_PROFILE=1`; the capability advertises `power_short`, `power_long` and `reset` again |
| C `kvmd-main` peripherals | Rust board daemon | fan, activity LED and panel handoffs recorded (`boardd-*-gate-20260917-a.json`, `observed-pending`); the ATX host LED inputs feed `inventory.powerLed` |
| KVMD system actions | `system-action-*` crates | session auto-lock; journaled reboot and shutdown behind the reviewed acceptance step |
| KVMD network writes | bounded owner writes | `network-write-gate-20260917-f.json` (`result=passed`: resolver write and restore; IPv6 skipped while the owner has no host network namespace) |
| Vendor update watchdog and slots | retained signed-slot pipeline | host chain verified (`7e0ab131…` bundle, `0743cc85…` runtime package); the device cutover needs the operator signing key |
| KVMD diagnostics | our diagnostic collector, store and export | host-side plus device receipts |

## Board facts recorded on 2026-09-17

- Our five units active; every KVMD unit inactive and masked.
- Composite gadget bound with all four functions.
- MSD partition mounted at our images directory with the reviewed image.
- Two video sockets present; the interface answers over TLS with the Rust
  backend identity.
- The reference stand serves a NanoKVM-branded build; desktop-surface parity
  matches except the product name, and our settings surface reports five tabs.
- `vendor-replacement-20260917-a.json` (`result=passed`): our five units active,
  every KVMD unit inactive and masked, the composite gadget bound with its four
  functions, the MSD partition mounted at our images directory with the reviewed
  image, both video sockets present, and the health endpoint answering with the
  Rust backend and our platform identity.

## Remaining acceptance items

1. Button gate (`boardd-button-gate-*.json`) — operator-owned; the button line
   is an unused input on this revision.
2. Armed reboot gate (`system-action-gate-*.json`) — needs the operator's
   separate reboot approval; the storage reboot probe is staged
   (`storage-reboot-gate-20260917-a.json`, all preserved facts intact).
3. Signed slot-serving cutover (`update-slot-serving-*.json`) — needs the
   operator signing key and the current enrollment request.
4. Physical confirmation of the fan, LED, panel, ATX LED and button handoffs is
   recorded as `passed` on 2026-09-17 (`boardd-*-gate-20260917-b.json`), and the
   plan schedules a release-stage re-verification of the same subsystems with
   fresh receipts before the release is accepted.
5. A 24-hour soak and the post-reboot acceptance run, both after the reboot
   approval.
