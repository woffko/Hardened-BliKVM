# Code review of 2026-09-29 (HEAD 694c301)

A review of the code for bugs, made after the first alpha; the review itself
fixed nothing. The findings are numbered R1-R23 by severity and are the input
of `docs/baselines/work-plan-20260929-review-fixes.md`.

## Method and coverage

- clippy over the workspace (all features, no test code) with the lints for
  casts, indexing, string slicing, locks held in scrutinees, ignored results,
  unsafe blocks and time subtraction; about 1000 hits were triaged;
- `cargo audit` (advisory database of 2026-09-25) and `pnpm audit --prod`:
  no advisories;
- line-by-line reading of the server, the administration helper, the video
  encoder (Rust and the x264/libvpx wrappers), the board owner's request path,
  the HID, ATX, network, Wi-Fi and TLS code, the RAW update step, the image
  builder, the deployment scripts, the unit files and the core of the web
  client. The update runtime, manager, supervisor and stager were sampled; the
  diagnostic crates, `tools/stage_device_*.py` and most settings pages were
  not read;
- throw-away tests in a copy of the workspace: reproductions (marked [T]) and
  random-input tests of every text parser found (several hundred thousand
  inputs each, a JPEG decoder run on damaged frames in a release build): no
  panic in a parser of ours;
- read-only probes of the test board: login time, route latencies with up to
  six concurrent slow reads, memory.

Marks: [T] reproduced by a test or a measurement, [C] derived from the code.
The SD image has not been booted from a card since the layout change (the
operator skipped the spare-card boot).

## High

R1. [T] Login: unauthenticated memory exhaustion. `POST /api/auth/login`
takes a JSON body up to axum's default 2 MiB, and the rate limiter stores
`"<address>:<username>"` for every name for at least the lockout time (600 s).
A wrong user name costs no Argon2 (13 ms), so the requests are cheap. Test: 2000
logins with 64 KiB names keep 131 MB of keys. The board has 1 GB (about 620 MB
free) and the server unit has no `MemoryMax`; a few hundred requests exhaust
it (`auth/routes.rs`, `auth/rate_limit.rs`).

## Medium

R2. [C] The lockout is shared by all clients. nginx connects from loopback and
the server keys the limiter on the connection's address, so the key is
`127.0.0.1:<user>` for everyone; nginx's `X-Forwarded-For` is ignored. Five
wrong passwords for the default `admin` name stop every login for 600 s, and
the right password is refused while it lasts because the check comes first.
Not run on the board, where it would lock the operator out.

R3. [T] Login timing and blocking. A right name takes about 205 ms on the
board (Argon2), a wrong one 13 ms, so the account name can be found by timing.
The hash runs on a runtime thread with the limiter's mutex held, so logins
queue behind each other.

R4. [C] An open terminal survives switching the Terminal feature off. The
switch is checked when a terminal opens (server and helper) and never
afterwards; an open root shell continues until the session expires. The
PicoClaw relay checks its switch every second (`terminal.rs`).

R5. [T] A foreground script that leaves a background child hangs the helper.
`run_foreground` kills only the interpreter and joins its output readers,
which end when every holder of the pipe has exited: `sleep 4 &` keeps the run
4.003 s, `sleep 200 &` would keep it 200 s against the 60 s limit while the
server gives up at 75 s and the root helper stays. Descendants are never
killed (`admin-helper/src/scripts.rs`).

R6. [T] The HTTPS setting misreports and does nothing. On the board
`GET /api/vm/tls` says `enabled:false` while port 80 answers 308 to HTTPS,
because the shipped nginx configuration redirects unconditionally; the switch
only inserts or removes a marker block beside that redirect.

R7. [C] An unconfirmed network change is not reverted after a restart. The 60 s
(Wi-Fi main connection) and 120 s (IPv4 addressing) reverts are sleeping
threads; an owner restart or a reboot inside the window leaves the persistent
NetworkManager change, which is the case the revert exists for. The firewall
keeps a journal and reconciles; the network settings do not.

R8. [C] An ATX pulse holds the owner's shared lock. `AtxExecutor::Gpio` runs
the pulse inside the async task and the runner sleeps for the hold time (5 s
for a long press, 0.5 s otherwise) while `handle_request` holds the state lock.
HID, media and settings wait; the server's 5 s client timeout expires around
the end, and the web client turns remote control off after an error.

R9. [C] The HID release ledger costs four fsyncs per key or button event: it is
stored before and after each change of the held state (temporary file, fsync,
rename, directory fsync) on the SD card, under the owner's lock. Mouse moves
are not affected. Typing bursts and the paste menu (one request per press and
release) are limited by it, and the card wears. Not measured on the board
(needs the HID lease).

