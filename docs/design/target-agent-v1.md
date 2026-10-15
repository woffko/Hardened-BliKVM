# The target agent, Windows (2026-10-14)

Plan: `docs/baselines/work-plan-20261014-target-agent.md`. Gates and
slots: `docs/baselines/audit-20261014-target-agent.md`.

The board sees its target as a USB host and one picture. The agent is a
program on the target, with the user's rights, that tells the board what it
cannot see and does what typing does badly. It is off by default: the
operator's "Target agent" switch (Settings > Device) turns the channel on.

## The channel

A vendor-defined HID collection (usage page 0xFF00, usage 1): Windows,
Linux and macOS open one without a driver and without administrator rights,
it needs no network, and nothing else on the target reads it.

The collection is not a USB function of its own. The board's controller,
the H616's MUSB in its H3 configuration (`drivers/usb/musb/sunxi.c`), has
four IN and four OUT endpoints, and the keyboard, both mice and the virtual
media take all of them. While the switch is on, the absolute mouse's
interface (`hid.usb1`) carries two collections: the mouse as report 1, its
items as pinned but for the ID, and the agent as report 2, 63 bytes each
way (`crates/bli-kvm-usb-gadget/src/hid_descriptors.rs`, pinned by its hash
and checked item by item). The absolute mouse is the one interface no
firmware drives (it is not a boot device); its OUT endpoint, which the mouse
never used, carries the agent's reports to the board. Its device,
`/dev/hidg1`, is one the owner's unit already allows: the channel needs no
system part.

Every absolute mouse report follows the descriptor `hid.usb1` has at that
moment, read from configfs: report ID 1 with the agent's collection, none
without, and nothing is written for a descriptor that is neither. A report
of the wrong form would be read by the target as another one, a button
among them.

## The protocol

`crates/bli-kvm-agent-protocol`, built for the board and for the agent.
A message is JSON, at most 16 KiB, cut into frames of the 63 bytes after
the report ID: the frames' version, first and last flags, the message's
number, the frame's place, the payload's length (at most 51 bytes) and a
CRC-32 of the rest. A report of another ID or a frame that is none is
refused; a lost, reordered or interleaved frame drops its message, and the
next first frame begins a new one.

The agent begins with a hello: the protocol's version, its program, version
and commit, and its system (the product as Windows' settings name it, the
release, build and revision, the architecture, whether the session is
locked). The board answers with its own, or refuses one of another
protocol, with odd text or a system out of its bounds, with the reason;
unknown fields are ignored so that a side of another version is read and
refused, not misread. Then either side calls the other: a numbered
request, answered or failed. Either side answers a ping (a ping and its
pong fit in one report each); the board answers nothing else. The board
calls the agent for:

- reads: the system; the active monitors (an id, name, EDID maker and
  product, place and size in the desktop's pixels, refresh, primary,
  scale, whether it is the board's capture); the keyboard layouts (the
  window in front's and those installed, with their language and name);
- the operator's actions (chosen 2026-10-14): switch the window in front
  to an installed layout; set the monitors' mode (extend, duplicate, only
  the board's, only the others, the board's the primary), undone after
  15 s unless kept, and keep it; read and write the clipboard's text (at
  most 6000 UTF-8 bytes, tabs and line ends its only control characters);
  lock the session; and keep the operator's word on which monitor is the
  board's (`set_board_monitor`, by the monitor's id, or none).

Every value is bounded in the crate and checked on both ends (`reads.rs`);
an answer must fit its call.

## The board

The owner (`crates/bli-kvm-control-broker/src/target_agent.rs`) keeps:

- The switch, in `/var/lib/hardened-blikvm-gadget/target-agent.env`, off
  unless it says `enabled=yes`. Switching writes the file, closes the
  channel's device, gives `hid.usb1` the agent's collection or takes it
  away (`bli_kvm_usb_gadget::replace_hid_function`: the gadget is unbound,
  every link taken out, the function's attributes written, the links put
  back in the order a boot and an attach give them, and the gadget bound;
  the target sees the whole device leave and come back, an attached image
  stays attached) and opens the channel again. A gadget that cannot be
  changed puts the switch back. The HID bind before the owner starts reads
  the switch, so a boot builds the gadget as it says; the owner makes the
  present gadget follow it when it starts. A stopping owner takes the
  collection away and leaves the switch as it is: an update may roll back
  to a slot from before the agent, which writes the mouse's reports without
  an ID (with the switch on, an owner's restart is two re-enumerations of
  the target's USB device). An owner that crashes cannot do so; a rollback
  after such a crash leaves the mouse misread until the switch is turned
  off or the board restarts. A card image leaves the source board's switch
  out.
- The channel: one task owns `/dev/hidg1` while the switch is on, opened
  without blocking. It reads every report (an OUT endpoint nobody reads
  stops taking them, and the agent's writes would hang), answers the
  agent's hello and pings, and carries the owner's calls with a deadline
  (1 ms to 10 s). An agent heard within 10 s is connected; reports the
  target does not take within a second are dropped with what waits.
- Linux 5.19's HID function removes its device when the gadget is unbound,
  makes it again when it is bound, and resets the device's wait queues as
  it does. Every media operation and the switch therefore pause the channel
  first, which closes the device, and resume it after; the agent sees the
  device go and come back and says hello again.

- The agent's drive: a second LUN of the mass-storage function, read-only
  and not removable, a 2 MiB FAT12 image labelled `BLIKVMAGENT` (a FAT
  label has at most 11 characters, so not `BLIKVM-AGENT` as the plan said)
  with the agent, a README and its SHA-256 (`scripts/build-agent-drive.sh`,
  reproducible with mkfs.fat's invariant values and fixed times). A signed
  slot carries only its programs and its web tree, so the slot carries the
  agent and the image under `share/web/agent/`, and the owner takes the
  image from the slot it runs from. The drive is presented while the switch
  is on and the gadget is not in HID-only mode: beside an image (LUN 1; the
  virtual media keep LUN 0) or alone (LUN 0, not removable). Only a
  removable LUN 0 is an image of the operator's, so the drive alone is no
  attached media; a swap or an eject keeps the drive. The kernel makes a
  LUN only while the function is not linked: the LUN is made with the
  function, and removed before it. A stopping owner takes the drive away
  too, as a slot from before the agent would take it for an image.

The control socket has `target_agent` (the switch), `target_agent_state`
(the switch, the channel, why its device is not open, the agent's hello,
the last refusal, the messages each way) and `target_agent_call` (waited
for on the channel's handle, not the shared state's lock, so HID and media
never wait for it; a call other than a ping carries the HID lease's
identity, and the owner makes it only while that lease is the live one).
The control socket's messages may be 24 KiB, room for the agent's answers.
The server serves `GET` and `POST /api/target-agent` and
`POST /api/target-agent/call` (a read or an action only under the HID
lease the session holds now) through the owner; the platform names the
feature `target_agent`.

## The page

- Settings > Device: the switch, with a confirmation that says the USB
  device leaves the target and comes back; the connected agent, the
  target's Windows and its monitors with the board's marked, and beside
  each other monitor "This is the board's", which tells the agent (with a
  line asking for it while none is); the
  monitors' modes, with a dialog that counts down to the change's undoing
  and keeps it; "Start the agent on the target" while no agent is
  connected and remote control is on: after a confirmation, Win+R, a pause
  for the Run box, and a PowerShell line typed at the US layout's
  positions (the target's layout in front must be English) that finds the
  drive by its label and starts the agent from it, its window hidden.
- The quality monitor's last row, Agent, says the state at a glance where
  the board offers the agent: off, the channel closed, waiting for an
  agent, or connected with its version (and locked while the target's
  session is); it is the plan's short status, opened from the menu bar's
  QM, and fits without scrolling in Basic.
- The page keeps the agent's state, and while an agent is connected and
  remote control holds the lease, reads its monitors and layouts (at once
  and every 15 s); nothing is asked of the agent without the lease.
- The absolute mouse uses the agent's layout while it knows the board's
  monitor: the desktop is the box around the active monitors, the screen
  the board's monitor in it (`web/src/lib/agent-layout.ts`); the layout
  kept on the board otherwise (`docs/design/screen-area-v1.md`).
- The Keyboard menu shows the target's layout, switches it among those
  installed, switches it to what the operator types when asked (a choice
  of this browser), carries the clipboard's text both ways and locks the
  session after a confirmation. What is typed: the on-screen keyboard's
  language, and the letters of the phone's own keyboard, which hands the
  page text, not keys, and so cannot say its language switch. The page
  types Latin letters at the US layout's keys and Cyrillic ones at the
  Russian layout's, so it cuts the text into runs of one script and,
  before a run whose layout is not the target's, waits until the keys
  before it are typed, asks the agent for an installed layout of that
  script (English for Latin, Russian for Cyrillic) and gives the window
  100 ms to take it (`web/src/lib/agent-typing.ts`). The layouts are read
  again as the phone's keyboard opens; a shortcut (a modifier held)
  switches nothing.

## The target

`crates/bli-kvm-target-agent`, built by `scripts/build-target-agent.sh`
with `cargo-xwin` for `x86_64-pc-windows-msvc`, the C runtime linked in,
reproducibly (two builds in different directories give the same bytes,
which `scripts/check.sh` checks).

- `hardened-blikvm-agent.exe`: the agent. Its icon is the project's mark
  (the bracket with its copper edge and the "b" from the logo, on a white
  rounded tile, `assets/agent.ico` made by `tools/build_agent_icon.mjs`),
  compiled into both programs' resources with their version and commit by
  `build.rs`. The icon's menu says its name, version, commit and whether
  it is connected, and offers "Show what the agent reads", Stop (the
  channel closes the collection) or Start, "Start at sign-in" (the program
  copied to the user's local application folder and named in the user's
  Run key; again to take it away) and Exit; in Russian on a Russian
  Windows. A thread keeps the channel: it finds the collection again
  whenever it goes, says hello with the system, answers the board's calls
  with its reads and actions (`reads.rs`, `actions.rs`), and pings every
  3 s. A monitor's id is the 64-bit FNV-1a hash of Windows' name for its
  device (`\\?\DISPLAY#...`, which names its connector too), its letters
  in one case, as 16 hex digits: the same while the monitor stays on its
  connector. The board's monitor is the one the operator chose, kept for
  the user in `HKCU\Software\HardenedBliKVM`, value `BoardMonitor`; or,
  without a choice among the active monitors, one with a known capture's
  EDID (`names::BOARD_CAPTURE_EDIDS`). That list is empty: gate 2 found
  that the operator's capture gives the target a copy of the operator's
  own monitor's EDID (both read `BNQ 8024`), as KVM captures often do, so
  an EDID cannot tell them apart. Until a monitor is the board's, the
  modes that need it say so, and the page asks which one it is.
- `hardened-blikvm-agent-test.exe`: the channel's test tool. It finds the
  board's collection (the board's vendor 0x1d6b and product 0x0104, usage
  page 0xff00, usage 1, 64-byte reports each way), opens it for overlapped
  reads and writes, says hello and sends pings (1000 by default) and says
  their round trips, or with `watch` pings every 200 ms and says when the
  channel went and how soon it came back. It appends what it says to
  `hardened-blikvm-agent-test.txt` beside itself.

The agent's side of the protocol (`client.rs`) and the names it gives
(`names.rs`) are portable and tested on Linux; Windows' calls are in
`hid.rs`, `reads.rs`, `actions.rs`, `autostart.rs`, `channel.rs` and
`tray.rs`.

## Security

- Off by default; switching changes the USB device the target sees and is
  logged by the owner.
- The agent does only what the protocol names: no command runs, no file
  outside its own is read, no network; it runs with the user's rights.
- The board answers only the agent's hello and pings, and refuses a
  request before a hello. Every read and action is the operator's, from
  the page, under the HID lease the operator holds, checked by the server
  and again by the owner.
- The board keeps nothing it reads from the agent beyond its hello and the
  counts of messages; the clipboard's text passes through and is not
  stored or logged.
- "Start the agent" types into the target, as the operator's keyboard
  would, only after a confirmation and under the lease.
