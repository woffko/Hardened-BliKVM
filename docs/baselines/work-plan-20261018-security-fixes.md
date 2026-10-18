# Work plan: fixes from the security audit of 2026-10-18

Follows `docs/baselines/audit-20261018-security-review.md`, which verified an
external audit against the code and the device's model (the operator has
full rights to the board); item names (HIGH-05, MED-09, N2, ...) are that
note's. Only the findings that hold and fit the model are planned; the
others are recorded there and left as they are.

## Rules

As in the earlier plans: English repository text, secrets only through
`project-memory-exec` (never in arguments, logs or receipts, and never a
program on its standard input under it), every change shipped signed and
verified on the test board, the design and audit documents updated with
evidence under `evidence/private`, no local path or lab address in a
tracked file. For this plan:

- One commit per item. An item starts with its test, run against the code as
  it is: a test that fails confirms the item, and the fix makes it pass; the
  commit names the test. An item whose test passes on the code as it is is
  recorded as "not confirmed" (with what was tried) and left unchanged.
- No new HTTP route (`tools/test_plan_route_drift.py` pins the count) and no
  new capability. A request or a state may gain a field with a default where
  an item names it (B2, D4).
- Two tracks, because they ship differently. The application slot carries
  the web server, the board owner, boardd, the platform broker, the video
  proxy, the page and the agent (phases B, C, D). Units, tmpfiles, sysusers,
  nginx and Janus configuration, the admin helper, the update runtime, the
  RAW update, the QUIC gateway, the encoders and the kernel module are the
  system's: they ship in the runtime packages, the card image and a RAW
  bundle (phase E).
- Compatibility: an application slot runs on beta.4's system and on the
  system of phase E; the system of phase E keeps beta.8's and phase D's
  slots working, so a rollback to either still starts. A change of owner or
  location ships in an order that never leaves a slot unable to read its
  state (E5 says how).
- An item's own tests run before its commit; the full
  `RUST_TEST_THREADS=16 sh scripts/check.sh` runs before the last commit of
  each phase and before every slot or system build. A failure under the
  shared host's load is run again and the flaky test is named in the note.
- The Windows agent: its portable part is tested on Linux; what calls
  Windows is kept small, linted with `cargo xwin clippy` and built twice to
  the same bytes, as `check.sh` does. Its behaviour on Windows is shown on
  the target by the operator.
- Device rules, as before: the HID lease is never taken, no input is sent to
  the target, no ATX pulse, logins are never failed on the board (the login
  limiter is tested on the host only), the operator's media are left alone,
  and the settings, switches, firewall mode, video settings and engine
  choice end as found; the "Target agent" switch ends as the operator leaves
  it. Probes that only watch may run on the board;
  `quic_input_device_probe.mjs` takes the lease and is not run there.
- The target is the operator's own computer and screen: the operator starts
  every build of the agent there and makes every check of it; checks from
  the development host on the target only read. Never ask for a reply while
  something is shown full screen on the target.
- Consent: every slot install and the RAW install happen at the operator's
  word. An install the classifier denies stops the work and is reported,
  never worked around. Nothing is published unless the operator says so.
- Phases end in gates; a missed gate ends the plan there with the findings
  recorded, and what was finished stays.

## Where the work starts

- Branch `wip/corrective-audit-checkpoint`, published up to `89bc1e8`
  (0.1.0-beta.8); `check.sh` green on `89bc1e8`.
- The test board runs slot 96 (`0.1.0-beta.8`) on beta.4's system, the
  agent's switch on. Read on 2026-10-18: systemd 249 without the BPF
  framework (no `SocketBind*`), cgroup BPF available (`IPAddressDeny`
  works), 80 of the journal's 128 history records used, the slot roots
  clean.
- Until E1 ships: a power loss or a timeout while an application update
  copies a slot can leave a temporary file that stops the board's
  application from starting at every boot (N2). Its recovery is to remove
  that one file from `/var/lib/hardened-blikvm-exec` (SSH or the card).
  Slot installs in this plan keep power on and are not interrupted.

