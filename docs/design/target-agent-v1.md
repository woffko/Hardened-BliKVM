# The target agent, Windows (2026-10-14)

Plan: `docs/baselines/work-plan-20261014-target-agent.md`. Gates and
slots: `docs/baselines/audit-20261014-target-agent.md`. The review of
0.1.0-beta.7 and its fixes: `docs/baselines/audit-20261017-review.md`
(their plan, `docs/baselines/work-plan-20261017-review-fixes.md`, gives the
decisions this note follows since).

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

Every absolute mouse report follows the descriptor `hid.usb1` has, read
from configfs once after each change the owner makes to the gadget (the
gadget crate counts them, `bli_kvm_usb_gadget::changes`): report ID 1 with
the agent's collection, none without, and nothing is written for a
descriptor that is neither. A report of the wrong form would be read by the
target as another one, a button among them.

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
  15 s unless kept, and keep it (only the board's or only the others from
  a mode that shows none of them first extends over every monitor, the
  monitors left are moved so that the top left one is at the desktop's
  origin, as Windows' own "show only on" does, and a change that fails
  half way is put back at once); read and write the clipboard's text (at
  most 6000 UTF-8 bytes, tabs and line ends its only control characters);
  lock the session; and keep the operator's word on which monitor is the
  board's (`set_board_monitor`, by the monitor's id, or none).

Every value is bounded in the crate and checked on both ends (`reads.rs`);
an answer must fit its call.

Since the security review of 2026-10-18:

- While the target's session is locked, the agent answers a clipboard read
  or write, a layout switch, a monitors' change and its keep with the
  refusal "the target's session is locked" (D1): the locked screen is not
  the operator's desktop. The other calls, and those the board makes to
  learn the agent's state, are answered as before.
- The agent takes `hid.dll` (and any DLL that is not one of Windows' known
  ones) from the system folder only: it loads it at run time with
  `LOAD_LIBRARY_SEARCH_SYSTEM32`, its load configuration asks the same for
  every DLL (`DependentLoadFlags`), and it sets the process's default DLL
  directories at start, so a DLL beside it, in a downloads folder for
  example, is never loaded (D2). `tools/check_agent_imports.py` checks the
  built program's import table and load configuration.
- The agent opens its HID channel unshared, so another program cannot open
  the device beside it while it runs (D3); the agent's test tool is
  refused meanwhile.

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
  changed puts the switch back, and the channel's wish with it; the flag
  that gadgets built from scratch follow is put back too. The HID bind before the owner starts reads
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
  target does not take within a second are dropped with what waits. A
  request that cannot reach the target ends its call at once: not
  connected while the device is closed, timed out when the target takes
  nothing in time or the queue is full; one already with the target waits
  for its answer. A refusal from the target is kept within the state's
  bounds (control characters out, at most 160 characters).
- Linux 5.19's HID function removes its device when the gadget is unbound,
  makes it again when it is bound, and resets the device's wait queues as
  it does. Every media operation, the switch, and a HID request that finds
  the gadget gone and builds it therefore pause the channel first, which
  closes the device, and resume it after; the agent sees the device go and
  come back and says hello again. A storage change that fails part way
  binds the gadget again before it answers.

- The agent's drive: a second LUN of the mass-storage function, read-only
  and not removable, a 3 MiB disk with a DOS partition table and one 2 MiB
  FAT12 partition from 1 MiB, labelled `BLIKVMAGENT` (a FAT label has at
  most 11 characters, so not `BLIKVM-AGENT` as the plan said), with the
  agent, a README and its SHA-256 (`scripts/build-agent-drive.sh`,
  reproducible with sfdisk's fixed id, mkfs.fat's invariant values and
  fixed times). Windows finds no volume on a fixed disk without a
  partition table: the first image, a file system alone, was listed on
  the operator's target as a 2 MiB "external hard disk" with no partition
  and no drive letter (gate 5). A signed
  slot carries only its programs and its web tree, so the slot carries the
  agent and the image under `share/web/agent/`, and the owner takes the
  image from the slot it runs from. The drive is presented while the switch
  is on and the gadget is not in HID-only mode: beside an image (LUN 1; the
  virtual media keep LUN 0) or alone (LUN 0, not removable). Only a
  removable LUN 0 is an image of the operator's, so the drive alone is no
  attached media; a swap or an eject keeps the drive. The kernel makes a
  LUN only while the function is not linked: the LUN is made with the
  function, and removed before it. LUN 0 is rebuilt as the gadget has it:
  the operator's image, or empty where the target ejected it (a removable
  LUN may be built empty), never the owner's record of a selection. A
  stopping owner takes the drive away too, as a slot from before the agent
  would take it for an image; when the storage cannot be built again, LUN 1
  is taken out alone.

