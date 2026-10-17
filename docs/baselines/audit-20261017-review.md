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
or why it was left. Each test failed on the code before its fix (the
commits say how) and passes after; the work's log is
`evidence/private/review-20261017/fix-log.md`.

| Item | Plan | Status |
| --- | --- | --- |
| R1 | C1 | fixed, 0c78808: `the_clipboards_text_ends_at_its_zero_or_at_its_blocks_end` |
| R2 | C2 | fixed, 3272448: `a_timer_that_fires_while_a_second_change_is_made_puts_back_the_first_undo_point` |
| R3 | C3, D1 | fixed, 201796d (the agent: `an_agent_of_another_commit_takes_the_place_of_the_one_running`, `the_status_says_when_the_agent_is_not_the_boards`; the board's build names its commit) and d87bb3e (the page: `target_agent_start_contract.mjs`); shown on the target at phase F |
| R4 | B1 | fixed, 5f358cf: `a_failed_switch_off_leaves_the_channel_as_the_switch` |
| R5 | B2 | fixed, e8c3784: `a_refused_agent_change_leaves_later_gadgets_as_before` |
| R6 | D2 | fixed, 2d289b5: `agent_typing_contract.mjs`, a refused switch |
| R7 | C4, D3 | fixed, d769704 (the agent: `only_a_change_that_waits_is_kept_and_one_not_saved_is_still_put_back`) and 9bd8550 (the page: `target_agent_reads_contract.mjs`, a refused Keep) |
| R8 | B3 | fixed, bb30cf9: `the_agents_drive_follows_the_switch_beside_an_ejected_medium`, `the_agents_drive_goes_even_when_the_storage_cannot_be_rebuilt`, `an_empty_lun_zero_is_a_removable_one_and_the_drive_can_go_alone` |
| R9 | D4 | fixed, cae6c12: `target_agent_start_contract.mjs`, Win+R held back 800 ms |
| R10 | B6 | fixed, 3ed21c2: `a_refusal_from_the_target_is_kept_within_the_states_bounds` |
| R11 | D5 | fixed, f395d1c: `a_monitors_change_waits_longer_than_a_read` (server), `target_agent_reads_contract.mjs` (a change answered after 4 s) |
| R12 | C5 | fixed, 4e20b87: `only_the_consoles_session_serves_the_board` |
| R13 | D7 | fixed, 6081692: `tools/test_watching_probes.mjs` |
| R14 | B7 | fixed, f4f8231: `a_call_whose_request_cannot_go_ends_at_once` |
| R15 | D6 | fixed, dce2610: `target_agent_reads_contract.mjs`, a read of the layouts answered after a switch |
| L1 | B8 | fixed, 6abb6e2: `a_gadget_built_again_for_a_hid_request_closes_the_channel_meanwhile` |
| L2 | C6 | fixed, e35548d: `the_edid_stands_in_for_no_choice_and_marks_only_a_monitor_of_its_own` |
| L3 | E | done, c56a96b: the stores' own tests |
| L4 | E | done, 9a41f1e: the owner's tests in every feature set |
| L5 | E | done, 02e20ac: `a_second_lun_is_made_before_the_link_and_goes_before_the_function` and the media executor's |
| L6 | E | done, f52f6d0: `target_agent_reads_contract.mjs`, `agent_typing_contract.mjs` |
| L7 | D8 | fixed, acaed9d: `target_agent_contract.mjs`, the settings open for 11 s |
| L8 | C7 | fixed, e766d55: `the_boards_path_is_found_among_a_configurations_names` (the repeated reads were Windows' calls; no test can fail on them) |
| L9 | C7 | fixed with L8, e766d55 |
| L10 | B9 | fixed, 09d8379: `a_target_agent_calls_lease_is_refused_as_a_lease` |
| L11 | C8 | done, 6ee5d55: the protocol's tests |
| L12 | E | done, dce2610 (the patch of the page's state, with R15) and ce57ef4 (the switch under the lease): `agent_typing_contract.mjs`, `target_agent_reads_contract.mjs` |
| L13 | B10 | fixed, 7ae51f6: `absolute_reports_read_the_descriptor_once_for_each_change_of_the_gadget` |
| U1 | B4 | shown, and fixed with R8 in bb30cf9: `the_agents_drive_puts_back_no_medium_the_target_ejected` |
| U2 | B5 | shown, and fixed in 7f0679b: `a_storage_change_that_fails_binds_the_gadget_again` |

## Gates

- **Gate B** (2026-10-17): B1 to B10's tests failed before their fixes and
  pass after; the owner's tests (194 with all features, and each feature
  set `check.sh` runs), the media, gadget, HID and core tests pass.
  `scripts/check.sh` on 7ae51f6 passed every step, each failure under the
  shared host's load run again: five full runs each stopped at one timing
  contract of the page, which phase B did not touch, while another
  session's fuzzers, test oracles and builds held the load average between
  6 and 57; every step before the QUIC video contract passed in the third
  and fourth runs; that contract passed alone on its second try, the QUIC
  share contract after it on its second, and every step after them passed.
  Flaky under that load: `tools/playwright/mjpeg_upscale_contract.mjs`
  (its watchdog's count, "drawn 7.3 of 10 a second"),
  `tools/playwright/screen_area_contract.mjs` (a pointer moved before the
  page had the board's answer), `tools/playwright/vp8_range_contract.mjs`
  ("drawn 2.5 of 2.7 a second"), `tools/playwright/quic_video_contract.mjs`
  (frames drawn per second, a pinch's buffer) and
  `tools/playwright/quic_share_contract.mjs` (the page keeping up on the
  narrow path; a session that started again); the logs are in
  `evidence/private/review-20261017/`.
- **Gate C** (2026-10-17): C1 to C8's tests failed before their fixes
  (written first as the agent behaved, then fixed) or, for L8, L9 and L11,
  are the tests of the code moved, and pass after; the agent's 25 portable
  tests pass on Linux, its Windows code is clean under `cargo xwin clippy`
  and `check.sh` built it twice to the same bytes. `scripts/check.sh` on
  6ee5d55 passed every step, run again under the shared host's load: the
  first run stopped at `tools/test_gui_parity_app.mjs` (a 404 in the
  console, a flake known under load), which then passed alone; the steps
  after it stopped at `tools/playwright/vp8_range_contract.mjs` and, from
  there with each page contract tried up to five times, at
  `tools/playwright/quic_video_contract.mjs`, five failures at a load of 44
  (frames drawn per second); with the other sessions' jobs paused at the
  operator's word, the steps from that contract on passed, it on its third
  try and the QUIC share contract on its fourth.
- **Gate D** (2026-10-17): D1 to D8's tests failed on the page or the
  server before their fixes and pass after (the contracts with a played
  board; the server's and the control protocol's own tests). Two of the
  contracts' assumptions from before were wrong and are corrected: the
  Monitors menu closes when its Keep dialog is clicked, as a click beside
  it does, and was only read while it was closing; and a Keep button with
  antd's loading icon kept the name "loading Keep" while the icon left,
  so the Keep is disabled while it waits instead. `tsc`, `eslint` and
  `prettier` are clean.
- **Phase E**: L3, L4, L5, L6 and L12 moved code under the tests that
  already covered it (L11 went with C8); none was left.
- **Gates D and E**: `scripts/check.sh` on cc60094, the branch's tip with
  phases D and E and the design note, passed every step in one run, with
  the other sessions' jobs paused (load average 13 falling to 2); no page
  contract needed a second try.
- **Gate F, the board** (2026-10-17): slot 96 (0.1.0-beta.8, source
  cc60094, the bundle's SHA-256 `6152cc8d...ff70`) installed through the
  update API at the operator's word ("Ставь, как соберётся") and
  confirmed; 95 is the previous slot. Every setting and switch as found
  (the wide state compared before and after); the services active; the
  agent's switch on as the operator left it, its channel open, the
  board's commit cc6009432dd191b38fa028e59fe42dfdfe94ad09 in its state
  (until now every slot's hello said `unknown`); the agent's drive from
  slot 96 (its agent's SHA-256 `53875510...b929`, built from cc60094) on
  LUN 0, alone and not removable, as no image of the operator's was
  attached, as found. The watching probes passed: the GUI walk at a
  desktop's and a phone's size (no failed API request), WebRTC H.264 and
  VP8, the QUIC video and touch reach.
- **Gate F, the target (F3)** (2026-10-17), the operator's checks on the
  target with slot 96, in the operator's words:
  - R3 and R9: "Я проверил отдельно, работает" (checked on their own, it
    works). Read afterwards from the development host, read only: the
    sign-in copy is now slot 96's agent (SHA-256 `53875510...b929`,
    "0.1.0 (cc6009432dd1...)") where slot 92's (`465934e2...07aa`) was
    before, the sign-in entry still names it, it runs in the console's
    session, and the board shows it connected with the board's own
    commit, no message dropped.
  - R6: the on-screen keyboard's language "после закрытия открытия
    остается" (stays after control was off and on). Found with it: a
    layout switched on the target itself (Alt+Shift) is not followed by
    the on-screen keyboard, which keeps its language; left so at the
    operator's word and said in the design note and the README.
  - R7: "клик вне окна оставляет да" (after a click outside the window,
    Keep keeps the change).
  - R11: the Keep dialog said "около 3 секунд" (about 3 s) for a mode
    change, about the 3 s the owner allowed one before the fix.
  - R12 and the sign-in start across a restart: "пропускаем" (skipped).
- **Phase G** (2026-10-17): the table above has every item; the design
  note and the README are cc60094's, with the on-screen keyboard's
  one-way switch found at F3. The board as found: its wide state (69
  values) read again after F3 differs from the one read before slot 96
  only in the slot (96 confirmed, 95 the previous) and the clock; the
  agent's switch is on as the operator left it, and the agent connected.
  No release was asked for.
