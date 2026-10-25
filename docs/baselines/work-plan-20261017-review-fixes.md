# Work plan: fixes for the review of 0.1.0-beta.7 (2026-10-17)

Follows `docs/baselines/audit-20261017-review.md`; item names (R1, L1, U1,
...) refer to its findings. The review looked at the release 0.1.0-beta.7,
whose work is `docs/baselines/work-plan-20261014-target-agent.md`, recorded
in `docs/baselines/audit-20261014-target-agent.md`, with its design in
`docs/design/target-agent-v1.md`.

## Rules

As in the earlier plans: English repository text, secrets only through
`project-memory-exec` (never in arguments, logs or receipts, and never a
program on its standard input under it), every change shipped as a signed
slot and verified on the test board, the design and audit documents updated
with evidence under `evidence/private`, no local path or lab address in a
tracked file. For this plan:

- One commit per item. An item starts with its test, run against the code as
  it is: a test that fails confirms the item, and the fix makes it pass; the
  commit names the test. An item whose test passes on the code as it is, or
  that cannot be shown on the host, is recorded in the review note as "not
  confirmed" (with what was tried) and left unchanged, unless the item says
  how it is shown otherwise.
- No new HTTP route (`tools/test_plan_route_drift.py` pins the count) and no
  new capability. Protocol changes only where an item names them: the agent
  protocol keeps version 1 (a field added with a default, a refusal through
  the existing Failed message); the owner's control protocol may gain a
  field in the agent's state.
- An item's own tests run before its commit; the full
  `RUST_TEST_THREADS=16 sh scripts/check.sh` runs before the last commit of
  each phase and before every slot build. A failure under the shared host's
  load is run again and the flaky test is named in the review note.
- The Windows agent: its portable part (`names.rs`, `client.rs`) is tested
  on Linux; what calls Windows is kept small, linted with
  `cargo xwin clippy` and built twice to the same bytes, as `check.sh` does.
  A decision that needs Windows' behaviour is shown on the target at phase F.
- Device rules, as in the plan of 2026-10-14: the HID lease is never taken,
  no input is sent to the target, no ATX pulse, logins are never failed, the
  operator's media are left alone, and the settings, switches, firewall mode,
  video settings and engine choice end as found; the "Target agent" switch
  ends as the operator leaves it. Probes that only watch may run on the board;
  `quic_input_device_probe.mjs` takes the lease and is not run there.
- The target is the operator's own computer: the operator starts every build
  of the agent there and makes every check of it; checks from the
  development host on the target (WSL) only read, never change, and are made
  only because the operator allowed them ("это всё на хосте же"). Never ask
  for a reply while something is shown full screen on the target.
- Consent: every slot install happens at the operator's word. A slot install
  the classifier denies stops the work and is reported, never worked around.
  Nothing is published unless the operator says so.
- Phases end in gates; a missed gate or a spent time box ends the plan there
  with the findings recorded, and what was finished stays.

## Where the work starts

- Branch `wip/corrective-audit-checkpoint` at the commit of this plan,
  pushed up to `478bad9`; `check.sh` green on `478bad9`.
- The test board runs slot 95 (`0.1.0-beta.7`, source `8dfb152`) on beta.4's
  system, the agent's switch on, the operator's image on LUN 0 and the
  agent's drive on LUN 1.
- The target runs the agent from the board's drive; its sign-in copy is
  slot 92's agent (`6e50a26`), which R3 keeps from being replaced.

## Decisions

Defaults; the operator may change one before the work starts. A decision
changed in the work is changed in `docs/design/target-agent-v1.md` with the
reason.

