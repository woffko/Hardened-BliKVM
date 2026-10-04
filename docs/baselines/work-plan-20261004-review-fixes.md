# Work plan: fixes for the 2026-10-04 code review (2026-10-04)

Follows `docs/baselines/audit-20261004-review.md`; item numbers (Q1, Q2, ...)
refer to its findings. The review looked at the work of
`docs/baselines/work-plan-20261003-h264-choice-quality-monitor.md` (the
engine as the board's choice, the video menu, the quality monitor), whose
outcome is in `docs/baselines/audit-20261004-h264-choice-quality-monitor.md`
and whose design is `docs/design/h264-engine-choice-and-quality-monitor-v1.md`.
Rules as in the earlier plans: capability-gated surfaces enforced by the
server and the board owner, English repository text, secrets only through
`project-memory-exec` (never in arguments, logs or receipts, and never a
program on its standard input under it), every change shipped as a signed
slot or through the encoder's deploy script and verified on the test board,
audit and design documents updated with evidence under `evidence/private`.
Additions for this plan:

- One commit per item. Every fix comes with a test that fails on the code as
  it is now, named in the commit. A [C] finding is reproduced first; one that
  cannot be reproduced is recorded in the review note as "not confirmed" and
  left unchanged. A fix that would need a new route, a protocol change or a
  design decision outside this plan is not improvised: it is recorded in the
  review note as an open item.
- No new HTTP route (`tools/test_plan_route_drift.py` pins the count), no new
  capability, and the encoder's report stays schema 1.
- An item's own tests run before its commit; the full `scripts/check.sh` runs
  before the last commit of each phase and before every slot or package build
  (a failure under host load is rerun and the flaky test named). The tests
  this plan makes load-proof are shown under load by the tool of A1; the
  numbers are the gates.
- Phases end in gates with numbers where they have one. A gate that is missed,
  or a time box that runs out, ends the plan there with the findings
  recorded; what was finished stays.
- Device rules, as in the plan of 2026-10-03: the HID lease is never taken, no
  ATX pulse is sent, logins are never failed, media the operator attached or
  uploaded is left alone, and the operator's switches, firewall mode, video
  settings, WebRTC quality and QUIC switch stay as found. The engine choice
  ends as the operator left it (hardware). A probe never prints or stores the
  GUI password. Nothing of the plan is left on the board. Nothing is
  published.
- Consent: installing the slot and the encoder on the test board and
  switching the engine there (phase F) happens only with the operator's
  consent in the goal text (decision 7).

## Where the work starts

- Branch `wip/corrective-audit-checkpoint` at d58ee97, pushed; `check.sh`
  green on it; the tree clean but for the review note and this plan.
- The test board runs slot 72 `0.1.0-alpha.16` (source 1498dce), the update
  runtime `0.1.0~dev36` with the encoder driver on, and the H.264 encoder
  `4e3d9a8c` with the report paths in the server's drop-in. The engine choice
  is hardware; the screen is still (half a frame a second).
- What the review found, in short: the monitor loops without
  `transferControlToOffscreen` and takes the page down where a worker is
  refused (Q1); a new server test fails under load (Q2); the engine choice is
  offered without the encoder that understands it (Q3); the periodic key
  frame of a still picture is a delta frame in a third to a half of the cases
  on the board, and two older tests fail under load (Q4); and a row of smaller
  faults (Q5 to Q13).
- The load tool and the baseline do not exist yet: the review's numbers (96
  busy loops on 64 cores) were made by hand and are kept under
  `evidence/private/review-20261004`.

## Decisions

The plan takes these defaults; the operator may override one before the work
starts. A decision changed in the work is changed in
`docs/design/h264-engine-choice-and-quality-monitor-v1.md`, with the reason.

1. (Q1) The monitor needs workers and `OffscreenCanvas`. Where the browser
   lacks them or refuses the worker, the window opens and says so: no rows, no
   timer, no request. A drawing fallback on the main thread is not built: it
   would bring back the 4 to 5 ms a second that the worker removed, and a
   second renderer to keep right.
