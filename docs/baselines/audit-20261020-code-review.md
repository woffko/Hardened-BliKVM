# Verification of an external code audit (2026-10-20)

The operator handed over a static audit of the branch at `3af8839` made by
another assistant on 2026-10-20 (read only, no board; kept as
`evidence/private/code-audit-20261020/external-audit-report.md`) and asked
for a work plan from it. This note records what each finding turned out to
be; the plan is `docs/baselines/work-plan-20261020-code-fixes.md`. Finding
names (AUD-01 to AUD-21, and their parts such as AUD-05.2) are the audit's;
V1 to V4 were found while verifying it. Item names of the earlier plan (E1,
E2, ...) are those of `docs/baselines/work-plan-20261018-security-fixes.md`.

## The model

The model of `docs/baselines/audit-20261018-security-review.md`, unchanged.
The board is the operator's, who may become root through the single web
account and holds the card. Out of model: what only the operator can do.
In model: parties without the operator's rights (clients on the network,
other origins, the target as a USB host, servers the board downloads from,
forged or interrupted updates); an unattended session (the session lock and
the terminal's password check); a compromised unprivileged component that
does not hold the operator's authority (the download worker, PicoClaw's
runtime, the encoders, the QUIC gateway, Janus) reaching root or that
authority, while an escalation from the web server's own account gains
nothing; and bugs that cost function or availability.

## Method

Four verifications ran side by side, one per area (the RAW update and the
update stores; the download worker, the admin helper and the server's reads
of the worker's files; the board owner, boardd and the brokers; the server,
the page, the encoders, the QUIC gateway and the agent), each against the
code at `3af8839`. Experiments in throwaway copies of the tree settled what
reading could not: native rollback cycles against the application journal;
an interrupted exec-store deletion and a staging directory cut before its
receipt; 1 MiB of another image written over an ext4 root (`blkid`,
`debugfs`); a descriptor kept open for writing across `fchmod` and a rename,
and a read lease refused while it is open; curl 8.5.0 with a host that
starts with a hyphen and with non-canonical IPv4 forms; the helper's stderr
tail cut inside a UTF-8 character; a fan policy fed a wall clock stepped
back; two IPv4 changes inside one revert window; axios 1.18.1 with a wrapped
`params` object; axum 0.8.9 under a client that aborts a multipart upload;
`systemd-tmpfiles` 255 on a broken fragment. The reports are in
`evidence/private/code-audit-20261020/`. The board was read once, read only
(`board-facts-20261021.txt`): the application journal holds 83 records in
25,569 bytes, the exec store nothing but slots 98 and 99 and `current`.

## Findings

