# Work plan: the target agent, Windows first (2026-10-14)

The board sees its target as a USB host and as one picture over HDMI. It
cannot tell the target's operating system, its monitors beyond the captured
one, or its keyboard layout, and it cannot read or set anything there but
by typing. The operator asked for an agent: a program that runs on the
target, comes from a drive the board presents itself, and tells the board
what it cannot see and does what typing does badly: the monitors and their
layout (so that the absolute mouse lands on the captured monitor by
itself, `docs/design/screen-area-v1.md`), switching the monitors' mode, the
keyboard layout read and kept in step with the page, the clipboard, and
more of what does not work today. It is an option, off by default ("Run
the agent on supported systems"). Windows comes first; Linux is a plan of
its own, after this one (its desktops differ, X11 against Wayland, and it
needs downloads to build and test).

Rules as in the earlier plans: English repository text, secrets only
through `project-memory-exec`, one commit per item with its own tests,
`scripts/check.sh` green before each slot build, slots and system parts
installed on the test board at the operator's word and verified there, the
operator's settings ending as found. The agent runs on the operator's own
computer (the target): the operator starts it and does every check there;
the agent's builder never sends input to the target unasked.

## What is there today

- The board owner (`bli-kvm-control-broker`, the application slot) builds
  the USB gadget in configfs (`crates/bli-kvm-usb-gadget`): a keyboard, an
  absolute and a relative mouse (`hid.usb0` to `hid.usb2`, descriptors
  pinned in tests) and one mass-storage LUN for virtual media; VID 0x1d6b,
  PID 0x0104. It only writes HID reports; nothing reads a `hidg` device.
- Every change of virtual media unbinds and binds the whole gadget: the
  target sees every function leave and come back.
- The owner's unit allows `/dev/hidg0` to `/dev/hidg2` only, and the unit
  is a system part (the update runtime's package), not the slot's.
- Default-off features follow `docs/design/admin-features-v1.md`: a switch
  kept by the owner, enforced by the server and again below it.
- The host builds Windows programs: Rust's `x86_64-pc-windows-msvc` target
  and `cargo-xwin` with the Windows SDK already in its cache.

## Decisions taken

- The channel is a vendor-defined HID interface of the board's own (raw
  HID, usage page 0xFF00, 64-byte reports each way): Windows, Linux and
  macOS read it without a driver and without administrator rights, it
  needs no network, and nothing else on the target listens to it. A serial
  port (CDC-ACM) was the other candidate: drivers everywhere too, but other
  programs open COM ports, and it reads as a modem to a security product.
- The agent is one Rust program for Windows (`x86_64`), with the user's
  rights: no service, no administrator, no network, no listening socket.
  It shows itself in the notification area while it runs.
- The protocol is a crate of its own, built for the board and for the
  agent, so that both ends are the same code and are tested on the host.

## Decisions for the operator

- Signing: Windows warns about an unsigned program (SmartScreen) and an
  administrator may block one. An Authenticode certificate costs money and
  an identity; without one the release gives the program's SHA-256 and the
  operator allows it once. To decide before phase 5.
- New crates for the agent (`windows` and `windows-sys` from crates.io,
  MIT or Apache-2.0) need a download, once.
- Which actions the agent offers (phase 4), and whether it may start
  itself at sign-in.

## Phase 0: the Windows build (gate 0)

1. A skeleton `bli-kvm-target-agent` crate (`crates/`), a Windows
   executable whose code outside the Windows calls builds and is tested on
   Linux; `scripts/build-target-agent.sh` builds it with `cargo-xwin`,
   statically linked, reproducibly (the same source gives the same bytes),
   with its version and source commit in its resources.
2. The operator runs it on the target: it shows its icon and its version,
   and says what SmartScreen and Defender said.

Gate 0: the program starts on the operator's Windows; two builds give the
same SHA-256. Time box: 4 hours.

## Phase 1: the channel (board, application slot and system part)

1. `crates/bli-kvm-agent-protocol`: messages as JSON in frames of 64-byte
   reports (a version, a message number, the fragment's place, the length
   and a CRC-32), messages up to 16 KiB, requests with numbers and answers
   to them, a hello with both sides' versions, ping. Tests: round trips,
   fragments out of order or lost, a frame that is not one, a message too
   long, two messages interleaved.
2. The gadget: a fourth function `hid.usb3` with the vendor descriptor,
   pinned in tests, present only while the agent switch is on (the device
   the target sees changes only when the operator turns it on).
3. The owner: a task that reads and writes `/dev/hidg3`, keeps the agent's
   session (connected, its hello, the last messages), finds it again after
   a re-enumeration (virtual media swaps), and gives the server two
   requests on its control socket: the agent's state, and a call (a request
   and its answer, with a deadline). Calls that act on the target or read
   from it need the operator's HID lease, as typing does.
4. The switch "Target agent" in the administration features, off by
   default, kept and enforced as the others (server, owner).
5. The system part: the owner's unit allows `/dev/hidg3` (update runtime
   `0.1.0~dev39`); a slot on an older system says the agent is unavailable
   and why.
