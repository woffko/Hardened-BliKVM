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
and commit. The board answers with its own, or refuses one of another
protocol or with odd text, with the reason; unknown fields are ignored so
that a side of another version is read and refused, not misread. Then
either side calls the other: a numbered request, answered or failed. The
one call so far is ping; a ping and its pong fit in one report each.

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

The control socket has `target_agent` (the switch), `target_agent_state`
(the switch, the channel, why its device is not open, the agent's hello,
the last refusal, the messages each way) and `target_agent_call` (waited
for on the channel's handle, not the shared state's lock, so HID and media
never wait for it). The server serves `GET` and `POST /api/target-agent`
through the owner; the platform names the feature `target_agent`.

## The target

`crates/bli-kvm-target-agent`, built by `scripts/build-target-agent.sh`
with `cargo-xwin` for `x86_64-pc-windows-msvc`, the C runtime linked in,
reproducibly (two builds in different directories give the same bytes,
which `scripts/check.sh` checks).

- `hardened-blikvm-agent.exe`: the agent, an icon in the notification area
  with its name, version and commit, and Quit.
- `hardened-blikvm-agent-test.exe`: the channel's test tool. It finds the
  board's collection (the board's vendor 0x1d6b and product 0x0104, usage
  page 0xff00, usage 1, 64-byte reports each way), opens it for overlapped
  reads and writes, says hello and sends pings (1000 by default) and says
  their round trips, or with `watch` pings every 200 ms and says when the
  channel went and how soon it came back. It appends what it says to
  `hardened-blikvm-agent-test.txt` beside itself.

The agent's side of the protocol (`client.rs`) is portable and tested on
Linux against a played board; Windows' calls are in `hid.rs` and `tray.rs`.

## Security

- Off by default; switching changes the USB device the target sees and is
  logged by the owner.
- The agent does only what the protocol names; the board answers only its
  hello and pings, and refuses a request before a hello.
- The board keeps nothing it reads from the agent beyond its hello and the
  counts of messages.