| Finding | What holds | Class | Severity | Plan |
| --- | --- | --- | --- | --- |
| AUD-01 | A power loss while the boot step writes the root is not recovered for most of that window. Once the first MiB is written the root partition carries the payload's UUID, so the initramfs stops on `root=` before the hook runs, and U-Boot loads the kernel and initrd from that same partition. Only after the root's `fstab` is merged does the restore of `Writing` work. The audit's partition scan in the hook would never be reached. | Functional: an interrupted update leaves the board unbootable; recovery is the card on another computer | high | Y1 |
| AUD-02 | (a) The update supervisor and the exec helper take `systemSequence` from their carried JSON (1 on the board) and never from the root's `system-sequence` file (7), so an application bundle that needs a newer system is refused. The static JSON files of the audit do not exist. (b) `/etc/hardened-blikvm` is carried whole, so a board updated by RAW keeps its own `nginx.conf` (V1). | Functional; hardening that does not ship | low; low-medium | Y6, Y7 |
| AUD-03 | The history rolls over at 128 records, but the journal is capped at 64 KiB. Native rollback records take about 803 bytes, so the 81st such cycle is refused with `JournalTooLarge`, and every later one too: nothing recovers it. A gap of E2, whose test used small records. The board's records take about 308 bytes. | Functional | medium | Y3 |
| AUD-04 | (A) The exec store's garbage collection lists its slots before it recovers an interrupted deletion, so a deletion cut short stops the application at every boot: a gap of E1. (B) A staging directory cut before its receipt (a window of milliseconds) does the same. The silent skip of a bad temporary is refuted: the listing refuses it. | Functional | medium; low | Y2 |
| AUD-05 | (05.2) The download unit's template keeps `StateDirectory=hardened-blikvm-picoclaw-runtime` (0755) for every instance. An ISO download's worker can therefore replace the PicoClaw executable, which nothing checks before it runs. At its next start it runs as the runtime's user, with the model's API key and the MCP token (screenshots, and HID through the operator's lease). A gap of E4. (05.1) A descriptor opened for writing survives the server's `fchmod` and rename, so the worker can change an image after the server checked it. (05.3) The unit has no `RuntimeMaxSec`. | Defense in depth | medium; low; low | Y4 with S17, S7, Y5 |
| AUD-06 | boardd feeds the fan policy the wall clock. The policy refuses a time earlier than its last one, the supervisor then returns without its failsafe, and boardd drops the error. After a backward step (a forged NTP answer, a manual correction) the fan keeps its last command until the clock catches up: off at 95 °C for 600 samples in an experiment. Buttons, LEDs and the display stop the same way. | In model, direct (an on-path NTP answer); hardware safety | medium | S1 |
| AUD-07 | Hostname, SSH, QUIC and video settings run `systemctl` synchronously under the owner's global lock, so HID leases and their expiry wait behind them, for seconds and up to systemd's job timeouts. The `tailscale up` scenario is refuted: no server sends that request, and the page's Tailscale goes to the admin helper, which bounds it. ATX is refuted: the pulse runs outside the lock. | Functional | low-medium | S3 |
| AUD-08 | IPv4: a second change inside the revert window replaces the journal's `before` with the unconfirmed state and skips the first timer; a resolver-only edit clears the journal but keeps the timer, so a restart loses the revert; a failed apply clears the journal; one pending revert serves every interface. Wi-Fi `prefer` partly holds (the page confirms at once). "Permanent lockout" is overstated. | Functional (anti-lockout) | medium | S2 |
| AUD-09 | The owner opens an image without `O_NONBLOCK`, after unbinding the gadget, so a FIFO swapped in between the check and the open hangs it. The catalog's read is bounded by size but opened by path. Only the web server's account can write either place. | Out of model (from the server's own account); availability | low | S12 |
| AUD-10 | (10.2a) A host label that starts with `-` passes; curl then ignores its pin, but glibc's DNS refuses the name. (10.3) A 3xx without `Location` ends the hops and its body counts. (10.4) The stderr tail is cut inside a UTF-8 character and the helper aborts (it fails closed). (10.5) NAT64, 6to4, IPv4-compatible, documentation and benchmarking ranges count as public. Refuted: 10.1 (a private first hop is allowed by design), 10.2b (curl connects to the checked literal), and 10.3's 204 (curl empties the file). | Hygiene of E9 | low | Y8 |
| AUD-11 | The server reads the worker's directory on its async threads. PicoClaw's install status is read with no size limit and follows links: a link to `/dev/zero` runs the server into its memory limit. Every status read blocks on a FIFO, which survives restarts there (`RuntimeDirectoryPreserve=yes`). The audit's unbounded download status is refuted: that read is bounded. | Defense in depth; availability | low-medium | S6 |
| AUD-12 | (1) `owners_agree` misses a project account the image dropped, and a non-project uid that an image account reuses; no carried path is owned by one today. (2) Upload, stage and cancel are not serialized (they fail closed), and a killed helper's `upload.tar` is never removed. (3) A failed unmount in `prepare_root` lets the restore write over the still mounted root. | Defense in depth; functional | low | Y9 |
| AUD-13 | Refuted as a security finding: every directory named is root's or the server's alone, and the owner's writers already write 0600 with `fsync`; several of the audit's file names are wrong. Left: shortcuts and Wake-on-LAN read an unreadable file as empty and write in place without a lock, so a concurrent request or a torn write wipes every entry; `/etc/hosts` and `resolved.conf` are rewritten in place. | Functional | low | S10 |
| AUD-14 | The staged `previous` mode is refuted: that is the designed rollback. Persist errors are logged and the change still answers `Applied`, so a tightening that was not persisted is lost at the next boot. | Functional | low | S9 |
| AUD-15 | `http.get` wraps `params` twice. The PicoClaw history asks for 100 sessions and gets 20; there is no 400, as the query has no `deny_unknown_fields`. The system log's line count is ignored (always 200). | Functional | low | S13 |
| AUD-16 | (16.1a) The terminal's unlock does not check the lockout again after its permit, so N parallel guesses from one session pass a limit of 5 per 10 minutes. (16.1b) Its password has no length cap (axum's 2 MB body). (16.2) The server trusts `X-Forwarded-For` from any loopback peer. nginx overwrites the header, but the QUIC gateway, Janus, the encoders and PicoClaw's runtime reach 127.0.0.1:18080, and could guess logins from a fresh source each time or lock the operator's address out. The audit's download worker cannot: it is denied localhost. (16.3) The lock setter's race is theoretical: one poll, no yield. (16.4) The QUIC sweep drops a lease without releasing it; in the page, `/api/ws`'s teardown releases it, otherwise it lasts at most 30 s. 16.1c is refuted: an open terminal is cut, not waited for. | Session protection; defense in depth | low-medium (16.1a, 16.2); low; none (16.3) | S4, S5 with Y10, S8 |
| AUD-17 | (17.3) A forced key request is used up before a hardware encode that fails, so on a still screen the next key comes at the 2 s refresh; the key itself is not lost. (17.1a) Strides have no `i32` bound; unreachable. (17.1b) `vpx_img_wrap` is given data: pointer arithmetic only, and the audit's fix would leak (V3). (17.2) A short plane panics in safe Rust; unreachable from its caller. | Functional; hardening | low; none | Y11; optional |
| AUD-18 | (18.1) A certificate set while the link waits for the server's Hello is dropped, so WebTransport fails its hash check until the link reconnects or the next renewal. (18.3) `Started` goes out before the slow-key check, and the key wait is not tied to a stream, so a new stream's first key can be skipped (500 ms, not 1.5 s). 18.2 is refuted: no such loop exists. Agent: (18.4) an OVERLAPPED write left pending is unreachable with a 1 s non-alertable wait; (18.5) the mode's unchecked tag reads plain data through a bounds-checked index; (18.6) Hello authenticates nothing, so answering before it changes nothing. | Functional; hardening | low; none | Y12; optional |
| AUD-19 | (19.1a) With two PicoClaw connects in flight for one session, the first one's end releases the second one's lock, and the first relay's loop spins on a dropped close sender (V2). (19.2b) An upload is renamed over its name inside the field loop, before a second file part or the catalog can refuse it. (19.3) The password change is not serialized; only the operator can race it. Refuted: 19.1b (no messages are kept in the browser) and 19.2a (the handler sees the aborted body and removes its file). | Functional | low; very low | S14, S15, S16 |
| AUD-20 | A relative mouse button replayed as released after a restart is written to `hid.usb1` only, so `hid.usb2` keeps it pressed. GPIO line descriptors are a `RawFd`, never closed (init-failure paths only). Refuted as defects: the platform broker's inline wait (its peers are root and the server) and the missing flush (the 1 s tick flushes). | Functional | low | S11 |
| AUD-21 | `verify-systemd-units.sh` installs 6 of the 10 tmpfiles fragments (control-transition, firewall, hid-ledger and video are missing; the audit names others), and `--cat-config` passes a broken one, which `--create --root` refuses. | Tooling | low | Y13 |

## Found while verifying

- V1. The RAW update carries `/etc/hardened-blikvm` whole, `nginx.conf`
  included, so a board updated by RAW keeps the `nginx.conf` of the image it
  was first written from. A board written from 0.1.0-alpha.1's image lacks
  R22's headers (the Content-Security-Policy and two more), which came in
  alpha.2. Every release since has had the same file. The next system
  release changes it again (E13's TLS 1.2 ciphers). The test board has the
  new file only because gate E staged its configuration by hand. The same
  carry stops any later change of nginx, S5's proxy socket included.
  Hardening, low-medium (Y7).
- V2. In AUD-19.1a, the first connection's relay loop selects on a close
  receiver whose sender was dropped, and spins on a worker thread until it
  ends. Functional, low (S14).
- V3. The audit's fix for AUD-17.1b (a NULL data pointer) would make
  `vpx_img_wrap` allocate a frame buffer at every call and leak it.
- V4. Four of the findings are gaps in the 2026-10-18 work: AUD-03 in E2
  (rollover by count, not by size), AUD-04 (A) in E1 (the exec store's
  collection), AUD-05.2 in E4 (the template's PicoClaw directory) and V1 in
  E13 (for boards updated by RAW). Their tests passed; the cases they
  missed get tests of their own here.

## Where the audit is wrong

- Its section 4, the check of the 2026-10-18 fixes, cannot be relied on.
  Six of the commit IDs it cites do not exist (`4c469b0`, `42d1a00`,
  `2274b68`, `625a0ba`, `d23cebf`, `e6fa3fd`). Most rows describe code other
  than the item: B4 as removing `dangerouslySetInnerHTML` (it bounds the HID
  socket), C2 as the firewall process's timeout (it presents the checked
  image by its descriptor), D3 as the kernel module's checks (it opens the
  agent's channel unshared), E6 as Tailscale's key file (it makes the image
  store sticky), and E7 as root's password hashes (it limits the update
  confirmation to its client). Those items' record stays the 2026-10-18
  note's table and gates.
- Severities are overstated. AUD-01 is high, not critical. Of the eight
  "high" findings, AUD-03, AUD-04, AUD-05, AUD-06 and AUD-08 are medium,
  AUD-07 low-medium, AUD-02 low, and AUD-09 out of model.
- Facts. The root process that opens images is the board owner, not the
  control broker (`hardened-blikvm-control-broker.service` runs as
  `hardened-blikvm-control`). Some files and functions do not exist: a
  tmpfiles fragment for the download directory, `remove_slot_directory`, a
  `state_file.rs` in the broker, `FrameSource`'s spin loop, `scale_half`,
  and a `blikvm:picoclaw:messages:v1` key. Some values are wrong: the
  directory's mode (0755, not 0750), the stderr threshold (4096, not 1024),
  `SLOW_KEY` (500 ms, not 1.5 s), AUD-15's 400, AUD-16.1a's "60" guesses,
  and C5's `try_acquire_owned`.

## Fixes

The plan's items as they were carried out on `wip/corrective-audit-checkpoint`
(each commit names the test that failed on the code before it and passes
after). Slot 101 (0.1.0-beta.9, `eb071e8`) carries phase S; update runtime
`0.1.0~dev42`, the encoder and the RAW bundle of sequence 9 carry phase Y
(dev40 was staged at gate Y; dev41 adds GY1, found there, and dev42 GY2,
found before publishing; the RAW bundle of sequence 8 carried dev41).
What the board showed is in "Gates" below.

| Item | Finding | Commit | Test that failed before | State |
| --- | --- | --- | --- | --- |
| S1 | AUD-06 | `689ad22` | a clock that goes back runs the fan at its failsafe duty; a policy error drives maximum cooling, or is actuation uncertain | fixed |
| S2 | AUD-08 | `03805d6`, `eda1702` | a second change, a resolver edit, another interface and a failed change keep the window's first state; WiFi's too | fixed |
| S3 | AUD-07 | `5d8a30d` | a slow system setting leaves HID leases answered (the lease waited 1.9 s) | fixed |
| S4 | AUD-16.1 | `33adfe6` | parallel unlocks cannot outrun the lockout (10 checked, now at most 2); the password is bounded | fixed |
| S5 | AUD-16.2 | `f3d7ed3` | with the proxy socket, a loopback TCP peer counts as itself | fixed; local run of the server |
| S6 | AUD-11 | `788c6b1` | a FIFO or a link in the worker's directory reads as nothing; the install status is a small regular file | fixed (no blocking thread needed: the open cannot block) |
| S7 | AUD-05.1 | `fa432d1` | a file still open for writing is not taken | fixed |
| S8 | AUD-16.4 | `9a9bcef` | a session that ended hands back its lease | fixed |
| S9 | AUD-14 | `99b7b7f` | a mode that cannot be persisted is not applied or confirmed | fixed |
| S10 | AUD-13 | `96ad586` | parallel additions keep every entry; a damaged file is not written over (shortcuts and WoL) | fixed; the owner's `/etc/hosts` and `resolved.conf` left in place: its sandbox grants the files, not their directories |
| S11 | AUD-20 | `6949b2c` | a replayed button release clears both pointer interfaces; a dropped driver closes its line handles | fixed |
| S12 | AUD-09 | `f6504b7` | an image swapped for a FIFO is refused without waiting | fixed; the catalog's read not confirmed (a race between two calls, the check refuses a FIFO or a link) |
| S13 | AUD-15 | `f7a7413` | no `http.get` wraps its parameters | fixed |
| S14 | AUD-19.1, V2 | `72ead25` | a second connection of one session is refused | fixed; the loop's end on a lost sender by reading |
| S15 | AUD-19.2 | `d9cdc0f` | an upload refused after its file leaves the image of its name | fixed |
| S16 | AUD-19.3 | `12c812a` | two password changes at once leave both passwords the same | fixed |
| S17 | AUD-05.2 | `2ddf2d9` | (the route to the installer, beside the old one) | fixed |
| Y1 | AUD-01 | `8613c0a` | an interrupted write is undone from the backup; a record without a backup, or a backup it does not name, is refused | fixed; the page already asked to keep power: unchanged |
| Y2 | AUD-04 | `31f12d5` | an interrupted deletion in the exec store does not stop a preparation; a staging cut before its receipt is cleaned | fixed |
| Y3 | AUD-03 | `6392b4b` | native rollbacks beyond the journal's size are all admitted (the 81st was refused) | fixed |
| Y4 | AUD-05.2 | `6d23177`, `eb071e8` | no image download writes a PicoClaw file; only the pinned executable is copied to be started; dev40 adds the installer | fixed |
| Y5 | AUD-05.3 | `cfca978` | a download worker ends within its transfer budget | fixed |
| Y6 | AUD-02 (a) | `1f43864` | the running root's system sequence counts when it is newer (supervisor and exec helper) | fixed |
| Y7 | V1, AUD-02 (b) | `87a24c4` | a new root's nginx configuration is the image's with the carried redirect | fixed; at gate Y the RAW boot kept the image's file |
| Y8 | AUD-10 | `4a83ad3` | hyphen labels, a 3xx without Location or a 204, a cut inside a character, the special ranges | fixed |
| Y9 | AUD-12 | `044d885` | carried owners keep accounts of the same kind; a root still mounted is not written over; one request at a time; a stale upload removed | fixed |
| Y10 | AUD-16.2 | `de789f6`, `eb071e8` | nginx reaches the server through a socket of its own | fixed |
| Y11 | AUD-17.3 | `90823ae` | a key request the encoder did not answer is asked again at once | fixed |
| Y12 | AUD-18.1, 18.3 | `8618141` | a certificate set while the link comes up reaches the server; a new picture's key frame is not left out for the old one's | fixed |
| Y13 | AUD-21 | `21e3c97` | the unit check takes every tmpfiles fragment; one that does not parse fails it | fixed |
| GY1 | gate Y | `fd8222e` | a failed native boot is tried again (after 15 s, at most four attempts in ten minutes) | fixed; dev41 |
| GY2 | gate Y, before publishing | `4d40461` | nginx starts with the system's configuration: a board's file carried by an older boot step is replaced, the redirect kept | fixed; dev42 |
| Optional | AUD-16.1c, 16.3, 17.1, 17.2, 18.4 to 18.6, 19.1b, 20 (mode), 09.1b | — | — | not done: not chosen |

Flaky tests, named as the plan's rules ask: `gpio_atx_profile_pulses_through_the_supplied_runner`
(its client's timeout at load 19; alone 3 of 3), and in the checks on
`fd8222e` the page contracts `quic_share_contract.mjs` (15.8 frames a second
under its floor at load 17) and `screen_area_contract.mjs` (a point sent
before the page dropped a cleared layout; alone 3 of 3); the third run
passed in full. In the checks on `4d40461` `screen_area_contract.mjs` again,
`quic_video_contract.mjs` (the page's frame count started again under load;
alone 2 of 2) and `vp8_range_contract.mjs` (its test source under 3 frames a
second) twice, all under another project's build; the fifth run passed in
full. The first full checks also
found tests of this plan's own that a loaded host broke (the revert window's
600 ms, `eda1702`) and older tests pinned to the files as they were
(`eb071e8`); the check on `eb071e8` passed in one run.

Design notes (W2): `board-daemon-v1.md` (S1), `wifi-v1.md` (S2: the revert
window of IPv4 and Wi-Fi is described there; `network-writes-v1.md`, which
the plan named, covers DNS and IPv6 only), `system-raw-update-v1.md` (Y1,
Y6, Y7, Y9), `admin-features-v1.md` (S17, Y4, Y10) and
`native-execution-helper-v1.md` (GY1); the README names the restore tool
(Y1).

## Gates

What was shown on the test board, at the operator's word for each install
(evidence under `evidence/private/code-audit-20261020`, `fix-log.md`).

**Gate S** (2026-10-21). Read before: no failed unit, file leases enabled,
the image store on ext4, the exec store's `current` slot 99, PicoClaw not
installed. Slot 100 (`eda1702`, phase S) was built from a clean tree but not
installed: at the operator's choice the board took slot 101 (0.1.0-beta.9,
`eb071e8`, archive `fb7972c2…d408`, manifest `5497e3c4…`), which carries
phase S and the server's halves of phase Y. Installed through the API and
confirmed; the exec store's `current` became slot 101; no failed unit;
boardd and the fan running (the composition's readiness accepts boardd
only with its fan active and driven, or latched at full duty, and its
thermal sensor read, as at every later start below); the operator's state
as before but for the slot and the clock; the watching
probes passed (the GUI walk on desktop and phone, WebRTC H.264 and VP8,
QUIC video, the touch reach).

