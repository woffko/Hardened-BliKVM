# Work plan: fixes from the code audit of 2026-11-06

Follows `docs/baselines/audit-20261106-code-review.md`, which checked an
external code audit against the code at `e2e1621` and the device's model;
finding names (AUD-01, ...) are that report's. Only the findings that hold
and fit the model are planned; the others are recorded there and left as
they are. This plan's items are N1, N2, ..., so as not to be confused with
the S and Y items of the 2026-10-20 plan.

## Rules

The rules of the earlier plans hold:

- English repository text, and secrets only through `project-memory-exec`.
- One commit per item, starting with a test that fails on the code as it
  is; "not confirmed" when that test passes, and the item is then dropped.
- No new HTTP route and no new capability.
- The two tracks (application slot, system) and their compatibility: the
  slot of this plan runs on beta.10's system (sequence 10, update runtime
  `0.1.0~dev45`) and on this plan's; the system of this plan keeps beta.10's
  slot running.
- `RUST_TEST_THREADS=16 sh scripts/check.sh` before each phase's last
  commit and before every build, with flaky tests named.
- The device rules and the target's rules: consent for every install, no
  probe while the operator uses the board, nothing on the board that changes
  its network, firewall, hostname, SSH, QUIC, video or fan settings, steps
  its clock, or installs or removes PicoClaw. N3, N5, N6, N8 and N12 are
  shown by host tests; the board gets reads and the watching probes.

## Where the work starts

- Branch `wip/corrective-audit-checkpoint` at `e2e1621`; `main` holds the
  same content (merge `582d64f`). 0.1.0-beta.10 is published (tag on
  `3adbe43`).
- The test board runs slot 106 on the system of sequence 10 (update
  runtime `0.1.0~dev45`), RAW floor 11, its settings as left at the release.

## Decisions

Taken as written on 2026-11-06, when the operator asked for the goal on
this plan (1: everything, then the release at the operator's word; 2: N8
fixed; 3: the optional items left out); the operator may still change any
of them before its item is done.

Changed by the operator on 2026-11-07, at gate A: the display button's
hold stays the reviewed 1000 ms after its 50 ms debounce, so N7's press on
the board is a little over a second, not "about 1 s" (a press of about a
second did not wake the panel, one of 1.5 to 2 s woke it once; the vendor's
program wakes after about 0.5 s). The operator chose to accept that over a
shorter hold; a press of about 1.2 s then woke the panel.

1. **Scope and release.** Phases A, Y and T, then a release 0.1.0-beta.11
   (slot 107, update runtime `0.1.0~dev46`, card image, RAW sequence 11) at
   the operator's word. The alternative is phase A alone as a slot, the
   system later.
2. **N8, the fan.** Only root on the board reaches manual fan mode, but it
   is hardware safety and small: fixed (recommended) or left.
3. **Optional items** (end of this plan): left out (recommended) or added.

## Phase A: the application slot (slot 107)

In order of what the operator notices.

