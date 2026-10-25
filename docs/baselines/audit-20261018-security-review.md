# Verification of an external security audit (2026-10-18)

The operator handed over a static security audit of the repository made by
another assistant on 2026-10-18 (read only: the Rust crates, the kernel
module, the native encoders, the Windows agent, the page, packaging and
scripts; kept as `evidence/private/security-20261018/external-audit-report.md`)
and asked for a fix plan for the findings that hold and that fit the
device's model ("устройство, где пользователь имеет полные права доступа к
устройству"). This note records what each finding turned out to be; the
plan is `docs/baselines/work-plan-20261018-security-fixes.md`. Finding names
(CRIT-01, HIGH-05, MED-09, LOW-04, ...) are the audit's; N1 to N10 were
found while verifying it.

## The model

The board is the operator's. The single web account is the operator's, and
through it the operator may by design become root (the admin helper's root
terminal), change the root password, switch on SSH, Tailscale, scripts and
RAW system updates. The operator also has physical access to the board and
its card, and the board has no secure boot. So:

- Out of model: whatever only the operator can do (an authenticated, unlocked
  session, root, the card) and that gives nothing the operator does not
  already have.
- In model, direct: parties without the operator's rights: unauthenticated
  clients on the network (LAN, Internet, Tailscale peers), other origins in
  the operator's browser, the KVM target as a USB host (it must not reach
  board files beyond the image presented to it), servers the board downloads
  from, forged or replayed updates.
- Session protection: the session lock after idle time and the terminal's
  password check exist to protect an unattended browser; someone holding a
  live session but not the password must not get past them.
- Defense in depth: a compromised unprivileged component that does not hold
  the operator's authority (the image download worker, the PicoClaw runtime,
  the video encoders, the QUIC gateway, Janus) must not reach root or that
  authority. The web server brokers the operator's authority by design, so
  an escalation that starts from the web server's own account gains nothing
  in this model, unless another component shares that account (N1) or the
  path is silent and survives reboots where the brokered one is visible.
- Bugs that cost function or availability are in model as bugs.

## Method

Four verifications ran side by side, one per area (the board's privilege
boundary; the RAW system update and the update stores; the web server and
sessions; the kernel module, the QUIC gateway, the agent and the units).
Each claim was checked against the code at `89bc1e8`; the audit's line
references were often stale. Small experiments settled what reading could
not: GNU tar 1.35 with the RAW update's own flags (links, setuid bits,
`security.*` attributes), `tune2fs -U` with malformed values, and the import
table and load configuration of the published agent (0.1.0-beta.8). The
board was read once, read only (`evidence/private/security-20261018/board-facts-20261018.txt`):

- systemd 249 without the BPF framework: `SocketBindAllow`/`SocketBindDeny`
  are not enforced there; `IPAddressDeny`/`IPAddressAllow` work through the
  kernel's cgroup BPF (`CONFIG_CGROUP_BPF=y`, no "not supported" in the
  journal);
- `fs.protected_hardlinks=1`, `fs.protected_symlinks=1`;
- the application journal holds 80 history records of its 128;
- the slot roots hold nothing but slots and `current`;
- the virtual media partition (1.3 GB used) is mounted inside
  `/var/lib/hardened-blikvm-gadget`, a path the RAW update carries.

## Findings

