# Work plan: what the code audit of 2026-11-06 left

Follows `docs/baselines/work-plan-20261106-code-fixes.md`, whose items N1 to
N17 are done and released in 0.1.0-beta.11
(`docs/baselines/audit-20261106-code-review.md`). This plan takes what that
plan left out or met on the way: its optional items, the one RustSec
advisory, and the page contracts that failed now and then while the release
was checked. Its items are R1, R2, ..., so as not to be confused with N1 to
N17.

## Rules

The rules of the earlier plans hold:

- English repository text, and secrets only through `project-memory-exec`
  (never a program on its standard input). On the board, SSH uses the GUI
  record (root's password follows the web password).
- One commit per item, starting with a test that fails on the code as it
  is; "not confirmed" when that test passes, and the item is then dropped.
  A flaky contract counts as failing when a stated way of reproducing it
  fails on the code as it is (a forced delay, a stubbed clock, a stated
  number of runs).
- No new HTTP route and no new capability.
- The slot of this plan runs on beta.11's system (sequence 11, update
  runtime `0.1.0~dev46`); no system part changes.
- `RUST_TEST_THREADS=16 sh scripts/check.sh` before each phase's last
  commit and before every build, with flaky tests named and run again in
  full.
- The device rules and the target's rules: consent for every install, no
  probe while the operator uses the board, nothing on the board that
  changes its network, firewall, hostname, SSH, QUIC, video or fan
  settings, steps its clock, or installs or removes PicoClaw. R1 to R3 are
  shown by host tests; the board gets reads and the watching probes.

## Where the work starts

- Branch `wip/corrective-audit-checkpoint` at `aa2fabc`; `main` holds the
  same content (merge `a5d5d49`). 0.1.0-beta.11 is published (tag on
  `e9e62ea`).
- The test board runs slot 107 on the system of sequence 11 (update runtime
  `0.1.0~dev46`), RAW floor 12, its settings as found at the release.

## Decisions

1. **Scope and release.** Phases A, T and B, then an application release
   0.1.0-beta.12 (slot 108 only: no system, image or RAW bundle) at the
   operator's word (recommended). The alternative is the code alone, the
   slot with the next release.
2. **N8 and N12 on the board.** Left (recommended): they change the fan's
   mode and install PicoClaw, which the device rules keep off the board. The
   alternative is a trial at the operator's word that puts both back.
3. **The SSH record.** The record `37c7917d` in project-memory is refused by
   the board; the operator replaces or deletes it. Not part of the code.

## Phase A: the board owner (slot 108)

- **R1. The firewall mode renamed into place once confirmed** (AUD-11.3's
  residue). `crates/bli-kvm-control-broker/src/firewall_runtime.rs`: the mode
  to remember is written and synced as `<path>.new` before the change, as
  today's write is (so a mode that cannot be kept is still refused), and
  renamed over `<path>` only once the transaction succeeded; a failure
  removes `.new`, and a `.new` left by a power loss is removed at the start
  without being read. Test: a transaction double that reads the persisted
  file during `confirm` finds the previous mode (today the new one); after
  success the file holds the new mode; a refused confirmation leaves the
  previous mode and no `.new`.
- **R2. resolv.conf put back when its write fails** (AUD-17.1).
  `crates/bli-kvm-control-broker/src/network_write.rs`: the previous
  contents are read before the file is truncated and written back when the
  write, the sync or the read-back fails (the sandbox grants the file, not
  its directory, so no rename). The write goes through a seam that a test
  can fail. Test: a write that fails after the truncation leaves the
  previous contents (today an empty or partial file).
- **R3. No TLS switch that leaves port 80 redirecting** (AUD-17.3).
  `crates/bli-kvm-control-broker/src/tls_settings.rs`: switching TLS off
  renders the configuration as today, then checks the port 80 server of the
  result; a redirect to HTTPS that is outside the owner's markers (an older
  or hand-written one) refuses the switch, nothing is written, and the
  reason is said. `bli-kvm-core::nginx_redirect` is unchanged (the admin
  helper and the RAW update use it). Test: a configuration with an
  unmarked `return 301 https://...` in port 80's server: today the switch
  writes it and leaves HTTP redirecting to a TLS that is off; after, it is
  refused; the marked block still switches as before.