R10. [C] Catalog reconciliation deletes entries when the store is absent.
Every catalog read drops the entries whose file is not a regular file; with
the store not mounted (the mount is `nofail` and not ordered before the owner
and the server) or a failing read, all entries go, the files stay and nothing
can register them again (`media.rs`).

R11. [C] The image sanitization is a list of known secrets. The PicoClaw state
(model API key, gateway token, chats), the stored scripts and the Tailscale and
download state are neither removed nor searched for, and the PEM and
passphrase checks would not see an API key. The staged root of the first alpha
had none of them (`tools/image/build_sd_image.sh`).

## Low

R12. [T] Enabling the session lock after it was off logs the caller out:
`last_seen` is not refreshed while the lock is off, so the caller's next
request counts as idle since the login. Reachable only through the API (the
settings offer 5-60 minutes).

R13. [C] Session and lock hygiene. The lock value is applied before it is
stored (a failed store answers 503 but keeps the new value); concurrent
writers share one temporary file name; expired sessions and their activity
entries are never removed; the clones that open streams hold keep the old
expiry after `retime`.

R14. [C] Upload leftovers. The temporary name `.name.N.upload` restarts its
counter with the process and nothing removes a stale one, so after an
interrupted upload the same name fails with "not writable" and the partial file
stays; the attached-image check is made only at the start, before the rename
over the target; file writes and the final fsync run on the async worker.

R15. [C] A capture size change rebuilds the encoder with the initial RTP
sequence number and the same SSRC (`video-encoder/src/main.rs`).

R16. [C] PicoClaw: the assistant lock is taken before the WebSocket upgrade; an
upgrade that never runs leaves the lock for 30 minutes and drops the
operator's keyboard and mouse meanwhile (`picoclaw/gateway.rs`).

R17. [C] The remote download's status poll takes a shared `flock` on the file
the helper locks exclusively without waiting, so a start that races a poll can
be refused with "download in progress".

R18. [C] The application and system update uploads answer a refusal before the
body is read (nginx does not buffer them): the browser may see a dropped
connection instead of the reason, as the media upload did before its check
route.

R19. [C] The legacy image dialog (`images.tsx`, not used with hardened media)
treats `applied:false` as success.

R20. [C] The default gateway is the first route in `system_time.rs` and the
lowest-metric route in `network.rs`.

R21. [C] `scripts/build-x264.sh` and `scripts/build-libvpx.sh` run
`rm -rf -- "$output"` on any absolute path.

R22. [C] Hardening gaps: no Content-Security-Policy; the release profile has
`panic = "abort"` without overflow checks (a panic ends the process, an
overflow wraps silently); the server never sends the `capture-status` events
that the web client's "no signal" overlay waits for; zune-jpeg 0.4.21 panics on
some damaged JPEGs in a debug build (release: none in 60000).

R23. [T] `native_ipc::tests::control_payload_cannot_renew_the_prefix_deadline`
failed once in a full `check.sh` at a load average of 35: its fixture unwraps a
write to a peer that has already left. Run alone: 0 failures in 60.

## Checked, no action

- The owner's connection limit (4) did not starve a cheap owner request: with
  four to six concurrent slow settings reads its 90th percentile stayed at
  23-38 ms (maximum 341 ms).
- A suspected stall of the owner through the board daemon's status read was
  not confirmed by any measurement. One-second stalls seen in the first probes
  also hit `/api/health`, which touches nothing, and did not reproduce with a
  raw-socket probe (3000 requests): network or client noise.
- Parsers: the multipart part parser, the JPEG readers, the route, nmcli,
  Tailscale and MAC parsers, the download URL and curl parsers, the RTP
  packetizers (a round-trip test of 2000 random access units) and the MJPEG
  reader never panicked.
- Dependencies and repository: no advisory in the Rust or the web production
  dependencies; no private key, lab address or hard-coded secret in tracked
  files.
- Origin checks cover every WebSocket upgrade; the update archive is
  extracted only after its signature is verified.

## Status (2026-09-29, slot 57 `0.1.0-alpha.2`, source 1533dc2)

Shipped on the test board: update runtime `0.1.0~dev34` (units, tmpfiles
entry for the HID ledger, helpers, nginx security headers), slot 57 through the
API, both WebRTC encoders through `deploy_vp8.sh`. Receipts are under
`evidence/private` (`*-seq57-*`). Each fix has a test that fails on the old
code (named below) unless noted.