**Gate Y, the staged system** (2026-10-21). Update runtime dev40
(`efac9930…2dec`), PicoClaw's installer account (`hardened-blikvm-pico-install`),
the server units with the proxy socket, the gadget's tmpfiles, nginx's
configuration from packaging (`nginx -t` passed), the encoder of Y11 and an
initramfs with dev40's boot step were staged by hand, then the board was
rebooted. The first native boot of that boot failed (GY1): its helper
session ran under the boot's load and was refused with the coarse
`Controller` error; nothing tried again, so the board owner, boardd, the
video proxy and the server stayed down. A copy of the board's exec and slot
stores, run on the host through the same garbage collection and
preparation, passed, which points to the load, not the stores. At the
operator's word the unit was started once by hand and the composition came
up on slot 101 (the collection then removed slot 98). After it: no failed
unit; the server listened on the proxy socket, and during the probes nginx
reached it only there (279 of 400 one-second samples with connections on
the socket, none on the loopback port); the installer socket listened; the
root named system sequence 7; the six watching probes passed; the state as
before but for the IPv6 privacy addresses, renewed by the reboot, and the
clock; the application journal unchanged (84 records, 25,846 bytes).

GY1 (`fd8222e`) makes update runtime dev41, whose files are dev40's but the
native boot unit (every program byte for byte the same, the RAW boot step
included); the check on it passed in full on its third run (two page
contracts named above as flaky under load). At the operator's word dev41
was installed over dev40 without a reboot.