1. (R3) The agent compares itself with the board: the board's hello names
   its commit. An agent whose commit is not the board's says so in its menu
   and tooltip, and the page says so too (the agent's state gains the
   board's commit; no new route). An agent started while another holds the
   session's mutex asks that one to leave when its own commit differs, takes
   its place, and refreshes the sign-in copy when the sign-in start is on;
   one of the same commit exits as now. "Start the agent on the target" is
   offered also while an agent of another commit is connected, as "Update
   the agent on the target". No program is fetched over a network: the
   newcomer is the one the operator started, from the board's drive or
   elsewhere.
2. (R12) The agent serves the board only while its session is the console's
   active session: in another session it closes the collection, its menu
   says why, and it opens the collection again when its session becomes the
   console's.
3. (R11) The page's calls that change the monitors wait up to 10 s (the
   owner's own limit); reads keep 3 s. The Keep dialog says how long the
   change took, from the call's round trip; the operator reads it out at
   phase F for the record.
4. (R7) Keep takes the lease first, like the other calls, and its dialog
   stays until the agent answers; a failure is said in the dialog, which
   keeps counting down, and Keep may be pressed again. The agent answers a
   Keep with nothing pending with a failure ("nothing is waiting to be
   kept"). "Don't keep" stays as it is: the agent puts the monitors back
   when the time is up.
5. (R8) A rebuild while the target has ejected LUN 0's medium keeps LUN 0
   removable and empty and the drive on LUN 1; an owner's stop removes LUN 1
   whatever else fails, so that an older owner can take the gadget apart.
6. (R2) One lock covers a change's read, apply and claim of the revert, and
   the timer's check and apply.
7. (R13) One predicate in the probes' shared helper says what a page that
   only watches may ask under `/api/hid`; every such probe uses it.
8. Lesser findings: L1, L2, L7, L8, L9, L10 and L13 are fixed; L3, L4, L5,
   L6, L11 and L12 only within phase E's time box, and only where tests
   already cover the code they move.
9. Not in this plan: a review of the rest of the branch against `main`
   (it is a plan of its own if the operator wants it), the sign-in start
   across a restart of the target (gate 5 of the plan of 2026-10-14; asked
   once at phase F, the operator may skip it again), and a release (only at
   the operator's word).

## Phase A: the start (host; at most 1 hour)

A1. `check.sh` green at the start (it was on `478bad9`); the review's notes
    in `evidence/private/review-20261017/`.
A2. Each item's test is written in the item's own commit (phases B to E);
    phase A only orders the work and fills the review note's table with one
    row per item.

## Phase B: the board owner and the gadget (host; at most 6 hours; gate B)

B1. (R4) A failed switch request restores the channel as well as the
    switch: `channel.want(was)` on the error path. Test (owner, with a
    gadget that fails `set_agent`): after the failure the state is
    `enabled == wanted` and a call is no longer refused as InvalidState once
    the gadget is well again.
B2. (R5) `set_agent()` restores its flag when a step fails. Test (executor
    on a fake configfs): a failed turn-on with no gadget leaves the flag off,
    and the next cold build has no agent collection; a failed turn-off, the
    reverse.
B3. (R8, decision 5) An empty LUN 0 is a selection of its own: the rebuild
    keeps it removable and empty beside the drive; the owner's stop removes
    LUN 1 whatever else fails. Test (fake configfs): no recorded selection,
    `lun.0/file` empty: adding and removing the drive succeed and LUN 0
    stays empty; a stop with a failing rebuild still removes `lun.1`.
B4. (U1) Shown or refuted with a test: a rebuild for the switch or HID-only
    mode after the target ejected a recorded medium. If shown, the rebuild
    keeps the medium out (the selection follows the LUN's state).
B5. (U2) Shown or refuted with a test: a mass-storage change that fails
    after its unbind leaves the gadget unbound. If shown, the gadget is bound
    again on the error path; the comment of L1 is corrected either way.
B6. (R10) A Refused reason from the target is cut and cleaned as a Failed
    one is (control characters out, at most 160 characters, never empty).
    Test (owner): an empty, a multi-line and a 200-character reason; the
    state encodes after each.
B7. (R14) A call dropped while the device is closed, or by a pause, fails
    at once (NotConnected) rather than waiting out its timeout. Test
    (channel): a call sent while closed, and one queued when a pause comes,
    end within 100 ms.
B8. (L1) The lazy `ensure_hid_gadget` of a HID request pauses and resumes
    the channel like every other gadget change. Test (owner): with the
    channel open and the gadget's directory gone, a lease request rebuilds
    and the channel reopens.
B9. (L10) The call's lease is checked with the shared validators, and a bad
    lease is refused with the lease's words. Test (core): a call with a
    zero lease id is refused as a lease, not a timeout.
B10. (L13) The absolute mouse's descriptor is read once per gadget change,
    not per report. Test (HID transition): reports after a switch use the
    new descriptor; reports between changes read configfs once.
- **Gate B**: B1 to B10's tests failed before their fixes (or the items are
  recorded as not confirmed) and pass after; the owner's, media and HID
  tests pass; `check.sh` green.

## Phase C: the Windows agent (host; at most 6 hours; gate C)

C1. (R1) The clipboard's text is read within the size Windows gives for the
    block (`GlobalSize`), and a block without a zero ends at its end. The
    scan is a portable function over a slice. Test (portable): a slice with
    no zero, one with a zero, one longer than the protocol's bound; the
    Windows part passes the slice of the block's size.
C2. (R2, decision 6) The revert's bookkeeping becomes a portable state with
    one lock around a change and around the timer. Test (portable): a timer
    that fires while a second change is applied leaves the first undo point
    (the earliest configuration) to be put back.
C3. (R3, decision 1) The agent with another commit than the board's says so;
    a newcomer of another commit asks the running agent to leave and takes
    its place, and refreshes the sign-in copy when the sign-in start is on.
    Test (portable): the decisions (same commit: exit; other commit: replace;
    sign-in on and running copy not the kept one: refresh) and the words.
C4. (R7, decision 4) `keep_displays` with nothing pending answers a failure.
    Test (portable revert state of C2).
C5. (R12, decision 2) The agent serves only from the console's active
    session. Test (portable): the decision from the session ids and states;
    the Windows part (session notifications) is linted and shown at phase F.
C6. (L2) The EDID fallback marks no monitor when more than one matches, and
    does not run when a valid choice is kept. Test (portable).
C7. (L8, L9) The board's monitor is resolved once per configuration read,
    by the chosen id (after an extend, again), and a change uses a copy of
    the configuration it read rather than reading it again. Test (portable
    where the matching is; the rest linted).
C8. (L11) The frame count without a vector. Test: the protocol's tests.
- **Gate C**: the portable tests pass; `cargo xwin clippy` clean; the agent
  built twice to the same bytes; `check.sh` green.

## Phase D: the server, the page and the probes (host; at most 6 hours; gate D)

D1. (R3) The agent's state names the board's commit beside the agent's; the
    page says when they differ and offers "Update the agent on the target".
    Test (contract with the played board): an agent of another commit shows
    the line and the offer; the same commit shows neither.
D2. (R6) The on-screen keyboard's layout sync takes the lease first, says a
    failure, and runs again when control is taken. Test (contract): a first
    `set_layout` refused with -409, then control taken: the switch is asked
    again before the first key.
D3. (R7, decision 4) Keep takes the lease, stays until answered, says a
    failure and may be pressed again. Test (contract): `keep_displays`
    refused once: the dialog stays with the reason and counts on; a second
    Keep succeeds and closes it.
D4. (R9) "Start the agent" waits until Win+R has been sent before its
    second for the Run box. Test (contract): with the first lease report
    held back 800 ms, the first character of the line comes at least a
    second after Win+R was sent.
D5. (R11, decision 3) Monitors' changes wait up to 10 s; the Keep dialog
    says the change's duration. Test (server: the owner receives a 10 s
    timeout for `set_displays` and `keep_displays`, 3 s for a read;
    contract: a change answered after 4 s opens the dialog).
D6. (R15) A read of the layouts does not overwrite a layout switch made
    after the read began. Test (contract or unit): a keyboard answer that
    arrives after a switch leaves the switch's layout in the state.
D7. (R13, decision 7) One predicate for a watching page's `/api/hid`
    requests in the probes' shared helper, used by all nine probes. Test
    (node): the predicate allows only `GET /api/hid/screen-area`; the
    probes pass `node --check`.
D8. (L7) The settings' agent section reads `targetAgentAtom`; its own 5 s
    read goes. Test: the existing contracts.
- **Gate D**: D1 to D8's tests failed before (or the items are recorded as
  not confirmed) and pass after; the contracts of the plan of 2026-10-14
  pass; `tsc`, `eslint` and `prettier` clean; `check.sh` green.

## Phase E: cleanups (host; at most 3 hours)

E1 to E6. (L3, L4, L5, L6, L11's neighbours, L12) Each a commit that moves
    code without changing behaviour, under tests that already cover it; one
    that is not covered is left and recorded.

## Phase F: the slot and the target (gate F)

F1. `check.sh` green; the signed slot built from a clean tree; the agent
    also built where the operator starts the development builds from.
F2. The slot installed through the update API at the operator's word, and
    verified on the board: the services, the agent's state, the drive on LUN
    1 beside the operator's image, the switch as the operator left it; the
    watching probes (GUI walk at a desktop's and a phone's size, WebRTC
    H.264 and VP8, QUIC video, touch reach) pass.
F3. The operator's checks on the target, recorded as the operator's words:
    - R3: the board's agent started while the old sign-in copy runs takes its
      place, and the sign-in copy is the board's agent afterwards (read on
      the target from the host, read only);
    - R6: the on-screen keyboard's Russian after control was off and on;
    - R7: Keep after a focus change keeps the change (or says why not);
    - R9: "Start the agent" connects within 10 s;
    - R11: the durations the Keep dialog shows for each mode change;
    - R12 (at the operator's choice; needs a second Windows user): an agent
      in a switched-away session lets go, the console's takes the board;
    - the sign-in start across a restart (asked once; may be skipped).
- **Gate F**: the slot installed and verified; F3's checks recorded.

## Phase G: the record (at most 2 hours)

G1. The review note's table: every item with its commit and test, or why it
    was left; the flaky tests named.
G2. `docs/design/target-agent-v1.md` for decisions 1 to 7; the README if a
    visible behaviour changed.
G3. `check.sh` green on the final commit; the board as found (the state
    compared with the start, as in the plan of 2026-10-14); a release only at
    the operator's word.

## Operator steps

- Change a decision before the work starts, if wanted.
- Agree to each slot install (phase F).
- Run the agent's builds on the target and make phase F's checks.
- Say whether R12's check and the restart check are made, and whether and
  when to publish.