## Decisions

Confirmed by the operator on 2026-10-18 ("подтверждаю").

1. Order: phases B, C and D make one application slot (`0.1.0-beta.9`);
   phase E makes a system release after it, led by E1 and E2 (availability).
   Alternative: E1 and E2 first, as a system release of their own.
2. While Windows' session is locked, the agent refuses reading and writing
   the clipboard, switching the layout and changing or keeping the monitors'
   mode, with the reason "the target's session is locked"; reads of the
   monitors, the layouts and the state go on. Alternative: the clipboard
   only.
3. Changing the password asks for the current one; root's password is set
   only after the web account's is saved.
4. `control.sock` and the platform broker admit root and the web server's
   account only.
5. The login limiter counts failures per source address (IPv6 by its /64)
   whatever the name, beside the present per source-and-name lockout; a full
   table evicts its oldest entry instead of dropping the newcomer.
   Alternative: refuse every login while the table is full of lockouts
   (closes the hole but lets a flood lock the operator out).
6. Remote downloads: the helper follows redirects itself and refuses a hop
   from a public address into a private, CGNAT or unique-local one; a
   private URL the operator types stays allowed. Alternative: public
   addresses only, enforced by `IPAddressDeny` in the worker's unit.
7. Janus listens (API and RTP) on a loopback address of its own,
   `127.0.0.2`, which `IPAddressDeny` closes for PicoClaw's runtime and
   every unit that does not feed or ask Janus. Alternative: an API secret
   only the server holds (RTP stays open on loopback).
8. The image download worker gets its own system user in no shared group;
   it writes only into a directory of its own on the media partition, and
   the server publishes the checked file into the store, as it does for
   uploads.
9. The journal keeps every pending record and the last 64 finished ones,
   with the generation before which records were pruned.
10. "Start the agent" and "Update the agent" run only a file whose SHA-256 is
    the board's agent's; the agent's state gains that digest.
11. N6 is recorded and tested where it matters; no new mechanism replaces
    `SocketBind*` on systemd 249.

## Phase A: facts and the operator's checks (before code)

A1. The board, read only (done 2026-10-18,
    `evidence/private/security-20261018/board-facts-20261018.txt`).
A2. The target (the operator): with the agent connected, lock Windows
    (Win+L) and, from the page on the phone (the target is the operator's
    own screen), take remote control and read the clipboard in the Keyboard
    menu. Text in the answer confirms D1 on Windows; a refusal makes D1 a
    guard only. Deferred at the operator's word ("проверим потом",
    2026-10-18): it is made with D1's check at gate B–D, and D1 is built as
    decision 2 says either way.
A3. Optional, only at the operator's word because it mounts: whether the RAW
    updates so far left copies of the images in the root under the media
    partition's mount point (N3); a bind mount of `/` read only shows it.

- **Gate A**: A1 recorded and the decisions confirmed (2026-10-18); A2
  deferred to gate B–D.

## Phase B: the web server (application slot)