**Gate Y, the image** (2026-10-22). Read from the board with the KVM pages
closed, release 0.1.0-beta.9, system sequence 8: card image
`c472df64…226d` (`.zst` `f352a918…a8b`), root file system `9f14e1cc…778`
(`.zst` `de3e34da…763`); the archive unpacks to the image and its first
partition is the root file system. Inspected: dev41; the installer's and
the download worker's accounts; nginx's configuration equal to packaging,
with the proxy socket; the server units with the proxy socket; the native
boot unit's retry; the installer socket enabled; system sequence 8; the
gadget's tmpfiles equal to packaging, the catalog empty, the image store
01770; no jiggler or revert files; no SSH host keys; the exec store's
`current` slot 101. The image's initramfs, built on the board as it was
read, holds the same files with the same digests as the board's own, so
the board's RAW boot step is the image's.

**Gate Y, the RAW install** (2026-10-22). The RAW bundle of sequence 8
(0.1.0-beta.9, `fd8222e`, `91fbb05c…ca15`) was built from the image's root
and its signature verified. With root's home saved, it was uploaded,
staged, written and booted (the board was back after 35 minutes), and kept
the certificate, the SSH host keys, the account, the slot, the hostname,
the session lock and the SSH setting. On the new root: no failed unit; the
native boot ready at its first attempt; dev41 and system sequence 8; the
server on the proxy socket and the installer socket listening; nginx's
configuration the image's (Y7; this board carries no port-80 redirect);
the boot step the image's; the exec store slots 99 and 101 with `current`
on 101; the application journal unchanged. The operator's state as before
the install, but for the RAW sequence and the clock (the H.264 engine's
report appears once an H.264 viewer starts it; the engine choice was kept).
Confirmed: sequence 8, floor 9. Root's home restored. The six watching
probes passed again, none asked for the HID lease, and nginx reached the
server only through the proxy socket (275 of 400 samples, none on the
loopback port). PicoClaw is not installed on the board, so its checked copy
was shown by the host tests only (Y4).