2. (Q3) The engine choice is offered only where the encoder's report is
   configured (`BLI_KVM_H264_ENCODER_REPORT`). The drop-in that names it is
   installed together with the encoder that writes the report, so it says the
   encoder of the plan of 2026-10-03 is there. A board with the old drop-in
   shows the one H.264 entry again until `deploy_vp8.sh` has run.
3. (Q6) H.264 HW is disabled only where the encoder says it has no engine to
   offer at all (the unit names no device: no reason given). Where it gives a
   reason (busy, no device node, three frames in a row failed) the entry stays
   choosable and shows the reason, and choosing hardware again while x264
   encodes posts the choice again: that is the retry the encoder waits for.
4. (Q7) `h264_engine` and `webrtc_kbps` are refused (`unsupported_setting`)
   where the video controls are not enabled on the server. So far only the
   manifest hid them.
5. (Q12) The monitor's rows stay on the canvas, as C4.1 decided. The design
   note says plainly that a screen reader gets the window's name only and
   that the rows cannot be selected. No text alternative is built: text kept
   current in the document is what cost the main thread 4 to 5 ms a second.
6. (Q4) The encoder measures the time between key frames in the frames'
   capture time, the clock of the gate that admits the still picture's
   refresh, and not in the time its encodes begin. Confirmed in E2 before it
   is changed.
7. (consent) The plan installs the slot and the encoder on the test board and
   switches the engine there only when the operator has said so in the goal
   text. Without it the plan asks once before the first board step, and
   without consent finishes everything on the host and lists the device steps
   as pending.
8. (release) Nothing is published. The slot of F2 stays a test slot, label
   `0.1.0-alpha.17`, like slots 67 to 72; a release is the operator's word.