6. A small Windows test tool from phase 0's crate: finds the interface,
   sends hello and ping, prints the answers and their round trips.

Gate 1, on the board with the switch on: the target lists a "HID-compliant
vendor-defined device" without asking for a driver; keyboard and mouse
work as before; the test tool exchanges hello and 1000 pings (round trip
median and 95th percentile recorded); after a virtual media attach the
channel is back by itself within 5 s; with the switch off the gadget is as
before. Time box: 2 days.

## Phase 2: the agent reads (Windows)

1. Finding the board: the HID interface by VID, PID and usage page,
   opened with overlapped reads and writes; it reconnects when the device
   goes and comes back.
2. Hello: Windows' product name, version and build, architecture, the
   agent's version, whether the session is locked.
3. Monitors: the active paths (`QueryDisplayConfig`), each monitor's name,
   its EDID maker and product, its place and resolution in the desktop
   (negative places included), the primary one, the scale; which one is
   the board's (the capture bridge's EDID as the target reads it, found at
   this gate).
4. The keyboard: the layout of the window in front, and the layouts
   installed, with their names.
5. The icon's menu: connected or not, the board's name, Quit.

Gate 2: on the operator's target the agent's monitors, places and
keyboard layouts are what Windows' settings show; the board's monitor is
the one the KVM shows; the agent idles at no measurable processor use.
Time box: 2 days.

## Phase 3: the page

1. An Agent section (Settings, and a short status in the menu bar): the
   agent connected or not, Windows and its version, locked or not, the
   monitors with the captured one marked.
2. The absolute mouse's layout from the agent: while it is connected, the
   desktop and the captured monitor's place come from it
   (`hid_screen_area`), and the manual layout stays for a target without
   an agent.
3. The keyboard menu shows the target's layout.

Gate 3: on the operator's target with two monitors and no manual layout,
the mouse lands on the captured monitor at its four corners and middle, on
the desktop and on the phone; the page shows the target's layout. Time
box: 1 day.

## Phase 4: the agent acts (at the operator's choice)

1. The keyboard layout: switch it from the page (the window in front is
   asked to change), and, as an option, keep it in step with the page's
   keyboard language.
2. The monitors' mode: extend, duplicate, only the board's monitor, only
   the others; the board's monitor made the primary one. A change that
   leaves the KVM without a picture is undone by itself after 15 s unless
   the page confirms it, as Windows' own "Keep changes" does.
3. The clipboard, text, both ways, each time at the operator's action: the
   target's to the page, and the page's to the target (in place of typing
   it, which a keyboard layout spoils).
4. Lock the session.

Gate 4: each action on the operator's target, and the mode change that
hides the picture undone by itself. Time box: 2 days.

## Phase 5: the agent's drive, and starting it

1. A second LUN of the mass-storage function, read-only and not
   removable, a small FAT image labelled `BLIKVM-AGENT` with the agent,
   a short README and SHA256SUMS, made from the slot's files; present
   while the switch is on, and not in HID-only mode. Virtual media keep
   LUN 0.
2. "Start the agent" in the page, after a confirmation and with the HID
   lease: Win+R and a PowerShell line that finds the drive by its label
   and starts the program; offered when the target said it is Windows, or
   when the operator says so.
3. "Start at sign-in" in the agent's menu, at the operator's choice (the
   user's Run key); and its removal.

Gate 5: on a target that never ran it, the switch on, the drive appears,
"Start the agent" connects it within 10 s; with "Start at sign-in" it is
back after the target restarts. Time box: 2 days.

## Phase 6: release

`scripts/check.sh`, a signed slot, update runtime `0.1.0~dev39`, the card
image and the RAW bundle, installed on the test board and verified there;
the agent's program with its SHA-256 (signed if so decided);
`docs/design/target-agent-v1.md`, the audit note, the README; a release
when the operator says to publish. Time box: 1 day plus the operator's
checks.

## Security

- Off by default. Turning it on changes the USB device the target sees and
  is recorded on the board.
- The agent does only what the protocol names: no command runs, no file
  outside its own is read, no network. What it reads from the target
  leaves it only when the page asks, and the actions and the clipboard
  need the operator's HID lease.
- The board keeps nothing it reads from the target beyond answering the
  page (the clipboard is not stored, logged or put in receipts).
- The agent trusts the USB device as the target trusts a keyboard: a
  device that could send it messages could type anything already. It
  answers the board's hello only with the version and the system, and a
  newer or older protocol is refused with a reason.
- The program is built from this repository, reproducibly, and its
  SHA-256 is in the release.

## Operator steps

- Approve the agent's crates' download, once (phase 0).
- Run the agent's builds on the target at gates 0, 2, 4 and 5, and the
  test tool at gate 1.
- Agree to the slot and system installs (the update runtime with
  `/dev/hidg3`, phase 1; the release, phase 6).
- Decide signing before phase 5, the actions in phase 4 and sign-in start.
- Say when to publish.
