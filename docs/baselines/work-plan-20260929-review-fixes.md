# Work plan: fixes for the 2026-09-29 code review (2026-09-29)

Follows `docs/baselines/audit-20260929-review.md`; item numbers (R1, R2, ...)
refer to its findings. Rules as in the earlier plans: capability-gated
surfaces enforced by the server and the board owner, privileged work only in
the owner or a dedicated helper, English repository text, secrets only through
`project-memory-exec` (never in arguments, logs or receipts, and never a
program on its standard input under it), every change shipped as a signed slot
or the update-runtime package and verified on the test board, audit and design
documents updated with evidence under `evidence/private`. Additions for this
plan:

- One commit per item. Every fix comes with a test that fails on the code as it
  is now. A [C] finding is reproduced first; one that cannot be reproduced is
  recorded in the audit as "not confirmed" and left unchanged. A fix that would
  need a new route, a protocol change or a design decision outside this plan is
  not improvised: it is recorded in the audit as an open item.
- No new HTTP route (`tools/test_plan_route_drift.py` pins the count) and no
  new capability.
- An item's own tests run before its commit; the full `scripts/check.sh` runs
  before the last commit of each phase and before every slot build (a failure
  under host load is rerun and the flaky test named in the commit that fixes
  it; see E5).
- Device rules: the board is only read unless an item says otherwise, and
  every setting an item changes is set back; nothing takes the HID lease while
  the operator controls the board; no ATX pulse is ever sent on the board (C2
  is tested with a fake runner); the login lockout is tested in a unit test
  of the server, never by failing logins on the board; media the operator
  attached or uploaded is left alone; a probe never prints or stores the GUI
  password.
- Consent: a package or slot install restarts the server or the owner and ends
  every session, so it is done only with the operator's consent (decision 4).

## Decisions

The plan takes these defaults; the operator may override one before the work
starts.

1. (R6) HTTPS stays on. The setting becomes a read-only display of the
   redirect that nginx really has; it is not a switch. Turning HTTPS off on a
   hardened device is not a feature to offer, and today the switch does
   nothing.
2. (R9) The HID release ledger moves to tmpfs, next to the firewall's pending
   journal. An owner restart still finds it; a reboot resets it together with
   the USB gadget, which the host sees re-enumerate, so no key stays held.
3. (release) The fixed build is prepared as `0.1.0-alpha.2`; alpha.1's release
   notes then mark it superseded. Nothing is published by this plan (phase H).
4. (consent) The plan installs the package and the slot on the test board only
   when the operator has said so in the goal text. Without it the plan stops
   before phase F and asks once.
5. (R22) The Content-Security-Policy is enforced from the start; the
   browser-side walk of E3 must see no violation, because there is no route to
   collect reports.
6. Not in this plan, recorded in the audit as accepted: HSTS (a self-signed
   certificate on a LAN address; a pinned browser would be locked out by a
   certificate change and nginx already redirects), `overflow-checks` in the
   release profile (an overflow would abort the process; none was found by the
   fuzzers), and the server's `capture-status` events for the "no signal"
   overlay (a missing feature, not a fault).

## Phase A: login and sessions (R1, R2, R3, R12, R13)

A1. (R1) The login route takes a body of at most 4 KiB, a user name of at most
    64 bytes and a password of at most 1024 bytes (First Setup and the password
    change enforce the same bounds) and refuses more with a client error before
    any hashing; a refused request records nothing in the limiter. The limiter
    keeps at most 1024 keys: expired entries go first, then the oldest entry
    that is not locked; a newcomer never evicts a locked entry.
    Test: 5000 logins with maximum-size names leave the limiter under 1024
    keys and 256 KiB; a 5 KiB body answers 413 (today it answers 401 and keeps
    the key).
A2. (R2) The limiter, and the terminal unlock that shares it, key on the client
    address nginx passes in `X-Forwarded-For` (nginx sets exactly
    `$remote_addr` and drops what the client sent), trusted only when the
    connection itself comes from loopback and the value parses as an address;
    otherwise the peer address is used. A lockout of one address does not
    affect another.
    Test through the router: five failures from one forwarded address lock it
    and not a second address; the header from a non-loopback peer and a
    malformed header are ignored.