9. Not in this plan, recorded in the review note as accepted or left: a text
   alternative for the rows (decision 5), the main-thread fallback (decision
   1), Q14 (bumping `time` needs the registry, and the advisory database
   needs a download: asked once if the operator wants it), Q15 (the probes'
   certificate check is the repository's pattern and a plan of its own), the
   web build's chunk-size warning, `GET /api/application-update/status`
   taking half a second, H.264 Direct on the board, browsers other than
   Chromium, and the findings of the earlier plans that are still open (the
   QUIC gateway's certificate made before the clock is set, the gate that
   lets about twenty of thirty frames a second through, the firewall status
   that may be five seconds old after a change).

## Phase A: a tool and a baseline (host only; at most 2 hours)

A1. `scripts/stress-under-load.sh RUNS LOAD -- COMMAND...` runs COMMAND RUNS
    times beside LOAD busy loops. LOAD is at most four times the cores and one
    and a half times by default; every run has a time limit (300 s unless
    `STRESS_TIMEOUT` says otherwise); it prints each run's status and a last
    line `stress: RUNS runs, N passed, M failed (LOAD busy loops)`; it exits 0
    only if every run passed; the loops are stopped on every exit and on
    SIGINT and SIGTERM, and each carries a marker in its command line so that
    they can be counted. `check.sh` runs `sh -n` over it and a unit test
    (`tools/test_stress_under_load.py`): runs of `true` and of a failing
    command give the right counts and statuses; an interrupted run leaves no
    loop behind.
A2. The baseline at HEAD, receipts under `evidence/private`: the tool at LOAD
    one and a half times the cores, 12 runs of each of: the server's test
    `the_quality_answer_is_built_once_a_second_from_the_reports`; the plan's
    server tests together (`quality`, `demand`, `engine`, `encoder_report`);
    the encoder's `frame_socket` tests; the encoder's library tests (40 runs:
    the failure of Q4's third test is one in six); and `frame_socket` of
    6522ff5 unpacked from Git into a scratch directory, to show that Q4's
    second test did not start with the plan. The failure counts go into the
    review note as "before".
A3. (Q16) `tools/device/engine_switch_probe.py` gets the SPDX line its
    siblings have.

## Phase B: the page (host; at most 5 hours; gate B)

B1. (Q1, decision 1) The monitor's body checks, before it does anything, that
    the browser has `Worker`, `OffscreenCanvas` and
    `HTMLCanvasElement.prototype.transferControlToOffscreen`; creating the
    worker, handing over the canvas and the posts are inside a guard together
    with the worker's `error` and `messageerror` events. A browser that
    cannot, or a worker that fails, makes the window show one line, English
    and Russian ("The quality monitor needs a browser with workers and
    OffscreenCanvas.") in place of the rows: no canvas is left, there is no
    timer and no request; the header, the level buttons and the close button
    stay. The remount that a canvas already handed over needs after a change
    of mode stays, and is told apart from a failure by a flag on the canvas:
    one remount a mode, never a loop. Test (the contract
    `quality_monitor_contract.mjs`): with the method removed, and with
    `Worker` refused, the monitor opened from the stored layout shows the line
    within a second, has made at most one canvas after three seconds and none
    is left (a mutation observer; 22,000 to 30,000 today), never asks
    `GET /api/vm/quality`, and the toolbar and menus work and no error screen
    shows (today the second case ends in the router's error screen); QM
    closes and opens it again; with both present nothing changes (the
    existing cases).
B2. (Q9, Q11) The Direct path attributes no encoder: the Codec and Bitrate
    rows come from the page's own picture (`H.264`, its size, `-`). The
    monitor follows `devicePixelRatio`: a `matchMedia` listener renews the
    scale when the page is zoomed or moved to another display, and the canvas
    is drawn again at the new size. Test: B3's for the first; the contract
    for the second: a device scale factor changed through the browser's
    emulation from 1 to 2 doubles the canvas's pixel width within two
    seconds.
B3. Unit tests of the monitor's pure functions with `node --test`
    (`tools/test_quality_monitor.mjs`, in `check.sh`): the file imports
    `web/src/lib/quality-monitor.ts` through Node's type stripping and skips
    with a named reason where `process.features.typescript` is off.
    `pageRates` (a reset, a sample out of order, nulls); `basicRows` for QUIC,
    WebRTC, Direct and MJPEG (the Speed's places on both sides of one megabit
    a second, the Delay's mark, the Codec row of Direct); `extendedSections`
    per path; `loadLayout` (garbage, partial, huge and negative values).
- **Gate B**: B1's and B2's cases pass and the contracts of the plan of
  2026-10-03 still do; the unit tests pass; `tsc`, `eslint` and `prettier`
  are clean; the host gate (`quality_monitor_gate.mjs`) again: the open
  monitor costs the main thread under 2 ms a second in Basic and in Extended
  (median of seven pairs, as in gate C4, where it was 0.77 and 0.24).

## Phase C: the server (host; at most 6 hours; gate C)

C1. (Q5) The quality answer makes no rate across a pause. A stored report is
    forgotten when the file is absent, refused or older than three seconds;
    two reports are paired only when they were written no more than three
    seconds apart; a unit's processor time is not compared with a reading
    older than ten seconds (`units` takes the time as an argument, for the
    test). Test: a report, a pause beyond three seconds, a report: `fps`,
    `gatePerSecond` and the engine's rates are null (today a rate over the
    pause); two reports a second apart: the rates as before; a unit read, and
    again ten seconds later: null (today a share over the span).
C2. (Q3, Q7, decisions 2 and 4) One function says whether the server offers
    the WebRTC quality and the engine choice. The manifest lists
    `h264_engine` only with the Janus address, the H.264 demand socket and the
    encoder's report configured (inside the video controls' block, as now);
    `POST /api/vm/screen` with `h264_engine` or `webrtc_kbps` answers
    `unsupported_setting` unless the setting is offered. The design note's
    text about the right is then true. Test: the manifest without the report
    path lacks `h264_engine` and with it lists it; both settings are refused
    with the video controls off and served with them on; the existing happy
    paths are unchanged. The tests that post `webrtc_kbps` all run with the
    video controls on; if a probe or a contract is found to rely on the old
    behaviour, `webrtc_kbps` keeps it and Q7 says so.
C3. (Q8) The store and the send of the engine's choice, and of the WebRTC
    quality, happen under one lock of the gateway, so two choices made at once
    leave the file, the channel and the encoder with the same one. Reproduce
    first: the function the route calls (`webrtc::choose_engine`, taken out of
    `set_h264_engine` for the purpose) from eight threads, 2,000 alternating
    choices each, compared at the end with the file; if the disagreement
    cannot be provoked in 50 repetitions, Q8 is "not confirmed" and no lock is
    added.
C4. (Q10) `read_encoder_report` opens the file first and reads the
    modification time from the open file (`File::metadata`). The existing
    reader tests pass; there is none for the race itself.
C5. (Q2) The server test writes the report after the login and sets its
    modification time explicitly. The plan's other server tests are read for
    the same assumption (a wall-clock bound that a slow login can break) and
    mended alike.
- **Gate C**: the plan's server tests, 30 runs under the tool of A1 at one and
  a half times the cores, no failure (the isolated test of Q2, and the four
  groups together); `cargo test -p bli-kvm-server` and `check.sh` pass; the
  route count is unchanged.

## Phase D: the menu and the notes (host; at most 4 hours)

D1. (Q6, decision 3) H.264 HW is disabled only where the encoder reports no
    engine and gives no reason. With a reason it is choosable: its tooltip says
    why the engine cannot be had now and that choosing it asks the encoder to
    try again (English and Russian); choosing hardware while the board's
    choice is hardware and x264 encodes posts `h264_engine` 1 again, and a
    second later the menu reads what encodes again. Nothing changes for the
    other states. Test: the contract's case "no engine to be had" is split.
    With a reason: enabled, the tooltip shows the reason and the retry text,
    and a click posts `h264_engine` 1 even where the board's choice is already
    hardware. Without a reason: disabled, a click posts nothing (today's
    behaviour).
D2. (Q11) A refused choice shows the sentence the page owns ("The board did not
    change the H.264 engine", English and Russian), not the server's rejection
    code. Test: the stand-in refuses with `unsupported_setting`; the message is
    the sentence.
D3. (Q12, Q16) The design note says what the plan decided: the rows' limits for
    assistive technology (decision 5); that the report's schema 1 is closed
    (the server refuses a field it does not know, so a new field needs schema
    2 and both sides); the monitor's four test globals and who reads them; and
    the stress tool under "Tests".

## Phase E: the encoder (host; at most 6 hours; gate E)

E1. (Q13) The demand connection skips an overlong line within a bound: at most
    4 KiB in all, and a client that sends more without a newline loses its
    connection. Test: one MiB without a newline, then `kbps 3000`: the
    connection ends, the viewer count drops, the bitrate is not taken; a line
    of 100 bytes and then `kbps 2500` still works (the existing test).
E2. (Q4, decision 6) First confirm. With the tool of A1 and a scratch build
    that logs, for every key-frame decision, the time since the last key frame
    in the time the encode began and in the capture time, the failing runs of
    `on_a_still_picture_a_key_frame_comes_when_it_is_asked_for` show the first
    under two seconds and the second at or over it. The board's receipts of
    gate C5 show the same by other means (25 to 33 of 69 or 70 refresh frames
    of the still picture were delta frames; key frames two or four seconds
    apart). Then the decision is made in capture time: `key_due(last, capture,
    every)` as a function of its own, and the last key frame's time is the
    capture time of the frame that made it (also one the encoder made by
    itself). If the log does not confirm it, the cause is found first and this
    item is rewritten in the review note. Test: the function (exactly two
    seconds apart is due; a millisecond under is not; the gate's refresh
    spacing of 2.0 to 2.03 s is due every time) and the integration test,
    which then passes under load.
E3. (Q4) `a_client_that_does_not_read_loses_its_queue_and_holds_nobody_up`
    (`frames.rs`) paces its publisher by the reading client: the publisher
    never gets more than 16 frames ahead of the last id the reader has taken,
    which the reader publishes in an atomic. The slow client is still never
    read, the assertions keep their meaning, and the bound for "publishing
    waited for a client" stays.
E4. The plan's two timing-sensitive encoder tests (`frame_socket.rs`: a key
    frame within 900 ms of the choice, three to five reports in 3.3 s) passed
    in the review's six runs under load. They are run in gate E and widened
    only where they fail, with the reason in the commit.
- **Gate E**: 30 runs under the tool at one and a half times the cores, no
  failure, of the `frame_socket` tests, of the encoder's library tests (the
  40-run set of A2 repeated) and of E1's test; the two encoder tests the
  review found failing are among them (the server's is gate C's).

## Phase F: documents and the board (at most 8 hours; gate F)

F1. The design note's Record gets "Review fixes": what changed against the
    design and why, the numbers of A2 before and of the gates after. The review
    note gets its status table (a line per finding: fixed with the commit and
    the test, not confirmed, accepted, left). The outcome note of 2026-10-04
    gets a line that points to the review.
F2. `scripts/check.sh` green; the slot (the next sequence above the confirmed
    one, label `0.1.0-alpha.17`) and the encoder built as before, and
    installed with consent: the encoder through `deploy_vp8.sh` (E1 and E2
    changed it), the slot through the update API; receipts as for slots 67 to
    72.
F3. On the board, every check named, each with a receipt:
    a. `GET /api/vm/quality`: fresh answers (the median of the reads), the
       version in the second answer, none slower than 50 ms;
    b. the series of gate C5 with the monitor open and closed: QUIC with the
       engine and with x264 (the frame tap), WebRTC H.264 with either engine,
       VP8. The Basic numbers stay within the bounds of gate C5, and the
       processor time of the web server and the encoder together grows by
       less than 2 % of a core with the monitor open;
    c. the still picture's refresh frames from the frame tap (120 s, QUIC,
       each engine): every one is a key frame and they come two seconds apart
       (E2; today 25 to 33 of 69 or 70 are deltas), at the cost of at most
       0.02 Mbit/s;
    d. the engine switched from the menu five times each way over QUIC and
       over WebRTC, each within 2 s;
    e. a busy engine: `tools/device/ve_device_hold.py` (new: opens the
       engine's device node and holds it for a time, nothing else; copied
       with `scp` and run over SSH as a file, with no viewer watching) holds
       it while a viewer comes in H.264. x264 encodes; the report and the menu
       say why; H.264 HW stays choosable (`engine_busy_device_probe.mjs`, new,
       from the page). After the holder has left, one click on H.264 HW brings
       the engine back within 2 s, and the choice is hardware again at the
       end;
    f. the GUI walks (desktop and mobile) with the monitor closed, open in
       Basic and (desktop) in Extended: no failed request, no page error;
       VP8 and H.264 over WebRTC; MJPEG latency (3 runs; the probe reads SNTP
       every two seconds now); the firewall probe;
    g. the kernel's log: no line more than before; no probe file, process or
       holder left on the board.
- **Gate F**: a. to g. hold, with their numbers.
F4. End state: every administration switch and the operator's settings as
    found (compared field by field with `c5-state-after-20261004.json`; only
    the plan's new fields differ), the engine choice hardware, the slot, the
    encoder and the receipts recorded in the review note and the design note;
    the memory note `blikvm-corrective-status` updated; the branch pushed.

## What the operator should expect

- A browser that cannot run workers gets a one-line note in the QM window
  instead of a page that fails.
- After the engine failed or was busy, the video menu says why and lets H.264
  HW be chosen again.
- A board whose encoder predates the plan of 2026-10-03 shows the one H.264
  entry until `deploy_vp8.sh` has run.
- The server refuses the engine and the WebRTC quality where the video
  controls are off; the page already hid them.
- A still picture gets a key frame every two seconds, as the design said;
  that costs at most 0.02 Mbit/s more on the board's still screen.

## Risks

- Offering the choice only with the report path changes what an older drop-in
  shows; that is the point, and the operator reads it here.
- The stress tool slows the host while it runs; it is bounded, and it stops
  its loops on every exit and on interrupt.
- E2 changes the cadence of key frames for a still picture; gate F counts
  them on the board. A moving picture is unaffected: its key frame still
  follows two seconds of capture time.
- C2's gate on `webrtc_kbps` could meet a client that relied on the old
  behaviour; the tests and probes are searched first, and the setting keeps its
  behaviour if one is found.
- The busy-engine check (F3 e) holds the engine's device node for a few
  seconds; the driver gives it to one opener at a time and the holder asks
  for nothing else. It runs with no viewer watching and the engine choice
  restored.

## Order and operator steps

The review note and the plan are committed first, in that order and
unchanged. Then A, B, C, D, E, each item tested and committed, the gates in
order; then F. Operator: consent for the board steps of F (decision 7), the
word to publish anything (decision 8), and, if wanted, the advisory
database's update and the `time` crate's bump (Q14: a download, asked once).
When the board cannot be reached, all the host work is finished and the device
steps are listed as pending.
