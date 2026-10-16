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

## Slot 88: phases 1 to 5 (2026-10-15)

Phases 2 to 5 were built before the operator's checks of gates 1 and 2,
which they are checked against together: the reads and the page, the
actions, the agent's drive, "Start the agent", the sign-in start, and the
agent's icon and menu (`docs/design/target-agent-v1.md`).
`scripts/check.sh` passed on `878428d`
(`evidence/private/agent-20261014/check-878428d.log`; the run on
`9996a33` stopped at a test that still named the control socket's former
limit, and the one on `e38981d` at the source boundary, which now allows
the agent's icon). Slot 88 (`0.1.0-beta.7`, `878428d`, bundle
`dd6bba36d935695c537fa6c73438650d7db82929ac3722b054654977238dad01`,
carrying `share/web/agent/` with the agent
`007fd9ef9d08494aa2327978b98af2e7604ab264fda42c8c5b3086a5a9c0d792`, its
sums and the drive's image) was installed in place of slot 87 at the
operator's word ("Ставь") and confirmed
(`evidence/private/update-to-seq88-agent.json`). On the board after it:
the services running, the agent's files in the protected copy, the gadget
as before, the switch off.

## Gate 1: the channel (2026-10-15, slot 88)

The operator turned the switch on from the page; the owner logged it, and
the board then had `hid.usb1` with the agent's collection (report length
64) and the agent's drive alone on LUN 0. On the target (Windows 11 Pro
for Workstations 26H2, build 26340):

- The collection is listed without a driver asked for:
  `HID\VID_1D6B&PID_0104&MI_01&COL02`, "HID-compliant vendor-defined
  device", status OK (the operator's PowerShell).
- Keyboard and mouse work as before, by the operator's word.
- The test tool: hello, then 1000 pings answered of 1000 with none lost,
  round trip median 2.000 ms, 95th percentile 2.017 ms, maximum
  7.004 ms; a second run 2.000, 2.016 and 3.011 ms.
- With `watch` for 120 s while the operator detached and attached an
  image: the channel went four times and came back by itself, at most
  0.85 s later. Twice the device left with the gadget's rebinding (back
  in 0.85 s); twice the board answered "hello first", as the owner closes
  the channel and forgets the agent around every media operation even
  when the device stays (back in 0.01 s).
- With an image attached, the image was on LUN 0 and the agent's drive
  on LUN 1.
- With the switch turned off again: `hid.usb1` with its pinned
  descriptor and report length 7, the operator's image alone on LUN 0,
  the agent's drive gone, keyboard and mouse working, by the operator's
  word.

Gate 1 passed. Evidence: `evidence/private/agent-20261014/gate1-*.txt`.

## Slot 89: typing, the quality monitor's row (2026-10-15)

`scripts/check.sh` passed on `85f0242`
(`evidence/private/agent-20261014/check-85f0242.log`). Slot 89
(`0.1.0-beta.7`, `85f0242`, bundle
`c65960b2f7a01e98e4babb242e75bc57029b31c936d5cf86084c87aa7b29aa2c`, the
agent `cb3b1d89e3762db0a57502b471145133d02a6616cc8397329730f15dc91f81f2`)
adds the quality monitor's Agent row, the agent's session kept across the
channel's pauses, the on-screen keyboard readable on the dark page (the
operator's phone showed its keys white on white) and the target's layout
following the letters typed on a phone (the operator: the phone's
keyboard did not switch the target's languages). It was installed in
place of slot 88 at the operator's word ("ставить") and confirmed
(`evidence/private/update-to-seq89-agent.json`): the services running,
none failed, the agent's files in both copies of the slot; the switch on,
as the operator had left it, and the agent connected again by itself
after the owner's restart
(`evidence/private/agent-20261014/state-{before,after}-89.json`).

Finding: the owner, started again with the operator's image attached,
logged "the agent's drive could not be changed: ImageMissing". It read the
image back from the kernel, which names a LUN's file from the image
store's own root to a process outside the former owner's namespace
(`/<image>.iso`); the drive stayed away beside
the image, and reconnecting the image would have failed the same way,
until the image was chosen again. Fixed in `e02e459` (the selection read
back is the image of that name in the store).

## Gate 2: the agent's reads (2026-10-15)

The operator's screenshot of "Show what the agent reads" (the agent
`878428d`, on slot 88):

- Windows 11 Pro for Workstations 26H2 (build 26340.9616), x86_64;
- two monitors, both "BenQ LCD" with the EDID's maker and product
  `BNQ 8024`, 2560x1440 at 59.951 Hz: at 0,0, 100 %, primary; at 2560,0,
  125 %;
- the layout in front English (United States) - US (en-US, `04090409`);
  three installed: en-US `04090409`, ru-RU `04190419`, et-EE `04250425`.

Finding: the board's capture gives the target a copy of the operator's
own monitor's EDID, so no EDID tells the board's monitor from the other,
and `names::BOARD_CAPTURE_EDIDS` stays empty. The agent now gives each
monitor an id from Windows' name for its device and keeps the operator's
word on which one is the board's, which Settings > Device asks for
(`7db8038`); `scripts/check.sh` passed on it
(`evidence/private/agent-20261014/check-7db8038.log`). Slot 90
(`7db8038`, bundle
`96f021a46df9081d7701ad8fd3d957558f6d7f47975e236c2a3768f28d3f662f`, the
agent `0610b2c0694586026aa0eeb269805d58bea110c34627686ae1a04befa3b73bfe`)
carries it and the drive's fix.