A status worker's instance failed while its client, the server, left
during a page walk, as at gate E; it was reset. It comes back after page
walks and is left to a later item: a worker whose client has left should
end without a failure.

**GY2, before publishing** (2026-10-22 and 23). Preparing the release
showed that a board on beta.8 would not get the new nginx configuration: its
RAW update is written by its own boot step, beta.4's, which carries the
board's `nginx.conf` over the new system's (Y7's rule acts only from a boot
step that has it). The board would keep proxying over loopback TCP while the
new server units name the proxy socket, so the server would count every
login through nginx as one address, and a few wrong passwords from anyone
would lock everyone out; the TLS 1.2 ciphers of E13 would not arrive. The
test board did not show it: its configuration was staged by hand. At the
operator's word ("Сначала исправить") GY2 (`4d40461`) puts the system's
file back before every start of nginx; the check on it passed in full on its
fifth run (the first four stopped at page contracts under another project's
build, named above; the operator chose to wait for it to end).

At the operator's word update runtime dev42 (`275fa816…7bee`; against dev41
the administration helper, nginx's drop-in and reference file, every other
program byte for byte the same) was installed without a reboot. beta.8's
`nginx.conf` (beta.4's, proxying to the loopback port) was put in place and
nginx restarted: its start put the system's file back (the step's journal
line; the file the reference's, root's, 0600), and a login probe reached the
server only through the proxy socket.

