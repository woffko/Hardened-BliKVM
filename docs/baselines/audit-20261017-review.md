# Code review of 0.1.0-beta.7 (2026-10-17)

The review the operator asked for after the release 0.1.0-beta.7 ("сделай
полный аудит кода на ошибки и прочее"). Its fixes are planned in
`docs/baselines/work-plan-20261017-review-fixes.md`; this note keeps the
findings and, in the table at its end, what became of each.

## Scope and method

- Reviewed: the release's change, `v0.1.0-beta.5...478bad9` (117 files,
  about 13,600 lines): the target agent (protocol, Windows program, board
  owner's channel and drive, server routes, page), the desktop layout of
  the Mouse menu, the on-screen and phone keyboards' fixes, and the device
  probes. Not reviewed: the rest of the branch against `main` (509 commits,
  about 160,000 lines), which the earlier reviews of 2026-09-29 and
  2026-10-04 covered in part.
- Nine angles looked for candidates (line by line, removed behaviour,
  cross-file, language pitfalls, wrappers, altitude, reuse, simplification,
  efficiency); every candidate had a verifier of its own, a sweep for gaps
  followed and its candidates were verified too. Several were reproduced in
  throwaway copies of the tree with probe tests (a failed switch-off, the
  agent flag, the drive after an eject, a Refused reason, the rollback to a
  slot from before the agent); those copies are not kept: the plan turns
  each reproduction into a test of the repository.
- The review's notes: `evidence/private/review-20261017/` (candidates and
  verdicts, the findings as reported, the reviewed diff, a verifier's log).
- CONFIRMED: reproduced or shown from the code. PLAUSIBLE: the path is real;
  its trigger depends on timing or on durations not measured on the target.

## Findings

R1 is the most severe; two of them (R1, R10) can be set off by any program
on the target.

- **R1** `crates/bli-kvm-target-agent/src/actions.rs:140` (CONFIRMED). The
  clipboard's read scans the locked text for its zero up to a fixed bound
  (24,000 units) and never asks `GlobalSize`: a block another program put on
  the clipboard without a terminating zero is read past its end, which
  crashes the agent or sends the bytes after it to the board as text.
- **R2** `crates/bli-kvm-target-agent/src/actions.rs:364` (CONFIRMED). A
  monitors' change reads the configuration to put back and applies the new
  one before it claims the pending revert, with nothing keeping the 15 s
  timer out meanwhile: a change made as an earlier one's timer fires records
  the earlier change's result as its undo point, and the safety revert then
  restores and saves the configuration it was meant to undo (a choice
  without a picture made permanent).
- **R3** `crates/bli-kvm-target-agent/src/autostart.rs:53` (CONFIRMED). The
  copy kept for the sign-in start is written only when the item is turned on
  from a program that is not that copy, and nothing refreshes or compares
  it; a second agent exits silently on the session's mutex. After a board
  update the old agent keeps starting at sign-in, and the board's newer
  agent cannot replace it (the operator's target is in this state: the Run
  key names slot 92's agent while the board runs slot 95).
- **R4** `crates/bli-kvm-control-broker/src/v2.rs:899` (CONFIRMED,
  reproduced). A switch-off that fails after the channel was told it is not
  wanted restores the switch file but not the channel: the switch shows on,
  no agent can connect, and every call is refused until a later switch
  request succeeds or the owner restarts.
- **R5** `crates/bli-kvm-media-transition/src/lib.rs:338` (CONFIRMED,
  reproduced). `set_agent()` stores its flag before its fallible steps and
  keeps it on failure, so the next gadget built from scratch follows the
  failed request rather than the switch: a failed turn-on can later expose
  the agent's collection to the target with the switch off.
- **R6** `web/src/hooks/useAgentLayoutSync.ts:43` (CONFIRMED). The on-screen
  keyboard's layout sync asks the agent without taking the lease, drops the
  refusal, and does not try again when control is taken: Russian on the
  on-screen keyboard can then type at the US layout's keys.
- **R7** `web/src/pages/desktop/agent-displays-keep.tsx:39` (CONFIRMED). Keep
  closes its dialog before `keep_displays` is answered and drops a failure:
  a Keep that never reaches the agent looks kept, and the monitors change
  back 15 s later without a word; the agent also answers Done when nothing
  is pending.
- **R8** `crates/bli-kvm-media-transition/src/lib.rs:304` (CONFIRMED,
  reproduced). Adding or removing the agent's drive rebuilds the mass
  storage from the selection; after an owner restart (every slot install)
  with the image ejected by the target, there is none, the change fails, and
  the request still answers Applied: the switch is on and no drive appears.
  At an owner's stop the same failure leaves the drive's LUN in place, which
  a slot from before the agent, restored after a failed activation, cannot
  remove (the target is left without keyboard, mouse and media).
- **R9** `web/src/pages/desktop/menu/settings/device/target-agent.tsx:46`
  (PLAUSIBLE). "Start the agent" times the Run box's second from when Win+R
  is queued, not from when it is sent: a slow lease can let the line run
  into another window.
- **R10** `crates/bli-kvm-control-broker/src/target_agent.rs:257`
  (CONFIRMED, reproduced). A Refused message from the target is kept as it
  came, while the state's encoder refuses an empty, long or multi-line
  reason: one such frame makes every later state fail, and the page shows
  the agent's switch off and disabled until a valid hello or a restart.
- **R11** `crates/bli-kvm-server/src/target_agent.rs:89` (PLAUSIBLE). Every
  call waits 3 s, set when only reads existed; a monitors' change between
  "only the board's" and "only the others" makes two mode sets before it
  answers. A slower change shows an error, opens no Keep dialog, and is
  undone 15 s later.
- **R12** `crates/bli-kvm-target-agent/src/tray.rs:66` (CONFIRMED). The agent
  is one per session and serves calls whatever session is on the console: an
  agent left in a switched-away or remote session stays the connected one
  (its reads and actions act on the wrong session, its clipboard is another
  user's), and two sessions' agents both answer.
- **R13** `tools/playwright/quic_input_device_probe.mjs:375` (CONFIRMED).
  Seven of the nine probes that fail on any `/api/hid` request still fail on
  the page's read of the desktop layout (`cc68e2e` fixed two).
- **R14** `crates/bli-kvm-control-broker/src/target_agent.rs:562`
  (PLAUSIBLE). A request dropped while the device is closed, or by a pause,
  leaves its caller to wait out the whole timeout, then report
  `backend_unavailable` rather than failing at once.
- **R15** `web/src/hooks/useTargetAgent.ts:63` (PLAUSIBLE). The 15 s read of
  the layouts can overwrite the layout a switch just set, so the typing path
  skips a switch it needs until the next read.

## Lesser findings (verified, cut from the list by its size)

- **L1** `crates/bli-kvm-control-broker/src/v2.rs:1339`: the lazy
  `ensure_hid_gadget` builds without the channel's pause and resume (only
  when the gadget's directory is gone); its comment claims a recovery from
  an unbind that it does not make.
- **L2** `crates/bli-kvm-target-agent/src/names.rs:86`: the EDID fallback
  takes the first match and also runs when the chosen monitor is only
  inactive (dormant: the list is empty).
- **L3** `crates/bli-kvm-server/src/screen_area.rs:81`: its store repeats
  `webrtc.rs`'s.
- **L4** `crates/bli-kvm-control-broker/src/v2.rs:119`: the state's 27-field
  literal is written in eight constructors.
- **L5** `crates/bli-kvm-usb-gadget/src/lib.rs:327`: the second LUN's steps
  repeat the first's.
- **L6** the monitors' list is written twice (Monitors menu, settings).
- **L7** the settings' agent section keeps its own state and a second 5 s
  read instead of `targetAgentAtom`.
- **L8** `crates/bli-kvm-target-agent/src/reads.rs:248`: `is_board` reads
  every monitor again for each path.
- **L9** `crates/bli-kvm-target-agent/src/actions.rs:357`: the board-primary
  change and `only_after_extending` read the configuration again.
- **L10** `crates/bli-kvm-core/src/control_v2.rs:670`: the call's lease is
  checked by hand, and a bad lease is reported with the wrong words.
- **L11** `crates/bli-kvm-agent-protocol/src/frame.rs:74`: a vector built
  only to be counted.
- **L12** a layout switch and its patch of the page's state are written in
  two places.
- **L13** each absolute-mouse report reads the configfs descriptor once more.

## Leads not yet confirmed

- **U1** `crates/bli-kvm-media-transition/src/lib.rs:555`: with a recorded
  selection, a rebuild for the switch or HID-only mode may put back a medium
  the target ejected.
- **U2** `crates/bli-kvm-usb-gadget/src/lib.rs:567` and `:606`: replacing or
  removing the mass storage unbinds first and does not bind again on a
  failure; the lazy `ensure_hid_gadget` does not rebind a gadget that is
  present but unbound.

## Refuted

The short HID write (f_hid writes whole reports); the dropped task handle
(release builds abort on a panic, systemd restarts the owner); the switch
under the shared lock (needed, as the device goes while unbound); the
quiescence stripping the agent on a refused release (idempotent); the
on-screen keyboard's armed modifier; the descriptor check racing the switch
(serialised); the drive's path read back after a restart (it is on the root
file system); the order in which the drive is stored (by design);
`current_selection()` with a path without a file name; the tray blocked by
its reads (the message box keeps dispatching); sharing the frame code with
QUIC; reading monitors and layouts at once (the agent serialises calls).

## Status

Filled in by the plan's work: the commit that fixed an item and its test,
or why it was left.

| Item | Status |
| --- | --- |
| R1 to R15, L1 to L13, U1, U2 | open |