| Finding | Outcome |
|---|---|
| R1 | Fixed 39c4fdb, 61b9961. Tests `login_refuses_oversized_bodies_and_names_without_remembering_them`, `many_distinct_keys_stay_bounded_and_keep_lockouts`. Board: 64 KiB and 5 KiB bodies answer 413, a 200-byte name is refused before the limiter. The server units have `MemoryMax=192M` (`MemoryHigh` left out: page cache from an upload would be throttled by it); the server sat at 1.4-2.5 MB through the GUI walks, so the limit is far above four times the peak. |
| R2 | Fixed 1634d3b. Tests `a_lockout_belongs_to_the_forwarded_client_not_to_nginx`, `client_address_trusts_the_forwarded_header_only_from_loopback`. Not exercised on the board (a lockout there would lock the operator out). |
| R3 | Fixed e99a823. Test `a_wrong_user_name_costs_one_verification_like_a_right_one`. Board: right name 200 ms, unknown name 198 ms (was 205 / 13). |
| R4 | Fixed 92a4cd8. Test `close_tests::an_open_terminal_ends_with_its_switch_or_its_session` (the relay itself has no socket-level test); board: an open shell closed 0.94 s after the switch went off (`terminal_switch_probe.mjs`). |
| R5 | Fixed 7f4d25a. Tests `a_background_child_does_not_hold_the_foreground_run`, `the_time_limit_kills_the_whole_group`. Board: a script with `sleep 20 &` returned in 75 ms. |
| R6 | Fixed 8ea0865 (decision 1). Tests `the_shipped_redirect_counts_as_https_being_on`, manifest test, network contract. Board: `enabled: true`, `network_tls` read-only. |
| R7 | Fixed 8998c0e. Tests `an_unconfirmed_change_is_reverted_by_the_next_owner`, `an_unconfirmed_addressing_change_is_reverted_by_the_next_owner`. Board: Wi-Fi made the main connection, owner restarted inside the window, Ethernet metric back to 100 within 7 s, journal gone ("reverted after a restart" in the owner's log). The IPv4 path was tested with the fake runner only. |
| R8 | Fixed 42e3861. Test `a_running_pulse_holds_neither_the_state_nor_a_second_pulse`. No pulse was sent on the board. |
| R9 | Fixed af7a5df (decision 2). Measured before on the board (`hid-ledger-fsync-20260929-before.json`): 32 ms per key event with fsync on the card, 1.4 ms on tmpfs. The owner now runs with `/run/hardened-blikvm-hid/hid-ledger.json`. A key burst was not sent (needs the HID lease). |
| R10 | Fixed 9511989. Test `an_unmounted_or_unreadable_store_drops_nothing`. Confirmed on the board that the mount is `WantedBy=local-fs.target` and the services start after `local-fs.target`, so the ordering premise was not confirmed and no unit changed; the guard still covers an unmount or read error at run time. |
| R11 | Fixed f1ac032. `test_optional_feature_state_is_removed_and_var_lib_is_an_allowlist`; the alpha.2 image was built with the new checks. |
| R12 | Fixed 58870a9. Test `turning_the_lock_on_does_not_log_a_session_out`. |
| R13 | Fixed 58870a9. Tests `a_retime_reaches_the_clones_and_expired_sessions_are_swept`, `a_session_lock_that_cannot_be_saved_keeps_the_old_value`. |
| R14 | Fixed 74a97ac. Test `temporary_names_are_unique_and_stale_partials_are_removed`; the attached-image recheck before the rename reuses the start-of-upload check and has no dedicated race test. |
| R15 | Fixed 3bf1062. Test `a_rebuilt_encoder_continues_the_sequence_numbers`; the encoders were redeployed and both probes decode. No resolution change was forced on the board. |
| R16 | Fixed f46035d. Test `an_upgrade_that_never_runs_releases_the_lock` (the guard, not a real dropped upgrade). |
| R17 | Fixed 03bd281. Test `a_start_rides_out_a_status_reader_but_not_a_running_download`. |
| R18 | Not confirmed. A 40 MiB body sent to the system update upload with its switch off and to the application update upload with an invalid archive both delivered the refusal to the client through nginx (`upload-refusal-seq57.txt`). No change. |
| R19 | Fixed 2cac2bc. Test `tools/test_command_result.mjs`. |
| R20 | Fixed 96771c9. Test `the_lowest_metric_default_route_supplies_the_gateway`. |
| R21 | Fixed 5904890. Test `tools/test_build_scripts_guard.py`. |
| R22 | CSP, `nosniff` and a referrer policy fixed be8f599 (`csp_contract.mjs`, the packaging pin; the GUI walks on the board, desktop and mobile, report no console or policy error). Accepted, not changed (decision 6): HSTS, `overflow-checks`, the server's `capture-status` events. |
| R23 | Fixed cfdaaab. 0 failures in 100 runs beside eight busy loops (it could not be reproduced before either). |

Board end state after the checks: every administration switch off, firewall
Baseline, the operator's two images still in the catalog, no drive attached,
session lock 3600 s as found.