The card image was read again with the KVM pages closed, release
0.1.0-beta.9, system sequence 9: card image `dc62d428…59c1` (`.zst`
`7a0db6ae…d2e4`), root file system `b088ffcf…1e32` (`.zst`
`2175fa93…1942`); inspected as above, with dev42, the reference file
(0644 root) beside `nginx.conf` (both packaging's) and nginx's drop-in; its
initramfs holds the same files as the board's. The RAW bundle of sequence 9
(`4d40461`, `1eab85f4…61a3`) was built from it, its signature verified, and
installed at the operator's word with root's home saved: back after 34
minutes, everything carried as before; on the new root no failed unit, the
native boot ready at its first attempt, dev42 and sequence 9, nginx's file
the system's, the exec store 99 and 101 with `current` on 101, the
application journal unchanged. Confirmed: sequence 9, floor 10. Root's home
restored; the six watching probes passed, none asked for the HID lease, and
nginx reached the server only through the proxy socket (281 of 400
samples). The status worker's instance failed once more during the probes
and was reset.

The board ends with slot 101 and the system of sequence 9 (floor 10), the
operator's switches and settings as found (the RAW switch was on and stays
on), root's home as it was, no failed unit, and nothing of this work left
on its data partition.

## Release 0.1.0-beta.9