The control socket has `target_agent` (the switch), `target_agent_state`
(the switch, the channel, why its device is not open, the agent's hello,
the last refusal, the messages each way, and the board's commit, which its
hello names: the board's build sets it as the agent's build does) and
`target_agent_call` (waited
for on the channel's handle, not the shared state's lock, so HID and media
never wait for it; a call other than a ping carries the HID lease's
identity, checked as every lease is, and the owner makes it only while
that lease is the live one).
The control socket's messages may be 24 KiB, room for the agent's answers.
The server serves `GET` and `POST /api/target-agent` and
`POST /api/target-agent/call` (a read or an action only under the HID
lease the session holds now; a change of the monitors and its keep wait up
to 10 s, as Windows sets a mode or two before it answers, a read 3 s)
through the owner; the platform names the feature `target_agent`.

## The page

- Settings > Device: the switch, with a confirmation that says the USB
  device leaves the target and comes back; the connected agent, the
  target's Windows and its monitors with the board's marked, and beside
  each other monitor "This is the board's", which tells the agent (with a
  line asking for it while none is); the
  monitors' modes, with a dialog that counts down to the change's undoing
  and keeps it; "Start the agent on the target" while no agent is
  connected and remote control is on: after a confirmation, Win+R, a pause
  of a second for the Run box from when Win+R has been sent, and a
  PowerShell line typed at the US layout's positions (the target's layout
  in front must be English) that finds the drive by its label and starts
  the agent from it, its window hidden. The line runs only a file whose
  SHA-256 is the board's agent's (the agent's state names it, from
  `agent/SHA256SUMS` of the slot's web tree), so another volume labelled
  `BLIKVMAGENT`, an image on LUN 0 for example, starts nothing; without
  that digest the page does not offer the start (security review of
  2026-10-18, N7, D4). An agent whose commit is not the
  board's is said not to be the board's version, and the same start is
  offered as "Update the agent on the target", waited for until the
  board's agent is the connected one (decision 1 of the plan of
  2026-10-17). The section shows the page's one state of the agent.
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
  kept on the board otherwise (`docs/design/screen-area-v1.md`). So with
  the monitors extended the mouse keeps to the board's monitor, and
  duplicated or with the board's monitor alone it spans the picture.