B1. (HIGH-05, N9) The limiter of decision 5. Tests: from one address, five
    failures under five different names lock the address (today they
    don't); with the table full of lockouts, a new address's failures for
    the real name still lock it (today they are never recorded); the
    terminal's password check shares the per-source count. Effort M.
B2. (MED-04) `POST /api/auth/password` gains `currentPassword`, checked like
    a login (and counted by the limiter); the account is saved first, then
    root's password; a failure of the second puts the account back.
    Settings > Account gains the field. Tests: without or with a wrong
    current password the answer is 403 and neither password changes; with
    the right one both change; a failed save leaves root's untouched; the
    account page's contract sends the field. Effort M.
B3. (HIGH-06) Setting the session lock caps every live session's deadline at
    the new lifetime from now (never lengthens one). Test: two sessions of
    24 h, the lock set to 60 s without "lock now": the other session ends
    within 60 s (today after 24 h). Effort S.
B4. (MED-11) `/api/ws` limits messages and frames to 64 bytes; the PicoClaw
    gateway takes its lock only after the upstream answered. Tests: a
    17-byte message closes the socket; with the runtime unreachable, manual
    HID input over `/api/ws` is not refused while the gateway connects.
    Effort S.
B5. (MED-06) An upload of zero bytes is refused before anything is renamed.
    Test: an image stays byte for byte after an empty upload of its name
    (today it is emptied). Effort S.
B6. (MED-06) Catalog writes use a unique temporary file opened with
    `O_EXCL|O_NOFOLLOW` and an advisory lock around read, change and
    replace, in the server and the owner (the admin helper's in E10). Tests:
    two writers at once leave a catalog with both entries; a link planted at
    the old fixed name is not followed. Effort M.
B7. (LOW-03) Every handler that sends a control request checks the
    restricted-surface flag. Test: with it off, each answers 503. Effort S.
B8. (LOW-05) The serial path's pattern `^/dev/[A-Za-z0-9._-]+$`; unset
    picocom options left out; `crypto.getRandomValues` where
    `randomUUID` is missing; logout clears PicoClaw's keys. Tests: a path
    with `:` or `[` is refused; the command has no "null"; logout leaves no
    PicoClaw key. Effort S.
B9. (E5's first step) The server and the owner read the media catalog from
    its new place (a directory of the web server's, named in E5) when it
    exists, and from `/etc/hardened-blikvm-gadget` otherwise; they write it
    where they read it. Tests: either place is found and kept. Effort S.

## Phase C: the board owner, boardd, the platform broker (application slot)

C1. (MED-01) `control.sock` admits a peer whose uid is 0 or the web server's
    (resolved by name at start, as the encoders resolve theirs); others are
    closed and logged. If the name cannot be resolved only root is admitted,
    and the log says why. Known clients: the web server (current and
    candidate), boardd and the update exec helper (root). Tests: the
    admission rule; a listener whose admitted uid is not the test's refuses
    the test's connection (today it serves it). Effort M.
C2. (CRIT-02) The image path: `symlink_metadata`; links, non-regular files
    and files with more than one link refused; the file opened with
    `O_NOFOLLOW|O_CLOEXEC` (read-write when writable), `fstat` checked, and
    `/proc/self/fd/N` written into `lun.N/file`, so the kernel opens the file
    that was checked. Tests: a link in the store is refused (today
    presented); a hard-linked file is refused; the fake configfs receives the
    descriptor's path and the owner still reads the attached image back by
    name. On the board at the gate: the agent's drive, which the owner
    presents again when it starts, goes through the new path and reads back
    (the operator's media are not touched). Effort M.
C3. (MED-12 item 1) The owner's state files are written 0640 with
    `O_EXCL|O_NOFOLLOW` temporaries. Test: under umask 0007 a written file is
    0640 (today 0660). Effort S.
C4. (MED-12 item 4) boardd's BMP reader checks offset and rows with checked
    arithmetic and returns `DamagedImage`. Test: a 240x240 header followed by
    100 bytes returns the error (today it panics). Effort S.
C5. (MED-01) The platform broker admits root and the web server only, after
    its clients are listed from the code. Tests as C1. Effort S.
C6. (LOW-03) `normalize_interface` refuses `all`, `default`, `lo`, `.` and
    `..`. Test: each is refused. Effort S.

## Phase D: the agent and its start line (application slot)

D1. (HIGH-08) Decision 2 as a portable rule in `names.rs` that the channel
    applies before answering; the page shows the reason. Tests: the rule
    (today absent); the reads contract shows the refusal. On the target:
    A2's check, from the phone, now refused. Effort S.
D2. (HIGH-08) `hid.dll` is loaded at run time from the system directory
    (`LoadLibraryExW` with `LOAD_LIBRARY_SEARCH_SYSTEM32`, its five functions
    resolved), `SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32)` first
    in `main`, and the link sets `/DEPENDENTLOADFLAG:0x800`. Test: a tools
    check of the built program's import table and load configuration, run
    by `check.sh`: only Windows' known DLLs, `ntdll` and API sets, and the
    flag set (today `hid.dll` and no flag). On the target: the agent starts
    and connects. Effort M.
D3. (MED-13 item 3) The agent opens its HID collection without sharing. On
    the target: the agent's test tool is refused while the agent is
    connected (today it opens the device). Effort S.
D4. (N7) Decision 10: the start line selects the volume whose
    `hardened-blikvm-agent.exe` has the board's digest, and runs that one
    only; the server reads the digest from the slot's agent files. Tests:
    the command builder puts the digest in and runs one file; the start
    contract types it; the state carries the digest. On the target: Start
    and Update still connect within 10 s. Effort M.

- **Gate B–D**: `check.sh` green; slot 97 (`0.1.0-beta.9`) built from a clean
  tree, installed at the operator's word and verified on the board (the
  services, the state as found, the agent's drive read back for C2, the
  watching probes); the operator's checks on the target (D1, D2, D3, D4).
  A release only at the operator's word.

## Phase E: the system (runtime packages, card image, RAW bundle)

E0. The operator agrees to the system release's scope and time: the image is
    fetched from the board (KVM pages closed for about 10 minutes), and the
    RAW install takes about 40 minutes with `/root` saved first.
E1. (MED-09, N2) The stager makes its temporary files inside a marked
    staging directory or as `O_TMPFILE`, never in a store's root; before
    staging and before garbage collection it removes its own leftovers in
    both the slot store and the exec store, and still refuses unknown names.
    Tests: a leftover `.tmpAbc123` in the slot root, then an upload succeeds
    (today `UnexpectedRootEntry`); a leftover in the exec store, then a
    preparation succeeds (today refused, so the application does not start).
    Effort M.
E2. (LOW-01) The journal's rollover of decision 9, with a schema version
    bump read from the old one. Tests: 200 activations succeed (today the
    129th is refused); the newest's replay answers "already active"; a
    pruned one's is refused by name. Effort M.
E3. The RAW update, one commit and test each:
    (a) carry `-update`, `-update-v2`, `-slots-v2` and `-exec-ledger`
    (HIGH-04; a test like `every_ssh_host_key_type_is_carried`);
    (b) `--one-file-system` when archiving (N3; a stub `tar` records its
    arguments);
    (c) a failed stage removes its payload and `.new` files (N4);
    (d) owners carried by name: `/etc/passwd` and `/etc/group` kept at stage,
    and the boot step restores the backup when a carried uid names another
    user in the image (N5);
    (e) a file that changes while archived: tar's exit 1 is tried once more,
    then reported with its reason (HIGH-03's residual);
    (f) `root_uuid` must be a canonical UUID at stage and at boot (MED-08);
    (g) the image writes its system sequence into the root, and the minimum
    is the larger of the confirmed sequence plus one and the running
    system's; an unreadable floor refuses (MED-07);
    (h) digests of `state.tar`, `fstab` and `shadow` in the record, checked
    before the backup (HIGH-02, against a damaged card);
    (i) the boot step stops when `e2fsck -p` returns more than 1 (LOW-02).
    Effort L in all.
E4. (N1) Decision 8. Tests: the unit runs under its own user with no access
    to `/run/hardened-blikvm-gadget` or the admin helper's socket
    (`scripts/verify-systemd-units.sh`); the admin helper refuses its uid;
    a finished download reaches the store only through the server's
    publish, which refuses a name that is attached (MED-06's second part).
    Effort L.
E5. (CRIT-01) `/etc/hardened-blikvm-gadget` becomes root's (0755,
    `root:hardened-blikvm`, files 0640) with its contents re-owned by
    tmpfiles; the catalog moves to a directory of the web server's. Order:
    slot 97 already reads the catalog from the new place when it exists and
    from the old one otherwise (B9); the system release moves it; the
    owner check in boardd's configuration ships in the first slot after the
    system release, because an older system leaves the directory the
    server's. Tests: a packaging test that no root unit reads an
    `EnvironmentFile=` or a configuration under a directory another account
    may write (fails today); the catalog is found in both places; boardd
    refuses a configuration in a directory others may write. Effort L.
E6. (MED-12 item 3) The image store is 1770. Test: a packaging test. Effort S.
E7. (MED-01) The update confirmation runs with `--peer-user` for its one
    client, and its sockets are that account's, 0600. Tests: a packaging
    test; the confirmation refuses another uid. Effort S.
E8. (MED-03, N6) Decision 7, and `udp:5006` in Janus's bind list for a
    systemd that enforces it. Tests: every Janus port in its configuration
    is in the unit's list (fails today); PicoClaw's unit denies Janus's
    address; the WebRTC probes pass on the board at the gate. Effort M.
E9. (MED-05, N8) `--globoff`; `[ ] { }` refused anywhere in the URL;
    decision 6. Tests: a globbed URL is refused; the argument list has
    `--globoff`; a redirect from a public to a private address is refused
    (a local stub server; no outside network). Effort M.
E10. (MED-06) The admin helper's catalog writes, if any remain after E4, use
    B6's routine. Effort S.
E11. (MED-13 item 2) `demand.sock` admits its listed clients only, and both
    encoder listeners cap their connections and refuse beyond the cap
    instead of failing to spawn. Tests: the cap; a refused uid. Effort S.
E12. (MED-12 item 1) `UMask=0027` in the owner's drop-in. Test: packaging.
    Effort S.
E13. (LOW-07, N6) `msd-writable` sandboxed (`CAP_SYS_ADMIN` only,
    `NoNewPrivileges`, no network, a system-call filter);
    `system-update-booted` limited to localhost;
    `verify-systemd-units.sh` derives its list from `packaging/systemd` and
    fails on a unit it does not check (fails today), with exposure limits
    for the units it skips; nginx's TLS 1.2 ciphers ECDHE with AES-GCM or
    ChaCha20 only; `docs/design/quic-transport-v1.md` says what the board's
    systemd enforces. Tests: those checks. Effort M.
E14. (LOW-04) The image builder removes and checks `mouse-jiggler.env` and
    `network-revert-*.json`, and renders `nginx.conf` from packaging. Test:
    `tools/test_build_sd_image.py`. Effort S.
E15. The QUIC gateway's `rustls` 0.23.46 and `time` 0.3.47 (the RustSec
    record of 0.1.0-beta.8); `cargo audit` clean. Effort S.
E16. Optional, at the operator's choice: MED-10's bounded queue; HIGH-07's
    lab module refused anywhere in an image tree, without its device table,
    its reset masking the interrupt; MED-02's workspace kept under the
    runtime's home.

- **Gate E**: `check.sh` green; the runtime packages, the card image and the
  RAW bundle built from a clean tree; the RAW install on the test board at
  the operator's word (switches restored, `/root` restored), then the
  services, the state as found, the journal pruned, the slot roots clean, the
  watching probes and the operator's checks; a release only at the
  operator's word.

## Phase G: the record

G1. The review note's table: every planned item with its commit and test,
    or why it was left; the flaky tests named.
G2. The design notes: `admin-features-v1.md` (the switches are not a border
    between the server and root), `system-raw-update-v1.md` (what is carried
    and how), `quic-transport-v1.md` (N6), `target-agent-v1.md` (the locked
    session, the start line), `screen-area-v1.md` untouched; the README
    where something visible changed (the current password, the locked
    session's refusal).
G3. `check.sh` green on the final commit; the board as found; a release only
    at the operator's word.

## Size

Phases B, C and D: about three days of work and the gate with the
operator. Phase E: about four days, and its gate (image, RAW bundle, RAW
install) about three hours with the operator. Phase G: two hours.

## Operator steps

- Confirm or change the decisions.
- A2 on the target, and A3 if wanted.
- Agree to slot 97's install and, later, to the system release and its RAW
  install; keep KVM pages closed while the image is fetched.
- The checks on the target at each gate.
- Say whether and when to publish.