A3. (R3) Verification costs the same for every user name and runs off the
    lock: an unknown name is verified against a dummy hash with the account
    hash's parameters, built at start; verification runs on the blocking pool,
    at most two at a time, outside the limiter's mutex (lock to check, release,
    verify, lock to record; the lockout is checked again after the permit is
    taken).
    Test: a counter shows one Argon2 verification for a right and for a wrong
    user name; two concurrent logins do not hold the limiter while hashing.
A4. (R12) `last_seen` is refreshed by every validated request whether or not the
    lock is on, and turning the lock on refreshes the caller's entry, so
    enabling it never logs the caller out. Test: the reproduction from the
    audit.
A5. (R13) The session lock is stored first (through a uniquely named temporary
    file) and applied after; a failed store leaves the old value. Expired
    sessions and their activity entries are removed when a session is issued.
    The streams that keep a cloned session (terminal, PicoClaw, the heartbeat
    socket) ask the store whether their token is still active on their
    one-second tick instead of trusting the cloned deadline, so `retime` and a
    forced logout reach them (shared with B1).
    Test: a failed store keeps the old value; an expired session leaves the
    store; a stream open across a shortening `retime` ends at the new deadline.
A6. Board: `tools/device/login_time_probe.py` (a new tool made from the
    audit's probe; at most three wrong passwords per run, from one address)
    shows the right and the wrong user name within 25 % of each other; an
    oversized login answers 413; the GUI walk passes.

## Phase B: administration features (R4, R5, R6, R16)

B1. (R4) The terminal relay checks the Terminal switch and the session on a
    one-second tick, as the PicoClaw relay does, and closes the browser socket
    and the helper connection when either is gone; the helper ends the shell
    when its connection closes.
    Test: a server test with a fake helper socket and a WebSocket client:
    switching the switch off closes the relay within 2 seconds (today it stays
    open). Board: `terminal_switch_probe.mjs` extended (open, switch off, a
    close within 3 seconds, no shell left).
B2. (R5) `run_foreground` starts the interpreter in its own process group and
    kills the group when the interpreter exits and when the time limit passes,
    then reads the pipes to the end; the output produced so far is kept.
    Test: `sleep 30 & echo started` returns "started" in under 2 seconds
    (today 30); a child that ignores SIGTERM dies with the group at a shortened
    limit. Board: `scripts_switch_probe.mjs` with such a script.
B3. (R6, decision 1) `parse_conf` reports the redirect the port 80 block really
    has (the marker block or a `return 30x https://`); the capability manifest
    lists `network_tls` as read-only; the owner answers `capability_disabled`
    to a change; the page shows HTTPS on and disabled, with a note in English
    and Russian; the certificate view stays.
    Test: broker tests on the shipped configuration (enabled) and on one
    without a redirect; the manifest tests of the server; a Playwright contract
    for the page.
B4. (R16) The PicoClaw assistant lock is taken inside the upgraded connection,
    or released by a guard when the upgrade does not run, so a failed upgrade
    cannot hold the operator's keyboard and mouse for 30 minutes. Reproduce
    first.

## Phase C: board owner (R7, R8, R9, R10)

C1. (R7) A pending network revert (the Wi-Fi main connection, 60 seconds; IPv4
    addressing, 120 seconds) is written to a journal before the change is
    applied (`network-revert.json` in the owner's state directory: the profile
    and the previous settings) and removed on confirmation or after the
    revert. An owner that starts and finds one reverts at once, because nothing
    can still be waiting for a confirmation, retrying for up to 60 seconds
    while NetworkManager is not up, and removes it.
    Test: with the fake runner, arm, drop the state (a restart), start a new
    one: the previous settings are applied and the journal is gone; a
    confirmation removes the journal; a corrupt journal is discarded and
    logged. Board, if the Wi-Fi is joined: switch the main connection to
    Wi-Fi, restart the owner inside the window, and the Ethernet metric is
    back within 15 seconds; a confirmed change leaves no journal. Without a
    joined Wi-Fi the tests stand and the board check is recorded as not run.
C2. (R8) The GPIO pulse no longer holds the owner's shared lock: the ATX engine
    sits behind its own lock, the pulse runs on the blocking pool, and a second
    ATX request during a pulse is answered at once with an existing rejection
    (no protocol change) while HID, media and settings requests are served.
    Test: a fake runner that blocks for 2 seconds: a HID request completes in
    under 100 ms, a second ATX request is refused at once, the first completes
    and is journaled as before. No pulse is sent on the board.
C3. (R9, decision 2) `BLI_KVM_GADGET_HID_LEDGER` points to
    `/run/hardened-blikvm-hid/hid-ledger.json`; a tmpfiles entry creates the
    directory (root, 0700) and the owner unit lists it in `ReadWritePaths`
    (the source unit and its native rendering). The old file under
    `/var/lib/hardened-blikvm-gadget` is removed by the package. The image
    builder keeps removing it.
    Test: packaging tests pin the path, the tmpfiles entry and the
    `ReadWritePaths`; the ledger tests run in a temporary directory with the
    same ownership rules. Board: `tools/device/fsync_probe.py` (writes and
    removes small files on the card and on tmpfs, no HID lease) prints the cost
    of one ledger store on each; a real key burst through the HID latency probe
    only with the operator present and agreeing.
C4. (R10) The catalog is reconciled only when the image store is positively
    there: its filesystem differs from the root filesystem's and the directory
    can be read, and an entry goes only when `symlink_metadata` says the file
    is not found (any other error keeps it). Otherwise the catalog is served
    unchanged and the dialog says the store is not available. First confirm on
    the board, with `findmnt` and `systemctl show -p After`, whether the store's
    mount is ordered before the owner and the server; add the ordering only if
    it is missing.
    Test: a store that is not mounted or unreadable drops nothing; a missing
    file still drops its entry (the alpha.1 behaviour).

## Phase D: media, downloads, video and pages (R14, R15, R17-R20)

D1. (R14) Upload temporary names are unique (a random suffix instead of a
    per-process counter); a `.upload` file older than an hour is removed when
    the server starts; the attached-image check is repeated immediately before
    the rename (a rename over an attached image is refused with 409 and the
    temporary file removed); file writes and the final `fsync` run on the
    blocking pool.
    Test: an interrupted upload, then the same name again, succeeds and the
    leftover is reclaimed (today "not writable"); the attach race with a fake
    owner state.
D2. (R15) A capture size change keeps the stream's SSRC, continues its RTP
    sequence numbers and timestamps, and starts with a key frame.
    Test: the packetizer across an encoder rebuild; the WebRTC probes still
    decode. No resolution change is forced on the board.
D3. (R17) The remote download's status read cannot make the helper's exclusive
    lock fail: the read takes no lock (the helper replaces the file
    atomically) or the helper retries its lock for up to 250 ms. Reproduce
    first with a test that holds the shared lock while a start arrives.
D4. (R18) Reproduce with `upload_termination.mjs` against the application and
    the system update uploads (switch off, no space, too large). If the
    browser sees a dropped connection instead of the reason, the handler
    answers only after it has read what the proxy already forwarded (bounded
    to 1 MiB) and then closes the connection; if that is not enough the page
    shows the reason from the status it already reads. No new route.
D5. (R19) `images.tsx` treats `applied: false` as a failure and shows the
    reason, or is deleted when nothing imports it. Contract test in the style of
    `webrtc_unavailable_contract.mjs`.
D6. (R20) `system_time.rs` and `network.rs` share one default-gateway choice
    (the lowest metric). Test with two default routes.

## Phase E: build, units, headers and tests (R11, R21, R22, R23)

E1. (R11) The image builder becomes an allowlist: it removes the PicoClaw state
    (`var/lib/private/hardened-blikvm-picoclaw` and its cache), the stored
    scripts (`var/lib/hardened-blikvm-admin/scripts`) and the download and
    Tailscale state, then fails on any directory under `var/lib` that is
    neither on its list nor removed, so new state cannot leak silently, and
    searches the staged root for the API-key and token patterns PicoClaw
    writes. Pinned by `tools/test_build_sd_image.py`.
E2. (R21) `scripts/build-x264.sh` and `scripts/build-libvpx.sh` refuse an output
    path that is `/`, the home directory, a top-level system directory or a
    directory without the marker file they create, before `rm -rf`. Test: a
    shell test runs both with those paths; nothing is deleted.
E3. (R22, decision 5) nginx sends a `Content-Security-Policy` on the HTTPS
    server (`default-src 'self'`, `frame-ancestors 'none'`, `object-src
    'none'`, `base-uri 'self'`, the rest as the walk requires; no
    `unsafe-eval`, `unsafe-inline` for styles only if the UI library needs
    it), `X-Content-Type-Options: nosniff` and `Referrer-Policy: no-referrer`.
    A packaging test pins the headers and that no location redefines
    `add_header` (which would drop them). The Playwright walk on desktop and
    mobile listens for `securitypolicyviolation` and must see none, in the
    MJPEG and WebRTC views and the terminal too.
E4. (R1) The server units (candidate and current, source and native rendering)
    get `MemoryMax`, and `MemoryHigh` at 75 %, from the peak measured on the
    board during the GUI walk, a media upload and the video views: at least
    four times the peak and at least 128 MiB, recorded with the numbers.
    Packaging tests pin it.
E5. (R23) The `native_ipc` test fixture ignores a write to a peer that has
    already left instead of unwrapping it. Test: 200 runs of the test beside
    eight busy loops, no failure (it failed once at a load average of 35).

## Phase F: shipping

F1. Update-runtime package `0.1.0~dev34` (the constant and the supported list in
    `tools/build_debian_update_runtime_package.py`) with the unit changes (C3,
    E4, any ordering from C4), the administration helper (B2) and the download
    helper (D3), and the nginx configuration (E3), installed with the deploy
    helper; `nginx -t` before the reload. The owner restarts on the old slot
    with the new ledger path first.
F2. One slot, version `0.1.0-alpha.2`, with the server, owner and web fixes
    (A, B1, B3, B4, C, D1, D4-D6), installed through the API and confirmed; the
    encoders redeployed with `tools/device/deploy_vp8.sh` (D2). A failed device
    check is fixed and shipped in another slot, which then becomes the
    candidate.
F3. The audit gets a status line per finding (fixed: commit, slot, evidence
    file; not confirmed; open), and the documents that describe changed
    behaviour are updated (`docs/baselines/board-owner-acceptance.md` for the
    ledger, `docs/design/admin-features-v1.md` for the terminal and HTTPS).

## Phase G: board verification

G1. The device checks named in A6, B1, B2, B3, C1, C3 and C4, and the memory
    numbers of E4.
G2. `gui_walk_device.mjs` desktop and mobile with no API, page or CSP error;
    `media_gui_device_test.mjs` desktop and mobile; the VP8 and H.264 probes;
    the terminal, scripts, PicoClaw and remote-download switch probes, each
    switch set back off.
G3. End state: every administration switch off, firewall Baseline, the
    operator's drive state untouched, the session lock as found, the slot and
    package numbers recorded in the audit.

## Phase H: release candidate (not published)

H1. The last slot installed and verified in F2 and G is the release slot; the
    SD image is rebuilt from the board after G3 with the checks of E1,
    compressed, with `SHA256SUMS`, `IMAGE-INFO.txt` and English release notes
    that list what changed since alpha.1 and mark it superseded, staged with
    the slot's bundle under `~/.cache/hardened-blikvm/release-alpha2`.
H2. Publishing (tag, GitHub pre-release, PR text) waits for the operator's word;
    this plan publishes nothing.

## Order and operator steps

The plan and the audit are committed first. Then A, B, C, D, E, each item
tested and committed; F, G, H. Operator: consent for the installs (decision
4), consent for a real key burst (C3, optional), the leaked password's
rotation together with the update of the `gui` record (every board step needs
it), the word to publish (H2), and the optional first boot of the new image
from a spare card. When the board cannot be reached, all local work and H1's
build are finished and the device checks are listed as pending.

## Status (2026-09-29)

Phases A-H done on the test board except the items the audit's status table
marks not confirmed (R18) or accepted (decision 6): slot 57 `0.1.0-alpha.2`
(source 1533dc2), update runtime `0.1.0~dev34`, both encoders redeployed, the
device checks of phase G passed (login timing 200 / 198 ms, oversized login 413,
open terminal closed in 0.94 s, background-child script 75 ms, HTTPS on and
read-only, GUI walk desktop and mobile without an API, page or policy error,
media GUI test desktop and mobile, VP8 and H.264 probes), and the board is back
in its default state. Two departures from the plan text: `MemoryHigh` was not
set (page cache from an upload would be throttled by it), and the mount
ordering of R10 was found already in place, so no unit changed. The release
candidate is staged under `~/.cache/hardened-blikvm/release-alpha2` and is not
published.
