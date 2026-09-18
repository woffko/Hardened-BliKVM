# Upstream source baseline

Verified on 2026-08-30.

> Stand status (2026-09-15): the H616 board is present and ready
> (`<board-address>`, Armbian Jammy, KVMD and the vendor stack installed, `kvmd`
> gadget bound, `handoffReady=true`). An earlier "plain Ubuntu" note was a
> local-shell mistake and is retracted.

| Source                    | Revision                                                       | Role                                                         |
| ------------------------- | -------------------------------------------------------------- | ------------------------------------------------------------ |
| `blikvm/blikvm`           | `3e34c04a1dfa84636050585d12d5e585098258ea`, tag `v2.2.3-alpha` | Current BliKVM hardware and application reference            |
| `woffko/Bli-PiKVM`        | `6966a969f46f80172c5a1d1e0aa48be812625ac2`                     | PiKVM/KVMD-on-H616 procedure and MSD compatibility reference |
| Hardened NanoKVM checkout | `387793ab944437bbedc534d2ba231dd88d1fd2ca`                     | Reusable Rust/React/security/release source baseline         |

The inspected field image contained an older vendor checkout at
`26fa184b9305ea256ece8ed7aaec3b9ede29ab1f` and KVMD `3.291`. It is evidence
for the existing hardware integration only. It is not a production source or
update baseline.

Before importing code, record the exact file-level license and provenance.
Never silently replace one pinned revision with a newer working tree.

## Corrected provenance (2026-09-14)

The original BliKVM is an independent KVM stack, not a PiKVM fork, and it does
not use PiKVM's Python `kvmd` as its core daemon. Its hardware daemon is the
BliKVM-native C program `kvmd-main` (built from `src/blikvm_server.c` and
`src/kvmd/blikvm_{gpio,atx,fan,oled,rtc,switch}`), with ATX state on
`/var/blikvm/atx.sock` and `/dev/shm/blikvm/atx`. HID and mass storage are
handled through the Linux USB gadget (configfs, `/dev/hidg*`), video through
V4L2 -> uStreamer (MJPEG/H.264) and Janus for WebRTC.

The device under test is a hybrid: alongside the BliKVM vendor stack it runs
PiKVM `kvmd` 3.291, installed through the Bli-PiKVM line
(`RainCat1998/Bli-PiKVM`, the same project as `woffko/Bli-PiKVM`); the on-device
checkout is `srepac/kvmd-armbian` (`e67a3b4`). KVMD owns the single OTG gadget
on this device, which is why HID and MSD currently go through KVMD.

The GUI/management reference for parity work is Hardened NanoKVM
(`server-rust`), a different lineage (Sipeed SG2002/CVITEK) with no KVMD; its
video/WebRTC and board code is not portable to the H616. The approved parity
plan is `docs/design/gui-parity-plan.md`.
