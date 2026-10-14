# The target agent, Windows: gates and slots (2026-10-14)

Plan: `docs/baselines/work-plan-20261014-target-agent.md`.

## Gate 0: the Windows build

`3c940a7` adds `crates/bli-kvm-target-agent`, one program for the target's
Windows with the user's rights: an icon in the notification area with its
name, version and commit, a menu with Quit; it comes back when the taskbar
is started again and runs once per session. `scripts/build-target-agent.sh`
builds it on the development host with `cargo-xwin` and the Windows SDK
already in its cache (no download): the `x86_64-pc-windows-msvc` target,
the C runtime linked in, no debug information, the build's paths mapped
away and the linker's timestamp made from the content (`/Brepro`). The
first try left two builds apart in 20 bytes: the PE timestamp and the GUID
of the debug file, which followed the build directory's path; without the
debug file the bytes are the same. `scripts/check.sh` lints the Windows
code and builds the program twice in different directories, comparing the
bytes.

Two builds of `3c940a7` in different directories:
`c0ada86edab2fa558cafbcffd889fa48f2a68506217ba6f33bad6e4b4309be46`
(213,504 bytes, PE32+ for Windows, a GUI program; no path of the host in
it). The operator started it on the target from the host's disk: it
started without a warning from SmartScreen or Defender, and showed its
icon as "Hardened BliKVM agent 0.1.0 (3c940a7e681c)" with Quit (2026-10-14,
"Запустился без предупреждений"). A program copied from a local or USB
drive carries no mark of the web, which is what SmartScreen judges; one
downloaded from a release page would. Gate 0 passed.

## Phase 1: the channel (built 2026-10-14)

The plan's fourth HID function does not fit the board: its USB controller,
the H616's MUSB in its H3 configuration (Linux 5.19,
`drivers/usb/musb/sunxi.c`), has four IN and four OUT endpoints, and the
keyboard, both mice and an attached image take them all (read on the
board: `sun50i-h616-musb`, `sun8i-h3-musb`; the gadget's three HID
functions each with an OUT endpoint). The channel is report 2 of the
absolute mouse's interface while the switch is on, `/dev/hidg1` already
allowed by the owner's unit: no system part. Linux 5.19's HID function
makes its device again at every binding, so the owner closes the channel
around every change of the gadget. Evidence:
`evidence/private/agent-20261014/phase1-notes.md`.

`scripts/check.sh` passed on `c220c7c`
(`evidence/private/agent-20261014/check-c220c7c.log`). Slot 87
(`0.1.0-beta.7`, `c220c7c`, bundle
`13acfc45887dd9861439fdcab7be565b5f829353de369cb0e3d0c05b2bc388d0`) was
installed through the update API at the operator's word ("Ставь") and
confirmed (`evidence/private/update-to-seq87-agent.json`). On the board
after it: the owner, the server, the board daemon and the QUIC gateway
running; the gadget as before (`hid.usb1`'s pinned descriptor, report
length 7, no image attached); the platform offering `target_agent`; the
switch off, its file absent. Gate 1 waits for the operator's checks.

## The operator's decisions (2026-10-14)

- The actions of phase 4: all four the plan offers (the keyboard layout,
  switched from the page and optionally kept in step with its keyboard
  language; the monitors' mode, undone by itself after 15 s unless
  confirmed; the clipboard's text both ways, each time at the operator's
  action; locking the session).
- "Start at sign-in": yes, as an item of the agent's menu (the user's Run
  key), with its removal.
- Signing: none; a release gives the program's SHA-256, and the program
  is built reproducibly.