| Finding | What holds | Class | Severity | Plan |
| --- | --- | --- | --- | --- |
| CRIT-01 | `/etc/hardened-blikvm-gadget` is the web server's; the root board owner reads `atx.env` and `system-action.env` from it (the reboot and shutdown commands it runs as root after a confirmed action), and root boardd reads `boardd.json` without an owner check. LD_PRELOAD refuted: the owner is a static binary. | Defense in depth: silent and lasting root, reachable from the download worker (N1) | medium | E5 |
| CRIT-02 | `validate_image_path` follows a symlink (`metadata`, not `symlink_metadata`); the path goes to `lun.N/file` and the kernel opens it as root, so a link planted in the image store presents any root file, writable if the catalog allows, to the target. Hardlinks: blocked by `protected_hardlinks` and the separate partition. | Defense in depth (planting needs the web server's account, N1); crosses the target's boundary | medium | C2 |
| HIGH-01 | The switches change through `control.sock` without a presence check; the root password, Tailscale and SSH have no switch. | Out of model: the operator's own powers, brokered by design. The switches stop a client that skips the server's routes, not the server. Other group members can flip a switch but cannot use the feature (the admin helper admits only the server's uid): MED-01. | none | record |
| HIGH-02 | The boot step trusts `apply.json`; `state.tar`, `fstab` and `shadow` have no digests. | Out of model: only root writes the staging directory (0700, root units), and whoever holds the card can rewrite the root anyway | none | E3 (digests, against a damaged card) |
| HIGH-03 | The carried state is archived and restored as root with owners, modes, ACLs and attributes. Escalation refuted: links are stored as links, absolute ones restored last, `security.*` attributes not restored, hardlinks protected on the board, and every planted file comes back with the owner it had. Residual: N5, and tar's exit 1 for a file that changes while read stops the staging. | Defense in depth | low | E3 |
| HIGH-04 | Not carried: `-update`, and the live `-update-v2`, `-slots-v2` and `-exec-ledger`. After a RAW update the board starts the image's confirmed application, silently, with the image's floor; documented, and the release procedure installs the release's application first. | Functional (only the operator installs) | medium | E3 |
| HIGH-05 | The login limiter keeps at most 1024 keys and admits a new one only by evicting an unlocked key; a table full of lockouts records nothing more, and a key it does not hold is never locked. Wider than reported (N9): one table for every account and source, shared with the terminal's password check. | In model, direct: unlimited password guessing from one address, at Argon2's speed | high | B1 |
| HIGH-06 | With the lock on, a session's fixed deadline is never later than its idle lock, so its own streams cannot outlive it (that part refuted). Holds: tightening the lock retimes only the caller; other sessions keep their old deadline, up to 24 h, for their open streams. | Session protection | low | B3 |
| HIGH-07 | The lab module is built beside the encoder. Shipped or autoloaded: refuted (the bundle, the runtime package and the image take only the encoder module, and nothing loads a module on a device match). Its whole-memory DMA is a documented lab property (root only); the use-after-free is refuted (release runs at the last reference, which every mapping holds); its reset ioctl does not mask the interrupt. | Out of model (root-only lab tool) | none | E16 (optional) |
| HIGH-08 | `hid.dll` is imported statically and the load configuration sets no `DependentLoadFlags`, so the folder the agent runs from is searched first; `wtsapi32.dll` refuted (not imported). The agent serves clipboard and layout calls while Windows is locked (it checks only the console session). | In model, low: planting needs a folder others write to (a downloads folder, never the board's read-only drive or the user's own copy); the locked clipboard gives the KVM's operator what Windows' lock withholds (medium where that operator is not the Windows user) | low | D1, D2 |
| MED-01 | `control.sock` checks no peer: every `hardened-blikvm` member (the encoders included) can drive the root owner (ATX, reboot, network, media, switches). The platform broker checks no peer either (reads). The update confirmation runs without `--peer-user` on a group-writable socket. boardd refuted: its socket directory is root's. | Defense in depth | medium (owner), low (others) | C1, C5, E7 |
| MED-02 | `serve_picoclaw` checks no switch; the workspace path from `config.json` is not confined. Contained by the sandbox (its own dynamic user, a read-only system, `O_NOFOLLOW`); the runtime's binary owned by the server is deliberate. | Defense in depth | low | E16 (optional) |
| MED-03 | The Janus unit lacks `udp:5006`, which the board's systemd does not enforce (N6), so H.264 works there: latent. PicoClaw's runtime shares the host's network and reaches Janus's API (no secret) and RTP ports: it could watch the screen or inject RTP, which its profile withholds. | Functional (latent); defense in depth | low; medium-low | E8 |
| MED-04 | The password route asks for no current password and sets root's password before the account's. | Session protection: a live session without the password (a stolen cookie with its CSRF token, or XSS) sets a new one and then passes the terminal's password check | medium | B2 |
| MED-05 | Private ranges are reachable (the unit denies only localhost, link-local, multicast); `curl` runs without `--globoff`, and glob characters pass outside the file name. Query strings refuted (cut before the `.iso` check). Wider (N8): redirects are followed into private ranges. | In model through a redirect (a remote server makes the board send GET requests into the operator's network), low; the operator's own URL is out of model, its globbing a correctness bug | low | E9 |
| MED-06 | A zero-byte upload is renamed over an image of the same name before the catalog refuses size 0: the image is lost. A remote download replaces a file attached meanwhile. The catalog's temporary name is fixed, without lock or `O_NOFOLLOW`. Scripts refuted (`O_NOFOLLOW` is set). | Functional | low-medium | B5, B6, E10 |
| MED-07 | A missing or unreadable system floor reads as 1 (root or the card only); a fresh card accepts any signed system bundle, also one older than its own system. The "application sequence 1" refuted: those arguments are the recovery and boot-control sequences, 1 in every bundle. | Functional anti-rollback gap (only the operator uploads) | low | E3 |
| MED-08 | `root_uuid` is not validated. Injection refuted: the value is root's, and `tune2fs` refuses a non-UUID before anything else is written, which restores the backup. tune2fs's keywords (`random`, `time`, `clear`, `null`) pass. | Out of model; robustness | low | E3 |
| MED-09 | Temporary files land in the slot roots and cleanup ignores them, so an interrupted copy stops every later upload. Worse (N2): the protected exec store uses the same stager, and every boot prepares the confirmed slot through its garbage collection, which refuses any unknown name: one leftover stops the board owner, video proxy, boardd and web server from starting at every boot. | Functional (a power loss or a timeout during an update) | high | E1 |
| MED-10 | The frame channel is unbounded and the page's `Received` value unchecked; unbounded memory refuted (a group is dropped when the transport has not taken it, key frames come every 2 s, 8 connections, `MemoryMax=64M`); only a page of an authenticated session can send it. | Out of model | none | E16 (optional) |
| MED-11 | `/api/ws` keeps the 64 MiB message default (a HID frame is 16 bytes). The PicoClaw gateway takes its lock before connecting upstream, so manual input is blocked up to two 10 s timeouts when the runtime is down. | Session protection, low; functional | low | B4 |
| MED-12 | 1: the owner's state files come out 0660 under `UMask=0007`, latent (their directory is root's 0750). 2: `/mnt` files without `O_NOFOLLOW`, out of model (root-only directory). 3: the image store has no sticky bit, which lets the server's account swap a root file for a link (CRIT-02). 4: a damaged status image makes boardd abort on every start (fan, LEDs, OLED lost). | 1, 3: defense in depth; 2: out of model; 4: functional | low | C3, E6, E12, C4 |
| MED-13 | `ioctl` refuted (declared variadic). `frames.sock` 0666 refuted (each peer's uid is checked). One thread per connection: holds, bounded by `TasksMax`; `demand.sock` checks no peer. The agent opens the HID device shared: another process of the same user gains nothing, another Windows user little. Calls before Hello: out of model. | Defense in depth; low | low | E11, D3 |
| LOW-01 | Activations are refused at 128 history records (the journal stays valid). The test board has 80; a card written from an image starts with the source board's history. | Functional | medium | E2 |
| LOW-02 | `umount` skipped: refuted. `e2fsck -p`'s status is ignored. | Robustness | low | E3 |
| LOW-03 | Some handlers check only for a control socket, not the restricted-surface flag; `normalize_interface` accepts `all`, `default`, `lo`, `.`, `..` (no traversal: the last component is fixed). | Out of model; consistency | low | B7, C6 |
| LOW-04 | The image keeps `mouse-jiggler.env` and the board's `nginx.conf`; network revert records partly; `firewall-revert.json` refuted (on tmpfs). | Image hygiene | low | E14 |
| LOW-05 | The serial path's character range is wrong and missing picocom options become "null" (not exploitable: shell characters are refused first); a `Math.random` fallback off HTTPS; logout leaves PicoClaw's keys. | Functional, hygiene | low | B8 |
| LOW-06 | The interrupt handler claim refuted (no such code); `memset` and alignment refuted; the header bits are checked by the kernel. | none | none | record |
| LOW-07 | `msd-writable` has no sandbox, `system-update-booted` no address filter, `verify-systemd-units.sh` misses five units and most exposure thresholds, nginx sets no TLS 1.2 cipher list. | Hardening | low | E13 |

## Found while verifying

- N1. The image download worker runs as the web server's account. It is the
  one unit that fetches remote data, so its compromise holds the operator's
  authority: the admin helper admits its uid, it reaches `control.sock`,
  and it can write the gadget directory (CRIT-01) and the image store
  (CRIT-02). Defense in depth, medium (E4).
- N2. The exec store's leftovers stop the application at every boot
  (MED-09). Functional, high (E1).
- N3. RAW staging archives the virtual media store: `tar` runs without
  `--one-file-system` while the media partition is mounted inside a carried
  path. With images larger than the data partition the update fails; the
  restored copies hide under the mount point and take root space.
  Functional, medium (E3).
- N4. A failed RAW stage leaves its payload and `.new` files, which the next
  upload's space check counts and nothing removes. Functional, low (E3).
- N5. The carried state keeps numeric owners while users are allocated per
  image: an image whose allocation differs gives one service's files to
  another. Defense in depth, low (E3).
- N6. The board's systemd does not enforce `SocketBind*`, so the Janus and
  QUIC units' port lists do nothing there, and the QUIC design's "one port"
  holds only where systemd has its BPF framework. Hardening and a document
  correction, low (E8, E13).
- N7. "Start the agent" runs `hardened-blikvm-agent.exe` from every ready
  volume labelled `BLIKVMAGENT`, so an image on LUN 0 with that label, for
  example one downloaded from the Internet, is run as well. In model,
  low (D4).
- N8. Remote downloads follow redirects into private ranges (MED-05).
- N9. The login limiter is one table for the whole device and the terminal's
  password check (HIGH-05).
- N10. The audit's list of strengths names scrypt; the passwords are Argon2.
- N11. Found at gate B–D on the board: the board has no clock that keeps
  time while off, and the QUIC gateway starts at boot before the network
  time is synchronised, so its 13-day certificate is made by the clock of
  boot (on 2026-10-18 dated 2026-07-29) and replaced only after 7 days of
  running; until then the server refuses every QUIC session ("clock") and
  the page uses the standard transport. Functional, medium (E17).

## Fixes

The plan's items as they were carried out on `wip/corrective-audit-checkpoint`
(each commit names the test that failed on the code before it and passes
after). "Board" says where the fix was seen on the test board: slot 97
(0.1.0-beta.9, `1ab799d`) carries phases B to D; phase E is the system
release's and is seen at gate E.

| Item | Finding | Commit | Test that failed before | State |
| --- | --- | --- | --- | --- |
| B1 | HIGH-05, N9 | `946b5d9` | the limiter counts a source whatever the name; a full table still locks | fixed; slot 97 |
| B2 | MED-04 | `05d2ff7`, `fa8de18` | a password change without the current password is refused; root's password follows the account's | fixed; slot 97 |
| B3 | HIGH-06 | `eb9ba47`, `78d2804` | every live session, a QUIC page's too, is capped at a tightened lock | fixed; slot 97 |
| B4 | MED-11 | `33c2e9d` | `/api/ws` refuses a message over 64 bytes; PicoClaw's lock is taken once upstream answers | fixed; slot 97 |
| B5 | MED-06 | `f3c48d3` | an empty upload leaves the image of its name | fixed; slot 97 |
| B6 | MED-06 | `7a376e4`, `a43edb2` | catalog changes under a lock, through a file of their own | fixed; slot 97 |
| B7 | LOW-03 | `417c24d` | (passed before) | not confirmed: every unit with the owner's socket has the restricted surface; pinned by a test |
| B8 | LOW-05 | `4b7f03c` | the serial path's pattern, picocom's options, the UUID fallback, PicoClaw's keys at logout | fixed; slot 97 |
| B9 | E5's first step | `b91a95a` | (passed before) | not needed: the slot takes the catalog's path from the system's units; pinned by a test |
| C1 | MED-01 | `fa561cc` | the owner's socket refuses a peer other than root and the server | fixed; slot 97 (no peer refused on the board) |
| C2 | CRIT-02 | `f943eef` | a link or a second link in the store is refused; the LUN is given the checked file's descriptor | fixed; slot 97 (LUN 0 reads back the real path) |
| C3 | MED-12 | `2833f19` | the owner's state files are 0640 | fixed; slot 97 |
| C4 | (boardd) | `d57365d` | a damaged status image is refused, not a panic | fixed; slot 97 |
| C5 | MED-01 | `9104bdd` | the platform broker refuses another peer | fixed; slot 97 (disabled on the board) |
| C6 | LOW-03 | `e7591a6` | `all`, `default`, `lo`, `.`, `..` are refused | fixed; slot 97 |
| D1 | HIGH-08 | `158da00`, `2ec7452` | clipboard, layout and monitors are held back while Windows is locked; the session's own lock flag counts | fixed; slot 99, the operator's check at gate B–D (the desktop test alone missed Windows 11's lock screen on slot 98) |
| D2 | HIGH-08 | `5f82f8e` | `hid.dll` from the system folder only; the import check of the agent | fixed; the operator's check at gate B–D (slot 98) |
| D3 | MED-13 | `d3c1860` | the agent's channel is opened unshared | fixed; the operator's check at gate B–D (slot 98) |
| D4 | N7 | `446992c` | the start line runs only a file with the board's agent's SHA-256 | fixed; the operator's check at gate B–D (slots 98 and 99) |
| E1 | MED-09, N2 | `b286e56` | no stager temporary in a store's root; old ones are cleaned, also by the exec store | fixed; gate E |
| E2 | LOW-01 | `dae71b2` | the journal rolls its history over at 64 finished records | fixed; gate E |
| E3 | HIGH-02, HIGH-04, MED-07, MED-08, LOW-02, N3, N4, N5 | `c83cf11`, `43e7df2`, `49e48b6`, `de4677f`, `6ccb846`, `ed83bde`, `591888b`, `5ca99ad`, `7ffd42a` | the update state carried; one file system and a retry; a failed stage cleaned; carried owners checked; a canonical UUID; the running system's sequence in the floor; staged digests; `e2fsck` status 4 stops; nothing staged left once kept | fixed; gate E (the last after it, see below) |
| E4 | N1, MED-06 | `bd62801`, `724e9de` | the worker's own account (a sysusers fragment of its own beside the pinned identities); it fills only the file the server prepared; the server publishes | fixed; gate E |
| E5 | CRIT-01 | `73da924` | no root unit reads a configuration under a directory another account may write; the catalog copied once | fixed; gate E. boardd's own check of its directory ships in the first slot after the system release |
| E6 | MED-12 | `ad21c51` | the image store is sticky | fixed; gate E |
| E7 | MED-01 | `05e95d3` | the confirmation refuses another account | fixed; gate E |
| E8 | MED-03, N6 | `04a0c69`, `eb07bc0` | Janus's own loopback address, given to `lo` under a label; every non-client unit denies it; every Janus port in its unit | fixed; gate E (WebRTC probes) |
| E9 | MED-05, N8 | `bb7be32`, `2372b64` | glob characters refused; `--globoff`; a public-to-private redirect refused | fixed; gate E |
| E10 | MED-06 | — | — | not needed: after E4 the helper writes no catalog (pinned by a test) |
| E11 | MED-13 | `046fcb7` | the demand socket refuses another account and closes connections beyond its cap | fixed; gate E |
| E12 | MED-12 | `5b5803a` | the owner's umask 0027 | fixed; gate E |
| E13 | LOW-07, N6 | `0a1ac89` | the verifier covers every unit; the remount's and the boot report's sandboxes; nginx's ciphers | fixed; gate E |
| E14 | LOW-04 | `66fcf5e` | the image leaves the jiggler, the revert records and the board's `nginx.conf` | fixed; gate E |
| E15 | RustSec of beta.8 | `b35e147` | `cargo audit`: five advisories before, none after | fixed; gate E |
| E16 | MED-10, HIGH-07, MED-02 | — | — | not done: the operator's choice (2026-10-18) |
| E17 | N11 | `9ff9a62` | a certificate made by a clock months behind is replaced | fixed; gate E |

## Gates

Gate B–D (2026-10-18 and 19). Slot 97 (0.1.0-beta.9, `1ab799d`) went on at
the operator's word and was verified on the board: services, the state as
found but for the slot, the owner refusing no peer, LUN 0 reading back the
checked file by its path, the watching probes; QUIC video failed with
"clock" (N11, fixed by E17). The operator's checks on the target ran on
slot 98: the agent updated within 10 s (D4, D2), its test tool was refused
while it ran (D3), and with Windows locked a clipboard read from the phone
was refused, but as "another program holds the clipboard": the agent's
lock test, the desktop in front, still read `Default` behind the lock
screen of Windows 11 26H2. With the session's own lock flag (`2ec7452`,
slot 99) the read was refused as "the target's session is locked", the
clipboard read again after unlocking and the agent updated within 10 s.

Gate E (2026-10-18 and 19). From `724e9de` (check green in one run): slot 98
and update runtime `0.1.0~dev39`, whose files are dev38's and the download
account's sysusers fragment. At the operator's word: slot 98, then the
system staged by hand as the board's earlier system was (the package, the
account, the gadget's tmpfiles with the catalog's copy, the units in
`/etc/systemd/system`, which were packaging's at `89bc1e8`, nginx's
configuration, the video path with `deploy_vp8.sh`), and a reboot. Janus
then took neither address (`eb07bc0`): fixed on the board at once, WebRTC
probes passing. Slot 99 (`2ec7452`, check green with `screen_area_contract`
flaky under load and the check resumed from it) carried D1's fix. The card
image was read with the KVM pages closed (0.1.0-beta.9, system sequence 7)
and inspected: the download account and its membership, the gadget's
tmpfiles from packaging, no catalog in the new place and an empty one in
the former, nginx's configuration from packaging, no jiggler or revert
record, the image store 1770, the configuration directory root's, the
system sequence, dev39, Janus's unit and addresses, the units of the new
catalog, the owner's umask, the exec store starting slot 99. Its RAW bundle
(sequence 7) was installed with root's home saved: the first install wrote
nothing, as the board's initramfs was still beta.4's, whose boot step
refuses dev39's record (the staging by hand had not rebuilt it); at the
operator's word the image's initramfs, built on the board, replaced it,
and the second install wrote, booted and was confirmed. Carried: the TLS
certificate, the host keys, the account, the slot, the hostname, the
session lock, SSH, the switches (SSH, QUIC, Tailscale, mDNS) and the
catalog; the watching probes passed, QUIC video included; Janus listens on
its address alone. Confirming left the staged accounts' copies, which
`7ffd42a` removes (removed by hand on the board; the image of this gate
carries the runtime without it). The application journal holds 83 records
of 128: E2's rollover, tested on the host with 200 activations, first acts
on the board at its 128th, so the board's journal was not pruned in this
gate, only no longer stuck at the cap. The slot roots hold nothing but
slots and `current`. The board ends with slot 99 and the
system of sequence 7, the operator's switches and settings as found, root's
home as it was, and no unit failed (a status worker's instance that failed
when its client hung up was reset).

Flaky tests, named as the plan's rules ask; each passed alone or in a full
run without the load that stopped it:
`on_screen_keyboard_contract` (the second confirmation's race, gate B–D);
`screen_area_contract` (the pointer read before the page has the whole
desktop again, two in three alone at load 14); `quic_video_contract` and
`mjpeg_upscale_contract` (the page's upscaler draws in headless Chromium on
the CPU: a frame dropped, or fewer than 90 % of the frames drawn, while
another session's fuzzers held the host at load 12 to 18); the update
supervisor's `upload_worker_stages_retries_and_rejects_without_activation`
(its client's 5 s read under that load). The final full check on the final
commit ran after those fuzzers had ended.

Artifacts: slot 98 `86e40c76…8b93`, slot 99 `fcddf946…704a`, agent of slot
99 `d31c0d5e…8f7a`, update runtime dev39 `80095d07…0187`, encoder
`e42ac68a…78fb`, card image `603f3972…64f2` (zst `7656c9fe…0a2b`), system
root `d2e4e2d2…87be`, RAW bundle sequence 7 `c735993d…c549`.