Then slot 108 is built, installed at the operator's word, and the watching
probes run (GUI walks at both sizes, WebRTC H.264 and VP8, QUIC video, touch
reach). R1 to R3 change nothing on the board unless the firewall, the
network or TLS is changed there, which this plan does not do.

## Phase T: host tools, dependencies and contracts

- **R4. The restore tool's `debugfs` operands quoted** (AUD-14's residue).
  `tools/image/restore_raw_backup.py`: both paths of `dump` in double
  quotes, a path holding a quote or a newline refused. Test
  (`tools/test_restore_raw_backup.py`): with `TMPDIR` under a directory
  whose name has a space, an interrupted write is undone (today "the data
  partition holds no apply.json").
- **R5. `ctutils` 0.4.8** (RUSTSEC-2026-0385). `cargo update -p ctutils
  --precise 0.4.8 --offline` (its dependencies, `cmov` 0.5.4 and `subtle`
  2.6.1, are in the lock); no program on the board carries it. Check:
  `cargo audit --no-fetch --deny warnings` passes (today it reports the
  advisory); `docs/third-party-rust.md` gets the scan.
- **R6. The contracts' waits on a steady clock.** Fourteen contracts under
  `tools/playwright` time their `eventually` by `Date.now()`, which this
  host's wall clock steps by about 3.6 s every 32 s, so a wait can end
  before a timer it waits for (`target_agent_reads_contract.mjs:431` failed
  so, 1 in 10 alone). A shared `tools/playwright/eventually.mjs` times by
  `performance.now()`, and the fourteen use it. Test
  (`tools/test_eventually.mjs`): with `Date.now` stubbed to step 3.6 s, the
  old helper gives up before a condition that a 3 s timer makes true; the
  shared one waits for it.
- **R7. The screen area contract's cleared layout**
  (`screen_area_contract.mjs:217`, "the whole desktop again", seen on
  2026-10-21 and 2026-11-07): a point is sent before the page has dropped
  the cleared layout. Reproduced by delaying the clearing request's answer;
  fixed in the contract (wait for the page's own sign of the cleared
  layout) or, if the page maps with a layout it no longer has, in the page.
  Test: the contract with that delay fails today and passes after.
- **R8. The screen enlargement contract's keys**
  (`screen_enlargement_contract.mjs:343`, "keys typed into the menu were
  sent on", 2 against 1, once in about eight full runs). The cause is to be
  found first: the reports of a failing run logged with their times (a
  second key-down of the key pressed over the picture, or a key typed into
  the menu's field before it had the focus). Fixed where the cause is, with
  a reproduction that fails before; "not confirmed" if 40 runs alone and
  two full runs show nothing.
- **R9. The QUIC input contract's page** (`quic_input_contract.mjs:379`, the
  page's settings icon not visible in 15 s, once). As R8: the cause found
  first (the page's load, the fixture's gateway between cases), then fixed
  with a reproduction, or "not confirmed".
- **R10. The server's logout test under load**
  (`logout_upload_returns_unauthorized_over_real_http1`, a 10 s bound that
  timed out at load 47 and 57; 6 to 7 s alone). The bound also covers the
  account's setup and login, with their password hashing; what takes the
  time is measured first, and the bound is moved to the HTTP exchange it is
  about. Test: with the setup
  slowed (a stated delay before it), the test fails today and passes after.

## Phase B and the record

Gate B: slot 108 built from a clean tree after a green check, installed at
the operator's word, the state as found but for the slot, the watching
probes passing. The review note of this plan
(`docs/baselines/audit-20261107-remaining.md`) gets each item's commit and
test and what the board showed; a full `scripts/check.sh` passes on the
final commit in one run. With decision 1, the release 0.1.0-beta.12 (the
application only) follows the release recipe at the operator's word, and
the branch is merged into `main` by a pull request when the operator asks.

## Size

Ten items: three in the slot, seven in the host tools, the lock and the
contracts. One slot, and with decision 1 an application release.

## Operator steps

- The decisions above, and the SSH record (decision 3).
- Consent for slot 108's install, and the release and the merge at the
  operator's word.