By the operator's word ("да и да"): the reads are what Windows' settings
show, and the board's monitor is the right one (at 2560,0, 125 %). Gate 2
passed, with the finding that the board's monitor is the operator's
choice.

## Slot 90: the board's monitor chosen, the drive after a restart (2026-10-15)

Slot 90 was installed in place of slot 89 at the operator's word ("да")
and confirmed (`evidence/private/update-to-seq90-agent.json`): no unit
failed. The owner, started again with the operator's image attached, now
presented the image on LUN 0 (removable, read-only) and the agent's drive
on LUN 1 (not removable, read-only), the absolute mouse with the agent's
collection (report length 64); the switch on, and the agent connected
again by itself
(`evidence/private/agent-20261014/state-{before,after}-90.json`).

## Slots 91 to 94: the drive mounted, the Monitors menu (2026-10-15/16)

- Gate 5's first try on slot 90: "Start the agent" typed its line and no
  agent came. Read on the target at the operator's word ("А ты сам типа
  проверить это всё не можешь? это всё на хосте же"), read only: Windows
  listed the agent's drive as a 2 MiB "external hard disk" with no
  partition, no volume and no drive letter. The drive is now a disk with
  a DOS partition table and one FAT12 partition (`0908887`); slot 91 was
  built with it and not installed, as slot 92 came before the operator's
  next check.
- The operator, at gate 3: the KVM's mouse landed on the left monitor;
  the mouse menu's layout was not wanted; the monitors should change as
  in Windows' own choice, from a toolbar menu. The toolbar's Monitors menu
  (`6e50a26`) says the mode and changes it, and offers the board's
  monitor's choice; the mouse follows the agent's monitors in every mode.
- Slot 92 (`6e50a26`, bundle
  `a4fdeb1c9ad4f142ea93135970db6a3c6d76bfd82df1da424d809b520ae45cb7`, the
  agent `465934e296fb6208bd6f67250f96e2698a40efd3496c15f21f5f237d785b07aa`)
  was installed at the operator's word ("ставь") and confirmed
  (`evidence/private/update-to-seq92-agent.json`); on the target the
  drive was then `H:\`, labelled `BLIKVMAGENT`, with the agent of the
  slot's digest.
- The operator: switching the monitors' mode sometimes said
  "backend_rejected"; Estonian typed on the phone was skipped. The
  monitors kept by "only" now begin at the desktop's origin (`32d4845`),
  the agent's refusals reach the page with their reason (`1b83bc2`), and
  Estonian is typed in the target's Estonian layout, its keys read from
  Windows' own layout on the target (`802ccd6`). Slot 93 (`802ccd6`,
  bundle `2c56a6688527a3a8df8b1268b1976825cd938b349225dc154d302b4401ccfe58`)
  was installed at the operator's word ("ставь слот 93") and confirmed.
- The operator: the Monitors menu stayed empty. Read on the target: the
  monitors were duplicated, and the agent said both were the primary
  monitor, which the board refuses (`fb1d847`); the menu now reads the
  monitors as it opens and says why a read fails (`ae03f6e`). The full
  check on `ae03f6e` failed a measurement of the reads contract (a mouse
  report arriving late, fixed in `f3b1c78`), and the first run on
  `f3b1c78` failed the QUIC video contract's frame rate (24.3 of 30.3 a
  second; it passed alone, and the next full run passed). Slot 94
  (`f3b1c78`, bundle
  `5b5822566e6a50e4379e24d8451e9299d8a5ce4441e5c9fed887a8e36290b950`, the
  agent `e2822af5a310daffc1b95ab2db7f32f02d27243c61b9c3c94810aa6bdf39c6a1`)
  was installed at the operator's word ("ставь") and confirmed
  (`evidence/private/update-to-seq94-agent.json`): no unit failed, the
  image on LUN 0 and the agent's drive on LUN 1, the switch on, the agent
  connected again by itself.

## Gates 3 and 4 (2026-10-16, slot 94, the agent `f3b1c78` from its drive)

By the operator's word: "всё из 3 и 4 работает" — with two monitors and
no manual layout, the absolute mouse lands on the board's monitor at its
four corners and its middle, on the desktop and on the phone, and the
Keyboard menu shows the target's layout (gate 3); the layout switched
("раскладка перключается"), the phone's typing in its layouts ("андроид
работает"), the monitors' modes with a change that leaves the KVM without
a picture undone by itself, the clipboard both ways and locking the
session (gate 4). Gates 3 and 4 passed.

Found at the same check and fixed after it: the page's on-screen keyboard
sent a tapped modifier as a key with no code, which the board refused
(Ctrl+V did not reach the target and remote control turned off), showed a
pressed modifier white on white, and did not send the Russian layout's
letters (`911001d`); the Monitors menu's choice of the board's monitor
wrapped under its line on a phone (`d9cd653`).

## Gate 5 so far (2026-10-16)

- The read-only agent's drive appears with the switch on: `H:\`,
  `BLIKVMAGENT`, FAT, with the agent, its README and its sums, the agent
  of the slot's digest (read on the target).
- "Start the agent" connects within 10 s: by the operator's word ("да"),
  the agent started from the drive (`H:\hardened-blikvm-agent.exe`) and
  connected (`evidence/private/agent-20261014/gate5-watch-93.txt`).
- The sign-in start surviving a restart of the target: to be checked
  last, as the restart also ends this session on the target.

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