- The toolbar's Monitors menu, while an agent is connected (asked for by
  the operator at gate 3, so as not to open the settings each time): the
  mode the monitors are in as the agent's monitors show it (one desktop
  over them, one picture on all, or a single monitor), Windows' choices
  to change it (duplicate, extend, only the board's monitor, only the
  others; the last two once the board's monitor is known), and the
  monitors with "This is the board's" beside the others. Opened under
  remote control it reads the monitors again, taking the keyboard's and
  mouse's lease without input if no input took it yet, and says why a
  read failed; without remote control it says to enable it. A change from
  the menu or the settings waits in the page's one dialog, which counts
  down to the agent's putting it back and says how long the change took
  (decision 3). Keep takes the lease first, as every call does, and the
  dialog stays until the agent has kept the change: a Keep that failed is
  said, the countdown goes on, and Keep may be pressed again (decision 4).
  The monitors are read again after a change, once it is kept or put back,
  so that the mouse follows the mode.
- The Keyboard menu shows the target's layout, switches it among those
  installed, switches it to what the operator types when asked (a choice
  of this browser), carries the clipboard's text both ways and locks the
  session after a confirmation. What is typed: the on-screen keyboard's
  language, and the letters of the phone's own keyboard, which hands the
  page text, not keys, and so cannot say its language switch. The page
  types Latin letters at the US layout's keys, Cyrillic ones at the
  Russian layout's and õ ä ö ü š ž at the Estonian layout's (its keys read
  from Windows' own Estonian layout on the operator's target, AltGr as the
  right Alt, a dead key followed by a space), so it cuts the text into
  runs of one script and, before a run whose layout is not the target's,
  waits until the keys before it are typed, asks the agent for an
  installed layout of that script (English for Latin, Russian for
  Cyrillic, Estonian for a run with Estonian letters) and gives the window
  100 ms to take it (`web/src/lib/agent-typing.ts`). The Estonian layout
  has the Latin letters where the US layout has them: Latin letters join
  an Estonian run, and Latin text keeps an Estonian layout in front, its
  punctuation typed at that layout's keys. The layouts are read
  again as the phone's keyboard opens; a shortcut (a modifier held)
  switches nothing. The on-screen keyboard's switch runs while remote
  control is on, under the lease taken first; a refused switch is said in
  the keyboard's header and asked again as control is taken again. The
  page counts its layout switches, and a read of the layouts that began
  before the last of them is not kept, as the agent may have read them
  before the switch.
- The on-screen keyboard's switch goes one way, from the keyboard to the
  target. A layout switched on the target itself (Alt+Shift, Win+Space,
  the language bar) leaves the on-screen keyboard in the language chosen
  on it, and its keys, which send key positions, then type what the
  target's layout has at them; the Keyboard menu shows the target's layout
  from the next read. They agree again when the keyboard's language is
  chosen as the target's, or the target's layout is switched back. Seen
  on the target at the review's gate F and left so at the operator's word
  (2026-10-17).

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
  Windows. One agent runs in a session (its mutex): a second start of the
  same commit leaves the first; one of another commit asks the running
  agent to leave by its menu's Exit, which every agent since the first
  answers, takes its place within 5 s, and, while the sign-in start is on,
  refreshes its copy, which then follows the agent started last (decision
  1). While connected, the menu and tip say when the board names another
  commit ("not the board's version"). The agent serves the board only
  while its session is the console's: in another (switched away from, or
  remote) it closes the collection and says why, and opens it again when
  its session is the console's (decision 2). A thread keeps the channel:
  it finds the collection again whenever it goes, says hello with the
  system, answers the board's calls
  with its reads and actions (`reads.rs`, `actions.rs`), and pings every
  3 s. A monitor's id is the 64-bit FNV-1a hash of Windows' name for its
  device (`\\?\DISPLAY#...`, which names its connector too), its letters
  in one case, as 16 hex digits: the same while the monitor stays on its
  connector. The board's monitor is the one the operator chose, kept for
  the user in `HKCU\Software\HardenedBliKVM`, value `BoardMonitor`, and
  none while the chosen one is not active; or, without a choice, the one
  monitor with a known capture's EDID, and none when more than one has it
  (`names::BOARD_CAPTURE_EDIDS`); it is resolved once for each
  configuration the agent reads. That list is empty: gate 2 found
  that the operator's capture gives the target a copy of the operator's
  own monitor's EDID (both read `BNQ 8024`), as KVM captures often do, so
  an EDID cannot tell them apart. Until a monitor is the board's, the
  modes that need it say so, and the page asks which one it is. A
  monitors' change is made from a copy of the configuration it read, and
  one lock covers a change from that read to its claim of the undo, the
  15 s timer from its check to its putting back, and a keep (decision 6,
  `revert.rs`); a Keep with nothing waiting is refused, and only a saved
  change stops waiting. The clipboard's text is read within the size
  Windows gives for its block (another program may give it no zero).
- `hardened-blikvm-agent-test.exe`: the channel's test tool. It finds the
  board's collection (the board's vendor 0x1d6b and product 0x0104, usage
  page 0xff00, usage 1, 64-byte reports each way), opens it for overlapped
  reads and writes, says hello and sends pings (1000 by default) and says
  their round trips, or with `watch` pings every 200 ms and says when the
  channel went and how soon it came back. It appends what it says to
  `hardened-blikvm-agent-test.txt` beside itself.

The agent's side of the protocol (`client.rs`), the names it gives and its
decisions (`names.rs`: the clipboard's text, the start beside another
agent, the console's session, the board's monitor) and the undoing of a
monitors' change (`revert.rs`) are portable and tested on Linux; Windows'
calls are in
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
- Only the agent of the console's session serves the board: an agent left
  in a session switched away from, or in a remote one, neither reads nor
  acts on a session the operator does not see, and two sessions' agents
  never both answer.
- The board keeps nothing it reads from the agent beyond its hello and the
  counts of messages; the clipboard's text passes through and is not
  stored or logged.
- "Start the agent" types into the target, as the operator's keyboard
  would, only after a confirmation and under the lease.

## What was shown on the test board

The gates of `docs/baselines/work-plan-20261014-target-agent.md` were
checked on the operator's target, Windows 11 Pro for Workstations 26H2
with two monitors of one EDID, and are recorded with their evidence in
`docs/baselines/audit-20261014-target-agent.md`: the reproducible build
(gate 0); the channel, its round trip of 2 ms and its return within a
second after a media change (gate 1); the reads, matching Windows'
settings, and the finding that the board's monitor must be the
operator's choice (gate 2); the absolute mouse on the board's monitor at
its corners and middle from a desktop and a phone, with no layout set by
hand (gate 3); the actions, a monitors' change without a picture undone
by itself among them (gate 4); and the drive and "Start the agent"
(gate 5; the sign-in start's surviving a restart was skipped at the
operator's word). The final slot is 95; no system part changed, so no card image
or RAW bundle goes with it.
