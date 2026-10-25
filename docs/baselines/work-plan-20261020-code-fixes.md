# Work plan: fixes from the code audit of 2026-10-20

Follows `docs/baselines/audit-20261020-code-review.md`, which verified an
external code audit against the code at `3af8839` and the device's model;
finding names (AUD-06, V1, ...) are that note's. Only the findings that hold
and fit the model are planned; the others are recorded there and left as
they are. Item names E1, E2, ... are those of
`docs/baselines/work-plan-20261018-security-fixes.md`.

## Rules

The rules of the 2026-10-18 plan hold unchanged:

- English repository text, and secrets only through `project-memory-exec`.
- One commit per item, starting with a test that fails on the code as it
  is; "not confirmed" when that test passes.
- No new HTTP route and no new capability.
- The two tracks (application slot, system), and their compatibility.
- `RUST_TEST_THREADS=16 sh scripts/check.sh` before each phase's last
  commit and before every build, with flaky tests named.
- The device rules and the target's rules, consent for every install, and
  gates.

For this plan:

- A slot item that needs a system change (S5, S17) uses the new path when
  the system provides it, and today's path otherwise. The slot so runs on
  beta.8's system, on the board's (sequence 7) and on this plan's. A system
  of this plan keeps beta.8's slot starting; that slot's PicoClaw install
  is refused there with a reason (Y4).
- Nothing on the board steps its clock, or changes its network, firewall,
  hostname, SSH, QUIC or video settings, or installs or removes PicoClaw.
  S1, S2, S3, S9, Y4 and Y7 are shown by host tests; the board gets reads
  and the watching probes.
- A power cut is tried only on a spare card, never the test board's own,
  and only at the operator's word (decision 2's alternative).

## Where the work starts

- Branch `wip/corrective-audit-checkpoint` at `3af8839` (published);
  `check.sh` green there. The 2026-10-18 plan is complete and not released.
