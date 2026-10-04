# Code review of 2026-10-04 (HEAD d58ee97)

A review of the work of the plan of 2026-10-03 (the H.264 engine as the
board's choice, the video menu, the quality monitor:
`docs/baselines/work-plan-20261003-h264-choice-quality-monitor.md`, its
outcome in `docs/baselines/audit-20261004-h264-choice-quality-monitor.md`):
the 28 commits from 6522ff5 to d58ee97, 64 files, 8852 lines added and 257
removed. The review itself fixed nothing. The findings are numbered Q1-Q16 by
severity and are the input of
`docs/baselines/work-plan-20261004-review-fixes.md`.

## Method and coverage

- clippy over the workspace (all targets): no warning. Again over the
  encoder (features `h264`, `turbojpeg`) and the server
  (`restricted-control-surface`), library and binary code, with the lints for
  casts, arithmetic, indexing, `unwrap`, `panic`, locks held across an await,
  ignored results and float comparison: 31 hits on lines the plan changed,
  all read; none is a bug (the counters are `u64`, the casts follow a bound or
  a decoder's limit);
- `tsc`, `eslint` and `prettier` over the page and the Playwright files:
  clean;
- the last `scripts/check.sh`, on the tree of d58ee97: 3202 Rust tests passed,
  none failed, 4 ignored, and the page's contracts passed;
- `cargo audit` with the advisory database of 2026-06-29 (not updated here):
  one advisory (Q14). `cargo deny` has no configuration in the repository, so
  its licence check says nothing; shellcheck and ruff are not installed, so
  the new shell and Python were read and checked with `sh -n` and
  `py_compile` only;
- line-by-line reading of everything the plan changed in the encoder
  (`demand`, `choice`, `engine`, `report`, `main`), the server
  (`encoder_report`, `quality`, `video_settings`, `webrtc`, the parts of
  `lib.rs`), the page (the monitor and its worker, the rows, `page-video`,
  the video menu, the screen views, the QUIC video worker), the packaging and
  deploy changes and the new tools and probes. The Playwright contracts and
  the host gate were skimmed, not read in full;
- experiments, with receipts under `evidence/private/review-20261004`: the
  page with `transferControlToOffscreen` removed and with the worker refused
  (`monitor_cases.mjs`, `monitor-without-offscreen.jsonl`); the plan's own
  tests and two older ones beside 96 busy loops on 64 cores, six runs each
  (`stress-*.txt`, the load average above 70 by the third run), the older
  ones also on the code before the plan (6522ff5, unpacked from Git in a
  scratch directory); the key frames of the still picture in the receipts of
  the plan's gate C5 (`c5-s7*-quic-*-basic-tap`).

Marks: [T] reproduced by a test or a measurement, [C] derived from the code.
The board was only read (its earlier receipts); nothing was run on it.
Not covered: browsers other than Chromium, the Playwright contracts under
load, anything the board would add.

## Medium

Q1. [T] The monitor loops without `transferControlToOffscreen`, and a
refused worker takes the page down
(`web/src/pages/desktop/quality-monitor/index.tsx`, the effect of
`MonitorBody`). A canvas can be handed to a worker once, so after a change of
mode the effect meets an exception on the old canvas and mounts a new one
(`setCanvasKey`). The same `catch` answers a browser that has no
`transferControlToOffscreen`: the new canvas fails the same way, without end.
With the method removed from `HTMLCanvasElement.prototype` the monitor made
22,000 to 30,000 canvases in three seconds (two with the method) and kept the
tab busy; Safari before 16.4 (an iPhone 7 stays below it) and some embedded
web views lack the method. The creation of the worker, the posts and the
timer are inside no guard: where the browser refuses a worker (a policy) the
effect throws, and the router's default error screen ("Unexpected Application
Error") replaces the whole page, the toolbar included. The open state is
remembered by the browser, so the page shows the error screen again at every
load, with nothing on it that closes the monitor. With the monitor closed the
same browser shows the page normally.

Q2. [T] A new server test fails under load
(`crates/bli-kvm-server/src/lib.rs`, the first assertion of
`the_quality_answer_is_built_once_a_second_from_the_reports`). The test writes
the encoder's report, then logs in (Argon2 in a debug build); the answer shows
no report older than three seconds, so on a loaded host the first read finds
the report stale and `first["h264"]` is null. Beside 96 busy loops it failed
in 3 of 6 runs alone and in 5 of 6 runs of the plan's four server test
groups; idle it passes in 3.3 s. The product is not at fault; the test
assumes a fast login.

Q3. [C] The engine choice is offered without the encoder that understands it.
The manifest lists `h264_engine` when the Janus address and the H.264 demand
socket are configured (`lib.rs`, the video controls' block). The encoder, its
units and the drop-in that names the encoders' reports on the server come from
`tools/device/deploy_vp8.sh`, not from the slot. A slot on a board whose
encoder and drop-in are older offers H.264 HW and H.264 SW that change nothing
(the old encoder skips the `engine` line), and the monitor has no board
numbers (no report path); the page cannot tell.

Q4. [T] The periodic key frame of a still picture is often a delta frame, and
two tests of the QUIC plan fail under load.
- `main.rs` decides a periodic key frame with `last_key.elapsed() >=
  KEY_FRAME_EVERY` (two seconds), where `last_key` is the moment the previous
  key frame's encode began, while the gate admits the still picture's refresh
  two seconds after the previous admission by the capture clock. When the
  first frame's decoding and conversion took longer than the second's, the
  refresh comes less than two seconds after `last_key` and is encoded as a
  delta frame; the next key frame is then up to two seconds late. On the
  board's still picture the refresh frames came 2.0 to 2.03 s apart and 25 to
  33 of 69 or 70 of them were delta frames in the receipts of gate C5 (the key
  frames came two or four seconds apart, never further). The design says a key
  frame every two seconds (Janus's RTP input cannot ask for one). The cause is
  derived from the code and the numbers; the plan confirms it with a log
  before it changes anything.
- `frame_socket::on_a_still_picture_a_key_frame_comes_when_it_is_asked_for`
  (the QUIC plan's) fails on exactly that assertion beside 96 busy loops: 3
  of 6 runs at d58ee97 and 6 of 6 at 6522ff5, so it did not start with the
  plan.
- `frames::tests::a_client_that_does_not_read_loses_its_queue_and_holds_nobody_up`
  (also the QUIC plan's) failed in 7 of 40 runs of the encoder's library tests
  beside 96 busy loops, in none of 16 runs alone: the reading thread's
  `read_exact` timed out after five seconds (`WouldBlock`) and the test
  panicked on its join. It publishes 200 frames of 256 KiB 0.3 ms apart
  (`stress-encoder-lib-more.txt`). [C] A reading thread that falls 8 MiB (32
  frames) behind is dropped by the hub as designed, after which only delta
  frames follow and the frame it waits for never comes; the pace is not one
  the product has.

## Low

Q5. [C] The first quality answer after a pause rates against old readings
(`crates/bli-kvm-server/src/quality.rs`, `encoder` and `units`). A report
that is stale, refused or absent does not clear the stored pair, so the first
fresh report after a pause is paired with the last one before it: the
encoder's `fps`, the gate's and the engine's rates are made over the whole
pause (a number near zero for a second instead of "-"). A unit's processor
share is likewise taken from the reading of the last time the monitor was
open.

Q6. [C] After the engine was given up the menu cannot ask for it again
(`web/src/pages/desktop/menu/screen/video-mode.tsx`). The design disables
H.264 HW where the report says the engine cannot be had, and the encoder
gives it up for good until the choice is made again; the page posts a choice
only where it differs from the board's, which it does not after a give-up.
The one way back is to choose H.264 SW (which makes the encoder look at the
device again) and then H.264 HW. The tooltip does not say so.

Q7. [C] The engine and the WebRTC quality are changed by any session when the
video controls are off (`crates/bli-kvm-server/src/video_settings.rs`,
`update_screen`). `video_controls_enabled` decides what the manifest offers
and nothing else; the request needs a session and a CSRF token. The design
note says the request is served "where the video controls are enabled".
Older than the plan for the WebRTC quality.

Q8. [C] Two choices made at once can leave the file, the channel and the
encoder with different values (`set_h264_engine`, `set_webrtc_bitrate`): the
store and the send are not under one lock. Not provoked; needs two requests
in the same instant.

Q9. [C] In the Direct mode the monitor shows another viewer's encoder
(`web/src/lib/quality-monitor.ts`, `servingEncoder`): it takes the encoder by
the browser's mode, and Direct has none of ours, so the Codec and Bitrate rows
show the engine and the bitrate of whoever else watches H.264.

Q10. [C] The report's time and content are read apart
(`crates/bli-kvm-server/src/encoder_report.rs`, `read_encoder_report`): the
modification time, then the open. A report written between the two gives the
new content the old time and the rates of that answer a wrong span.

Q11. [C] Two small faults of the page. A refused choice shows the server's
rejection code as the message (`unsupported_setting`, untranslated;
`video-mode.tsx`). The monitor's canvas scale is read when the view is made
(`window.devicePixelRatio` in a memo) and is not renewed when the page is
zoomed or moved to another display, until the window is resized or the level
changes: the rows stay blurred.

Q12. [C] The rows are drawn on a canvas: a screen reader gets the window's name
and nothing else, and nothing can be selected or copied. The cost of C4.1's
choice (the rows in the document cost the main thread 4 to 5 ms a second); the
design note says it for the contracts, not as a limit for the user.

Q13. [C] Older than the plan: the demand connection skips an overlong line
without a bound (`crates/bli-kvm-video-encoder/src/demand.rs`, the `read_until`
after "Too long: drop the rest of it"). A client of the demand socket (the web
server's user and the unit's group) that sends bytes without a newline grows
the encoder's memory without end. The unit's limits bound the harm; the
exposure is a compromised peer in the group.

## Information

Q14. [T] `cargo audit`: `time` 0.3.45, RUSTSEC-2026-0009 (stack exhaustion
when parsing RFC 2822 text; fixed in 0.3.47), pulled in by `wtransport`,
`rcgen` and `x509-parser` of the QUIC gateway, which parse no such text. The
advisory database on this host is from 2026-06-29. Not from the plan.

Q15. [C] The plan's probes (`engine_switch_probe.py`,
`engine_menu_device_probe.mjs`, `quality_monitor_device_probe.mjs`) do not
verify the board's certificate (its fingerprint is recorded, not compared)
and send the GUI password over that connection: the repository's pattern for
its device probes, on a LAN, for a test board.

Q16. Smaller. `tools/device/engine_switch_probe.py` has no SPDX line (its four
siblings have). The monitor's worker keeps four test globals
(`qualityMonitorRows`, `qualityMonitorLabels`, `qualityMonitorSections`,
`qualityMonitorDraws`) in the shipped build, which the contracts and probes
read. The server reads the encoder's report with `deny_unknown_fields`, so a
field added to schema 1 makes an older server refuse the whole report (a new
field needs schema 2; not written down). The web build warns of chunks over
1 MiB (it did before the plan).

## Checked, no action

- The encoder's choice, read through state by state (stream opened, engine
  given up, refused, probe only where the device is not held, a choice made
  again, a viewer's arrival that repeats the choice): no wrong transition. The
  counters across streams and switches neither lose nor double count
  (`hardware_before` and `finished`). A switch keeps the packetizer, so the
  sequence numbers, the SSRC and the frame socket's numbering go on, and the
  choice bumps the generation, so the next frame is a key frame.
- The report is rewritten within about 1.5 s while a viewer watches and
  frames come; with no capture frames its numbers do not change, it is not
  rewritten, goes stale after three seconds, and the monitor then shows no
  board numbers (accepted).
- The quality route sits behind the session middleware (an anonymous request
  gets 401); one build a second serves many readers; the report reader is
  bounded (4 KiB, strict fields, one reason of 120 characters at most); the
  update status read has its own timeout and concurrency limit, so the
  background version read cannot pile up.
- The monitor's window keeps pointer, mouse, wheel and touch events from the
  page's handlers; its worker and its timer end with the window; rates are
  null for a sample out of order or after a reset; the English and Russian
  texts have the same keys (48 and 9).
- The once-a-second request does not change the session's idle lock: the
  Power menu already polls every five seconds.
- Repository: no password, key, address or local path added by the plan; all
  28 commits carry the attribution line; no evidence file is tracked.

## Baseline under the load tool (A2, before the fixes)

Made with `scripts/stress-under-load.sh` (1be9448) at 96 busy loops on 64
cores, one and a half times the cores, on the code of that commit (the review's
d58ee97 with the tool added); receipts under
`evidence/private/review-fixes-20261004/baseline`. Every failure is the test
the review named and the assertion it named.

| Set | Runs | Failed | Test and line |
|---|---|---|---|
| the server's `the_quality_answer_is_built_once_a_second_from_the_reports` alone | 12 | 2 | `lib.rs:2417` (Q2) |
| the plan's four server test groups together | 12 | 2 | the same test, `lib.rs:2417` |
| the encoder's `frame_socket` tests | 12 | 2 | `on_a_still_picture_a_key_frame_comes_when_it_is_asked_for`, `frame_socket.rs:240` (Q4) |
| the encoder's library tests | 40 | 2 | `frames::tests::a_client_that_does_not_read_loses_its_queue_and_holds_nobody_up`, `frames.rs:470` (Q4) |
| `frame_socket` tests of 6522ff5, before the plan | 12 | 5 | the same still-picture test, `frame_socket.rs:239` |

The review's hand-made runs failed more often (3 of 6 and 5 of 6 for the
server test): the load average there was above 70 by the third run, and the
failure rate follows the load. The gates after the fixes are 30 runs under
this tool with no failure (a failure rate of one in six leaves 30 clean runs
a chance of under 0.5 %).

## Status after the fixes

Written on 2026-10-04 at the end of the fixes of
`docs/baselines/work-plan-20261004-review-fixes.md`, on branch
`wip/corrective-audit-checkpoint`: a line for each finding, with the commit
of the change and the test that failed on the code as it was and passes now.
Where the plan asked for a confirmation first, it is named. The numbers of
the gates are in the Record of
`docs/design/h264-engine-choice-and-quality-monitor-v1.md` ("Review fixes,
2026-10-04"); the receipts are under `evidence/private` (not in Git).

| | Status | Change | Test, and what it showed |
|---|---|---|---|
| Q1 | fixed | d667726 (B1) | `quality_monitor_contract.mjs`, the cases without `transferControlToOffscreen` and with the worker refused: 30,430 canvases in three seconds and the router's error screen before; at most one canvas, the note, no request and no error screen after (gate B) |
| Q2 | fixed | 996d41f (C5) | `the_quality_answer_is_built_once_a_second_from_the_reports`: 2 of 12 runs failed beside 96 busy loops before, 30 of 30 passed after (gate C) |
| Q3 | fixed | 3a85cd2 (C2) | `the_servers_own_video_settings_are_offered_and_served_only_where_configured`: the engine was offered without the encoder's report before |
| Q4 | confirmed first, fixed | 960a93f (E2), 1598274 and 3beeea9 (E3) | the logging build: 7 of 40 runs of the still-picture test failed, the time since the last key frame 1979.9 to 1999.6 ms in the time the encode began and 2001.3 to 2007.9 ms in capture time; `key_due`'s tests; the test 40 of 40 after, the frame socket's five tests 30 of 30 and the library's 40 of 40 under load (gate E); the board (gate F) |
| Q5 | fixed | a5ccd72 (C1) | `quality::tests::a_pause_makes_no_rate` (fps 60.0 over the pause before), `two_reports_are_paired_only_within_three_seconds`, `a_unit_is_not_compared_with_a_reading_older_than_ten_seconds` |
| Q6 | fixed | 6b08d47 (D1) | `h264_engine_menu_contract.mjs`, the engine busy while it is the board's choice: the entry was disabled before; now it is choosable with the reason and the retry text, and a click posts `h264_engine` 1 |
| Q7 | fixed | 3a85cd2 (C2) | the same test as Q3: with the video controls off both settings were served before |
| Q8 | confirmed first, fixed | b3fdade (C3) | `webrtc::tests::two_engine_choices_made_at_once_agree` and `two_quality_choices_made_at_once_agree`: the file and the channel differed in 50 of 50 repetitions before, in none of 50 after |
| Q9 | fixed | 2902564 (B2) | `tools/test_quality_monitor.mjs`, "Basic over Direct attributes no encoder to the page" (the Codec row was `H.264 HW 1920x1080`), and the contract's Direct case |
| Q10 | confirmed first, fixed | 1846708 (C4) | `encoder_report::tests::a_report_and_its_time_come_from_one_file`: a time that was not its file's in 141 to 197 of about 10,900 reads, in 20 of 20 repetitions before, none in 50 after |
| Q11 | fixed | 2902564 (B2, the pixel ratio), e8b3e8e (D2, the refused choice) | the contract's pixel-ratio case (264 pixels wide after the ratio went from 1 to 2, 528 now) and the menu contract's refused choice in English and Russian |
| Q12 | accepted, written down | 0aac97b (D3) | the design note's "The window": a screen reader gets the window's name and not the rows; the plan decided against a text alternative, which cost the main thread 4 to 5 ms a second |
| Q13 | fixed | 9ca364d (E1) | `demand::tests::a_line_that_does_not_end_is_skipped_within_a_bound_and_then_the_connection_ends`: the connection stayed before; it ends after 4 KiB now |
| Q14 | left | | not in the plan: bumping `time` needs the registry and the advisory database needs a download; the operator's step (below) |
| Q15 | left | | the repository's pattern for its device probes, on a LAN, for a test board: a plan of its own |
| Q16 | fixed, in part | 79a4b39 (A3), 0aac97b (D3) | `tools/test_plan_tool_headers.py` (the SPDX line); the design note names the worker's five test globals and says that schema 1 is closed; the web build's chunk-size warning is left, as it was before the plan |

Found while fixing, and not in the review: a third timing-sensitive test of
the encoder, `frames::tests::a_key_frame_on_request_is_made_once_per_cooldown`,
failed in the first `scripts/check.sh` run on the tree of D3 (a load flake of
the host, which two fuzz jobs of the operator also load): it waited for the
first of twenty request bytes to be read, and the other nineteen asked again
after the key frame it published. With 50,000 bytes the test as it was failed
three times in three. Mended in 3beeea9; no finding of the review is
"not confirmed".

What the operator has to decide, if wanted: the advisory database's update and
the `time` crate's bump for Q14 (a download); the word to publish anything
(none of the slots of the fixes is published).