- **N1. The page learns of a lost lease** (AUD-05, and AUD-09's residue).
  `web/src/lib/hid-lease.ts`: `HidLeaseSession` takes an `onLost` callback,
  called from both paths of `invalidateLease`. `web/src/hooks/useHidLease.ts`:
  on it, `dropSharedSession()` if that session is still the shared one;
  `ensureSession` treats a session without a live lease as missing; and
  when the QUIC session that carries the lease ends (`leaseTransportAtom ===
  'quic'`), the shared session is dropped and the focus confirmation reset,
  so the next input, `hold()` or agent call takes a new lease. Test
  (`target_agent_reads_contract.mjs`, fails today): after control is taken,
  renew is answered `{applied: false, rejection: "lease_expired"}`; no
  further `target-agent/call` poll is made on the dead lease, and opening the
  Monitors menu sends `acquire` before its call. A second case in
  `quic_input_contract.mjs`: after the gateway stops, the next `hold`
  acquires first.
- **N2. Power and Reset re-confirm, and say a refusal** (AUD-10).
  `web/src/lib/restricted-power.ts`: on a 400 whose message is the focus
  confirmation's, reset the cached confirmation, confirm again and retry
  once. `web/src/hooks/useRestrictedPower.ts`: catch the error and show a
  warning, so a refused press is not silent. Test (a new contract on
  `app_harness.mjs`): one power action applied; the next answered once with
  that 400; the click is retried after `GET`/`POST /api/hid/focus` and
  applied, with no page error; a refusal that stays shows the warning.
- **N3. mDNS and the TLS state leave the owner's lock** (AUD-08).
  `crates/bli-kvm-control-broker/src/v2.rs`: `Mdns`/`MdnsState` and
  `TlsState`/`TlsSet` join `SystemSettings::serves`, their state moves into
  `SystemSettings`, the mDNS switch is noted from there; `tls_settings.rs`:
  `SystemTlsRunner::run` gets the bounded runner of the firewall's processes
  (timeout and output cap) and `write_conf` syncs; `board_status.rs` reads
  through `take(16 KiB + 1)` outside the lock. Test (`server.rs`, after
  `a_slow_system_setting_leaves_hid_leases_answered`): with a restart that
  takes 2 s, an mDNS switch leaves a HID acquire answered in under 200 ms;
  today it waits the 2 s.
- **N4. A switch whose restore failed is restored again** (AUD-11.4).
  `system_switches.rs`: `reconcile` stores the record (and the new image's
  name) only when nothing failed. Test: extend
  `a_unit_that_cannot_be_set_does_not_stop_the_others`: with the refusal
  cleared, a second reconcile restores SSH off; today it follows the record
  with SSH on.
- **N5. An IPv4 change back ends the confirmation window** (AUD-11.1).
  `ipv4_settings.rs` `set`: arm only when the addressing changed; inside the
  window, a change back removes the device from the pending revert and its
  journal, as Wi-Fi does. Test: set address A with DNS 1, then B, then A with
  DNS 2 in a 2 s window: no window is left, the journal is empty and DNS 2 is
  still set after 4 s.
- **N6. No IPv4 or Wi-Fi edit while the boot revert is retried**
  (AUD-11.2). `ipv4_settings.rs`, `network_settings.rs`: a reconciling flag
  set while the retry thread runs; IPv4 and Wi-Fi writes answer
  `PendingUnreconciled` meanwhile, and the thread's journal writes keep
  records it did not make. Test: a journal for eth0 and a runner that fails
  once; `reconcile()` then `set` is refused; today it applies and is undone
  about 5 s later.
- **N7. The LED and the display button on a fast tick** (AUD-06).
  `crates/bli-kvm-boardd/src/lib.rs`: a 20 ms tick drives the LED and the
  button; the thermal, fan and display work run when the sample interval is
  due. `button.rs`: the policy gets the raw level and starts from the
  inactive one. Tests: the loop body as `board_tick(now, due)` with mock
  lines and the example configuration: one lit window of 100 ± 20 ms per
  2 s period, and one wake after a 1.1 s hold (today none); an active-low
  button wakes once on a press (today never). On the board: the operator
  presses the display button for a little over a second (the hold is
  1000 ms after a 50 ms debounce; changed on 2026-11-07, see Decisions) and
  looks at the LED.
- **N8. Manual fan mode through the supervisor** (AUD-02, decision 2).
  `crates/bli-kvm-fan-supervisor`: `apply_manual(command)`, refused unless
  the supervisor is active, records what it applied; `boardd/server.rs`: the
  manual duty starts at the maximum, manual writes go through it, and back
  in Auto the next sample writes the policy's command. Test (`server.rs`):
  Manual straight after start applies no `Off`; manual 20, Auto, one sample
  at 80 °C applies 100 (today it stays 20). Not tried on the board.
- **N9. Focus bindings kept again** (AUD-03). `crates/bli-kvm-server/src/hid.rs`:
  `extend_bindings` becomes an upsert that adds a swept entry back within
  the store's bound; it is used where a live lease restores the bindings, on
  a report and on a renew. Test: remembered bindings, swept, kept again with
  a new expiry: `bindings` returns them (today `None`).
- **N10. PicoClaw's lock knows its connection** (AUD-04).
  `crates/bli-kvm-server/src/picoclaw/mod.rs`, `gateway.rs`: the lock keeps a
  connection number from a counter; `Active` and the release guard carry it;
  a relay's teardown, renew and watchdog act only on their own number; the
  session's DELETE still releases by session. Test (`mod.rs`): acquire,
  release as DELETE does, reopen the same session, drop the old guard: the
  lock is still held (today it is gone).
- **N11. The agent's layout key forgotten when the agent leaves**
  (AUD-16). `web/src/hooks/useScreenArea.ts`, with its decision moved into
  `web/src/lib/screen-area.ts`: the key is cleared when the agent's layout
  goes and after a failed write. Test (`tools/test_screen_area.mjs`): layout
  M written, agent gone, kept area K, agent back with M: M is written again
  (today skipped).

Then slot 107 is built, installed at the operator's word, and the watching
probes run (GUI walks at both sizes, WebRTC H.264 and VP8, QUIC video, touch
reach).

## Phase Y: the system (update runtime dev46, encoder, image, RAW)

- **N12. PicoClaw's onboarding runs a checked copy** (AUD-01).
  `crates/bli-kvm-admin-helper/src/picoclaw.rs`: `onboard` first copies the
  installed binary with `picoclaw_exec::prepare` (size and SHA-256 compiled
  in) into a directory of the unit's private `/tmp`, and runs that copy;
  `main.rs` passes the pinned digest and size; the comment that the helper
  "starts nothing" is corrected. No unit changes. Test: a stand-in binary
  whose bytes differ from the pin makes `prepare` fail and writes no
  `config.json` (today it runs); a matching stand-in that writes `$0` shows
  it ran from the private copy.
- **N13. A client that leaves is no failure** (AUD-07).
  `crates/bli-kvm-update-supervisor/src/status_socket.rs`: no request byte
  before end of file, and a broken pipe or reset while answering, end the
  connection with success; every other error stays exit 96. Tests: a client
  that connects and leaves, and one that sends a frame and leaves before
  the answer, both give `Ok` (today `UnexpectedEof` and `BrokenPipe`). On the
  board after the GUI walks: no `hardened-blikvm-update-status@` instance in
  `systemctl --failed`.
- **N14. A missed forced key frame is retried at once** (AUD-12, encoder).
  `crates/bli-kvm-video-encoder/src/main.rs`, `gate.rs`: a shared retry flag
  set when a forced key could not be made; the reader lets the next capture
  through and clears it. Test (the reader's decision in `gate.rs`): a still
  frame under a new generation, the key missed 60 ms later, the frame at
  300 ms is let through (today `Unchanged` until the 2 s refresh).
- **N15. A group given up forgets its unreceived frames** (AUD-12,
  gateway). `crates/bli-kvm-quic-gateway/src/video.rs`: `unreceived.forget()`
  after both `give_up` calls. Test: one frame sent, the group given up on a
  new picture: the lag is zero (today 1 s).

Then update runtime `0.1.0~dev46` (`test_dev46_...`), the encoder through
`deploy_vp8.sh`, all at the operator's word; with decision 1, the card image
and RAW sequence 11 follow the release recipe.

## Phase T: host tools

- **N16. The card image's cleanup removes dotfiles** (AUD-15).
  `tools/image/build_sd_image.sh`: each `dir/*` removal becomes
  `find dir -mindepth 1 -delete`, and the build fails if
  `/var/lib/hardened-blikvm` is not empty afterwards. Test: the cleanup on a
  staging tree holding `.account-x` leaves the directory empty (today the
  file stays).
- **N17. The restore tool claims the root partition** (AUD-14).
  `tools/image/restore_raw_backup.py`: a block device is opened
  `O_RDWR | O_EXCL`, and `EBUSY` refuses with "the root partition is mounted
  or in use". Test (`tools/test_restore_raw_backup.py`): with `os.open`
  raising `EBUSY` for `O_EXCL`, the tool refuses and the root is unchanged
  (today it is written).

## Phase W: the record

The note gets each item's commit and test, what the board showed, and what
was left; with decision 1, the release follows the recipe and its record.

## Optional

Not planned unless decision 3 adds them:

- AUD-11.3's residue: the firewall mode written as `.json.new` and synced
  before a confirmation, renamed only once it succeeds (a power loss
  between them keeps the previous mode).
- AUD-17.1: resolv.conf's previous contents written back when a write fails
  (network writes are off on the board).
- AUD-17.3: a TLS switch that would leave port 80's redirect as it is is
  refused (the TLS switch is off on the board).
- AUD-14's `debugfs` operands quoted.

## Size

Seventeen items: eleven in the slot (three of them in the page), four in the
system, two in the host tools, each with its test. One slot, one system
build, and with decision 1 an image and a RAW bundle.

## Operator steps

- The decisions above.
- Consent for slot 107, for update runtime dev46 and the encoder, and with
  decision 1 for the image fetch (pages closed) and the RAW install.
- A press of the display button of a little over a second after slot 107
  (N7).
- The release, at the operator's word.