- The test board runs slot 99 on the system of sequence 7 (update runtime
  `0.1.0~dev39`), RAW floor 8. `7ffd42a` is in no system build yet. Read on
  2026-10-21: the application journal holds 83 records in 25,569 bytes, and
  the exec store holds nothing but slots 98 and 99 and `current`. Remote
  download is off (the operator's switch).
- Until Y2 ships, a deletion in the exec store cut short (a power loss
  during an activation's cleanup) stops the application at every boot. Its
  recovery is to remove the `.delete-*` or `.deleting-*` entry from
  `/var/lib/hardened-blikvm-exec` (SSH or the card). Until Y1 ships, the
  board keeps power while a RAW update writes its root.

## Decisions

Taken as written on 2026-10-21, when the operator asked for the goal on
this plan; the operator may still change any of them before its item is
done.

1. Order: phase S makes one application slot (100); phase Y makes one
   system build (update runtime `0.1.0~dev40`, a card image and a RAW
   bundle of sequence 8), which also carries `7ffd42a`. Both join the
   unpublished 2026-10-18 work in one release, published only at the
   operator's word. Alternative: Y2 and Y3 (availability) first, as a system
   build of their own. Changed by the operator on 2026-10-21 ("dev41"): the
   system build is update runtime `0.1.0~dev41`, which adds GY1, found at
   gate Y (a failed native boot is tried again); dev40 was staged on the
   board for the gate's first reboot. The slot became 101 (100 was built
   but not installed; see the review note's gates). Changed again by the
   operator on 2026-10-22 ("Сначала исправить"): the release waits for GY2
   (nginx starts with the system's configuration), so the system build is
   update runtime `0.1.0~dev42` with a RAW bundle of sequence 9.
2. AUD-01. The RAW install's page says that the board must keep power while
   its root is written. A host tool writes the backup kept on the data
   partition back to the root partition, with the card in a reader, and the
   README says when it is needed. A recovery boot from the data partition is
   left to a design of its own. Alternative: the recovery boot in this plan,
   with an experiment on a spare card first. The stage would copy the
   running kernel, initrd and a restore script to the data partition, and
   move the bootable flag there until the root is written.
3. AUD-05.2, in two parts.
   - The PicoClaw runtime starts only a checked copy. A root step before
     each start checks the installed `picoclaw` against a digest pinned for
     its release, and copies it into a directory of root's that the unit
     runs it from.
   - PicoClaw's install and removal move to a socket, a unit and an account
     of their own, so no ISO download can write a PicoClaw file.

   Alternative: the checked copy only. The ISO worker could then still stop
   PicoClaw from starting, but not run code as its user.
4. AUD-08. During a revert window, a second change keeps the first state to
   revert to, per interface. The journal is never cleared while its timer
   runs, and a failed apply leaves the pending revert in place.
   Alternative: refuse a second change until the first is confirmed or
   reverted, and the page says why.
5. V1. The RAW update keeps the image's `nginx.conf` and applies to it the
   HTTPS redirect the carried file had (the owner's marker block). The rest
   of `/etc/hardened-blikvm` is carried as today. Alternative: the owner
   renders `nginx.conf` at each start from a template the system ships,
   with the redirect setting.
6. AUD-16.2. nginx reaches the server through a unix socket that only
   nginx's account may open, and `X-Forwarded-For` is trusted there alone;
   a loopback TCP peer counts as itself. Alternatives: a secret header that
   nginx adds, read from a file root makes at boot; or record the finding
   and leave it (8-character minimum, Argon2).
7. AUD-06. On any fan policy error, the fan runs at its failsafe duty until
   the policy answers again. That is full speed, so it can be heard.

## Phase F: facts and decisions

F1. The board, read only (done 2026-10-21,
    `evidence/private/code-audit-20261020/board-facts-20261021.txt`).
F2. The decisions confirmed.

- **Gate F**: F1 recorded and the decisions confirmed.

## Phase S: the application slot

S1. (AUD-06) boardd's fan policy, buttons, LEDs and display take a
    monotonic clock (`Instant` since start). The wall clock is kept for
    what is shown. Decision 7: the fan supervisor applies its failsafe duty
    on a policy error instead of returning. `policy_errors_do_not_reach_the_actuator`
    changes with it, and `board-daemon-v1.md` says so.
    Tests: with the wall clock stepped back 10 minutes and 95 °C, the fan
    runs at its failsafe duty (today it stays off for 600 samples); a
    button press after the same step is seen. Effort M.
S2. (AUD-08) Decision 4, for IPv4 and Wi-Fi `prefer`.
    Tests:
    - DHCP, then .50, then .60 without a confirmation: the window ends in
      DHCP (today in .50).
    - A resolver edit inside the window keeps the journal, so a restart
      still reverts (today the journal is gone).
    - A change on `wlan0` keeps `eth0`'s revert.
    - A failed apply keeps the journal.

    Effort M.
S3. (AUD-07) Hostname, SSH, QUIC and video settings run on blocking
    threads behind locks of their own, as Wi-Fi, IPv4 and the firewall
    already do, and each command gets a deadline and is killed after it.
    The unused Tailscale request of the owner gets `--timeout`, as the
    admin helper's has. Test: a fake runner that blocks 2 s in a hostname
    change; a concurrent HID lease is answered within 200 ms (today after
    2 s). Effort M.
S4. (AUD-16.1a, 16.1b) The terminal's unlock checks the lockout again after
    its permit, as login does. Its password is capped at 256 characters and
    its body at login's limit.
    Tests: with `login_max_failures=1`, ten parallel wrong unlocks make at
    most two verifications (today ten); a 300-character password gets 400.
    Effort S.
S5. (AUD-16.2, decision 6, the server's half) When the system's unit names
    a proxy socket, the server listens on it as well. It takes
    `X-Forwarded-For` from that socket only, and counts a TCP peer as its
    own address. Login and unlock take their source from what both
    listeners give. Without that socket, today's rule holds.
    Tests:
    - With the socket named, a TCP request from 127.0.0.1 with
      `X-Forwarded-For: 198.51.100.9` is limited as 127.0.0.1 (today as
      198.51.100.9).
    - Over the socket, the header counts.
    - Without the socket, today's test holds.

    Effort M.
S6. (AUD-11) The server reads every file in the worker's directory through
    one helper, on a blocking thread. The helper opens the file with
    `O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC`, requires a regular file by `fstat`,
    and reads at most its limit plus one byte.
    Tests: a FIFO for the status reads idle within 1 s (today it hangs); a
    link to `/dev/zero` for PicoClaw's install status reads as none (today
    it is read without end). Effort S.
S7. (AUD-05.1) After its `fchmod`, the server takes a read lease
    (`F_SETLEASE`, `F_RDLCK`) on its own descriptor of the downloaded file.
    The kernel refuses the lease while any writable descriptor or shared
    writable mapping is open. A refused lease refuses the image. Test: a
    file the worker still holds open for writing is not published (today it
    is). Effort S.
S8. (AUD-16.4) The QUIC sweep releases the HID lease of every session it
    ends. Test: a lease noted, the web session revoked; the sweep queues
    its release (today it is dropped). Effort S.
S9. (AUD-14) A firewall mode is persisted before its change is confirmed. A
    failed write rolls the change back and answers with the error. Test: a
    persist path that cannot be written; a change does not answer `Applied`
    (today it does). Effort S.
S10. (AUD-13) Shortcuts and Wake-on-LAN addresses are written through
     `state_file::replace` under a lock of their own, and an unreadable
     file is not written over. The owner writes `/etc/hosts` and
     `resolved.conf` through a temporary file and a rename where its
     sandbox allows; where it does not, that is recorded.
     Tests: two parallel additions keep both entries; with a damaged file,
     an addition fails and leaves the file (today it empties it). Effort S.
S11. (AUD-20) A replayed button release clears both pointer interfaces, and
     GPIO line descriptors are owned and closed.
     Tests: replaying a released button writes a zero report to `hid.usb2`
     too (today only `hid.usb1`); a line requested, its driver dropped, can
     be requested again. Effort S.
S12. (AUD-09) The owner opens an image with `O_NONBLOCK` and checks it by
     `fstat` (regular, one link, size) before the gadget is unbound. It
     reads the media catalog through a descriptor opened the same way. Test:
     a FIFO swapped in for an image is refused within 1 s, and the gadget
     stays bound (today the owner hangs). Effort S.
S13. (AUD-15) The page passes PicoClaw's and the system log's query
     parameters flat. Test: a page contract stubs the routes and reads
     `limit=100` and `lines=500` after 500 is chosen (today neither). Effort
     S.
S14. (AUD-19.1a, V2) PicoClaw's gateway keeps a connection id with its lock,
     and releases only its own. A second connect for the same session while
     one is pending is refused, and a relay ends when its close sender is
     gone.
     Tests: two parallel connects for one session give one connection and
     one refusal; a dropped close sender ends the relay (today it spins).
     Effort S.
S15. (AUD-19.2b) An upload replaces its image only after the multipart body
     has ended and the catalog has accepted it, and its temporary file is
     removed on every other path. Test: a body with two file parts leaves
     the existing image byte for byte (today it is replaced). Effort S.
S16. (AUD-19.3) A password change holds the setup lock across the account,
     root's password and their rollback, and hashes on a blocking thread.
     Test: two parallel changes leave the account matching root's last
     password, with a fake admin socket. Effort S.
S17. (AUD-05.2, decision 3, the server's half) The server sends PicoClaw's
     install and removal to their own socket when the system has it, and
     to the download socket otherwise. It reads the install's status where
     that system writes it. Test: with the new socket present, the request
     goes there. Effort S.

- **Gate S**: `check.sh` green; slot 100 built from a clean tree, installed
  at the operator's word and verified on the board (the services, the state
  as found but for the slot, boardd and the fan running, the watching
  probes). A release only at the operator's word.

## Phase Y: the system (runtime packages, card image, RAW bundle)

Y0. The operator agrees to the system build's scope and time. The image is
    fetched from the board with the KVM pages closed for about 10 minutes,
    and the RAW install takes about 40 minutes, with `/root` saved first.
Y1. (AUD-01, decision 2) A host tool writes the backup kept on the data
    partition back to the root partition (Linux, the card in a reader).
    `system-raw-update-v1.md` and the README say when it is needed, and the
    page's RAW install step asks to keep power (the slot's page). Test: on a
    card image file whose root was half overwritten, the tool restores the
    root byte for byte. Effort M.
Y2. (AUD-04) The exec store recovers interrupted deletions before it lists
    its slots (A). A staging directory without a receipt, or with an empty
    one, that is root's, on the same device, holds no link and nothing but
    an empty `.hardened-blikvm`, is removed as abandoned (B).
    Tests: a deletion marker and its tombstone, then a collection succeeds
    (today `UnexpectedRootEntry`); the three cut stagings are removed
    (today `UnsafeAbandonedStaging`). Effort S.
Y3. (AUD-03) Before an admission, the journal prunes its oldest finished
    records until it fits under its cap with the new record and its
    completion. The count limit stays.
    Tests: 200 native rollback cycles are all admitted (today the 81st is
    refused); a journal already at the cap is pruned at its next admission,
    so a board that hit it recovers. Effort M.
Y4. (AUD-05.2, decision 3, the system's half) `picoclaw.service`'s root
    step checks the installed binary against its pinned digest, and copies
    it into `/run/hardened-blikvm-picoclaw-exec` (root's, 0755), from which
    `ExecStart` runs it. A mismatch refuses the start with a reason. A
    socket and template unit of their own, under an account in no shared
    group (a sysusers fragment, as E4's), install and remove PicoClaw. The
    download unit loses `StateDirectory=` and refuses those requests.
    Tests:
    - A packaging test: the download unit has no PicoClaw directory, and
      PicoClaw's executable is in root's directory.
    - The helper refuses and does not copy a binary whose digest differs,
      and copies a matching one.

    Effort L.
Y5. (AUD-05.3) The download unit gets `RuntimeMaxSec=` just above curl's
    2-hour budget. Test: packaging. Effort S.
Y6. (AUD-02a) The update supervisor and the exec helper take the larger of
    their configured `systemSequence` and the root's `system-sequence`
    file. Test: file 8, configuration 1; a slot whose minimum is 8 is
    accepted (today refused). Effort S.
Y7. (V1, decision 5) The RAW boot step keeps the image's `nginx.conf` and
    applies the carried redirect to it. The redirect's rendering moves into
    a crate that the owner and the RAW update share.
    Tests: the carried file has the redirect and the image's has not; after
    the restore, the image's file has it (today the carried file wins).
    Without the redirect, the image's file is unchanged. Effort M.
Y8. (AUD-10) The admin helper's download, one commit each:
    (a) host labels that start or end with `-` are refused;
    (b) only a 200 ends the hops, and the output is truncated before each
    hop;
    (c) the stderr tail is kept as bytes and decoded once;
    (d) NAT64, 6to4, IPv4-compatible, documentation and benchmarking ranges
    are not public.
    Tests: `-a.example` is refused; a 302 without `Location` and with an
    ISO body is an error; the byte sequence that panics today is kept;
    `64:ff9b::c0a8:101` and `::c0a8:101` are not public. Effort S.
Y9. (AUD-12) The RAW update, one commit each:
    (a) owners are checked both ways, and a device project account must
    exist in the image or own no carried file;
    (b) a lock on the staging directory across receive, stage and cancel,
    and a stale `upload.tar` removed before the space check;
    (c) a failed unmount keeps `Writing` and reboots without touching the
    device.
    Tests: the two owner cases are refused; two parallel uploads give one
    busy answer; with the unmount failing, the device is unchanged and the
    record still `Writing`. Effort M.
Y10. (AUD-16.2, decision 6, the system's half) The server's units name the
     proxy socket, in a directory only nginx's account and the server may
     enter, and nginx proxies through it. Test: packaging (nginx's
     `proxy_pass` and the units agree); on the board, the probes log in
     through it. Effort M.
Y11. (AUD-17.3) A forced key whose hardware encode returns nothing asks for
     the key again, and keeps its old generation and last key. Test: a
     stand-in encoder that fails once; after the failure a key is due
     (today it is not). Effort S.
Y12. (AUD-18.1, 18.3) The QUIC gateway sends Hello again once the link is up
     if the certificate changed meanwhile. A new stream gives up the old
     stream's groups and key wait before the slow-key check.
     Tests: a fake server that delays its Hello while the certificate
     changes gets a second Hello; a key for stream 2 after an unanswered
     key for stream 1 is sent. Effort S.
Y13. (AUD-21) `verify-systemd-units.sh` takes every fragment in
     `packaging/tmpfiles.d` and runs `systemd-tmpfiles --create --root` in
     its test root after sysusers, failing on a bad line. Test: a broken
     fragment in a copy fails the script (today it passes). It may go
     first, as it checks the other items' packaging. Effort S.

- **Gate Y**, as gate E of 2026-10-18:
  - `check.sh` green, and the runtime packages, the card image and the RAW
    bundle built from a clean tree.
  - At the operator's word, the board's system is staged by hand: the
    package, PicoClaw's install account and units, the download unit, and
    nginx's configuration with the proxy socket. Then a reboot.
  - The image is read with the KVM pages closed.
  - The RAW bundle is installed with `/root` saved.
  - Verified: the services, the state as found, the journal, the exec
    store, and the watching probes (WebRTC, QUIC video, a login through
    the proxy socket). If PicoClaw is installed, it runs from its checked
    copy.

  A release only at the operator's word.

## Phase W: the record

W1. The review note's Fixes table: every item with its commit and its test,
    or why it was left; the flaky tests named.
W2. The design notes:
    - `board-daemon-v1.md`: the clock and the failsafe;
    - `network-writes-v1.md`: the revert window;
    - `system-raw-update-v1.md`: the write window and its restore,
      `nginx.conf`, the sequences;
    - `admin-features-v1.md`: PicoClaw's checked copy and installer, the
      proxy socket;
    - the README where something visible changed (keeping power during a
      RAW update, the restore tool).
W3. `check.sh` green on the final commit; the board as found; a release
    only at the operator's word.

## Optional

Only at the operator's choice. None of these has an effect in the model.

- The agent: on a failed write, `CancelIoEx` and a waiting
  `GetOverlappedResult` (AUD-18.4); the display mode's tag checked
  (AUD-18.5); requests before Hello dropped (AUD-18.6).
- The encoder: `vpx_image_t` filled by hand (AUD-17.1b, not the audit's
  fix, V3); strides bounded by `i32` (AUD-17.1a); a short plane an error,
  not a panic (AUD-17.2).
- The server: the terminal registered with the shutdown's upgrades
  (AUD-16.1c); the lock setter under the session table's lock (AUD-16.3);
  PicoClaw's keys cleared on a 401 (AUD-19.1b).
- The platform broker's directory mode check (AUD-20); a CD-ROM's size
  taken from its descriptor (AUD-09).
- From 2026-10-18: A3 (copies under the media mount point) and E16.

## Size

Phase S: about three days of work and its gate with the operator. Phase Y:
about three days, four with decision 2's alternative, and its gate about
three hours with the operator. Phase W: two hours.

## Operator steps

- Confirm or change the decisions.
- Agree to slot 100's install and, later, to the system staging, the image
  fetch and the RAW install; keep the KVM pages closed while the image is
  fetched.
- Say whether and when to publish, with the 2026-10-18 work.