Published on 2026-10-23 at the operator's word ("публикуй"), together with
the unpublished work of the security review of 2026-10-18, as the
pre-release
[`v0.1.0-beta.9`](https://github.com/woffko/Hardened-BliKVM/releases/tag/v0.1.0-beta.9)
(the annotated tag on `9e9a00d`, which adds only documents to the sources
named below). Its files, as uploaded and as GitHub reports them (SHA-256):

| File | From | SHA-256 |
| --- | --- | --- |
| `hardened-blikvm-application-0.1.0-beta.9-seq101.tar` | slot 101, `eb071e8` | `fb7972c297f5101e4f3579d1c3906f0273c5d8a8da9c4278a1aee49cadc6d408` |
| `hardened-blikvm-system-0.1.0-beta.9-seq9.tar` | RAW system update of sequence 9, update runtime `0.1.0~dev42`, `4d40461` | `1eab85f42b9d9f93b945c3147e0c50798d16c0c9fea716269c7dbc2191a361a3` |
| `hardened-blikvm-h616-sd.img.zst` | the card image of that system with slot 101 | `7a0db6ae0f06c5f519645e0030501c9053b466aa96d5de6ae91efada8316d2e4` |
| `IMAGE-INFO.txt` | the image's description | `5532fb225cf58d213f3f14268a48b2b6996c913d8bbe07442e272eb67f17ab28` |
| `hardened-blikvm-agent.exe` | the target agent, the same bytes as on slot 101's drive | `1265ac120ef29f6f88b53654cce61c43334ccc78bf36f365eec8217ed487395a` |
| `SHA256SUMS` | the checksums above and the unpacked image's (`dc62d428466034575ac6d681203f5085c11b13269cd419a037657337e8d959c1`) | |

What was checked is in the gates above: `scripts/check.sh` passed in one
run on `9e9a00d`; `cargo audit` reports RUSTSEC-2026-0382, not reachable
(`docs/third-party-rust.md`). beta.8's and beta.4's notes now say that
beta.9 supersedes them. The test board is left with this release's
application and system, the operator's settings and switches as found.
