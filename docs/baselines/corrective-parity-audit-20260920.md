# Corrective parity audit baseline (2026-09-20)

This baseline reopens reference parity after the prior acceptance harness
reported `overall=complete`. It distinguishes source-confirmed defects, live
read-only observations and hardware checks that still require a controlled
device gate. Historical receipts remain evidence of the narrower checks they
performed; they do not prove requirements that those checks did not exercise.

## Version and device baseline

| Subject | Baseline |
| --- | --- |
| BLi-KVM source | `95a051a` (`main`) at the start of the corrective work |
| Hardened NanoKVM reference source | `origin/main` at `ff7f42c9605e232056ac2ebe90a70dab4c817245` |
| Imported web provenance | `387793ab944437bbedc534d2ba231dd88d1fd2ca` |
| Vendor video source map | `/mnt/d/debug/blikvm_v4_video_settings_source_map_for_agent.md` (operator-supplied, host-local) |
| Installed application | confirmed sequence 14, generation 4, anti-rollback floor 14 |
| Serving unit | `hardened-blikvm-server-current.service` |
| System partition | about 5.0 GiB usable; 4.8 GiB used and 158 MiB available at audit time |
| Data/staging partition | about 12 GiB usable; about 522 MiB used at audit time |
| Virtual-media partition | about 9.6 GiB usable; the authorized `kubuntu.img` removal left 24 KiB used and 9.1 GiB available |
| Current-card layout | unchanged; no repartitioning was performed |

Private addresses, credentials and device receipts remain under the ignored
`evidence/private/` boundary. The source commit above describes the repository,
not every board-local systemd override; release reproducibility is therefore an
open gate.

## Corrective requirement matrix

Plan phase A 2026-09-26 (`docs/baselines/work-plan-20260926.md`, design
`docs/design/admin-features-v1.md`). Administration features are offered but
switched off by default; the board owner persists the switches in
`/var/lib/hardened-blikvm-gadget/admin-features.json`, the web server refuses a
switched-off feature (403) and the helper doing the work re-reads the switch
and refuses on its own. Verified on the board through the API, each probe
leaving its switch off:

- Terminal (slot 28): refused while off; with the switch on, a password
  unlock issues a one-time ticket and the WebSocket opens a root shell on a
  PTY with window resize; a reused ticket is refused
  (`evidence/private/terminal-switch-20260926-b.json`,
  `tools/playwright/terminal_switch_probe.mjs`). The serial terminal page
  types `picocom` into the same shell; `picocom` is installed on the board.
- Scripts (slot 29): refused while off; upload, list, foreground run (root,
  bounded output), background run, invalid names refused, delete
  (`evidence/private/scripts-switch-20260926-a.json`,
  `tools/playwright/scripts_switch_probe.mjs`).
- Remote ISO download (slot 30, update runtime 0.1.0~dev23): an unprivileged
  helper unit that is the only one with outbound network access. Refused
  while off; invalid schemes and non-ISO names refused; a second download
  while one runs is refused; a 64 MiB ISO fixture served on the board's LAN
  address downloaded with progress (24.9 % to 94 % observed), appeared in the
  image list and was deleted; a file without the ISO 9660 magic reported
  `failed` and left no file (`evidence/private/remote-download-switch-20260926-a.json`,
  `tools/playwright/remote_download_probe.mjs`). A URL on the board's own
  loopback failed under the unit's IP policy and published nothing.
  Switching the feature off stops a running download (unit test); the
  reference has no separate cancel control. The Download menu, which also
  carries the local upload, is now offered whenever the image store is
  configured.

- PicoClaw (slots 31-33, update runtime 0.1.0~dev24): refused while off;
  with the switch on the pinned v0.2.8 arm64 release installed from the
  upstream GitHub release with progress (download, extract, install), a
  model was configured and the key never appeared in any API response, both
  agent profiles were applied, and the runtime started. Through the real
  assistant sidebar a chat reached a scripted OpenAI-compatible model on the
  board's LAN address (`tools/device/fake_openai_model.py`), which made the
  runtime call the KVM screenshot tool (the image reached the browser as an
  observation) and the actions tool (one HID report written through the
  operator's lease); manual input was held back as `assistant_active` while
  the operator's control stayed on; the chat appeared in the history under
  the runtime's opaque key and was read and deleted; switching off stopped
  the runtime; uninstall removed the binary and the runtime's home
  (`evidence/private/picoclaw-switch-20260926-c.json`,
  `tools/playwright/picoclaw_switch_probe.mjs`). The PicoClaw Appearance
  switch now changes the server switch, and the menu bar takes the terminal,
  scripts and PicoClaw visibility from the server switches. A real model
  needs the operator's own API key; that check is the operator's.
- Local image upload: the Download menu is shown with its file input and
  the remote URL field (switched off) on slot 33.

After the phase A probes every administration switch is off on the board,
PicoClaw is uninstalled and no test fixture is left running.

Plan phase B 2026-09-26 (slots 34-35, update runtime 0.1.0~dev25), verified
on the board through the API (`tools/playwright/device_settings_probe.mjs`,
`evidence/private/device-settings-20260926-b.json`, passed):

- About lists the board's address (from the owner, read through
  NetworkManager) and the system image version (the image builder now
  records it in `/etc/hardened-blikvm-image`; this hand-built board falls
  back to the OS release: Armbian 26.8.3 jammy, Ubuntu 22.04.5 LTS).
- Diagnostics observes the HID gadget (USB controller bound and
  enumerated), the ATX surface and the virtual-media surface.
- SSH switch: off stopped and disabled the OpenSSH unit and port 22 closed;
  on restored both. The owner skips the SysV compatibility step, which
  cannot write outside its sandbox (found by the first probe run).
- Network: the reference DNS page with DHCP or manual IPv4. Manual with the
  current address, mask and router reported a pending revert (119 s), was
  kept, then DHCP again, kept; the address stayed reachable throughout. An
  unconfirmed addressing change reverts after two minutes (unit test).
- The session lock duration is persisted (set, read back, restored) and was
  still in force after the reboot.
- USB gadget reconnect applied and the target host enumerated the gadget
  again; frame detect off/on and the reference pause call answered; the
  streamer takes the setting from its environment (`--drop-same-frames`).
- Tailscale as in the reference (`tools/playwright/tailscale_probe.mjs`,
  `evidence/private/tailscale-20260926-a.json`, passed): stop, start,
  restart, a login URL from Tailscale, uninstall (apt, 52 s) and install
  from Tailscale's repository with our pinned key (47 s); the board ended
  installed and running, not logged in. Completing the login needs the
  operator's Tailscale account.
- KVM reboot from Device settings (`tools/playwright/kvm_reboot_probe.mjs`,
  `evidence/private/kvm-reboot-20260926-a.json`, passed): refused without
  the operator's confirmation, rebooted with it and served the GUI again
  after 61 s; every unit active, HID gadget configured, all administration
  switches still off.
- USB wakeup: declined by the operator on 2026-09-26; not implemented.
- The board's streamer unit in `/etc/systemd/system` was replaced by the
  packaged one (backup in `/root/backups`); new images carry it.

Plan phase C 2026-09-26: mobile (Pixel 7 profile) and desktop text walks of
every toolbar menu and settings tab against the reference 2.0.42
(`evidence/private/parity-20260926/`). Gaps found on mobile and fixed (slot
36, verified again by the walk and the device probe
`evidence/private/device-settings-20260926-c.json`): About now lists the IP
addresses, image version and hostname in the reference order (the hostname
is editable; the owner applies it through systemd-hostnamed and /etc/hosts,
renamed and restored on the board); IPv6 offers DHCPv6 and manual as well
(DHCPv6 and back to SLAAC on the board); the image dialog has the per-image
delete and USB reconnect. The IPv6 addresses the owner reads are unescaped
and listed on About from slot 37.

Remaining differences, identical on desktop and mobile, for the operator's
approval:

- Off by default by the operator's decision: Terminal, Scripts, PicoClaw and
  remote ISO download (the reference test device has them on).
- Our additions: the "Enable remote control" confirmation (the focus model
  that binds keyboard and mouse to the visible target) and the Diagnostics
  tab.
- Not applicable to this board: the JPEG quality control (the capture
  encoder ignores it), Virtual Disk, Virtual Network and OLED (hardware the
  board lacks), the memory limit and swap settings (for the reference's
  256 MB board, shown there under Device > Advanced and Tailscale).
- Declined by the operator: USB wakeup.
- Placed differently: HID-only is a Device setting, not in the mouse menu;
  Reset HID is not offered; USB reconnect in the image dialog re-enumerates
  the whole gadget, HID included.
- Updates: signed application updates instead of the reference's preview
  channels and offline packages; RAW system updates are plan phase E
  (implemented later the same day as a signed upload with its own default-off
  switch).

The operator approved this list on 2026-09-26, together with RAW system
updates being off by default and uploaded rather than fetched from a feed.
Still waiting for the operator: a check on the phone.

Observed while testing: the media catalog still lists `kubuntu.img` although
the file is no longer in the image store; the catalog entry is stale and is
left for the operator to delete from the image list.

Plan phase D 2026-09-26 (firewall on the device): not started, as the plan
requires. Migrating the legacy iptables rules and enabling the owned nft
table can cut the board off the network, so it needs a recovery path first:
a UART console, physical access to the board, or a card with the current
image that the operator has checked boots. Nothing on the board's firewall
was changed. The operator chose the card from the phase E1 boot check as
the recovery path, so phase D starts after that check.

Plan phase E1 2026-09-26 (card image from the final board state, slot 38,
update runtime 0.1.0~dev26): `tools/image/build_sd_image.sh` fetched the
board and assembled `hardened-blikvm-h616-sd.img` (28 GiB, layout p1 root
12 GiB, p2 data 14 GiB at `/mnt`, p3 virtual media) in the development
image directory outside the repository.

The operator's first card (image `.img.zst` SHA-256 `63eda31f...`) booted
Linux but the panel stayed dark and the web interface did not answer. Read
back from the card (the boot area matched the image byte for byte; first
boot had generated the keys): the board owner failed at the NAMESPACE step
because the image excluded `/etc/NetworkManager/system-connections`, which
is in its ReadWritePaths, so native boot stopped and neither the panel
daemon nor the server started; the rendered nginx unit was skipped because
its conditions named the board's excluded certificate. The builder now
keeps an empty, root-only `system-connections` directory, points the nginx
conditions at the first-boot TLS pair, and fails when a unit that starts
needs a path the image lacks. Rebuilt image: `h616-sd-20260926-f`
(`.img.zst` SHA-256
`882e4726d2c3ba35c31da427de520ecfdfbddd2ad03e446910d863d5667c15d3`). The
operator wrote it to the card and reported on 2026-09-26 that the board
booted and was reachable, completed First Setup, and confirmed that the
built-in status panel works. Checked from the network without logging in: the
web server answered `/api/health` and the capability manifest (so native
boot started the application), HTTPS served a new certificate generated on
the first boot (`CN=blikvm`), and SSH asked for the root password. The
first boot ran before the clock was set, so the certificate starts on
2026-07-29; the board has no battery-backed clock. Checked in the image's
root file system: nginx uses the TLS pair the first boot generates; the
`/boot` initramfs (built on the board into `/var/tmp`, never `/boot`)
carries the RAW system update step; the first-boot provisioning and the
boot report units are enabled; every administration switch is off; `fstab`
names the new partitions by UUID.

Plan phase E2 2026-09-26 (RAW system update, `docs/design/system-raw-update-v1.md`;
slot 38, update runtime 0.1.0~dev26):

- Implemented: signed bundles built from the image builder's root image
  (`tools/build_system_raw_bundle.py`, system key kept outside the
  repository), a default-off "Allow raw system updates" switch persisted
  by the board owner and enforced by the server and again by the root
  administration helper, staging on the data partition, the boot-time write
  from the initramfs with a verified backup, read-back, automatic restore
  (failed or interrupted write, no boot report within two boots, operator
  rollback), carried device state and merged password hashes, and the
  System Update card on the update page (desktop and mobile).
- Host tests: the crate's tests (including real zstd, e2fsprogs and tar on
  image files, and the builder's bundle staged and written by the real
  verifier), the builder's tests and the server's upload rules;
  `scripts/check.sh` green.
- On the board (`tools/playwright/raw_system_update_probe.mjs`,
  `evidence/private/raw-system-update-20260926-a.json`, passed): off by
  default; upload, install, confirm and rollback refused with 403 while
  off; switched on, the upload was refused before any byte was kept because
  this hand-built board's 5 GB data partition cannot hold the update and
  its backup (by design; the new card layout has 14 GiB), and nothing
  staged could be installed, confirmed, rolled back or discarded; switched
  off again and refused. The card renders on desktop and mobile with the
  switch off (`evidence/private/raw-system-update-ui-20260926-*.json`).
- The initramfs step is packaged but the board's own `/boot` initramfs was
  not rebuilt. An initramfs built into `/tmp` on the board carries the step,
  GNU tar, zstd and e2fsprogs, and the step's binary runs in it
  (`evidence/private/raw-system-update-device-20260926.txt`); `/boot` was
  unchanged afterwards.
- Found and fixed on the board: its nginx configuration had been rendered
  before the upload locations and the `/internal/` block existed, so large
  image uploads were capped at 16 MiB. It was rendered again from the
  packaged configuration with the board's certificate paths (backup in
  `/root/backups`); HTTPS answered with the same certificate. The image
  builder now points the card's nginx at the TLS pair the first boot
  generates, instead of the board's excluded certificate.
- A bundle from the same build as the rebuilt E1 image is ready for that
  test: `hardened-blikvm-system-0.1.0-dev34-seq1.tar` in
  `h616-sd-20260926-f` (SHA-256
  `bd26e77399a2b835d17afcbb117ab6f2f1c3a0eec058f773d6e8d2372c18c289`;
  root image shrunk to 4.65 GB, 1.16 GB compressed; signature checked
  against the shipped trust root). The bundle built from the faulty first
  image is superseded.
- Waiting for the operator: the device test needs a spare card written from
  the new image (install that bundle and check that the
  account, TLS certificate, SSH host keys, root password, slot and switches
  survive, then confirm; then check that a new root that never reports
  itself booted is replaced by the backup on the third boot).

Final GUI comparison 2026-09-26 (reference Hardened NanoKVM 2.0.42 against
slot 26, desktop). The generic parity probe could not capture the
reference's settings or icon-only menus, so both GUIs were walked with a text
probe (every toolbar menu, every settings tab). Fixed and re-verified on the
board: Wake-on-LAN (menu gate, confirmation, address history routes),
keyboard shortcuts and leader key, HID-only labels, mouse jiggler read,
session lock default (one hour), video-source state, About (running slot,
mDNS, uptime), menu order and the System default section.

Remaining differences:

- Intentional: terminal, scripts, PicoClaw and remote ISO download stay off
  (exec features); SSH has no surface; the JPEG quality control is hidden
  (the capture encoder ignores it); HID-only and Reset HID are not in the
  mouse menu (HID-only is a Device setting); Tailscale is read-only; the
  update page offers signed application updates.
- Gaps: USB gadget reconnect and local image upload in the virtual media
  panel; MJPEG frame detect; USB wakeup; KVM self-reboot in Device settings;
  IP addresses and image version on About (the unprivileged server cannot
  enumerate interfaces); IPv4 DHCP/manual and interface details in System >
  Network; the session lock is not persisted across server restarts; the
  Diagnostics component states (HID, power, virtual media) are not observed;
  RAW system update; a detailed mobile-layout comparison.

Device checkpoint 2026-09-26 (operator testing on desktop and Android).
Slots 21-25 were installed through the API on the native profile; update
runtime 0.1.0~dev19. Fixed after operator reports, each with a regression:

- HID after a cold boot: the owner's `DeviceAllow=/dev/hidgN` entries were
  resolved before the owner created the nodes, so HID stayed denied until a
  restart. `hardened-blikvm-hid-gadget.service` binds the pinned functions
  first; after a cold boot 179/179 reports applied without intervention
  (`evidence/private/hid-control-after-coldboot-20260926.json`).
- Remote control dropped after about 17 s: concurrent focus refreshes each
  took a new challenge and one confirmation was refused; the refresh is
  single-flight (`tools/playwright/hid_control_endurance_probe.mjs`, 120 s
  held).
- Android text input: IME compositions were retyped per prefix, Cyrillic was
  dropped, and the soft keyboard pushed the image out of reach. Compositions
  send differences, Cyrillic uses JCUKEN positions (`tools/test_hid_text.mjs`),
  and the viewport resizes the content. The target's layout must match the
  typed script; automatic layout switching was deferred by the operator.
- VP8: the mode switch was unreachable, scale and touch pan/pinch did not
  apply, and capture changes left a frozen stream; all fixed. Touch dragging
  follows the cursor like the RustAdmin mouse mode.
- Updates: updater slot garbage collection now runs on a native journal, and a
  completed rollback is archived automatically.
- Board storage: the root partition was grown from 5 to 12 GiB (the data
  partition was recreated at 4.8 GiB and its contents restored); the unused
  journal bind shadowed by armbian-ramlog was disabled in fstab.

Open: firewall deployment on the device, the final GUI comparison with a fresh
authenticated reference capture (reference credentials are not enrolled), SD
card boot verification, RAW system update and automatic keyboard layout
switching.

Device checkpoint 2026-09-25 (first native API update). A board reboot under
the native profile came up by itself (`boot-native` ready on slot 15, VP8
decoded). The first application updates through the API on the native profile
then exposed six defects, each fixed with a regression test and redeployed as
update runtime 0.1.0~dev12 to dev17:

1. The update status worker read only the legacy journal; the status API
   answered 503 on schema 3 (`observed_status`, dev12).
2. The rendered native candidate unit bound `/run/credentials/%n` without a
   credential, so systemd refused to start it (226/NAMESPACE); slot 16 left
   the board without a web server for several minutes (renderer fix and a
   render guard, dev13).
3. A unit whose last run failed never counted as stopped, so recovery was
   refused until an operator intervened (`StoppedAfterFailure`, dev13; the
   approach was cross-checked with a second model).
4. The helper's protected store was never garbage collected; after three
   slots every activation was refused (dev14).
5. A failure after admission left the rejected candidate running, or no
   server, until the next native boot; the activation worker now restores the
   confirmed composition itself and logs the readiness error (dev15, dev16).
6. A complete activation with confirmation needs 78 helper requests but the
   session budget was 64, so every confirmation failed (budget 160, dev17).

Slot 20 (source `d684001`, signed with the production key) was then uploaded,
activated and confirmed through the API: generation 35, floor 20, previous
slot 15 kept as the one-step rollback; all components run from the protected
copy, VP8 decoded 1920x1080
(`evidence/private/update-seq15-to-seq20-native-20260925.json`,
`evidence/private/vp8-webrtc-device-20260925-seq20.json`), and a reboot
brought slot 20 up as the current server. Open items: after a native rollback,
`migrate-native-idle` must archive the completion before the next GUI update
(an operator step today); the root filesystem is 96% full (about 200 MB free
after `apt-get clean`), close to the upload staging reserve; the web
auto-reconnect in slot 20 still needs a check by the operator on a phone.

Device checkpoint 2026-09-25 (native profile and VP8 live). Application slot
15 (source `ec04bc5`, bundle `f4934252…`, two identical signed builds, host
verification passed) was uploaded and activated through the API; the legacy
candidate path confirmed it (generation 7, floor 15). The native cutover then
migrated the journal to schema 3 and `boot-native` reported `ready`: owner,
video proxy, board daemon and current server all run from the root-owned
protected copy of slot 15, readiness probes passed and HTTPS answered 200
(`evidence/private/native-profile-cutover-20260925-a.json`). Four earlier
attempts failed on board-specific behaviour and were rolled back from the
backup each time: systemd 249 omits empty exec hook lists, the unmasked
legacy recover unit's Before= edge, SIGTERM exits reported as CLD_KILLED/15,
and identity observation racing the exec of a just-started unit; a rollback
also removed the helper configuration. All are fixed with regressions.

The VP8 path is deployed: the encoder reads the uStreamer stream, Janus runs
streaming-only with its WebSocket on loopback (it needed its own readable
configuration directory and AF_INET6 for libwebsockets), and headless
Chromium on the host negotiated through `/api/stream/webrtc`: ICE connected,
`video/VP8` 1280x720 frames decoded
(`evidence/private/vp8-webrtc-device-20260925-a.json`). Latency, full-motion
load with real capture and desktop/mobile GUI acceptance remain open.

P3 firewall finding 2026-09-24. Loading the generated rules into the board's
real nft 1.0.2 inside a fresh network namespace (the device firewall was not
touched) showed that this nft stores the owned table's mode comment but omits
it from `-j` JSON. Both the mode executor and the owner check would therefore
have rejected their own table on the target. They now read the marker from the
text listing when JSON lacks it and then require a comment-free JSON table;
the device JSON for all four modes verifies, and an over-permissive rule is
still rejected. The restrictive modes gained an opt-in, local-network-only UDP
allowance for the Janus WebRTC media range; the device nft accepted it and
reports it as `{"range": [20000, 20099]}`, which the verifier now requires
exactly. Deployment and reachability tests of the firewall modes remain open.

P6 image checkpoint 2026-09-24. `tools/image/build_sd_image.sh` builds a
development SD image from the running test board: one read-only SSH session
streams the raw SPL/U-Boot area (sectors 1-2047, `eGON.BT0` verified at 8 KiB)
and a root archive without known secrets; assembly under fakeroot removes
per-device state, fails on any leftover private key, and writes a 28 GiB MBR
image (12 GiB root, 14 GiB data at /mnt, MSD remainder) plus the p1 root
filesystem as the RAW system payload. A first-boot unit regenerates SSH host
keys, TLS and snakeoil keys, the USB gadget serial and the machine id; root
uses the expired Armbian default and First Setup recreates the web account.
The first assembly was discarded after inspection found the web account,
its backups (including a KVMD-era TLS key), Tailscale node state and the
NetworkManager secret still present; the builder now removes and checks them.
The image is not reproducible from source (it is derived from the board) and
is not release-accepted; boot and RAW recovery need the separate card.

Full `scripts/check.sh` passed on `315ec75` (exit 0, 179 test result groups).

Device checkpoint 2026-09-24 (native profile, prepare phase). With operator
approval, `tools/device/deploy_native_profile.sh prepare` ran on the H616 test
board: state, units and configuration were backed up to
`/var/backups/hardened-blikvm-native`, update-runtime 0.1.0~dev11 replaced
dev9, the root-owned helper configuration and protected roots were created and
`hardened-blikvm-update-exec.socket` was enabled. Serving units stayed active.
A helper session from the coordinator UID then: prepared a protected root-owned
copy of confirmed slot 14; reported the board-server unit clean; refused
`ValidateStart` for the current server because its installed unit still has
legacy recover dependencies; refused identity observation and `Select` before
observed quiescence. A foreign UID could not connect to the helper socket.
Observing a running unit waits for a stop until the deadline, as designed.

The cutover is deliberately pending: the live board-owner binary differs from
slot 14's, so the native composition must run a slot built from current
source. Signing slot 15 with the production key needs the operator (the
prepared command is `release-seq15/sign-seq15.sh` on the D: build volume).

Checkpoint 2026-09-24 (late). Native admission now runs end to end through
`activation-socket --native-profile` and the root helper; the server requests
admission only in the rendered native units and the GUI accepts the
`prepared` phase. Update package 0.1.0~dev11 ships the helper units disabled.
Two independent cold aarch64 builds of the prototype bundle from committed
source `8a43cfa` produced byte-identical archives
(`f3ba8b839d33a196170cbdb05453cb5e64fd182d6f1cd1af726b883ab992f8fe`, 116
members); an earlier pair from `ff0bc68` was also identical. The `/mnt/d`
volume is currently mounted without Unix permission metadata, so bundle
outputs were staged on the WSL filesystem while build caches stayed on D:.
Full `scripts/check.sh` passed on `8b57b00` (exit 0, 177 test result groups).

P4: the vendor LIBX264-VIDEO source remains unavailable. Read-only device
inspection found the cedrus driver decode-only. A bounded synthetic encoder
benchmark on the device selected VP8 at 720p as the royalty-free software
path and rejected AV1 and VP9 (`docs/design/h616-software-video-encoder.md`).
The codec-independent pipeline core (frame gate, latest-frame slot, RFC 7741
packetizer) is implemented; the libvpx-based encoder stage is not built yet
because compiling downloaded third-party C sources awaits operator approval.

Native execution helper slices 3 to 5 are implemented on the host: the bounded
helper session server with peer UID authentication and a production systemd
backend (queued jobs of a disconnected coordinator are cancelled and
re-observed by the next session), the `bli-kvm-update-exec-helper` binary with
a protected root-owned configuration, the coordinator `ExecutionClient` that
binds reported identities to the signed digest of its own verified slot file,
explicit `migrate-native-idle` and helper-driven `boot-native` supervisor
commands, and native-profile units. The managed native units are rendered from
the base units by `tools/render_native_profile.py`; they execute the protected
copy, drop legacy recover/confirm dependencies, the candidate health reporter
and install links, and verify in their own synthetic systemd root. Full
`scripts/check.sh` passed on commit `072a3be` (exit 0, 176 test result groups).
No package, device installation or native activation-socket path exists yet;
these are host foundations, not device or release evidence.

Native execution helper slices 1 and 2 are implemented on the host, as recorded
in `docs/design/native-execution-helper-v1.md`: the closed execution protocol,
canonical archive rebuilding from an untrusted updater slot, protected
preparation from that slot, and protected selection under a one-step rollback
ledger. Runtime (84), manager (39), stager (25), supervisor (31) and protocol
(14) tests and their all-target Clippy with warnings denied pass. The helper
server, coordinator client, native profile units and any device deployment are
still open.

Aggregate failure `26f5c70ff0c44f569ea902d975e9c58c` (exit 101, RuntimeBusy in
`exact_cleanup_replays_each_partial_file_state_after_reopen`) is diagnosed and
fixed. flock belongs to the open file description. A child forked by another
test thread (the `process_identity` and `composition_systemd` tests spawn
`/bin/sleep` and `/bin/sh`) shares that description until its exec, so closing
only the store's descriptor could leave the runtime locked after the store was
dropped. Reproduction: the runtime library suite failed 8 of 26 parallel runs,
the challenge tests alone 0 of 40, and the challenge plus spawning tests 4 of 40.
`HealthChallengeStore` now unlocks the description explicitly on drop, as does
`UploadStorage`, the other directory lock without an acquisition retry. Runtime
exclusion is unchanged: contention still returns RuntimeBusy while a store is
alive. After the fix: 0 of 80 spawning-set runs and 0 of 25 full-suite runs
failed. The manager, stager, diagnostic and system-action stores already retry
lock acquisition, which bounds this window for them.

Two further load-dependent test failures surfaced in reruns and were corrected
in the tests only. The diagnostic export client tests used a 30 ms response
limit for every exchange, so a delayed peer could turn an expected
closed/invalid response into a timeout; strict limits are now used only for the
timeout assertions. The control broker test that checks response ordering
before diagnostic submission wrote its request after starting a 500 ms read
timeout on another worker thread; the request is now buffered before the handler
starts. Under an artificial 8-process CPU load, the broker, server, update
runtime, supervisor, video proxy and peer adapter library suites passed 15 of 15
runs each.

Full `scripts/check.sh` with the D: target cache and offline Cargo then passed
(exit 0, 2026-09-24, 175 test result groups). That source state also contains
the closed execution-helper protocol in `bli-kvm-update-protocol::execution`
(three new tests). This is host evidence; it does not change any device or
release gate.

Native protected artifact preparation is now implemented as
`update-runtime::native_execution::NativeExecutionStore`: independently verified
archives pass local version/system/native-inventory policy before protected
publication; held destination identities retain the separate store lock. The
new stager policy gate also rejects idempotent acceptance after policy changes
and is never invoked for bad signatures or payload hashes. All 25 stager tests,
two targeted native-execution tests and stager/runtime all-target Clippy passed.
The signed preparation pipeline test uses an unprivileged fixture; root deployment
and service/coordinator integration remain untested and incomplete. A new full
aggregate is required after these implementation changes.

After the green aggregate below, the stager gained `open_protected` for a
separate root-owned execution store. It rejects an unprivileged caller and walks
all ancestors from `/`, refusing writable/foreign-owned/symlink components;
normal update-user staging retains its existing behavior. Root-validation gates
repeat the protected walk. All 23 stager tests and all-target Clippy with warnings
denied pass. This enables reuse of the existing private archive snapshot and
signed-copy publication machinery; production helper/startup wiring and a real
root-owned deployment test are still open. The aggregate below predates this
implementation change.

Aggregate rerun `5789e82a5654423886e0da877c95d971` succeeded with exit 0 in
476.461 seconds, finishing 2026-09-24T02:38:22Z. This closes the stale-fixture
aggregate regression below for the tested source state. The output includes
successful typing, frontend build/lint and synthetic network/firewall/video
contracts. It does not establish installed native recovery, physical behavior or
new release-bound acceptance. Subsequent implementation changes require their
own checks.

Production privilege review `0d7a4387ff4c4888a2d1f96fe63cec7a` completed with
exit 0 (115.127 seconds). Source inspection rejected its allowlist/path-only
startup authorization: the packaged board owner runs as root with hardware and
network write access, while update-owned slot contents remain mutable. The
native recovery design now requires independent signature verification and a
protected execution copy at the privileged boundary, without reopening locked
manager state. Identity remapping, crash space, disconnect-after-dispatch and
rollback-policy requirements are explicit. This is a reviewed design constraint,
not implemented or deployed recovery. Aggregate rerun
`5789e82a5654423886e0da877c95d971` was still running at the last observation;
no terminal success is claimed.

Fresh aggregate `9b08d1804d2c4e8894faf5e5e56274d5` failed with exit 101 after
116.120 seconds: runtime had 72 passing tests and one stale fault-fixture
expectation. The old cleanup case introduced legacy apply evidence before
restoration but expected a completed rollback, contradicting the new early
conflict guard. The fixture now separates preexisting legacy-apply rejection
(no controller calls or journal transition) from late apply publication during
readiness (completion retained, exact cleanup refused/replayed after interruption).
The previously failing native pipeline test passed in 16.52 seconds after this
test-only correction. A full aggregate rerun is required; the failed run is not
reported as green and interrupted-cleanup coverage was retained.

Execution authorization update: the operator explicitly selected native OpenCode
Longrun for this Goal's long commands, replacing the incompatible Codex-bridge
requirement. All other command/device/secret approvals remain in force. A fresh
scripts/check.sh run is now being submitted with offline Cargo and the D: host
target cache; its result is pending, not passed. Earlier bridge-blocker notes are
historical and must not prevent using the approved native adapter.

Native recovery now rejects a surviving legacy confirmed-apply request before
recording recovery intent, including higher-priority Prepared/timeout/boot paths
that bypass credential callbacks. Direct restore also rejects before controller
preflight/quiescence, rather than discovering the conflict only during completion
cleanup. The boot regression verifies both refusal points, no service calls,
unchanged journal/selection and retained apply/challenge evidence. Targeted boot
and credential-loss regressions passed; runtime all-target Clippy and diff checks
passed. This closes a host recovery-ordering defect, not the production profile
or deployed recovery gate.

P4 source-provenance follow-up: rechecked the four public uStreamer branch refs
and inspected v4.11 at `496c9300fc3dcb8ed5aec0da0c5c8eb78c0bc386`. Its pinned
encoder header and parser/dispatch source support CPU/HW/optional OMX/NOOP, not
LIBX264-VIDEO. The top-level Makefile adds no provenance for the delivered backend.
Exact source links and bounded-search limits are recorded in
`docs/design/h616-h264-source-contract.md`. Reproducible H616 software encoder
source remains unresolved; this source review is not H264 playback, performance,
device or release acceptance evidence.

Signed web readiness now checks Content-Type for root/HTML and lowercase
JS/MJS/CSS/WASM/SVG asset routes before accepting body bytes. Missing, duplicate
or wrong media types fail closed; JavaScript accepts text/javascript and
application/javascript. Parameters and media-type case are supported. Other
manifest resources retain size/hash checks without an invented MIME requirement.
All six web-related runtime tests passed (67 filtered), including signed-tree
delivery, MIME cases, body bounds and cancellation. Runtime Clippy and diff checks
passed. This is not browser execution, authenticated API health or TCP endpoint/PID
binding; those production readiness gates remain open.

Runtime contention contract completed for current CLI/activation socket: RuntimeBusy
maps to existing busy exit 66 and protocol Busy, not Unavailable. Cross-process
CLI regression holds the runtime directory lock in the test parent while a real
activation worker acquires separate manager locks: activation refuses without
journal/pointer/runtime mutation, inspect-recovery remains available, and activation
succeeds after release. All 31 supervisor tests passed; supervisor all-target
Clippy and diff checks passed. This tests cooperating current binaries only and
does not close production native provisioning or device recovery acceptance.

Shared-runtime concurrency fence added: challenge-store open holds an exclusive
nonblocking flock on the validated directory inode for its lifetime. A competing
current store returns RuntimeBusy before touching credentials, even when manager
slot/state roots differ. Tests cover contention, independent roots, release and
byte-preserving reopen without extra lock files. All 10 challenge tests and all
30 supervisor tests passed; runtime/supervisor Clippy and diff checks passed.
This advisory lock does not establish persistent journal ownership, exclude old
binaries, or authorize historical orphan cleanup. Production provisioning and
latest full aggregate remain open. Runtime test inventory is now 72; no claim is
made that all 72 were rerun together.

Native effective-unit preflight strengthened: systemctl show now includes After,
Requires, Wants, Requisite and BindsTo. Direct dependencies on legacy recover,
confirm socket or apply services are rejected before start dispatch; clearing a
legacy startup hook alone is no longer enough. Missing properties fail closed,
while the external ustreamer dependency remains permitted. All 12 systemd-adapter
tests passed (59 filtered); runtime all-target Clippy and diff checks passed.
This does not establish a cycle-free transitive graph, safe enablement links or
production native profile installation; those gates remain open.

Boot failure gate strengthened: expired attempt makes no service/journal change;
failed restored readiness retains Restoring, pending, exact credentials and floor
1; retry acknowledges the same intent before cleanup. The targeted boot regression
passed. Board readiness now rejects contradictory `failsafe_latched` reports unless
reported fan duty is 100, matching fan-supervisor's successful maximum-duty latch
contract. All three board-readiness tests passed, including duty boundaries
0/40/99/100/101 and the real Status request. Runtime all-target Clippy and diff
checks passed. These are host checks, not measured fan/PWM or device recovery
evidence; the new board test brings the runtime inventory to 71 tests, not a claim
that all 71 were rerun together.

Native boot/restart runtime dispatch added: `boot_native_composition` routes exact
generation/schema to idle startup, fresh completed-recovery startup, or durable
credential-aware restoration. Still-live pending returns explicit AwaitingHealth
without service/probe calls or deadline renewal. Legacy schema 1 is rejected,
never migrated implicitly. Recovery inspection/CLI now report journal schema.
Regression covers failed admission delivery through completed recovery and fresh
completion replay, plus idle and live-pending branches. Targeted runtime test
passed (1/69 filtered), all 39 manager and 30 supervisor tests passed; all-target
Clippy for the three crates passed. Production CLI mutation wiring, permissions,
effective units, device evidence and latest full aggregate remain open.

Production CLI inspection entry point added: `inspect-recovery` calls manager
inspection without opening credentials or invoking legacy watchdog. Explicit
receipt fields omit completion health bindings and do not claim current health.
All supervisor targets passed: 9 library + 15 binary + 5 CLI + 1 upload-contract
tests. Integration evidence covers absent runtime, damaged pending candidate,
unchanged journal/pointer/credentials; receipt regression covers binding omission
and strict argument parsing. Supervisor Clippy `-D warnings`, fmt and diff checks
passed. The initial corruption fixture failed on the deliberately read-only slot
file; the fixture now temporarily enables owner write and restores its original
mode around test-only corruption/repair. Production permissions are unchanged.
Installed boot units and native mutation dispatch remain unmodified and open.

Native idle boot foundation: `UpdateSupervisor::start_idle_composition` now
implements the schema-3 idle/confirmed startup branch with exact generation,
empty runtime, verified full inventory, quiescence and fresh identity-fenced
readiness. It performs no journal/floor mutation. Targeted regression
`idle_boot_requires_native_idle_and_fresh_readiness_without_journal_writes`
passed (1 passed, 69 filtered), covering legacy/stale refusal, startup/probe
ordering, readiness failure, byte-identical journal and preserved orphan token.
Runtime all-target Clippy with `-D warnings` passed. Production CLI dispatch,
effective service graph, privileged authorization, installed recovery and a fresh
full aggregate remain open; this does not close P5.

Production integration review `e928b83a23bb46639db62a4c2474ea86` completed with
exit 0 in 105.107 seconds. Source review confirmed the existing native activation
pipeline already owns admission/EOF/quiescence ordering. The review's proposed
daemon IPC and boot APIs are not implemented APIs; its pseudocode must not be
treated as a tested implementation. The checked disposition and entry-point
matrix are in `docs/design/native-application-recovery.md`. No native profile or
schema migration has been deployed. The latest full aggregate still predates
credential-loss and Prepared-before-publication changes; a fresh run is required.

Native issuance regression result: `c52ef2122dba48b58dcdf5a36e424dcb` succeeded,
exit 0, 29.048 seconds, with all 69 runtime tests passed. This supersedes the
pending runtime-regression note below and covers Prepared-before-publication.
Manager remains at 39 passing tests from the preceding combined run; no manager
source changed during the issuance-order work. Production CLI/systemd routing,
boot/restart readiness and release-bound device recovery remain required.

Native issuance ordering checkpoint: combined regression job
`4b991bf1016f4edb89aa1afc8a7564cf` succeeded, exit 0 in 31.045 seconds,
with 39 manager and 66 runtime tests. Subsequent native issuance now generates
challenge identity in memory, persists Prepared via `issue_after_admission`,
then publishes credential files. Ordinary admission rejection creates no orphan
files; ambiguous admission can leave Prepared but not newly published credentials.
Publication failure therefore has a durable exact pending owner. Legacy issuance
is unchanged. Five admission-filter tests passed, including real anti-rollback
refusal with an empty runtime; an additional interrupted-issuance/reopen test
passed and confirmed InterruptedPrepare intent, retained pending, unchanged
confirmed selection and no credential/apply files. Runtime all-target Clippy,
format and diff checks passed. The combined result predates this ordering change;
complete runtime regressions remain pending. Existing historical orphan ownership
and production CLI/unit integration remain open.

Native pending credential-loss implementation (2026-09-23): added
`begin_native_recovery_with_credentials` with a schema-3-only exact pending
identity callback. Existing reasons and recorded intent take precedence; schema
1/2 bypass credential inspection. Intact evidence leaves state unchanged, errors
refuse mutation, and matching partial/absent evidence writes HealthChallengeLost
intent without pointer/floor/pending cleanup. Journal, anchor and pointer are
revalidated after the callback. Schema 3 accepts this reason through restoration,
completion and archival; schema 2 continues rejecting that native reason.
Runtime `begin_native_credential_recovery` binds this to `inspect_exact`, preserves
inspection errors, refuses a legacy apply request in the credential-check path,
and performs no service actions or credential deletion.

Verification: all 39 manager tests passed, including schema separation, exact
identity, loss restoration/archive, unchanged evidence, callback failure and
state/pointer substitution, reason precedence and reverse-time refusal. The
runtime integration test passed its complete/state-only/token-only/absent/mismatch
matrix and checked retained bytes, unchanged selection and absent legacy apply.
Both crates' all-target Clippy with `-D warnings` and diff checks passed. Gemini
review `d334b8f96c9a4780a127ea629ab1ee5f` completed exit 0; reason precedence and
schema isolation advice were adopted, but panic-on-mismatch was rejected.
Full combined regressions remain to run. Pre-Prepared orphan ownership, production
CLI/boot integration and device recovery remain open; this change is host-only.

Aggregate result: `633269c22327474c900a4c004375b265` succeeded with exit 0,
duration 425.387 seconds, terminal timestamp 2026-09-23T18:51:41Z. This
supersedes pending notes for that job. The run includes the board archive
failure fix and browser contracts. It was submitted before the new
`inspect_exact` Rust edits and is not asserted as full coverage of them; those
have the separately recorded eight challenge tests and runtime Clippy evidence.

Native credential inspection checkpoint: `HealthChallengeStore::inspect_exact`
now classifies absent, state-only, token-only and complete credentials against
an exact operation/binding/target, without deleting files or granting confirmation
authority. Existing exact completion cleanup uses the same validation path.
The reopen/crash-state test now asserts classification and byte-for-byte retention
before cleanup for all four states. Eight challenge tests passed; runtime
all-target Clippy with `-D warnings`, formatter and diff checks passed. This is
the read-only classifier needed for loss reconciliation; durable rejection,
orphan ownership and production routing remain open. These Rust edits occurred
after aggregate job `633269c22327474c900a4c004375b265` was submitted, so its result
must not be treated as whole-worktree coverage of this new change.

Installed config-path observation (2026-09-23): read-only receipt
`evidence/private/corrective-rotation-config-20260923-b.json` identifies the
recovery unit's supervisor configuration source as
`/etc/hardened-blikvm/update-supervisor.json`. Its allowlisted fields specify
slotRoot `/var/lib/hardened-blikvm-slots-v2`, stateRoot
`/var/lib/hardened-blikvm-update-v2`, but runtimeRoot remains
`/run/hardened-blikvm-update` (no `-v2` suffix). Configuration schema is 1,
systemSequence is 1 and healthTimeoutMs is 60000. Do not infer runtimeRoot from
the slot/state suffix when planning credentials, rollback or cleanup. Only
paths and numeric limits were retained, without configuration secrets or key
material. The first receipt inspected rotation drop-ins alone and found no
credential source; the second inspected the complete unit-file chain. It does
not claim fully verified effective configuration or signed-slot health.
Aggregate job `633269c22327474c900a4c004375b265` is pending terminal observation.

V2 state observation (2026-09-23): private receipt
`evidence/private/corrective-v2-state-20260923.json` reports schema 1, generation
4, floor 14, active=confirmed slot 14, previous slot 13, one history record and
no pending operation. V2 current resolves to the same slot 14 observed running.
The effective current/candidate units still require and order after recovery.
LoadCredential metadata is reported as `[unprintable]` by systemd, so the
credential source has not been established by this observation. The snapshot
is read-only, non-atomic and does not validate signatures or authorize migration.

Board archive failure regression: reused the real publication-block fault tests
for the board builder. Before the fix, injected tar failure incorrectly returned
success and gzip failure left a temporary archive behind. The builder now uses
Bash `set -euo pipefail` and creates the temporary archive inside the cleaned
staging directory. The aggregate syntax gate now invokes Bash for that script.
All eight prototype/board archive and board-builder tests passed, including
old-archive/checksum preservation on tar/gzip failures and repeatable real tar
output. Targeted BasedPyright and diff checks passed. This source change follows
the successful aggregate recorded below and requires an updated aggregate run.

Operator acceptance override (2026-09-23): the user explicitly confirmed that
the ATX Power button, Reset button and ATX indicator work and requested that
they be marked working without further testing. These three checks are accepted
on the user's direct manual evidence for the currently installed device and are
excluded from further test runs for this task. This is operator-reported evidence;
no automated receipt or binding to the newly built, uninstalled archive is claimed.
Other device requirements retain their existing scope.

Latest full host aggregate: job `974fe80529534caaa5a709e9eb73de62` completed
successfully, exit 0, in 455.418 seconds, finishing at 2026-09-23T18:26:27Z.
This supersedes pending notes for that job and covers the current native lifecycle
and board bundle verifier source changes. The final output includes successful
Network, Firewall and Video browser contracts, web build/lint and formatting.
It remains host/synthetic evidence rather than physical device acceptance.

Fresh composition observation (2026-09-23): private receipt
`evidence/private/corrective-composition-20260923.json` records a bounded,
read-only SSH observation of nine units and hashes of their running executables.
Owner, boardd, board-video-proxy, ustreamer and server-current were running;
server-candidate, board-server and legacy server were inactive; update-recover
was active/exited. Crucially, server-current executed from
`/var/lib/hardened-blikvm-slots-v2/app-00000000000000000014-2e295a6b171d5b503405273ea89d74fb206d9a79085db8bbf421b1437e6ee354/bin/bli-kvm-server`,
while the observed OLD `/var/lib/hardened-blikvm-slots/current` resolved to
slot 11. The old pointer is not authoritative for the live server. Current,
candidate and recovery units have installed `10-rotated.conf` drop-ins;
current/candidate also have status-API and application-write overrides.
Owner/proxy/boardd execute from `/usr/libexec/hardened-blikvm`.
Observed server SHA256 is
`df9eefcf813c9eaf305b3d8ebb481b88558eb644e39ebc84349ce78a86c56496`;
the newly built server SHA256 is
`01839961e40ca0c328a96e52cdd3f15d74bf86217d2ab30e307c661d0de1e6f5`.
Owner and proxy hashes also differ from the new build. No installation was
performed. This observation is not race-free PID identity or application-health
proof; inspect the effective v2 configuration and authoritative journal before
constructing the deployment/rollback transaction.

Fresh read-only device preflight (2026-09-23):
`evidence/private/corrective-handoff-preflight-20260923.json` was captured through
`project-memory-exec` using the enrolled SSH asset and the pinned private
known-hosts file. It observed configfs mounted, UDC present, gadget `hardened`,
and both `kvmd.service` and `kvmd-main.service` inactive. No device mutations
were performed. The old handoff probe reports `handoffReady=true` from component
presence alone; this is NOT proof of current owner identity, application slot
selection, safe replacement, rollback, HID lifecycle or release acceptance.
Before installing the new bundle, inspect the actual active composition and
its rollback path. The legacy read-only runtime activation gate assumes inactive
units and absent runtime roots; it must not be used blindly against this already
configured board. Current full host check job
`974fe80529534caaa5a709e9eb73de62` has been submitted; its terminal result has
not yet been observed.

Repeat-build checkpoint: job `dc84493a9cf046ed8fd551fac1acb64c` succeeded,
exit 0, in 21.042 seconds. The archive in `corrective-test-20260923-b` has the
same SHA256 as the first archive:
`3887d563fe4cc7f9a67a0da0d8042fc4c505df357cf6d3f733a21f987c70a121`.
The second archive independently passed the 52-member bundle verification;
receipt: `evidence/private/corrective-board-bundle-20260923-b.json`.
Both builds reused the same ARM64 compiler cache. This proves repeat-build
archive equality in that environment, not independent clean-build reproducibility
or device acceptance. The next aggregate run must include the current native
lifecycle and bundle-verifier changes; the earlier aggregate green is insufficient.

Board test artifact verification (2026-09-23): build job
`42389ffce32f47b5af8339259d27efc2` succeeded, exit 0, in 84.094 seconds. ARM64
release compilation, the builder's ELF checks and TypeScript/Vite build passed.
The archive at
`/mnt/d/debug/blikvm/hardened-blikvm/corrective-test-20260923-a/hardened-blikvm-board-profile-aarch64.tar.gz`
has SHA256 `3887d563fe4cc7f9a67a0da0d8042fc4c505df357cf6d3f733a21f987c70a121`.
The board bundle verifier checked all 52 payload members; its receipt is
`evidence/private/corrective-board-bundle-20260923-a.json` (confirmed ignored).
The manifest remains board-owner-test with release/device acceptance false.
This is host artifact evidence only; installation and rollback remain untested.

While reviewing that gate, incomplete SHA256SUMS coverage and duplicate archive
or checksum entries were found to be accepted. The verifier now requires exact
payload coverage, rejects duplicates and ambiguous checksum paths, and checks
the member-count limit while iterating instead of after loading every header.
Regression fixtures cover omitted server checksums and both duplicate forms.
All 12 verifier/staging tests passed; targeted BasedPyright reported zero errors,
warnings or notes; `git diff --check` passed. A second build is still required
before claiming repeat-build equality, and latest aggregate checks are pending.

Combined native confirmation verification: job
`abd65d9fce154d3ca5403b8d89c3953d` completed exit 0 in 36.052 seconds: all 37
manager and 65 runtime tests passed, including the expanded confirmation fault
matrix. This supersedes pending notes for that job. Production deployment is
still unverified.

Next test-delivery checkpoint (2026-09-23): prepare an existing board-owner-test
bundle from the current worktree, without enabling schema-3 migration in the
installed profile. This is an intermediate device-test artifact, not completion
of native production recovery or release acceptance. The board-profile builder
now stages on its selected output volume rather than host `/tmp`; shell syntax,
four existing board-builder contract tests and diff checks passed. The installed
Rust toolchain includes `aarch64-unknown-linux-musl`, and the existing board target
cache and pnpm store were verified. Build output is intended for a distinct
`corrective-test-20260923-a` directory, leaving earlier bundles intact.

Native candidate routing verification: Longrun job
`710360c5e29b4693940ab3e61593111b` completed exit 0 in 21.04 seconds, all 65
runtime tests passed. This supersedes the focused-test timeout note below.
Subsequent implementation adds supervisor native confirmation bound to the
complete stored challenge pair and startup process identities, fresh probes and
manager confirmation. Success uses exact durable-history cleanup, with separate
cleanup-only replay; failure must not emit legacy confirmed-apply. The fault
matrix now includes confirmation refusal/replaced identity and successful cleanup
after reopen. Manager/runtime all-target Clippy passed; execution of this expanded
test matrix is pending, so the new confirmation wiring is not yet marked verified.

Native candidate routing follow-up (2026-09-23): `activate_native_candidate` now
uses explicit schema-3 admission/execution rather than the legacy activation
helper/watchdog. Native status is a separate manager/supervisor API; startup can
inspect active schema 3 but still refuses recovery markers. The existing fault
matrix now explicitly migrates before exercising native activation and checks
that no legacy confirmed-apply request is emitted. Manager/runtime all-target
Clippy and diff checks passed. The first focused test invocation was terminated
at the short-command 30-second limit after compilation, with no pass/fail result;
no matching test process remained. A Longrun test rerun is required before marking
this pipeline transition verified. Production entrypoints remain unwired.

Runtime migration follow-up (2026-09-23): the 62-test runtime baseline passed in
job `e89f9bf916424a6a9bb745122a1b9c9e` (exit 0, 29.043 seconds). Afterward,
`UpdateSupervisor::migrate_native_idle` was added with complete confirmed-inventory
verification, exact completion cleanup and refusal of orphan full/partial
credentials or legacy confirmed-apply requests. Three new focused runtime tests,
runtime all-target Clippy and source diff checks passed. Production migration and
schema-3 activation/startup/confirmation dispatch remain unwired.

Native confirmation follow-up (2026-09-23): `confirm_native_activation` now uses
exact generation/operation/target/binding, trusted readiness and fresh post-probe
time with full-document revalidation. Success advances the floor once; failure
persists native `RecoveryPending` without performing legacy pointer rollback or
claiming restoration. Host regressions cover readiness refusal, deadline crossing,
boot change, damaged target, changed journal, reversed clock, replay and another
update after confirmation. All 37 manager tests, all-target Clippy and runtime
all-target compilation passed. Production/runtime wiring is still open, and the
earlier full check does not cover these later manager changes.

Native admission/execution follow-up (2026-09-23): schema-3-only exact-generation
APIs now persist Prepared, execute after the caller's admission-delivery gate,
revalidate full journal and fresh time after quiescence, and preserve native
recovery responsibility on failure. Prospective serialization checks reserve
capacity for normal confirmation or rollback/restoration/completion/archival before
Prepared is stored. A reopen-to-recovery-to-next-admission host regression keeps
the previous receipt archive intact. All 34 manager tests, all-target Clippy and
runtime all-target compilation passed. Native confirm and production runtime
wiring remain open; the earlier aggregate check predates this implementation.

Native idle migration foundation (2026-09-23): schema 3 and
`migrate_native_idle` are host-implemented with exact generation, verified
selection, pre-cleanup serialized-capacity validation, post-cleanup full-document
revalidation and intact completion archival against terminal history. Legacy
mutation remains rejected; native recovery preserves schema 3. All 32 manager
tests and all-target Clippy passed. The read-only Gemini schema review job
`f917c904f1914d47b175a1a6e2e60abe` completed exit 0 (45.058 seconds). Native
prepare/execute/confirm, runtime migration wiring and production integration are
still open; no device was migrated. This implementation follows the aggregate
check below and is not covered by that earlier aggregate result.

Post-aggregate manager correction (2026-09-23): a same-boot timestamp before
Prepared creation was accepted by pending activation replay and by legacy health
confirmation. The new regression failed first at replay, then at confirmation
after fixing replay, reproducing both gaps. Both paths now return RecoveryRequired
without mutating the journal, pointer or floor. The regression verifies byte-exact
journal preservation and successful confirmation at the inclusive creation-time
boundary. All 28 manager tests and all-target manager Clippy with warnings denied
passed. This code change follows the successful full check below; that earlier
aggregate result is not a full rerun of this patch. Native schema-3 review job
`f917c904f1914d47b175a1a6e2e60abe` was submitted separately; no result is claimed here.

Production native boot integration remains open after another read-only unit
review: both server roles depend on completion of the recovery oneshot, three
managed hardware components still execute base-package binaries and can be
enabled independently, and the candidate retains legacy confirmation authority.
The exact findings and required effective-unit/enablement checks are recorded in
`../design/native-application-recovery.md`. No production units or running-device
state were changed by this review.

Full check job `2ca9b81c1e64450ebc82a5986123dc1b` is now terminal **succeeded**,
exit code 0, duration 428.41 seconds, finished 2026-09-23T17:10:30Z. Command:
`env CARGO_TARGET_DIR=/mnt/d/debug/blikvm/hardened-blikvm/target-host CARGO_NET_OFFLINE=true /home/w0w/BLi-KVM/scripts/check.sh`.
The terminal output includes successful web build, zero-warning ESLint, Prettier,
and Network/Firewall/Video synthetic browser contracts. This supersedes earlier
pending notes for this job and closes the aggregate host-check gate for the tested
implementation. Subsequent edits before result retrieval were documentation-only.
It does not close physical-device, production native recovery, reproducible H.264,
SD/RAW or release-bound acceptance gates. A P4 follow-up also records upstream
issue #363's missing-libx264 report as external corroboration, not local acceptance,
in `../design/h616-h264-source-contract.md`.

| Area | Confirmed evidence | Current status | Required closure |
| --- | --- | --- | --- |
| Acceptance | The report accepted markerless JSON receipts and could select an older passing receipt after a newer failure. GUI comparison skipped settings, request and capability differences when either side was empty. | Host implementation corrected; fresh release acceptance remains open | Schema-v2 probes now require explicit coverage and release context; the report uses only the newest receipt and rejects missing markers. Re-run every release gate against final artifacts. |
| HID lease continuity | Focus refresh changes nonce/time-derived bindings while the active lease adapter requires exact identity. The client ignores an HTTP-success response whose body says `applied=false`. | Host implementation and regressions passed; physical gate open | Session/target identity is stable across focus refresh, every lease response checks `applied`, stale HTTP work is aborted, and logout/auth/WebSocket teardown revoke the lease. Verify reconnect and observed physical input. |
| HID release safety | Renew can discard expiry release events; no periodic runtime expiry path was established. Mouse coalescing can discard button, wheel and relative-motion semantics. | Host implementation and regressions passed; physical gate open | Renew and autonomous one-second expiry execute release batches and retry ledgered failures. Browser queue generations cancel stale work; relative deltas are added and button/wheel reports remain ordered. Verify on an operator-observed target. |
| Control confirmation | Cancelling the focus dialog clears only the current promise; queued input can open it again immediately. | Local browser gate passed; physical gate open | The toolbar now has explicit enable/disable control. Input is passive while disabled, Cancel is sticky, disable/blur/unmount release the lease, and ATX retains a separate action-specific confirmation. |
| UI/API/capabilities | Initial live UI hid Device, Network and Firewall; contracts were incompatible. Corrective code now has granular gates, native contracts and local desktop/mobile regressions. | Host implementation substantially corrected; remaining coverage and device gates open | Verify deployed manifests/endpoints, remaining page-open/error cases, full host checks and release-bound device evidence. |
| Firewall | Corrective code includes four-mode rules, semantic read-back, journaled transactions, scoped replacement, HTTP/UI and opt-in owner dispatch. Real nft namespace checks verify apply/rollback/recovery. | Open, high severity until deployment prerequisites and device gates pass | Complete HTTPS preflight, legacy migration, autonomous owner integration and packet/device checks for IPv4/IPv6, HTTPS/DHCP/Tailscale/WebRTC. |
| MJPEG controls | The API advertises resolution but the screen menu does not render its component. Requested and delivered settings are not consistently synchronized. | Open | Wire resolution and state read-back end to end; prove which quality controls affect the stream. |
| H.264 | Upstream V4 sources contain the software `LIBX264-VIDEO` to shared-memory sink and Janus path. The installed product advertises only MJPEG. | Open | Port the reviewed software path and separately resolve Direct versus WebRTC delivery; measure FPS, latency, CPU, memory and temperature before advertising it. |
| Packaging and application update | The live board required local unit/drop-in alignment. The signed payload contains components that are not all switched atomically with the server. | Open, high severity | Make a clean package reproduce the live topology, define component version compatibility, align proxy limits and strengthen health/rollback checks. |
| Bootable card image | Existing system-image fixtures and validators do not produce a complete bootable H616 card image. | Missing | Build a clean reproducible image with a larger system partition and an initially empty virtual-media partition; test boot on separate media. |
| RAW system update | Application slots are not a RAW OS updater; no production partition writer/read-back/recovery flow is connected. | Missing | Produce RAW payloads from the same build as the card image, validate layout and trust, stage outside the target system, write in maintenance/recovery, read back and test recovery. |

## P2 implementation checkpoint

### Goal-to-artifact closure checklist (refreshed 2026-09-23)

This is a navigation and closure checklist, not a passing acceptance manifest.
Checkpoint results below remain scoped to their source state and test coverage.

| Goal requirement | Concrete implementation / evidence entry point | Remaining acceptance evidence |
| --- | --- | --- |
| P2 named capability files | `web/src/types/capabilities.ts`, `web/src/lib/capabilities.ts`, `crates/bli-kvm-platform-bli/src/lib.rs`, `crates/bli-kvm-server/src/lib.rs`; `tools/playwright/network_contract.mjs` | Deployed manifest/endpoint agreement, unsupported-child absence, all required actions and desktop/mobile error/read-only cases |
| DNS/IPv6/Firewall/Tailscale contracts | Server network/firewall/Tailscale modules; `tools/test_network_outcome.mjs`, `tools/playwright/{network_contract,firewall_modes_contract}.mjs` | Fresh real applied/read-back/rejection evidence; Tailscale read-only status is not implemented mutation parity |
| P1.4 physical HID lifecycle | HID transition/policy/adapter, server authentication/WS/session-lock paths, `tools/playwright/restricted_control_lifecycle.mjs` | Operator-observed long input/idle, focus-refresh, network loss, cancel/blur/unmount/logout, desktop/mobile |
| P3 real firewall behavior | Broker `firewall_*` modules, `tools/check_firewall_nft_namespace.py`, `tools/check_firewall_https.py` | Target legacy rule migration, restart reconciliation and IPv4/IPv6 HTTPS/DHCP/Tailscale/WebRTC connectivity with rollback |
| P4 MJPEG controls | Server `video_settings.rs`/`video_engine.rs`, screen resolution/FPS/quality components, `tools/playwright/video_settings_contract.mjs` | Actual resolution/FPS/quality effects and requested/captured/queued measurements |
| P4 reproducible H.264 | `docs/design/h616-h264-source-contract.md`, `docs/baselines/h616-vendor-artifacts.json` and the operator source map | Reproducible encoder provenance, single capture owner, WebRTC and Direct behavior, FPS/bitrate/CPU/RAM/temperature/latency and switch measurements |
| P5 release and transactional application recovery | Build scripts, Debian builders, packaged units/drop-ins/Nginx; `docs/design/{application-update-composition,native-application-recovery}.md` | Complete native acknowledgement/cleanup and boot/watchdog/apply wiring, installed compatibility, reproducible artifacts and device failure/reboot recovery |
| P6 SD image | System-image fixture/validator tools are foundations only | Production H616 bootable image, verified boot geometry/peak staging, larger system and empty ISO partition, separate-media boot proof |
| P6 RAW update | System-image trust/layout validation foundations; no accepted production writer | Same-build RAW payload, external staging, maintenance writer/read-back/data preservation/UI/API/progress and separate-media recovery |
| General host gate | `tools/compare_gui_parity.py`, its tests, and the complete `scripts/check.sh` | One successful complete check on final source; individual Rust/default-feature and frontend checks do not cover all feature matrices or release build |
| P7 strict release acceptance | `tools/phase8_acceptance_report.py`, `docs/baselines/phase8-acceptance.md` | Newest explicit passing receipt for every required gate bound to final artifacts; all listed peripherals/media/ATX/network/systemLog/Tailscale/update/soak/reboot/recovery, `deviceUnrun=0` |

The existing card remains unchanged. The provisional 12 GiB system / 14 GiB
data-staging layout is not an approved or tested image geometry. Historical
receipts, synthetic systemd roots, protocol fixtures and static binary hashes do
not close physical, installed or reproducibility requirements.

Target clarification: the deployed BLi-KVM uses the native board-owner and
vendor implementation, not KVMD. Hardware/backend gaps must be checked against
https://github.com/blikvm/blikvm and the pinned local vendor sources. NanoKVM
remains a behavioral UI/HID reference, not evidence of the BLi runtime backend.

The frontend manifest now includes typed `network` and `system.actions` fields.
Network section visibility uses the typed network status. Device children and
the Network TLS/WiFi children mount only when their individual board features
are advertised. The leader-key hook waits for capability loading and skips the
legacy shortcut endpoint on BLi manifests without that feature.

Verification at this checkpoint: `web/node_modules/.bin/tsc --noEmit` passed,
and targeted ESLint passed for the changed capabilities, Device, Network and
leader-key files after Prettier formatting. These checks establish static
consistency only. DNS/IPv6 now mount according to their individual typed
capabilities. Read-only controls use a disabled fieldset and disabled mode
selector; each child save handler rejects writes, and the parent save dispatcher
skips non-writable sections. TypeScript and targeted ESLint also passed after
this change. Browser verification of these guards remains required.

Browser request coverage, DNS/IPv6
response alignment, mutation rejection handling and backend capability tests
remain open; P2 is not accepted yet.

IPv6 BLi-path follow-up: the UI reads the reported interface and kernel state
instead of assuming `eth0`, hides unsupported manual/DHCPv6 configuration, and
refuses writes when the observed interface state is unknown. Apply sends the
reviewed `{interface, disabled, confirm}` body and requires explicit application
plus matching interface read-back; failed refresh does not produce a success
message. Two behavioral outcome tests cover refusal, malformed/missing read-back
and wrong-interface/state responses, and are included in `scripts/check.sh`.
The tests, TypeScript and targeted ESLint passed. DNS contract adaptation and
IPv6 browser/device verification remain open.

DNS BLi-path follow-up: decode `resolver.servers`, edit only resolver addresses,
and send `{servers, confirm}`. IPv4 address/gateway controls and their redirect
behavior are excluded from this path. Apply requires `applied=true` and an exact
ordered `readBack.servers` match before marking the draft saved. Malformed or
oversized resolver responses fail visibly. Four combined DNS/IPv6 outcome tests,
TypeScript, targeted ESLint and `git diff --check` passed after this change.
Full browser coverage and device verification are still required for P2 closure.

The bounded local `tools/playwright/network_contract.mjs` probe now passes on
a fresh production web build. It exercises Device/Network page-open without
writes, absence of unsupported Device/TLS/WiFi/leader-key requests, disabled DNS
inputs under read-only capabilities, and DNS/IPv6 refusal with exact mutation
bodies and no success message. About omits KVMD for native inventory and tolerates
missing nested observations; the probe checks the native KVMD row is absent.
The probe is included in `scripts/check.sh`. Production build, targeted ESLint
and `git diff --check` passed. These are local fixture checks, not device evidence;
successful writes/read-back UI, mobile coverage, Firewall/Tailscale and the
remaining Device child contracts still need verification.

The network probe now also passes successful DNS and IPv6 writes on desktop and
390x844 touch/mobile layouts, with stateful read-back and exactly two expected
POST requests per successful scenario. This exposed and corrected a status
aggregation defect: an earlier DNS success hid the later IPv6 success. The shared
status now combines messages at the highest selected severity, retaining error
priority. The fresh production build, targeted ESLint, expanded browser probe
and whitespace check passed. Native device writes remain unverified.

Device mDNS follow-up: the switch now honors its own read-only capability,
requires an initial boolean observation, rejects `applied=false`/rejection,
reads the state back after an accepted toggle, and reports failures visibly.
Loading always settles after errors. The browser probe covers read-only mDNS,
refusal without optimistic state changes, and successful read-back on desktop
and mobile. Fresh web build, targeted ESLint, expanded browser probe and
`git diff --check` passed; no native device mutation was performed.

Device HID-only follow-up: added the same read-only, initial-observation,
explicit rejection and post-write read-back guards. Failures are visible and
loading settles. The browser probe now checks HID-only read-only behavior,
refusal without a false toggle, and successful desktop/mobile state changes,
including the exact `{enabled: true}` POST body. Fresh web build, targeted
ESLint, the expanded probe and whitespace checks passed. Physical gadget/media
behavior remains a separate unrun device gate.

Device Mouse Jiggler follow-up: enable and mode changes share one guarded write
path. Read-only/unknown state prevents writes; refusal and transport errors are
visible; accepted writes require relative-mode state read-back. The local probe
checks read-only, refusal without enablement, and successful desktop/mobile
enablement with the exact `{enabled: true, mode: "relative"}` body. Production
build, targeted ESLint, browser probe and whitespace checks passed. This does
not establish physical relative-mouse behavior on the target.

Device Session Lock follow-up: the server returns the observed `duration`, not
an `applied` flag. The UI validates the initial numeric duration and requires an
exact duration match after writing; read-only capability prevents edits and
failures are visible. The local probe checks read-only, mismatched duration
rejection and successful desktop/mobile saves with `{duration: 300}`. Fresh
build, targeted ESLint, browser checks and whitespace validation passed.

HTTPS UI follow-up: initial state comes from `GET /api/vm/tls`, not the browser
URL scheme. Read-only/unknown state blocks writes. The explicit confirmation is
retained; only `applied=true` without rejection schedules the redirect, and
unmount clears that timer. Rejected writes display an error without scheduling
a redirect. The browser probe uses API-enabled TLS over a local HTTP fixture to
prove state provenance, and counts redirect scheduling for accepted versus
refused writes on desktop/mobile. Fresh build, targeted ESLint, expanded probe
and whitespace checks passed. Actual proxy restart, target URL/port correctness
and TLS connectivity remain device/packaging gates.

Wi-Fi UI follow-up: consume BLi `available` (with the existing reference
`supported` fallback), validate state shape, honor read-only capabilities, check
explicit application/rejection for connect/disconnect, and refresh observed
connection state instead of assuming success. Password-bearing request errors
are no longer logged, and the password draft is cleared on success or close.
The local browser probe checks read-only, refused connect, and successful
desktop/mobile connect against stateful API fixtures. Fresh production build,
targeted ESLint, expanded probe and whitespace checks passed. Physical Wi-Fi
connectivity and disconnect behavior still require their device/browser gates.

Wi-Fi disconnect browser follow-up passed on desktop/mobile: a refused
disconnect keeps the connection visible; a subsequent accepted disconnect
clears it only after state refresh.

Tailscale contract follow-up: the native server now advertises read-only status,
matching the current owner's unconditional action refusal. A native view decodes
`installed/running/enabled/loginName/deviceIp/deviceName/message` instead of the
reference `state` enum, and mounts no install/login/action clients. The local
probe checks read-only tab visibility, observed package-unavailable status and
exactly one status GET with no action requests on desktop/mobile. Full control
parity remains open; read-only presentation is not evidence of implemented
installation or connection management. Production web build, targeted ESLint,
browser probe, Rust format check and whitespace check passed. Server capability
regressions still need to run for this manifest change.

Server capability regression follow-up passed: assertions now inspect individual
HID/power/system-action and board features instead of searching the whole JSON
for `read_write`. The restricted profile checks native Tailscale read-only,
granular Device/Network write exposure with opt-in/socket, and DNS/IPv6 read-only
without network-write opt-in. The full restricted server library suite passed
138 tests with two explicitly environment-gated nginx diagnostics ignored; the
default-build non-exposure regression also passed. The initial `--exact` filter
matched zero tests and was corrected before recording these results. The full
`scripts/check.sh` and device gates are still outstanding.

Firewall UI contract follow-up: the native view validates the actual flat
`FirewallView` fields instead of dereferencing reference-only `config`/`rules`
objects. It labels the existing baseline as managed rules, avoiding the opposite
reference-baseline meaning, shows reported nft output, honors read-only status,
requires explicit confirmation, rejects unsuccessful outcomes and checks mode
and active-state read-back. Reference modes remain explicitly unimplemented;
P3 snapshot/restore, reconciliation, helper output and mode parity are still
open high-severity work. Local desktop/mobile fixtures verify page-open without
writes, read-only controls, refusal without false activation and successful
enablement with `{mode: "baseline"}`. Fresh build, targeted ESLint, expanded
browser probe and whitespace checks passed. No live firewall changes were made.

Platform capability follow-up: the 24-test platform suite passes after updating
the obsolete hidden-Network expectation to the read-only contract and adding
explicit DNS/IPv6, unavailable-WoL and granular Device assertions. Rust formatting
and whitespace checks passed.

P3 source inspection confirms the outstanding failure paths in
`crates/bli-kvm-control-broker/src/firewall_settings.rs`: `systemd-run --wait`
has no explicit child-output transport; `open()` resets active state without
kernel reconciliation; `set_baseline()` substitutes an empty snapshot on read
failure and takes a new snapshot even when disabling. This cannot establish
restoration of the original ruleset. These findings remain open and must be
reproduced with injected-runner regressions before changing the privileged path.

P3 first regression: injected snapshot failure reproduced an incorrect `Applied`
response. The owner now rejects unreadable or oversized (>64 KiB) snapshots
before writing files or changing rules. The regression asserts both rejection
and the absence of mutation commands/files. All four firewall module tests,
Rust formatting and whitespace checks pass. This fixes only snapshot admission;
original-snapshot retention, helper output, restart reconciliation, reference
modes and connectivity/rollback device gates remain open.

P3 output follow-up: a multibyte nft-comment regression reproduced a panic at
the 64 KiB display boundary. Truncation now backs up to a valid UTF-8 boundary;
all five firewall tests pass. The transient nft helper now uses `systemd-run
--pipe` so captured stdout/stderr belong to the service rather than only the
launcher. Local `systemd-run --help` confirms the descriptor-forwarding contract;
target runtime output capture still needs a device gate. Rust format and
whitespace checks passed. No live rules were changed.

P3 rollback regression is now reproduced with a stateful injected runner:
enable followed by disable returns `Applied` but leaves the managed table in
the observed ruleset. The new
`disabling_removes_managed_table_without_replaying_unrelated_rules` test is
currently failing and intentionally retained as the next implementation gate.
An independent Gemini Flash conceptual review confirmed that `nft -f` is
additive and that rollback should be scoped to the application's table, with
explicit ownership validation on restart; global flush/replay would affect
unrelated host rules. No rollback implementation or live firewall change was
made at this checkpoint. The firewall suite is therefore not currently green.

P3 scoped rollback implementation supersedes that failing checkpoint: the owner
reads nft JSON table inventory, adopts only the exact inet table carrying its
versioned ownership comment, and deletes that table by kernel handle. Baseline
creation uses exclusive `create table` in the nft batch; the pre-activation full
snapshot is retained as `original.nft` and is never replayed over unrelated rules.
Every mutation checks observed table presence afterward. Status and mutation
paths reconcile kernel presence, so an owner restart recognizes its marked
table and repeated enable/disable is idempotent. Conflicting/unmarked legacy
tables are refused rather than adopted or removed automatically.

Eight injected-runner firewall regressions now pass, including the formerly
failing disable test, unrelated-rule preservation, restart, idempotency and
foreign-table refusal. Broker Clippy (`--all-targets -- -D warnings`), Rust format
and whitespace checks pass. The upstream nft manual confirms exclusive-create
and handle-delete semantics. Local `nft` is not installed, so actual nft grammar,
JSON comment/handle observations and target execution remain unverified gates;
reference modes, rule-content reconciliation, atomic snapshot-file hardening and
connectivity/recovery testing remain open.

P3 snapshot-file hardening: the system runner now bounds state-file content,
rejects symlink/non-directory ancestors and non-regular destinations, rejects
group/world-writable state directories, creates temporary files with mode 0600
using exclusive creation, fsyncs content, renames atomically and fsyncs the
directory. A real temporary-directory regression verifies replacement, private
permissions, temporary-file cleanup and file/directory symlink refusal. All nine
firewall tests, broker Clippy with warnings denied, Rust formatting and whitespace
checks pass. Target filesystem durability and injected I/O-failure coverage
remain unverified; this is not a power-loss recovery acceptance result.

P3 reference discovery: the local reference HEAD is `5b8ed3a`, while the pinned
`origin/main` is `ff7f42c`. Firewall API and service rules were therefore read
directly from the pinned Git objects. Their four-mode policy, HTTPS requirement,
60-second confirmation rollback, IPv4/IPv6 exceptions and unresolved Tailscale
behavior are mapped in `docs/design/firewall-reference-contract.md`. This is
source evidence for the port, not implemented mode parity or device acceptance.

P3 four-mode generator: `firewall_rules.rs` implements a pure, bounded-input
native nft renderer for baseline/moderate/restricted/paranoid. Native HTTPS and
syslog ports are explicit, restrictive modes require HTTPS, IPv4/IPv6 DHCP and
IPv6 control traffic are represented, and optional legacy backend isolation
requires a validated explicit interface/port rather than copied `eth0:8000`.
Three generator regressions verify policy distinctions, bootstrap exceptions,
HTTPS refusal and injection-resistant topology inputs. Tests, broker Clippy,
Rust format and whitespace checks pass. The renderer is not connected to live
mode changes yet: operation-bound confirmation/rollback, native port discovery,
v2 protocol/UI and actual nft parsing/packet behavior remain required.

P3 confirmation primitive: `firewall_confirmation.rs` represents prepared,
applied and rollback-required states bound to an operation ID, authenticated
session binding and boot ID. Confirmation before apply, at/after the 60-second
deadline, after clock reversal or after boot change is refused. A stale operation
cannot start rollback of another record; rollback latches and remains retryable.
Four state-machine regressions, broker Clippy, formatting and whitespace checks
pass. This primitive is not yet the runtime transaction: durable record encoding,
single-pending-operation admission, storage-failure handling, autonomous expiry,
kernel apply/read-back and API confirmation wiring remain open.

P3 pending-record codec: added a versioned, size-bounded JSON representation
with strict unknown/duplicate-field rejection and validation of identities,
requested mode and the exact confirmation interval. Round-trip tests retain
prepared/applied/rollback-required semantics; malformed, oversized, future-version
and ambiguous records are refused. Six confirmation tests, broker Clippy,
format and whitespace checks pass. The broker now directly uses the already
workspace-pinned serde dependency; lockfile review shows only this dependency
edge, no new package/version. Filesystem journal admission and runtime wiring
are still outstanding.

P3 journal admission: a single-slot pending journal now refuses overlapping
staging, persists phase transitions before accepting them, checks operation and
session identity before clearing, and retains pending work on storage errors.
An ambiguous failed initial save still occupies the slot and prevents kernel
application by returning failure. Rollback intent latches before storage I/O
and cannot be cancelled by a later confirmation. Eight confirmation/journal
tests pass, including injected save/clear failures and recovery from stored
records, alongside broker Clippy, formatting and whitespace checks. Tests use
an injected store; the production filesystem adapter and runtime transaction
integration remain open.

P3 filesystem journal adapter: pending records are loaded through non-following,
non-blocking file opens with regular-file/private-permission/link-count and size
checks. Save reuses the atomic synced state writer; clear writes an explicit
private tombstone through the same durability boundary. Empty/corrupt files are
errors, not absence. Real temporary-directory tests cover reopened journal
confirmation, tombstone recovery, permissions and symlink refusal. Ten
confirmation/journal tests, broker Clippy, format and whitespace checks pass.
libc moved from dev-only to normal workspace dependency for open flags, without
new packages or versions. Target filesystem/power-loss behavior and live runtime
application/expiry/rollback wiring remain unverified.

P3 transaction coordinator: `firewall_transaction.rs` now composes the journal
and a mode executor. Every changed mode is journaled before kernel application;
restrictive modes await identity-bound confirmation, while baseline/moderate
commit after verified read-back. Apply/read-back failure triggers rollback;
failed rollback retains the record for a later tick. Reopened pending work must
recover before new requests or confirmations. Tests verify ordering, overlap
refusal, confirmation, immediate rollback, retry, and restart recovery.
All 26 firewall-related tests, broker Clippy, format and whitespace checks pass.
The tests still use an injected mode executor: real nft mode observation/apply,
frozen configuration across restart, owner ticker and protocol/API/UI wiring
remain required before any runtime parity claim.

P3 real nft host gate: Ubuntu archive packages nftables/libnftables1
`1.0.9-1ubuntu0.1` and libnftnl11 `1.2.6-2build1` were downloaded through apt and
unpacked only into `/tmp/opencode/nft-validation`; no global install occurred.
`tools/check_firewall_nft_namespace.py` runs reviewed nft inside a fresh
unprivileged user/network namespace and refuses the parent namespace. Fixtures
are emitted by the Rust `firewall_rule_fixtures` example from the actual renderer.
The real parser caught an invalid quoted table identifier; correcting it made
all four policies pass syntax check and kernel application. The gate verifies
input/forward/output policies, actual JSON ownership comment/handle, and
handle-scoped deletion preserving an unrelated table. This also validates the
JSON table-inventory assumption for nft 1.0.9. Generator tests, broker Clippy,
Python compilation, Rust format and whitespace checks pass. No host or target
network rules were modified outside the disposable namespace. Packet-level
connectivity and target systemd-helper execution remain separate open gates.

P3 semantic observation: `firewall_observation.rs` checks the complete owned
table JSON against the generated mode policy: exact table marker, chain hooks/
priorities/policies, ordered rule expressions, address prefixes, ports, bootstrap
exceptions and verdicts. Only kernel handles and set ordering are normalized;
unexpected objects or modified rules are refused. The namespace gate now saves
actual nft JSON and the Rust fixture example's `--verify` mode checks all four
observations plus an injected unconditional-accept mutation for each mode.
Real nft observations passed; each overly permissive mutation was rejected.
Broker Clippy, format and whitespace checks passed. Integration into the mode
executor and broader target-version/configuration coverage remain outstanding.

P3 native mode executor: `firewall_executor.rs` now implements semantic mode
observation and a single handle-delete/exclusive-create nft batch for transitions.
It rejects unknown ownership/version, changed handles and rule-content drift,
then verifies the full candidate after applying. The isolated namespace gate
now runs the actual executor with the real file journal and transaction
coordinator, proving apply/read-back, timeout-triggered rollback via an injected
clock tick, confirmed-mode retention, same-boot process restart recovery and
preservation of an unrelated table. Altering the actual input policy to accept
is detected and blocks subsequent writes. The gate and broker Clippy/format/
whitespace checks pass. It uses a namespace-local direct nft runner, not the
target systemd runner; frozen config persistence, owner timer, API/UI integration
and real packet/device checks remain open.

P3 frozen executor configuration: the executor atomically persists a private,
versioned configuration and reloads those original inputs across restart.
External configuration drift blocks new transactions but does not prevent
rollback using the retained ports. Missing configuration with pending work,
malformed files and symlinked files are refused without replacement. The real
nft namespace recovery gate now changes the requested HTTPS port on reopening,
proves restoration with the original port and proves new writes are refused
until configuration reconciliation. Two file regressions, the namespace gate,
broker Clippy, Rust format and whitespace checks passed. Operator-driven config
reconciliation and owner/protocol integration are still required.

P3 typed wire preparation: the canonical four-mode enum now resides in core,
shared by renderer/journal/executor. Added strict change/confirm request types
with operation ID, session binding and issuance timestamp, and corresponding
control-v2 variants. Unknown mode strings, caller-supplied native port fields,
zero identities and zero timestamps are rejected. The broker currently returns
CapabilityDisabled for these variants until runtime attachment is implemented,
with a regression proving fail-closed dispatch. Core contract regression, 29
broker firewall tests, restricted-server/broker compilation, core/broker Clippy,
format and whitespace checks pass. API routing and runtime handling remain open.

P3 owner dispatch plumbing: an attachable, separately synchronized ModeRuntime
now handles typed change/confirm requests, validates identity/freshness, and maps
transaction outcomes to control refusals. Socket dispatch releases the shared
control-state mutex before invoking blocking nft work. An independent one-second
timer invokes recovery/expiry on the attached runtime, with bounded task count
and retained pending state on failure. Attaching this runtime disables the old
baseline mutation path. The 29 existing firewall tests plus a new freshness
regression pass; broker Clippy, format and whitespace checks pass. Board-profile
initialization does not attach it yet, so no runtime mode capability is active;
profile configuration, status/API/UI and autonomous-timer integration testing
remain required.

P3 typed mode status: core/control-v2 now carries verified mode, pending operation
ID, confirmation deadline, recovery-required and configuration-ready fields.
Runtime status uses the same isolated firewall worker, and protocol validation
rejects inconsistent pending/deadline pairs or recovery without a pending record.
Unrelated HID/ATX/media response handlers explicitly refuse this new response
type. Transaction status regressions, all 15 pre-existing protocol tests, the new
status-validation test, restricted-server/broker compilation, format and
whitespace checks passed. HTTP/UI consumption and profile attachment remain open.

P3 HTTP adapter: authenticated GET/POST `/api/system/firewall/mode` and POST
`/api/system/firewall/mode/confirm` now forward typed operations. Changes require
explicit confirmation; operation IDs come from OS randomness and session
bindings from a domain-separated hash of the authenticated session, never the
request body. Pending IDs are lowercase hex and deadlines are decimal strings.
A fake-owner socket integration test proves authentication/CSRF/confirmation
gates, change-to-confirm identity continuity and preservation of owner refusal.
Both new API tests, restricted-server Clippy, default-server compilation and
whitespace checks pass. The existing envelope uses nonzero `code` for bad-request
errors even with HTTP 200; the regression checks the actual API contract.
UI and board-profile activation remain unconnected.

P3 four-mode UI: a separate `system_firewall_modes` capability selects the new
API-backed view rather than confusing reference baseline with the legacy native
toggle. It validates status, checks mode and operation-ID read-back, shows a
pending deadline, polls only GET while pending, and requires a separate explicit
connectivity-confirmation click. Read-only, recovery and config-drift states
block writes. The new `firewall_modes_contract.mjs` probe verifies page-open,
Cancel, refusal and explicit successful confirmation on desktop/mobile and is
included in `scripts/check.sh`. Fresh production web build, targeted ESLint,
browser probe and whitespace checks pass. Board-profile/server capability
activation and timeout/rollback browser coverage remain open.

P3 profile activation wiring: server option/environment `BLI_KVM_FIREWALL_MODES=1`
is now required for the new HTTP surface and capability. The native board profile
attaches ModeRuntime when `BLI_KVM_FIREWALL_MODE_CONFIG` names a reviewed absolute
configuration file; required ports are decoded strictly, symlinks/oversized or
writable-by-others files are rejected, and boot identity comes from the kernel.
The existing firewall runtime directory stores frozen config and pending state.
Native gadget/HID/chardev-ATX/system-action/network-write profile compilation,
three configuration tests, two HTTP contract tests, Rust format and whitespace
checks passed. Neither environment switch was deployed to the device. Packaging
must preserve the configured owner environment during pending recovery; missing
configuration recovery, legacy-table migration, actual HTTPS health preflight,
bounded helper execution and device connectivity remain open prerequisites.

P3 helper bounds: firewall process capture now uses nonblocking stdout/stderr,
fixed-size reads, per-stream output limits and a monotonic deadline, killing and
reaping the launcher on error/timeout. The transient nft unit also has explicit
start/runtime/stop timeouts and disables password prompts. Real subprocess tests
cover successful output, failed exit, oversized output and a stalled child;
both pass with broker Clippy, format and whitespace checks. The only new unsafe
calls are documented fcntl operations on owned live pipe descriptors. Target
systemd timeout/output behavior still requires a device gate; this does not
replace that acceptance check.

P3 lost-launch-config recovery: the board profile now attaches a recovery-only
runtime when a pending record exists but the mode-config environment variable
is absent. Missing/invalid launch files also fall back to frozen configuration
only for existing pending work. This path never recreates lost frozen state and
refuses new changes. The real nft namespace gate proves rollback and write refusal
without a requested configuration; a file regression proves a missing launch
file does not discard pending recovery. Namespace gate, native-profile check,
runtime tests, broker Clippy, formatting and whitespace checks pass. Actual
owner-timer/device restart behavior remains to be measured.

P3 browser timeout/rollback follow-up: the expanded firewall-mode probe passes a
mobile scenario where the owner reports an expired pending operation, controls
become disabled, and polling observes restoration of the previous mode without
any additional browser POST. Existing explicit-confirmation/refusal/read-only
desktop/mobile scenarios also pass. This fixture-driven check does not measure
the real one-minute owner timer.

P3 HTTPS preflight: restrictive transitions now require a configured TLS probe
before journaling or kernel changes. It verifies the configured server name and
CA certificate, resolves only to 127.0.0.1 on the configured HTTPS port, disables
curl config/proxy/redirect behavior, uses bounded connect/total timeouts, and
requires the native Rust BLi health response. The owner runs this helper in the
host network namespace without CAP_NET_ADMIN. Missing probe or failed health
refuses the transition. All 37 firewall tests, Clippy and namespace transaction
gate pass; that nft-focused gate explicitly simulates HTTPS success. Real TLS
certificate/hostname failure cases and target systemd preflight are still open.

P3 real TLS preflight gate: `check_firewall_https.py` creates ephemeral test
certificates and a loopback TLS server, then invokes the actual Rust probe via
`firewall_health_gate`. Trusted certificate succeeds; wrong hostname, untrusted
CA, HTTP redirect and unhealthy JSON are refused. The probe now requires HTTP
200 in addition to the native health body and does not follow redirects. This
host gate is included in `scripts/check.sh`; it passed along with health unit
tests, broker Clippy, formatting and whitespace checks. Test keys are temporary
and deleted after the run. Target systemd namespace/TLS behavior remains open.

## P4 video-control checkpoint

Screen previously replayed saved FPS and quality through POST requests as soon
as the desktop mounted, even for unadvertised quality. Startup now only reads
owner settings, requires valid applied state and initializes requested FPS from
that response rather than local preference. Controls wait for valid read-back;
load failures are visible. A browser regression seeds conflicting saved
preferences and proves exactly one screen GET, no startup writes, and the owner
FPS selected in the menu. Fresh web build, targeted ESLint, browser probe and
whitespace checks passed; the probe is included in `scripts/check.sh`.
Resolution wiring, mutation outcomes, refreshed measured state and H.264 remain
open P4 work.

FPS mutation follow-up: explicit rejection/applied checks now precede owner
read-back, and the saved preference changes only after the returned FPS matches
the request. Fractional/out-of-range and concurrent submissions are refused.
Successful refresh updates captured/queued rates and dimensions; absent measured
values clear stale observations. Browser regression proves a refused update
keeps the old selection/preference, followed by a successful save and refreshed
measured rate, with exact GET/POST ordering. Build, targeted ESLint, expanded
browser probe and whitespace checks pass.

P4 Resolution wiring: owner read-back now carries an optional validated capture
resolution through control-v2 and the HTTP `requestedResolution` field. This is
separate from engine-measured width/height. Screen mounts Resolution only when
advertised and known, selects the owner-requested mode, checks mutation refusal
and matching read-back, and does not overwrite browser viewport resolution.
The video browser probe uses differing requested/measured dimensions and verifies
initial selection, refused change and successful change. Four server video tests,
broker compilation, web build, screen ESLint and browser/whitespace checks pass.
Actual device resolution/FPS behavior, quality efficacy and H.264 remain open.

P4 quality/source follow-up: Quality now checks explicit application and matching
owner read-back before saving its preference, rejects non-MJPEG use rather than
sending bitrate values as JPEG quality, and clears unknown selections. The
expanded local video probe passed refusal and successful quality-contract cases
with a fixture-advertised quality capability; this does not prove image-quality
efficacy on H616 or enable that capability on the device. Web build and targeted
ESLint passed. Pinned newer vendor H616/Janus sources and the unresolved libx264
build provenance are recorded in `docs/design/h616-h264-source-contract.md`.

P4 static vendor artifact gate: pinned uStreamer and Janus-plugin blobs were
downloaded to the D: build volume and matched against Git blob IDs and SHA-256.
The H616 ELF is AArch64, links libx264.so.163 and contains the LIBX264 encoder
markers/functions. The matching plugin's architecture/dependencies are recorded
in `h616-vendor-artifacts.json`. Neither binary was executed. This resolves the
binary-feature-presence question, while source/build provenance, target ABI,
runtime encoding and performance remain open.

P4 video rollback follow-up: a regression reproduced that candidate restart
failure restored the old env file but never restarted the previous service.
Rollback now restores the file and restarts the previous configuration. If either
step fails, read-back and subsequent changes return BackendUnavailable instead
of claiming an active engine. Five video-owner tests, broker Clippy, Rust format
and whitespace checks pass. Actual service failure/recovery remains a device gate.

P4 video env-write regression: a pre-existing symlink at the fixed `.env.new`
path reproduced an overwrite of an unrelated file. The writer now uses unique
exclusive temporary creation, rejects unsafe directory/destination types,
sets the intended engine-readable mode on the owned file descriptor, syncs the
file, atomically renames it and syncs the directory. The unrelated-file regression
now passes alongside all six video-owner tests, broker Clippy, formatting and
whitespace checks. Target filesystem failure/recovery remains a separate gate.

P4 measured-state parsing: replaced textual number searching with JSON parsing
of the observed `result.source`/`result.stream` contract. Captured and queued FPS
retain fractional precision; dimensions must be positive bounded integers and
cannot saturate or come from unrelated fields. Non-200 HTTP, malformed/failed
engine responses and oversized reads do not supply measurements. Eight engine
tests pass, including actual Unix-socket success/error responses, together with
the related video suite, server Clippy, format and whitespace checks. No new
hardware capture-rate claim is made.

P4 engine read deadline: measured-state response reads now share one monotonic
800 ms deadline instead of resetting the timeout for every read. A Unix-socket
regression feeds valid JSON slowly enough to exceed the total deadline while
remaining inside each former per-read timeout; the response is refused. All
nine engine tests, server Clippy and whitespace checks pass. Socket-connect and
target engine behavior remain subject to integration/device verification.

## P5 packaging checkpoint

The packaged Nginx configuration previously capped every request at 16 MiB and
buffered upload bodies in its `/run` temporary directory. Exact locations now
match the server limits: 128 MiB for `/api/application-update/upload`, and
4097 MiB for `/api/download/file` (4 GiB media plus 1 MiB multipart allowance).
Both disable request buffering; all other paths retain the 16 MiB cap.

`check_nginx_upload_contract.py` passed against Ubuntu nginx
`1.24.0-2ubuntu7.18`, downloaded through apt and unpacked only under
`/tmp/opencode/nginx-validation`. It uses the actual packaged configuration with
isolated loopback ports, temporary TLS material and a local upstream. Declared
large upload headers reach the upstream before the full body; over-limit and
non-upload paths return 413 without reaching it. Host/HTTPS forwarding is
preserved. This tests admission/streaming behavior, not transfer of a full 4 GiB
image. Basedpyright and whitespace checks pass. No global install, deployed
proxy reload or device upload occurred.

P5 bundle/profile alignment: the standard bundle's default feature set now
includes network-write-profile, matching the existing native board recipe.
The bundle and Debian runtime map now retain the reviewed board-owner socket
group drop-in and video tmpfiles template (the latter remains an inert example
pending provisioning). Standard bundle staging moved under the selected output
volume rather than the WSL `/tmp` disk. Seven Debian package-map/contract tests,
builder Basedpyright, shell syntax and whitespace checks pass. No full bundle
or Debian artifact was built: that long command remains subject to Longrun.
Provisioning, update-runtime composition and actual clean-install parity remain
open P5 requirements.

P5 archive publication failure regression: executing the actual publication
block with an injected failing tar reproduced a zero exit status and replacement
of the previous release by incomplete output. An injected gzip failure also left
an orphan temporary archive. The builder now explicitly requires Bash with
pipefail, and keeps its temporary archive inside the already guarded staging
directory on the output volume. Failed tar/gzip preserves the previous archive
and checksum and removes temporary output. The new test exercises the actual
publication/cleanup blocks; real tar/gzip also produce identical fixture archives
with different source mtimes, normalized metadata and a matching checksum.
Both tests pass; Basedpyright reports zero errors/warnings, Bash/sh syntax and
git diff whitespace checks pass. ShellCheck is unavailable. The regression is
included in scripts/check.sh. This is bounded publisher evidence, not a full
cross-build, full-release reproducibility result or crash-atomic archive/checksum
pair guarantee. Longrun tools remain unavailable, so the full build and full
scripts/check.sh are still unrun.

P5 candidate health reporter: a real loopback regression reproduced acceptance
of a slowly completed health response after about two seconds despite the
500 ms read timeout. Response reading now uses a single monotonic 500 ms
deadline, recomputes the remaining socket timeout and rejects late completion.
The existing 8 KiB response bound and native JSON health checks remain enforced.
Loopback tests cover the slow response, exact size boundary, one-byte overflow,
and successful one-use credential submission. All seven reporter tests and
all-targets Clippy pass; formatting and whitespace checks pass. Host compilation
used the D: target-host cache. This does not establish target systemd activation,
full application composition health, Unix confirmation-connect bounds or
end-to-end rollback timing; those P5 gates remain open.

P5 composition audit found a high-severity release gap: the signed application
payload includes owner, video proxy and boardd binaries, but the packaged native
units execute those components from /usr/libexec while candidate/confirmed
activation restarts only the slot server. Generic server health therefore does
not prove application-wide activation or rollback. The shipped legacy inventory
broker template also requires kvmd.service and cannot be used as the native
board provisioning recipe. Candidate/current update API environment parity is
not supplied by the board-server-only 94-application-update drop-in. Effective
device overrides were not inspected. Exact source-to-component mapping and the
required coordinated transition, health, recovery and acceptance contract are
recorded in docs/design/application-update-composition.md. Implementation and
device evidence for this composition gap remain required; P5 is not passed.

P5 coordinated transition foundation: ApplicationUpdateManager now exposes
activate_with_quiescence. The hook runs after durable Prepared intent and before
the candidate pointer switch; immediate/replayed outcomes do not invoke it.
A failed hook leaves Prepared intent and the previous selection for recovery,
including after a partial component stop. Tests inspect the real persisted
manager state inside the hook, reopen after failure and verify rollback without
raising the floor, and reject repeated quiescence on a confirmed replay.
All 14 manager tests and all-targets Clippy pass; formatting and whitespace
checks pass. The existing activate entrypoint retains its previous behavior.
The supervisor/native coordinator still needs to use this hook with bounded
component quiescence, full-composition start/readiness and recovery. This API
foundation alone does not resolve the composition gap or change device services.

P5 supervisor integration foundation: activate_if_generation_with_quiescence
now carries the hook through the existing generation/target checks and durable
manager activation. Failed activation retains its health challenge when pending
state exists or cannot be established, rather than deleting the only remaining
recovery anchor after a partial quiescence. A regression simulates interruption
after journal rollback but before confirmed-apply publication, reopens the
supervisor and verifies that the stale challenge regenerates the apply request
for the previous slot without raising the floor. Another verifies stale requests
cannot invoke quiescence and successful quiescence still awaits health.
All 17 update-runtime tests and all-targets Clippy pass; formatting and whitespace
checks pass. Native service execution, composition readiness and deployment are
still required. Existing CLI/socket callers do not yet supply native quiescence.

P5 owner quiescence prerequisite: the broker serve loop currently has no graceful
SIGTERM path, so systemctl stop alone is not evidence of HID release. Added
ControlV2State::begin_quiescence to close command admission before policy
disconnect, suppress jiggler ticks while disabled and permit retry of persisted
releases without reopening admission. Host tests verify held-key release, late
input rejection, idempotent repeated quiescence and retry after both the initial
release and its immediate fallback fail. All nine v2 tests with
hid-transition-profile and corresponding all-targets Clippy pass. These tests
use the existing local HTTP HID fixture, not an installed KVMD service or native
device acceptance. Graceful signal handling, in-flight worker draining and
coordinator invocation remain to be wired; native gadget/ATX/media stop behavior
still requires dedicated host and observed-device gates.

P5 graceful owner shutdown is now wired in the serving phase: SIGTERM/SIGINT
trigger listener closure, periodic-worker stop, up to three HID release attempts
and joining of accepted handlers/workers. JoinSet also prevents detached ticker
tasks when serving is cancelled. A 20-second drain limit reports failure rather
than clean quiescence; the board-owner unit allows 30 seconds to stop. Running
spawn_blocking work is not cancelled by dropping an async task, so the coordinator
must still require process exit and successful shutdown before a new owner.
The gadget-hid-profile host suite passes all 88 tests, including a regular-file
gadget fixture that receives a held key and then a zero keyboard report before
graceful exit, socket cleanup, in-flight worker draining and worker-panic failure.
All-targets Clippy and whitespace checks pass. Tokio signal support uses the
already locked signal-hook-registry version; no dependency versions changed.
These shutdown tests inject the stop future; actual OS-signal subprocess and
target systemd stop/start gates, startup-interruption handling and the full
native composition coordinator remain open.

P5 OS-signal host gate: tests/graceful_signal.rs launches the actual compiled
broker binary with a cleared environment and only temporary native HID fixture
paths. Separate SIGTERM and SIGINT cases acquire a lease, hold a key, signal the
owned subprocess and require exit success, a zero keyboard report, empty release
ledger and socket removal. A failed-release case removes the fixture endpoint
before SIGTERM and requires a nonzero exit plus retained pending releases.
The bounded three-case integration test passes, as do all-targets Clippy and
whitespace checks. Existing scripts/check.sh all-targets gadget-profile commands
include this integration test. This closes the host serving-phase signal gate;
actual character-device behavior, target systemd stop/start and startup-phase
interruption are still unverified. Longrun remains unavailable for full checks.

P5 native coordinator kernel: update-runtime/composition.rs defines a closed
application unit set and two-phase quiescence. Board/current/candidate/bootstrap
servers and boardd must all be observed cleanly stopped before stop dispatch
reaches the native video proxy and owner. Every backend must then be observed
clean before the hook may succeed. Running/queued, failed and unknown states
reject the transition; dispatch success is not substituted for observation.
Three host tests pass, including every unit in each non-clean state and command
dispatch failure; all-targets Clippy and whitespace checks pass. The controller
interface requires a shared finite deadline. Its real systemd adapter, clean
stop evidence parsing, startup/readiness, recovery orchestration and activation
caller wiring remain unimplemented. This is coordinator ordering logic, not
evidence of a completed native application transition.

P5 systemd stop adapter: update-runtime/composition_systemd.rs now implements
the controller with fixed unit names and an absolute systemctl executable,
cleared environment, no interactive authorization, asynchronous stop dispatch,
and bounded show polling under one monotonic deadline (maximum 45 seconds;
each client invocation at most two seconds). Stdout is capped at 4096 bytes;
failed, oversized and timed-out clients fail closed and are killed/reaped.
Killing the client does not cancel an already queued systemd job, so callers
must retain recovery intent. No production activation caller uses this yet.

Observation requires exactly the requested nine fields, a loaded inactive/dead
unit, zero main/control PIDs, no queued job, successful result and a normal
zero-status exit (or a never-started unit). Duplicate/missing/invalid properties
are rejected. Read-only host systemctl observations established the displayed
empty Job and never-started ExecMainCode=0 format; no host unit was stopped.
Six adapter tests exercise static subprocess fixtures, argv, nonzero exits,
output overflow, hung-client kill/reap, shared deadlines and running-state
timeout. All 26 update-runtime library tests and all-targets Clippy pass.
These are host adapter gates, not target stop/recovery evidence. Effective
unit kill policy/cgroup cleanup, successful termination semantics for every
producer, dependency deadlocks, coherent startup/readiness, recovery and caller
wiring remain required. Full scripts/check.sh is still blocked by unavailable
Longrun tools.

P5 stop-evidence follow-up: the adapter now requests twelve exact properties,
including KillMode, SendSIGKILL and ControlGroup. It rejects process-only/no-kill
policies or disabled final killing, and waits under the same deadline while a
group remains. Host read-only systemctl confirms the empty-group format for an
inactive unit. Nine composition/adapter tests pass with retained-group and
unsafe-policy regressions. This does not certify target group cleanup.

Source wiring review also found the activation worker's unprivileged account,
15-second service limit and response-after-activation ordering. Inline graceful
server stop risks waiting on the request whose response the worker has not yet
sent. Server main currently has no SIGTERM handler. These constraints are
recorded in application-update-composition.md and must be resolved through
coordinated admission/execution and shutdown; the production hook remains
unwired rather than introducing that cycle or broadening worker privileges.

P5 server shutdown implementation: server main now installs SIGTERM/SIGINT
handlers before initialization, closes admission through a shared shutdown
handle, and awaits Axum graceful drain plus tracked upgraded connections under
one 20-second deadline. Deadline expiry is an error exit. HID upgrades register
before handoff, reject late registration, observe shutdown before receiving
another frame, and retain their drain guard through session HID release. MJPEG
bodies register a shutdown wakeup so an idle upstream does not hold HTTP drain
open. Global middleware rejects new public/protected/static requests while
stopping; already admitted work remains subject to the drain deadline.

Three new host tests cover late upgrade rejection/drain tracking, public request
admission and idle video wake/drop. The first test build exceeded its 30-second
tool deadline during dependency rebuild and produced no result. After the
signal-enabled dependencies and subsequent source edits were checked, the
incremental targeted test build completed in 27.70 seconds with all three
passing. The full restricted server library suite then passed 146 tests with
two existing environment-gated Nginx diagnostics ignored; all-targets Clippy
and whitespace checks pass. No dependency version was changed for this work.
Actual server OS-signal subprocess, live WebSocket release, slow-request timeout
and target systemd gates remain required. The activation admission/execution
cycle and native composition wiring remain unresolved; P5 is still open.

P5 server OS-signal host gate: server/tests/graceful_signal.rs now starts the
actual compiled server with cleared environment, temporary web/state paths and
explicit absent backend sockets. It waits for a bounded loopback HTTP health
response, sends SIGTERM or SIGINT only to its owned unreaped child, and requires
a normal zero exit within three seconds. Both cases pass; cleanup kills/reaps
on failure. Health capture has an absolute response deadline and an 8 KiB cap.
All-targets Clippy and whitespace checks pass. Existing scripts/check.sh server
all-targets commands include the new integration test. The test uses the already
locked libc as a dev dependency; no dependency versions changed. This proves
idle serving-phase signal handling, not active HID/WebSocket release, stalled
request timeout, startup interruption or target systemd behavior.

P5 stalled-request signal gate: the real-process fixture now sends an incomplete
login JSON request and requires HTTP 100 Continue before SIGTERM, proving that
body consumption was admitted. Keeping that connection open produces exit code
1 after the server's 20-second drain deadline rather than a false clean exit.
All three signal cases pass (20.12 seconds total test runtime), with all-targets
Clippy and whitespace checks passing. The bootstrap, board, current and candidate
server units previously allowed only 10 seconds to stop; their TimeoutStopSec is
now 30 seconds so systemd can observe the internal drain result before forceful
termination. This is source packaging alignment, not installed target evidence.
Live WebSocket HID release and native activation/start/readiness/rollback remain
open. Full scripts/check.sh and release builds still require unavailable Longrun.

P5 live WebSocket drain gate: the HID lease fixture now also starts real loopback
Axum HTTP, performs an authenticated RFC6455 upgrade, and sends a masked keyboard
frame under the acquired lease. The fixture verifies the exact held-key report
and subsequent release for that lease. On shutdown it delays the release reply
and asserts that upgraded-connection drain cannot finish until the reply is
allowed; then both HTTP and upgrade drain complete. The new bounded test and
the existing logout/lease regression pass, as do all-targets Clippy and whitespace
checks. The fixture now returns Applied for release/report, reserving LeaseGranted
for acquire. This is real WebSocket transport with a simulated control broker,
not physical HID release or target SIGTERM evidence. Activation admission and
privileged execution coordination, complete native startup/readiness/rollback,
and release-bound device acceptance are still open.

P5 post-quiescence clock fence: UpdateSupervisor now samples kernel evidence
again after its quiescence hook and before the manager switches the slot. A boot
identity change, monotonic reversal, expired health deadline or failed clock read
rejects the transition while retaining Prepared intent and the recovery anchor.
Quiescence does not renew the original health budget. Boundary tests cover the
same-boot valid interval, exact deadline, expiry, reversal and changed boot;
all 27 update-runtime library tests, all-targets Clippy and whitespace checks
pass. This closes a time-validation gap in the supervisor hook path, not the
remaining admission/execution split or full composition transition.

P5 manager admission/execution split: admit_activation now persists Prepared
intent without quiescence or pointer selection and returns an explicit
ActivationPreparation outcome. execute_prepared_activation can resume that
operation after reopening the manager: it validates operation, health binding,
target, boot, time interval, phase, referenced slots and previous pointer before
calling quiescence, then samples caller-provided trusted clock evidence and
validates again before switching. Reverse time during execution is rejected.
No request supplies a service command; manager state/slot locks remain held.

Three new tests cover reopen/execute without early pointer change, invalid/stale
intent rejected before any stop, execution replay, post-quiescence clock failure
and recovery with unchanged floor. All 17 manager and 27 runtime tests pass;
both all-targets Clippy checks and whitespace checks pass. These manager APIs
are not wired into the network or systemd path. Runtime challenge binding,
generation-bound admission receipts, execution authorization and watchdog/handoff
serialization still need implementation: existing recovery treats Prepared as
interrupted, so asynchronous admission cannot simply race the current watchdog.

P5 serialized runtime handoff: UpdateSupervisor exposes generation-bound
activate_if_generation_with_admission. Its explicit SupervisorAdmission receipt
describes Prepared (operation, target, prepared generation and original deadline),
not AwaitingHealth. Delivery runs after durable preparation and before quiescence
while the manager retains its state/slot locks. The callback contract requires
bounded delivery and activation-stream shutdown before returning success. Delivery
failure skips quiescence and preserves recovery intent/challenge; a reopened
watchdog restores confirmed-apply state. Fresh kernel evidence is checked after
delivery and after quiescence. This serialized path avoids an unlocked Prepared
handoff window; separate-process resume still requires explicit coordination.

Two new regressions verify callback ordering, exact receipt binding, stale-request
silence and failed-delivery recovery. The initial 29-test run had one unexplained
Controller failure in the pre-existing systemd command-argument subprocess fixture;
the isolated test and subsequent full 29-test run passed. Clippy and whitespace
checks pass. The transient failure is not diagnosed or claimed fixed and remains
a test-reliability follow-up. Network receipt encoding/stream closure, narrow
systemd authorization, worker timing and full native lifecycle wiring are still
unimplemented; this callback API alone does not close P5.

P5 admission wire foundation: activation protocol now distinguishes an Admit
request and Prepared response from legacy Activate/AwaitingHealth. Prepared
receipts contain operation, target, prepared generation and monotonic deadline;
unknown fields, invalid identities and zero counters are rejected while full
u64 precision is preserved. UpdateActivationClient::admit requires exact request
ID/target, expected generation plus one, and response EOF under the existing
total deadline. It never retries. Legacy/admission clients reject each other's
success response rather than mislabeling the state. The current socket worker
explicitly refuses Admit until the native executor is wired; it cannot fall
through to legacy activation. No new update behavior is enabled on the device.

All 15 update-client and 11 protocol library tests pass, including separate
request kinds, mismatched generation/target, legacy cross-responses and held EOF.
All-targets Clippy for the changed crates and supervisor passes (server checked
as a dependent target); whitespace checks pass. Next requirements are bounded
Prepared delivery/stream closure, native execution authorization/timing,
HTTP/UI admission semantics and full lifecycle/rollback integration.

P5 activation delivery boundary: activation_socket now uses one absolute deadline
for the complete encoded response and explicitly shuts down the Unix stream after
the final byte. EOF is published even if duplicate stdin/stdout descriptors remain
open while execution continues. Zero writes, broken peers, expiration and blocked
writes return errors. The existing legacy AwaitingHealth path uses the helper;
its already-committed lost-response dispatch rule is preserved. Native admission
will require helper success before quiescence; Admit is still unavailable until
the full executor is connected.

Three Unix-pair regressions verify exact Prepared frame/EOF with retained duplicate
descriptors, refusal of late writes, closed-peer/expired-deadline failure and
nonreading-peer backpressure timeout. All 14 supervisor binary tests pass. Initial
Clippy reported test-module placement; it was corrected without suppression, and
all-targets Clippy plus whitespace checks pass. Actual systemd native worker
authorization/timing, production admission callback and complete start/readiness/
rollback remain open. The earlier intermittent systemd fixture failure remains
undetermined; this delivery test does not resolve it.

P5 native payload admission gate: generation-bound native admission now requires
the already verified manifest to contain nonempty server, control-broker, video
proxy and boardd executables at mode 0755, plus share/web/index.html at 0644.
Missing, duplicate, empty or incorrectly moded native entries fail before health
challenge issuance, Prepared delivery or quiescence. The legacy platform-broker
may remain in a bundle but is not started or counted as native board inventory.
Legacy activation compatibility is unchanged; this is an additional native gate.

A genuinely signed/staged legacy fixture without boardd passes the generic
stager but is rejected by native admission without changing manager state or
issuing a challenge. Per-component inventory regressions also pass. All 31
runtime library tests and runtime/supervisor all-targets Clippy pass, as does
whitespace checking. This establishes payload coverage only: executable identity,
system prerequisite revision, native startup/readiness and rollback evidence are
still required. Native network Admit remains disabled pending complete execution.

P5 executable identity primitive: process_identity.rs observes a supplied unit PID
through a retained proc directory descriptor, compares the kernel exe link's
device/inode with a caller-held verified executable, and requires stable process
start ticks across observations. Stat reads are capped at 4096 bytes; malformed,
dead, exited or inaccessible processes fail closed. Proc comm parsing handles
spaces, parentheses and newlines. No payload-supplied process path or command is
executed. This is a point-in-time identity check; callers must retain verified
slot files and recheck identities around native readiness probes.

Three host tests pass: current-process matching/unrelated-file refusal, a real
owned sleep subprocess whose anchored proc entry becomes invalid after exit,
and complex/malformed/dead stat parsing. Clippy and whitespace checks pass.
This does not prove ELF provenance, unit-to-PID binding, protocol readiness or
installed target identity. Integration with native startup and complete update
health remains required; the primitive is not yet called by production activation.

P5 unit-to-process identity observation: SystemdController now binds a fixed
application unit to the open verified executable. It requires exact Id, loaded
active/running state, a canonical nonzero MainPID, zero ControlPID, no queued job
and successful Result; duplicate, missing and unexpected properties fail closed.
It samples the unit twice and checks proc executable/start-time identity twice,
rejecting a changed PID or identity under the controller's shared deadline.

Three additional adapter regressions cover invalid unit/state/PID fields, live
process matching versus a wrong unit/executable, and MainPID changing between
queries. All nine systemd adapter tests, Clippy and whitespace checks pass after
correcting one comparison-style lint without suppression. Systemd responses are
fixtures while executable identity uses the live test process; no installed unit
was started/stopped. IPC readiness, coherent slot-file acquisition, candidate
startup and production activation/health integration remain required. The prior
sporadic subprocess Controller failure was not reproduced or diagnosed here.

P5 verified executable handles: stager open_verified_files and the manager wrapper
now return an opaque VerifiedSlotFiles selection after full published-slot
verification. Selected names must exactly match signed manifest entries; empty,
duplicate, metadata and traversal requests are rejected. Directory traversal is
descriptor-relative from the held root, rejects symlinks and checks immutable
directory ownership/device/mode. Selected files are rehashed through their open
read-only descriptors, checked against immutable published modes (0555/0444),
rewound and retained for process identity checks. Callers must keep the manager
lock and protect the slot from garbage collection while using the handles.

The new signed-slot regression verifies exact modes, rejected writes/paths,
unchanged manager state and retained original inode/content after a controlled
fixture pathname replacement; reopening the tampered tree fails. All 18 manager
tests, 16 stager integration tests, both Clippy checks and whitespace checks pass.
These handles still need integration with native component startup/identity and
IPC health; production Admit remains disabled until lifecycle execution is ready.

P5 native identity context: NativeSlotIdentity now retains the full verified file
selection and borrows the manager so its lock owner outlives the context. It maps
owner, video proxy, boardd and the chosen candidate/current server to their exact
slot handles, and requires all three alternative server units cleanly stopped.
Its observation returns a slot-bound four-process snapshot for comparison around
future bounded IPC probes. The systemd adapter implements the identity interface;
the context itself starts/stops no service and does not confirm update health.

A signed-slot integration regression verifies the selected handle contents for
all four native components in candidate and current modes, and rejects an extra
running server before collecting identities. The test passes along with manager/
runtime Clippy and whitespace checks. Service observations in this composition
test are fixtures; target coherent identities and native IPC health remain open.
Production admission/execution and rollback integration are still required.

P5 boardd IPC prerequisite: inspection found that the JSON-line endpoint allocated
an entire untrusted line before enforcing MAX_REQUEST_BYTES. Reads now stop at
4096 plus one overflow byte, preserving the existing invalid_request/close path
without buffering an oversized request. Regressions cover a finite 4 MiB
unterminated source, the exact line boundary, subsequent separate requests and
EOF. All 13 boardd server tests, all-targets Clippy and whitespace checks pass.
This fixes allocation bounds only. The serial connection loop still needs
reviewed time/admission bounds, and boardd shutdown plus native IPC readiness
integration remain open. No board hardware was accessed by these tests.

P5 boardd connection time bound: each accepted IPC connection now has one
two-second monotonic budget shared by all request reads and response writes.
Partial input and later requests do not refresh it; buffered requests check it
before dispatch and again after obtaining the state lock. Long-lived clients
must reconnect after this bounded exchange. Existing JSON-line framing and
invalid-request behavior are preserved within the budget.

Four Unix-pair regressions cover idle input, trickled bytes, nonreading-peer
backpressure and a request whose deadline expires while waiting for state; the
last case cannot change the fixture's fan mode. All 17 boardd server tests,
Clippy and whitespace checks pass. This is a transport/admission deadline, not
a bound on mutex acquisition or an already executing hardware operation; boardd
graceful shutdown and native update readiness remain required. No device mutation
or physical fan behavior was tested.

P5 boardd listener shutdown foundation: serve_until accepts a stop predicate,
closes admission and refuses queued commands once stopping (including after
state-lock acquisition). The nonblocking accept loop observes stop every 10 ms;
in-flight reads retain the shared connection deadline. Clean exit removes only
the socket matching the listener's recorded device/inode. Startup no longer
unlinks an occupied socket or unrelated file. Stale sockets outside systemd
require explicit cleanup; the packaged unit already has RuntimeDirectoryPreserve=no.

Three new fixture regressions cover listener stop/owned-path cleanup, occupied
socket and unrelated-file preservation, and queued mutation refusal. All 20
boardd server tests, Clippy and whitespace checks pass. The current run_serve
entrypoint still uses the non-stopping wrapper: OS signals, sampler/PWM shutdown,
hardware handoff and target evidence remain open. No installed services changed.

P5 fan shutdown prerequisite: the existing FanSupervisor::prepare_shutdown
contract requests 100-percent failsafe, whereas SoftPwmFan::stop explicitly
drives inactive and releases the line. Added a distinct quiesce_at_failsafe path
that requires full duty, stops/joins PWM, then reasserts the line and retains
ownership until resource/process teardown. Joining before the final write prevents
a late PWM phase from undoing that level. A worker panic or final write failure
is reported rather than counted as clean shutdown. BoarddState now exposes the
supervisor's existing shutdown preparation for subsequent runtime wiring.

The 52-test boardd suite passed before the additional final-write-failure case;
all six fan tests then passed with that regression, as did Clippy and whitespace
checks. Fixtures verify full-duty policy, worker join, no late writes, retained
line ownership and failure reporting. The actual electrical level after GPIO
descriptor closure and the cooling gap across owner transition remain UNKNOWN
until target hardware gates. Runtime OS-signal/sampler integration is still open.

P5 boardd runtime shutdown follow-up: run_serve_until now owns the sampler,
shares the stop flag with listener admission, and joins sampling before invoking
the existing full-duty shutdown preparation and PWM quiescence. Listener failure
also goes through join and fan cleanup. The sampler rechecks stopping after the
state lock, interrupts its interval on join, and closes admission on failure or
panic. Sampling policy behavior and configured hardware lines are unchanged.

The production CLI now registers SIGTERM/SIGINT before hardware initialization
and waits for the blocking runtime's cleanup result through the existing locked
Tokio signal implementation. No dependency versions or new unsafe code were
introduced. Host evidence: 56 library tests passed, including cancellation of a
60-second sampling interval, pre-stopped admission, sampler error and panic;
the binary test fixture exercises real SIGTERM/SIGINT in isolated child test
processes, including propagation of a cleanup failure after SIGINT. The two
binary tests passed again after adding this failure case. These use a fake
serving closure, not target GPIO or the deployed
daemon. Clippy and whitespace checks passed. No installed services changed.

Remaining limits: startup partial-initialization cleanup, poisoned-state fan
recovery, blocked device I/O and mutex waits, and actual electrical/cooling
handoff remain unverified. The systemd stop timeout remains the external bound;
forced termination cannot count as clean composition shutdown. Full native
activation wiring/readiness/recovery and release/device gates remain open.
Longrun MCP is unavailable, so full scripts/check.sh and long release/image
builds remain unrun under the required contract.

P5 boardd failure cleanup follow-up: dropping an active PWM actuator now joins
its worker and attempts the existing asserted/full-duty failsafe, rather than
detaching a thread that retains the GPIO driver through Arc. Already-quiesced
actuators perform no extra destructor writes. Poisoned board state is recovered
only for a shutdown cleanup attempt; even successful cleanup returns an error.
This does not treat poisoned driver internals as safe to reuse.

PWM GPIO write errors are no longer silently ignored: the first failure latches
actuation uncertainty, makes one best-effort asserted write and exits the worker.
Later apply/quiesce/stop cannot report clean after that failure or a joined panic,
including after a successful final asserted write. Drop cleanup is a best-effort
startup/unwind fallback with diagnostics, not a replacement for explicit checked
shutdown. No hardware pins, polarity, timing or failsafe level changed.

Host evidence: 60 library tests and two binary tests passed, plus Clippy and
whitespace checks. New fixtures cover driver ownership release after active
actuator destruction, absence of extra writes after explicit quiescence,
poisoned-state cleanup success/failure, and latched PWM write failure across
worker exit and repeated shutdown. Full target startup-failure injection,
blocked I/O, fan health/readiness integration and electrical handoff remain open.

P5 fan runtime health follow-up: FanActuator now has a read-only check_runtime
hook for known asynchronous failures (the default remains synchronous apply-error
reporting, not electrical/RPM verification). SoftPwmFan rejects a latched worker
failure, poisoned driver, or absent/finished required PWM worker. Boardd status
overrides stale supervisor state with actuation_uncertain, and fan mode/duty
mutations fail before changes, including same-mode requests. Sampling checks
runtime in manual as well as auto mode; the production sampler exits on runtime
failure, closing admission and propagating its failure through joined shutdown.

Host checks passed: 61 boardd library tests, two binary tests, nine supervisor
tests, Clippy and whitespace checks. The new state regression verifies auto and
manual status/failure reporting without additional actuator commands. This is
known-runtime-fault reporting only; no measured fan duty/RPM, target failure
injection, or complete native update readiness gate is claimed. Full check.sh
remains blocked by unavailable Longrun MCP.

P5 systemd client diagnostics follow-up: the adapter now preserves the operation
and OS error for spawn/pipe-configuration/read/wait failures, distinguishes exit
code from terminating signal, and reports output-limit failure separately.
Previously all these paths collapsed to Controller, including the historical
unexplained stop fixture failure. The adapter still uses the closed command/unit
set, cleared environment, bounded capture, original deadline and kill/reap guard;
no retry or stderr capture was added.

Ten adapter tests passed, including missing executable, exact nonzero exit code,
SIGTERM-terminated fixture and oversized output classification. All 39 runtime
library tests then passed; Clippy and whitespace checks also passed. The earlier
intermittent stop failure did not reproduce. Its cause remains UNKNOWN and its
reliability item stays open: richer diagnostics and passing reruns are not proof
of a fix. These subprocess fixtures do not exercise target systemd services.

P5 native readiness identity fence: NativeSlotIdentity::check_readiness now
collects the full held-slot process observation, invokes read-only probes for
owner/video proxy/boardd/selected server with those exact identities and the
caller-supplied deadline, then requires an identical full observation. Probe
failure stops the sequence without retry. Expired/late results are rejected.
The controller and probe implementations must themselves bound blocking work
by the same or earlier deadline; this wrapper does not preempt arbitrary code.

The signed-slot candidate/current regression passed for matching observations,
the four-component probe order, exact deadline forwarding, first-probe failure,
process start-time replacement during probes, and expiry before probing.
Clippy and whitespace checks passed. Actual peer-bound IPC, authenticated API,
web-asset and video readiness probes are still required and are not implemented
by these fixture callbacks. Production native admission/confirmation remains
unwired; this fence alone is neither a health receipt nor floor advancement.

P5 peer-bound IPC transport: native_ipc::exchange_line uses a nonblocking Unix
socket, checks Linux SO_PEERCRED against the observed process PID before writing
request bytes, and shares one deadline across all partial writes/reads. Requests
and responses are limited to one 8-KiB line. Connect failure/full accept queue is
not retried; incomplete/oversized responses and late results are rejected. PID
matching must be combined with the existing before/after start-time/executable
identity fence. This transport alone does not validate component readiness.

Four real Unix-socket host fixtures passed: exact peer/request/response plus
wrong-peer zero-disclosure, oversized/truncated replies, trickled response total
deadline, and a full Linux accept queue returning immediately. Clippy and
whitespace checks passed. socket2 is pinned to the already locked 0.6.5; lockfile
review confirmed no version or transitive-package additions. One narrow Linux
getsockopt block validates its initialized ucred buffer and returned ABI length.
The initial Socket-to-UnixStream conversion build error was corrected using
safe OwnedFd conversion before the passing checks. Typed board/owner/proxy
response probes and production activation integration remain open; no target
hardware or services were accessed. Longrun/full-release gates remain blocked.

P5 typed board readiness probe: board_readiness::probe sends only the real
boardd Status request over the bounded peer-PID-checked transport. Its closed
typed response rejects protocol drift, nonzero/error responses, missing data,
unknown fan modes/statuses, runtime uncertainty, missing thermal observation,
and required peripheral unavailability. Fan line/PWM period and required
peripherals come from an explicit trusted-system BoardRequirements value, not
from candidate metadata or observed response. Reported duty is bounded to 100;
this is still not physical RPM/waveform evidence.

Both targeted tests passed: serializer-contract negative cases built from the
actual boardd::Response/StatusView, and a real Unix-socket exchange whose request
is parsed by boardd::Request and must be Status. Clippy and whitespace checks
passed. A dev-only boardd dependency prevents fixture wire-format drift; lockfile
review showed no package-version changes. No device GPIO or service was touched.
Trusted configuration provisioning, owner/proxy/API/web probes and production
native activation/readiness/recovery integration remain open.

P5 owner transport framing follow-up: owner control uses a four-byte big-endian
length prefix, unlike boardd's line protocol. native_ipc::exchange_control_frame
now reuses the peer-bound nonblocking connect/write path, the real control frame
encoder and its 4-KiB payload limit. Response length is checked before allocation;
prefix/body reads share the original deadline. Zero/oversized lengths, partial
prefixes and truncated bodies fail closed. Typed decoding and request/operation
correlation remain the responsibility of the forthcoming owner probe.

All six transport tests passed, including real control encoder/decoder framing,
invalid prefix/body cases and a trickled body that cannot renew the deadline.
The existing boardd-line/peer/full-queue tests also passed after factoring the
common connection path. Clippy and whitespace checks passed. Lockfile review
confirmed only the local control-protocol dependency edge, without version
changes. Owner readiness semantics and production composition integration remain
open; transport success is not owner health or release acceptance.

P5 typed owner readback probes: owner_readiness::probe_readbacks now uses only
HidOnlyState, VideoSettingsState and BoardStatus V2 requests over the peer-bound
framed transport. Each request has a fresh OS-random operation ID. The real
protocol decoder validates schema and structure; exact request/operation IDs and
response variants are required. Rejected/Applied or mismatched replies stop the
sequence without replay. Desired video values use the core settings validator
and require explicit resolution. Required ATX LED inputs use the trusted board
requirements; no HID, video-setting or ATX mutation is sent.

The real Unix-socket/protocol regression passed all nine fixture cases: valid
three-request sequence, wrong version/request ID/operation ID, rejection, generic
Applied, invalid FPS, missing resolution and missing required power LED input.
Clippy and whitespace checks passed. Lockfile review confirmed only a local core
dependency edge, no version changes. This proves readback-path compatibility,
not HID/gadget reconciliation, measured capture health, authenticated web/API,
or complete owner readiness. These additional gates and production activation
integration remain open; no target services or hardware were accessed.

P5 video proxy shutdown follow-up: the proxy now registers SIGTERM/SIGINT,
closes admission even while waiting for a connection permit, tracks connection
tasks in JoinSet, and cancels/joins them under a two-second drain bound. A task
panic or drain failure is not reported as clean. The proxy is read-only, so
cancellation closes downstream/upstream sockets without a device mutation or
HID release ledger. The fixed /mjpeg-to-/stream route remains unchanged.

All ten library tests passed, including saturated single-permit shutdown with
a silent upstream and observed EOF on both peers. A separate integration test
ran the actual proxy executable with cleared environment/temp sockets, sent
SIGTERM and SIGINT with an unfinished request, and required successful exit,
socket removal and client closure. Clippy and whitespace checks passed. Only
the existing Tokio signal feature was enabled; no installed service changed.
Socket cleanup ownership/error reporting, target systemd stop observations and
native proxy readiness remain open. Future HTTP probes must not half-close
their request socket: the proxy intentionally treats downstream EOF as cancel.

P5 video proxy socket cleanup: the cleanup guard records the bound pathname's
device/inode and requires the same socket type/identity before unlinking. Normal
shutdown consumes the guard through an explicit checked finish; failure prevents
a successful process exit. Early-error/abort cleanup remains best-effort with a
diagnostic. Replacement regular files, sockets and symlinks are preserved; a
missing pathname is an error. This relies on the reviewed non-group/world-writable
runtime directory and does not claim protection against a privileged rename race.

All eleven library tests passed, including original inode preservation across
four replacement/missing cases. The actual-binary signal regression passed both
normal signals, then passed an added SIGTERM case requiring nonzero exit and
unchanged replacement-file contents. The expected child diagnostic was
"video proxy socket identity changed". Clippy and whitespace checks passed.
Target systemd observations, proxy stream readiness and complete native update
activation/recovery remain open. No installed services or hardware changed.

P5 proxy first-part transport probe: proxy_readiness now sends the fixed /mjpeg
request to the identity-bound Unix peer without half-closing the request side.
It requires HTTP 200, bounded/unique headers, a matching multipart boundary,
image/jpeg part type, an explicit bounded length (4 bytes through 4 MiB), complete
payload and JPEG SOI/EOI delimiters within the original deadline. It rejects
encoded/chunked or whole-response Content-Length framing until supported and
tested. This is only first-part transport evidence: the four-byte synthetic
fixture is deliberately not a decodable image, and no decode/FPS/capture claim
is made. Video health and target-format compatibility gates remain open.

Both targeted proxy tests passed: six header/length/truncation cases and the
actual proxy implementation against a fixture upstream, including request-half
preservation and upstream EOF after probe close. Clippy and whitespace passed;
dependency review showed only existing locked httparse/Tokio and local proxy
edges, without version changes. The broader runtime run was 49 passed/1 failed
in running_observations_are_bounded_and_invalid_deadlines_rejected. Its assertion
was improved to print the actual error, then all 50 passed on rerun. The original
failure had no concrete error detail and remains unexplained; the passing rerun
does not resolve systemd-fixture reliability. Full scripts/check.sh is still
blocked by unavailable Longrun MCP. Production activation remains unwired.

P5 HID reconciliation observation: V2 now has a read-only
HidReconciliationState request/response carrying the pending-release count from
the attached HID transition policy. Disabled owners or absent HID policies reject
the request; no lease is acquired and no release/actuation is triggered. The
owner readback sequence requires this fourth response to report zero outstanding
obligations. This is ledger observation only, not proof that HID endpoints are
present or physically functioning. Server HID/ATX mutation handlers explicitly
treat this response as an unexpected/internal result, never mutation success.

The gadget-profile broker regression passed with empty and pending persisted
ledgers and absent device paths, preserving ledger bytes. Owner probe regression
passed ten cases including pending releases rejection. All 16 control-protocol
tests passed, server cargo check passed, and broker gadget-profile/runtime Clippy
and whitespace checks passed. The new exhaustive response cases uncovered by
compilation were handled explicitly before these passing checks. No device
operations or installed-service changes occurred.

Before this change, four bounded runs of the existing 50-test runtime binary
all passed (roughly three seconds each). Neither intermittent systemd failure
reproduced; their cause remains UNKNOWN and the reliability item stays open.
Full native health/activation/recovery, endpoint evidence and release gates are
still outstanding; unavailable Longrun still blocks the full scripts/check.sh.

P5 web artifact binding: NativeSlotIdentity::verify_web_asset now validates an
HTTP-decoded body against an exact share/web/ file entry in the already verified
signed slot inventory, requiring the signed size, web-file mode and SHA-256.
Unknown paths, path aliases/traversal forms, non-web executables, old same-size
HTML, truncated bodies and appended bytes fail. The check uses the held manifest
and does not read an untrusted response-selected filesystem path or mutate state.

The signed-slot web-body regression, Clippy and whitespace checks passed. The
initial digest LowerHex formatting build error was corrected with explicit
per-byte lowercase hex before the passing checks. This is content binding only:
bounded HTTP fetching/decoding, selected server identity, authenticated API
readiness and complete bootstrap asset coverage still require integration.
Generic HTTP 200 or successful index hash alone must not confirm an update.
Physical backend/video gates, production activation/recovery and full release
acceptance remain open; full scripts/check.sh still requires unavailable Longrun.

P5 signed web HTTP probe: web_probe::verify_asset now fetches an exact manifest
web path over explicit IPv4 loopback TCP (unprivileged port), percent-encodes path
bytes, requests identity encoding and requires HTTP 200. Signed size bounds body
collection with an additional 8-MiB per-asset ceiling; fixed/chunked framing is
handled by existing Hyper. Encoded/ranged responses, trailers, redirects,
oversized/truncated bodies and hash mismatches fail. Connect, handshake, headers
and body share one deadline. HTTP operation/connection futures are driven together
without a detached task; cancellation drops the socket. Post-hash expiry is checked.

Four web-focused tests passed: fixed/chunked and invalid HTTP responses, stalled
body timeout with peer-observed EOF, signed-body validation, and real loopback
fetch accepting candidate HTML while rejecting an old equal-length body. Clippy
and whitespace checks passed. A missing test Duration qualification was corrected
before these results. Dependency review confirmed existing locked HTTP/Tokio
packages only, with no version changes. No target service or credential was used.
TCP endpoint-to-process binding, authenticated API readiness, complete bootstrap
asset enumeration and production activation/recovery remain open. This helper
alone cannot confirm an update; full scripts/check.sh remains Longrun-blocked.

P5 complete signed web inventory delivery: web_probe::verify_tree now verifies
the root route against signed index.html and fetches every share/web/ manifest
entry, including JS/CSS and lazy-loaded assets. Entries are preflighted for mode
and the existing per-asset size ceiling. All requests retain the same original
deadline, with only one body allocated at a time. The returned observation counts
manifest assets/bytes (excluding the extra root-route fetch). Unknown single-asset
paths are rejected before encoded-route allocation.

A signed multi-asset loopback fixture checks the exact route set and rejects
equal-length corrupt bodies independently at root, CSS, JS and index.html. Five
web-focused tests passed; the subsequent full runtime library suite passed 55/55,
with all-target Clippy and whitespace checks also passing. No dependencies or
target services were changed. Prior intermittent systemd fixture failures did not
recur here; their cause remains unresolved. This adds full inventory byte delivery
coverage, not MIME/browser execution, TCP process binding, authenticated API or
production activation integration. Those gates remain open, as do physical/device
acceptance and the Longrun-blocked full scripts/check.sh.

P5 native startup foundation: NativeSlotIdentity::start_and_observe requires all
seven closed composition units clean-stopped before dispatch. It starts owner,
video proxy, boardd and the selected candidate/current server in that order,
checking each process against its held signed executable before dispatching the
next component. A final composition observation rejects extra servers. Failure
stops further dispatch and leaves recovery to the transaction coordinator; this
method neither confirms health nor acknowledges rollback. Controller calls must
use the same deadline or an earlier one. Trusted effective unit dependencies are
still a system prerequisite, not proven by this host helper.

Systemd start accepts a typed native-only unit set (no bootstrap/board server or
arbitrary unit strings), with existing bounded client execution and diagnostics.
Timeout can leave a queued job and does not authorize another owner start.
Two focused regressions passed for candidate/current ordering, all-seven stop
preflight, signed file mapping, failure at every dispatch/identity stage, expired
budget, exact systemctl argv and failed start exit status. All-target Clippy and
whitespace checks passed. No installed units were started/stopped. Source review
again confirms the production activation worker still lacks native execution
wiring/authority and has a 15-second timeout; production admission, readiness,
recovery and effective slot-root units remain open. Full scripts/check.sh remains
blocked by the unavailable Longrun tools.

P5 startup authority correction: start_and_observe no longer relies on the caller
alone to establish durable selection. Before service observations/dispatch, it
requires manager status (which verifies referenced slots and the active pointer)
to select the held slot. Candidate startup requires matching AwaitingHealth;
current startup requires the confirmed slot with no pending transition. Prepared,
uninitialized state, wrong selection and wrong server role fail closed. Pending
candidate startup also requires the current kernel boot ID and a caller deadline
no later than the remaining persisted monotonic health budget. An expired caller
deadline is rejected before status validation. Manager locks remain held.

The signed-slot startup regression now exercises these failures without any
service calls, including current startup during Prepared, renewed candidate
budget and a persisted operation from another boot. Candidate/current successful
ordering and every dispatch/identity failure remain covered. Both native-start
tests, all-target Clippy and whitespace checks passed. Production coordinator,
worker authority/units, readiness and rollback wiring remain open; no target
operation or release acceptance was performed. Full scripts/check.sh is still
blocked by unavailable Longrun tools.

P5 update endpoint profile packaging: both prototype and board bundle recipes now
stage the same 94-application-update.conf for board-server, candidate and current
roles via one shared staging script. Previously the prototype omitted this file
entirely and the board recipe staged only board-server. The profile contains the
three fixed status/upload/activation socket endpoints and explicit write opt-in.
Update-runtime package source revision is now dev10, with all three drop-ins
mapped to inert documentation examples. Dev1-dev9 maps exclude these additions;
historical receipts and pinned device rehearsal profiles remain unchanged.

The real staging-script regression checks all three byte-identical 0644 outputs,
socket paths against socket unit ListenStream values, explicit write opt-in,
inert package destinations and legacy-map exclusion. Fifteen packaging/archive
tests and 51 related package staging/upgrade/service/installed-update regressions
passed. Basedpyright on the changed Python files reported zero errors/warnings;
Bash syntax and whitespace checks passed. ShellCheck is unavailable. No complete
bundle/deb was built (Longrun unavailable), no target configuration was installed,
and dev10 is not a released or device-accepted artifact. System provisioning must
still apply the shared opt-in consistently and bind it to the release/system
sequence; production native activation and effective unit inspection remain open.

P5 native start/legacy hook conflict: source inspection confirmed candidate
ExecStartPost invokes the generic health reporter. Native coordinated startup
holds manager locks and must perform full composition health itself, so executing
this hook can contend on those locks and uses the wrong confirmation contract.
SystemdController now checks a closed effective startup property set before start:
exact Id, loaded simple service and empty ExecCondition/ExecStartPre/ExecStartPost.
Missing, duplicated, extra, malformed or incompatible properties prevent dispatch
with IncompatibleStartup. Existing bounded capture/deadline rules apply. The
legacy candidate template is intentionally rejected by this native-only method;
production provisioning and coordinator-owned confirmation are still required.

Three native-start regressions passed, including exact argv, signed startup state
and eleven incompatible effective-contract cases with no start dispatch. Clippy
and whitespace checks passed. The existing legacy activation path was not changed
or certified; no target unit was operated. This guard does not establish complete
dependency safety, unit-file immutability against privileged changes, endpoint
binding or native recovery. Full scripts/check.sh remains Longrun-blocked.

P2 source recheck: Tailscale capability is already read_only in server manifest,
native.tsx renders observation only, and network_contract.mjs requires the sole
GET /api/extensions/tailscale/status on opening the page. No new capability
defect was established there; native Tailscale mutations remain unimplemented.

P5 supervisor candidate pipeline: activate_native_candidate now combines the
existing durable admission callback, quiescence, selected native startup and
identity-fenced readiness under one held manager lifetime. The caller deadline
cannot exceed the persisted remaining health budget before receipt delivery;
quiescence checks it before and after service operations. Startup and probes use
that same deadline. Identity must remain equal from completed startup through
readiness. Composition failures retain their diagnostic category. The result is
still AwaitingHealth: this method neither confirms nor advances rollback floor.
Failures retain the existing durable challenge/recovery obligations; caller must
coordinate stop and restoration before acknowledging recovery.

A signed-slot regression passed all eight scenarios: successful phase ordering,
receipt failure, quiescence failure, startup failure, probe failure, stale
generation, renewed budget and process replacement between startup/readiness.
It checks no service operations for rejected delivery/generation/budget, four
readiness probes on success, unchanged confirmed slot/floor, and AwaitingHealth
intent retained after startup/probe failure. All-target Clippy and whitespace
checks passed. This is a host controller/probe fixture, not effective target
systemd or hardware evidence. Production socket wiring, real complete probes,
authority/templates, coordinator confirmation and coherent rollback remain open.
Full scripts/check.sh is still blocked by unavailable Longrun tools.

P5 early composition compatibility gate: NativeStartController now requires a
read-only validate_start operation, implemented by the bounded effective-unit
query in SystemdController. The candidate supervisor pipeline validates owner,
proxy, boardd and candidate server before challenge creation/admission/quiescence.
Generation/target validation precedes even these read-only queries, and all
queries retain the caller deadline. The systemd adapter repeats validation just
before each start, rather than treating preflight as a permanent authorization.

The candidate pipeline regression now has nine scenarios. Incompatible candidate
startup (the last unit checked) leaves generation, pending state and challenge
unchanged and performs no admission/stop/start/probe. Stale generation performs
no preflight calls either; delivery requires all four preflights completed.
Thirteen native-focused runtime tests, all-target Clippy and whitespace checks
passed. No effective target unit was queried or modified. Production socket,
authority/slot-root provisioning, complete real readiness, confirmation and
coherent rollback remain open; full scripts/check.sh remains Longrun-blocked.

P5 native recovery source audit: reject_health/recover call rollback_document,
which restores the pointer and clears pending before any native service readiness.
watchdog_once runs manager recovery before service management, and confirmed-apply
is still server-only. These APIs cannot be directly composed into coherent native
rollback without new durable restoration state and native boot/watchdog routing.
docs/design/native-application-recovery.md now maps required ordering, proposed
rollback/restoration phases, compatibility prerequisites and crash-point tests to
the existing APIs and missing deliverables. It is explicitly not implemented.

A bounded text-only Gemini Flash review completed within its 20-second limit and
reinforced stop fencing, pre-teardown durable intent and full restoration gating.
Its suggestion of an atomic readiness barrier is treated as a bounded identity-
fenced observation, not a guarantee against later process failure. No project
files or secrets were supplied in that prompt. No target mutations, journal schema
changes or acceptance receipts were produced. Existing host tests remain prior
evidence; no new runtime test was needed for this design-only change. Native
recovery and full Longrun-blocked checks remain open.

P5 read-only recovery inspection: manager::inspect_recovery now validates the
journal and independently verifies the confirmed slot proof/system requirement,
without requiring a damaged candidate or retired slot to validate. It reports
generation, confirmed slot, pending metadata and pointer classification:
Confirmed/PendingTarget/Other/Missing/Unsafe. PendingTarget is only a journal/name
match, not validation of that candidate. Non-pointer I/O failures remain errors.
The API repairs no pointer, writes no journal and grants no startup/recovery
authority; rollback/restoration phases and coordinator integration remain open.

Regressions cover uninitialized/clean/Prepared/AwaitingHealth, an invalid candidate,
all five pointer classifications, repeated inspection with byte-identical journal
and unchanged pointer, invalid confirmed anchor, incompatible system sequence and
unsafe journal permissions. All 20 manager library tests passed, with all-target
Clippy and whitespace checks passing. No journal schema or target configuration
changed. Full scripts/check.sh and release/device acceptance remain unrun under
the existing Longrun/operator/hardware constraints.

P5 durable native rejection intent: begin_native_rejection now verifies exact
generation/operation and the rollback anchor, then atomically stores a schema-2
nativeRollback marker before any caller teardown. The marker binds operation,
intent generation and ExplicitHealthFailure. Pointer, floor and original pending
phase remain unchanged. Schema-1 serialization omits the new optional field;
schema-1 plus native intent, schema-2 without intent and mismatched/future intent
metadata are rejected. Legacy load_required entrypoints refuse schema 2, preventing
recover/reject/confirm from bypassing native restoration. inspect_recovery exposes
the persisted marker and remains usable with a corrupt candidate.

All 21 manager tests passed, including stale/wrong-operation rejection without
write, persisted intent across reopen, schema validation, unchanged pointer/floor,
and blocked legacy mutations. Manager all-target Clippy, runtime cargo check and
whitespace checks passed. No production caller uses this API yet, and no deployed
journal was migrated. Restoring state, native continuation/acknowledgement,
persistence-fault crash gates and release/system compatibility remain required;
schema-2 intent must not be deployed as a standalone recovery implementation.
Full scripts/check.sh remains blocked by unavailable Longrun tools.

### P5 native selection restoration verification (2026-09-23)

Resumed verification of the previously untested `restore_native_selection` patch.
The manager requires exact persisted intent and generation, verifies the confirmed
anchor before and after the trusted quiescence callback, restores the pointer and
stores `NativeRollbackPhase::Restoring`. Pending activation/history and rollback
floor remain unchanged. Re-entry repeats quiescence rather than trusting a prior
stop observation.

The focused regression passed for Prepared and AwaitingHealth origins: wrong
intent/generation, quiescence failure, anchor corruption during quiescence,
reopen after pointer replacement before journal persistence, stale replay, and
reopen/re-entry after Restoring. Legacy recovery continues to reject schema 2.

Verification: `cargo fmt -p bli-kvm-update-manager`; focused restoration test
(1 passed); complete manager library suite (22 passed); manager all-target
Clippy with `-D warnings`; update-runtime `cargo check`; `git diff --check`.
Rust checks used offline dependencies and the existing D: host target directory.

This is host selection-restoration evidence, not service recovery acceptance.
Restored-role startup authority, full composition readiness, acknowledgement,
persistence-fault coverage and native production boot/watchdog/apply routing
remain open. No target services or deployed journals were changed. Full workspace
checks/builds still require unavailable Longrun tooling; hardware gates remain
unrun.

### P5 restored composition startup and coordinator (2026-09-23)

`NativeSlotIdentity::start_restored_and_observe` now requires the exact journal
generation and rollback intent, Restoring phase, verified confirmed selection,
and a finite recovery attempt of at most 45 seconds. It reuses the seven-unit
clean-stop gate and ordered owner/proxy/boardd/current startup with signed
executable identity checks. Ordinary Current startup still rejects pending state.

`UpdateSupervisor::restore_native_composition` integrates exact recovery fencing,
full confirmed inventory verification and four-unit read-only preflight before
quiescence, selection restoration, startup and identity-fenced readiness callbacks.
It compares startup/readiness identities and the final journal inspection. Every
failure retains recovery obligations; even successful probes retain pending intent
until a future durable acknowledgement step. There is no production wiring yet.

Startup regressions cover premature phase, stale generation/intent, expired or
excessive deadline, wrong slot, unclean units and component identity failures.
Coordinator fixtures cover success, stale fence, incompatible preflight, stop,
start, probe and identity-replacement failures, and excessive recovery budget.
The initial test fixture tried staging while manager locks were held and correctly
failed StagerBusy; staging now occurs before acquiring the manager.

Verification: formatting, focused startup and pipeline regressions, all-target
runtime Clippy with `-D warnings`, and final runtime library suite 59/59 passed.
One earlier full-suite run failed the expected error assertion in the existing
systemd startup-hook fixture (58/59). An assertion now includes actual error and
fixture input; subsequent suites passed, but the original intermittent cause is
unknown and is not claimed fixed. No target services were changed.

Next required work: durable readiness-gated completion and crash-safe cleanup,
native boot/watchdog/apply/activation authority, real authenticated health probes,
compatibility provisioning and release-bound device evidence.

## Host typing gate follow-up

### Renewed aggregate host checks (2026-09-23)

Full check job `3cde34229a5f4e9f9f44f8c7152f20e8` exited 1 after 478.464s at
the source-boundary gate. The preceding Rust compile/test/Clippy matrix and HTTPS
health gate completed; subsequent gates in that run did not execute. The scanner
flagged the normative RFC1918 range definitions in firewall_rules.rs and
firewall_observation.rs as private device addresses. The fix permits only those
two complete exact lines at those exact source paths; neither file is exempted.
Four new scanner regressions pass (including modified prefixes, another path,
and extra addresses beside/on another line); Basedpyright reports zero diagnostics.
The regression module is included in check.sh. A fresh aggregate run is required
before claiming full success.

P4 provenance follow-up inspected the two previously unrecorded public uStreamer
branches at fixed commits: shm `ccf713dc1cbee167c5eccbc3396febcb2e13f876` and
semwait `1b2de094380a95cda7556b8d2c7e735b4b57ab8d`. Their encoder dispatch/header
does not expose LIBX264-VIDEO. Evidence and search limits are recorded in
h616-h264-source-contract.md; reproducible vendor software-encoder source remains
unresolved. No downloaded code or vendor executable was run.

P6 read-only coverage audit: `tools/build_system_image_test_fixture.py` generates
a 4096-byte pseudo-rootfs and 1024-byte pseudo-boot payload, inserting only ext4
and boot magic bytes into deterministic byte patterns and signing with an
ephemeral fixture key. These artifacts exercise verifier contracts; they are not
mountable production filesystems or proof of an H616 boot image. The existing
`bli-kvm-system-layout` validator requires both RootA and RootB with equal sizes
plus State, and limits MBR profiles to four primary partition numbers. Therefore
the requested preliminary 12 GiB system / 14 GiB data-staging / remaining ISO
geometry cannot be claimed from that A/B validator without explicitly accounting
for both roots, state, boot components and actual bootloader layout. Its encoded
reserved offsets (16/256 sectors, GPT restriction) are source assumptions, not
fresh hardware verification. Separate-media boot geometry, peak staging usage,
same-build RAW writer/read-back and data-preservation acceptance remain open.

Read-only production dispatch review identified a concrete native boot integration
trap: packaged current-server Requires/After update-recover, so a coordinator
running inside that recover unit cannot synchronously start current-server without
waiting on its own job. Existing CLI activation/watchdog/confirmation dispatch
remains legacy; recover's 15s, AF_UNIX-only, unprivileged and proc-restricted
service contract also does not match complete native readiness requirements.
The exact paths and constraints are recorded in native-application-recovery.md.
No unit or runtime source was changed during the pending aggregate check.

Fresh-dist GUI parity regression `1fa9caea6f2049bcb58ca251ccf5dd91` passed:
one real-app Chromium test, zero failures/skips, exit 0 in 5.025 seconds. This
covers desktop rendering and removed-endpoint avoidance with local mock APIs,
not release-bound desktop/mobile/device acceptance. Full `scripts/check.sh` job
`3cde34229a5f4e9f9f44f8c7152f20e8` was submitted with CARGO_NET_OFFLINE=true
and CARGO_TARGET_DIR=/mnt/d/debug/blikvm/hardened-blikvm/target-host. Its terminal
result remains outstanding; no duplicate full-check job has been submitted.

Restricted-control server job `8454b32097684258acf2be07217a0a88` completed
with exit 0 in 54.065 seconds: 147 library tests passed, two remained ignored,
and all three real signal tests passed. Fresh-dist GUI parity job
`1fa9caea6f2049bcb58ca251ccf5dd91` was submitted; result not yet inspected.

Fresh production web build job `d4988b0b6b1f4c898dbbd6ca77148bdc` completed
with exit 0 in 10.027 seconds (`corepack pnpm --dir web build`: TypeScript and
Vite). Browser regressions can now use the current local dist rather than the
September 20 build. This is not a reproducibility measurement or release/device
acceptance. Vite reported outdated Browserslist data and large chunks; neither
caused build failure. Restricted-control server test job
`8454b32097684258acf2be07217a0a88` has been submitted; result not yet inspected.

The user renewed permission for runtime tests, restricted-control server tests
and the fresh web build. Runtime job `7c0b525ea0f945b49f1069b4f044d4ea` passed
all 62 library tests, including the completed-startup scenarios, exit 0 in
27.042 seconds. This supersedes their previously unexecuted status below.
Fresh web build job `d4988b0b6b1f4c898dbbd6ca77148bdc` was submitted; its
terminal result has not yet been inspected. Next-admission advisory review job
`6ea643a6105e4c70995ee3ca2a39ca8d` exited 0; its historical-completion proposal
still needs explicit native idle/confirmation semantics and is not implemented.

Journal invariant correction: with a pending activation, validation previously
required exactly one matching Pending history record but did not reject an
additional Pending record for another operation. It now requires exactly one
Pending record globally. Regression coverage exercises Prepared, AwaitingHealth,
native RollbackPending and Restoring, rejecting an orphan while preserving valid
terminal history. All 27 manager tests and manager all-target Clippy passed;
diff checks passed. This closes a validation gap, not next-operation handoff.

Manager `begin_native_recovery` now persists generation-fenced intent for
interrupted preparation, changed boot, health timeout, pointer mismatch and
invalid target without changing selection or service state. Existing intent
retains its original reason; live candidates are left untouched and reversed
same-boot clocks rejected. All 26 manager tests and manager/runtime all-target
Clippy passed. HealthChallengeLost and production watchdog routing remain open.
The new completed-startup runtime test submission was denied Longrun permission;
those added scenarios remain unexecuted, and the prior 62-pass result does not
cover them. No alternate executor was used for that denied suite.

Completed-state startup implementation added: an exact completed receipt and
generation gate precedes full native quiescence/startup and fresh readiness,
without treating historical completion as present health. Runtime all-target
Clippy and diff checks pass. Added host success/stale/budget/preflight/stop/start/
probe/replacement scenarios are pending execution. Packaged boot routing and
next-operation handoff remain outstanding.

The host `restore_native_composition` coordinator now executes identity-fenced
readiness inside manager acknowledgement, then invokes exact receipt/generation
challenge cleanup. A cleanup error retains durable completion for cleanup-only
replay. Legacy apply requests are rejected and preserved. Coordinator fixtures
now use actual issued challenges and verify stale/substituted receipts, blocked
cleanup, reopen with state-only evidence and idempotent cleanup without service
events. All 62 runtime library tests and runtime all-target Clippy passed.
Production boot/watchdog/apply, completed-state startup/next-operation handoff,
persistence fault injection and device acceptance remain open.

Manager native acknowledgement now atomically transitions Restoring to a
schema-2 completion receipt only after a fresh trusted readiness callback and
pre/post exact journal, intent, anchor and selection checks. Receipt/history
validation retains operation/intent/completion generations, reason, failed target,
confirmed slot and health binding; pending is cleared without floor advancement.
Legacy mutation remains blocked. All 24 manager tests and manager/runtime
all-target Clippy passed; runtime all-target compilation passed. Gemini review
job `83e67b916b9d4dfeb903267d1bdc3276` exited 0 and supplied advisory design
feedback, not implementation or acceptance. Coordinator completion/cleanup,
next-operation handoff and production boot/watchdog/apply wiring remain open.

Native recovery cleanup foundation: `HealthChallengeStore::consume_exact`
validates operation/target/health-binding before removing surviving challenge
files, supports state-only/token-only/empty replay, and preserves mismatched or
malformed evidence. The shared document parser keeps the existing response
verification contract. Eight challenge tests and all 62 runtime library tests
passed; runtime all-target Clippy passed. This is a cleanup mechanism only:
durable completion authority and native production routing remain outstanding.

All-features check job `4b87fab7c5a046e1a02830540219f925` failed with E0004
in the restricted-control media endpoint: `HidReconciliationState` was missing
from its exhaustive response match. The response now explicitly maps to the
existing `applied: false, rejection: invalid_state` outcome. After this fix,
`cargo check --workspace --all-targets --all-features --locked --offline
--target-dir /mnt/d/debug/blikvm/hardened-blikvm/target-host` passed in 9.73s.
This closes compilation coverage only; feature-profile runtime tests and the
remaining full check.sh gates still require evidence.

Workspace job `dd39edaf3b944b3ba15d1e089f9d5350` completed successfully:
`cargo test --workspace --all-targets --locked --offline --target-dir
/mnt/d/debug/blikvm/hardened-blikvm/target-host`, exit 0, 84.094 seconds.
This verifies the default workspace matrix after the server capability assertion,
diagnostic fixture budget and systemd fixture execution corrections. Ignored
gated tests remain ignored; all-features/profile checks and full check.sh are
still outstanding. A proposed fresh `corepack pnpm --dir web build` Longrun
submission was denied permission, so it was not executed or retried through
another tool. Existing browser evidence remains tied to the prior local web/dist.

Full Python discovery job `02d3d98e33ef461aaa24c3616775b9db` initially failed
3 of 1189 tests. Updated two current route-count statements from 72 to 74 to
include the firewall mode/confirmation routes; no historical device receipt was
changed. The cutover renderer test now validates all three fixed loopback
destinations and upload streaming bounds in both installed and validation
configurations. Focused tests and Basedpyright pass. Full Python discovery then
passed **1189/1189** in 16.677 seconds. Output describing boots/devices is mocked
fixture output, not newly performed device operations.

Workspace job `8ee26059f8284893a1534252a614d161` failed after 71.080 seconds
at the runtime startup-hook fixture. Added diagnostics identified the actual
error as spawn `ETXTBSY` (Text file busy), rather than parser rejection or timeout.
Generated shell fixtures now run through `/bin/sh` only under cfg(test), avoiding
exec of the newly written script inode. Production still executes systemctl
directly with no retry; the missing-executable regression retains direct exec.
All 12 composition-systemd tests and runtime all-target Clippy pass. Full workspace
verification must be repeated before this gate can be accepted.

The remaining Node test block initially passed 21/22; real-app GUI parity still
called the schema-2 probe without role/evidence-context/source-revision arguments
and attempted to read a missing receipt before checking process failure. Updated
that test with explicit synthetic local-fixture context, exit-first diagnostics,
schema/result/role assertions and mandatory complete settings capture. Removed
the old best-effort empty-settings allowance. The fixture does not advertise
leader-key support, so no legacy shortcut request is expected. The About label
now matches the actual English locale (`About BliKVM`). The corrected test passes
against local web/dist with Chromium (no skip); Prettier/diff checks pass, and its
format check is included in check.sh. This synthetic context is not device or
release-bound acceptance evidence. The other 21 Node tests passed in the initial
run; this checkpoint does not claim a fresh production web build.

The remaining 26 modules in the first application/persistent-write unittest block
initially reported two fixture errors: the legacy `upload_proxy_streaming`
transform tests used the current packaged Nginx configuration, which already
contains upload streaming and is intentionally rejected by the migration.
Those transform tests now use an explicit representative legacy single-location
fixture, and a new test requires rejection of the current packaged configuration.
Production migration guards remain unchanged. The block now passes 130 tests;
Basedpyright for `test_upload_proxy_streaming.py` and diff checks also pass.

Fresh Basedpyright coverage of both comparator and its tests exposed 61 diagnostics
in `tools/test_compare_gui_parity.py`, primarily propagated unknown dictionary
types. Fixture/write helpers now use `dict[str, object]`, malformed inputs use
`list[object]`, nested-invalid coverage is constructed explicitly, and the invalid
tag case exercises public `compare` rather than a private helper. Removed the
unnecessary type-ignore from that fixture. The test file is now included beside
the comparator in the `scripts/check.sh` Basedpyright gate. Both files pass with
zero errors/warnings. The comparator, acceptance-report/manifest and system-image
fixture suites pass together (30 tests); no acceptance semantics were weakened.

Native OpenCode Longrun health now confirms this exact repository in allowed
roots with OpenCode automatic continuation enabled. The earlier unavailable
Longrun blocker is historical; the native adapter does not require a Codex bridge.
Workspace/all-target tests were submitted once as job
`3c83a57eda4f4bf786444998be5aad84` with locked offline dependencies and target
directory `/mnt/d/debug/blikvm/hardened-blikvm/target-host`. Its terminal result
has not yet been consumed; this entry does not mark that gate passed.

A subsequent Longrun readiness check reported receipt
`aa2d3085-e9cc-49a1-8697-89981354a850` as `revoked`. This refers to continuation
delivery, not proof that the process stopped or failed. The job has not been
resubmitted; its terminal result still requires recovery through the existing
job ID. Automatic delivery must not be assumed from the initial submission.

The existing job result was subsequently retrieved once: exit 101 after 124.175
seconds. The server library suite stopped at 121 passed / 1 failed / 2 ignored:
`network_writes_require_authentication_and_the_explicit_opt_in` expected all
network controls unavailable in default builds. Source review confirms native
DNS/IPv6 read-only observations are intentionally exposed, while WoL remains
unavailable without the restricted mutation surface. The assertion now separates
those contracts and checks authenticated GET success for both read-only routes.
Formatting and diff checks pass; the amended test awaits runtime verification.
The workspace gate is failed, not accepted, until the corrected run succeeds.

Workspace rerun `e235bb12593940e8babe0df735260a6d` exited 101 after 25.051
seconds, before reaching the server suite: diagnostic-producer-client had
17 passed / 2 failed. Real Unix-socket framing/reuse fixtures received
ResponseTimeout with their 30 ms test-only deadline instead of Accepted or
InvalidResponse. Their client helper now uses the existing 500 ms test wait
budget; dedicated pending-connect/stalled-write/stalled-response tests retain
the 30 ms limit. Production defaults and transport behavior were not changed.
The diagnostic suite passes 19/19, its all-target Clippy passes, and workspace
format/diff checks pass. The amended server network capability regression was
also run separately in the default build and passed. Whole-workspace acceptance
still requires a successful rerun; the precise scheduling cause of the socket
timeouts was not instrumented.

The first 21 application-write/persistent-write/reclaim unittest modules from
the check script passed together (130 tests). This does not cover the remaining
receipt, upload, GUI and retained-reboot modules in that block.

Independent checks completed successfully on the current worktree:
`cargo fmt --all -- --check`, `corepack pnpm --dir web lint`, and
`corepack pnpm --dir web exec prettier --check 'src/**/*.{js,jsx,ts,tsx,json,css,scss,md}'`.
These cover formatting and frontend lint only, not the full `scripts/check.sh`,
release reproducibility or device acceptance.

Additional independent gates passed: `scripts/verify-systemd-units.sh` and
`node --test tools/test_network_outcome.mjs tools/test_hid_report_queue.mjs tools/test_gui_parity_probe.mjs`
(8 tests). The systemd script uses a disposable synthetic root and offline
security analysis, not host or target account/service installation. It stages
top-level unit files but not role drop-ins, and supplies synthetic legacy kvmd
units; success does not prove native installed dependency compatibility. The
Node suite covers DNS/IPv6 applied/read-back rejection, HID queue ordering and
bounded local browser menu extraction, not physical HID or device parity.

The combined Debian runtime/updater, prototype archive, board-profile and
packaging-boundary suite initially passed 81/82. Its Nginx boundary assertion
still expected a single `proxy_pass` declaration, predating the two exact upload
locations. The test now requires exactly the default/application/media locations,
all three destinations equal to the fixed loopback backend, explicit 128m/4097m
upload bounds and HTTP/1.1 streaming without request/response buffering. The
default 16m bound remains asserted. The combined suite now passes 82/82 and
`git diff --check` passes. This updates contract coverage; it does not replace the
real Nginx streaming/413 gate or establish release reproducibility.

`basedpyright tools/test_packaging_boundaries.py` also passes with zero
errors/warnings. Both `tools/verify_persistent_status_ordering.py` and
`tools/verify_persistent_write_ordering.py` pass: their valid synthetic graphs
are accepted and injected ordering cycles are rejected. These are offline
ordering checks; they perform no boot or device action and do not validate the
not-yet-provisioned native recovery topology.

The previously reported comparator typing defect is corrected: receipt inputs
use explicit object types, runtime-validated type guards and bounded typed
collections rather than unparameterized dictionaries. No diagnostics were
suppressed and acceptance checks were retained. Malformed top-level JSON and
non-string role/result values now raise controlled validation errors rather
than AttributeError/TypeError. Basedpyright reports zero errors/warnings for
`compare_gui_parity.py`, `check_firewall_https.py` and
`check_firewall_nft_namespace.py`; all 15 comparator regressions and whitespace
checks pass. Full `scripts/check.sh` remains unrun due to the Longrun requirement.

## Card-image sizing baseline

The installed card is approximately 29.12 GiB. A provisional new-image layout
is 12 GiB for the system, 14 GiB for persistent data and update staging, and the
remaining space for an initially empty virtual-media partition. This is not yet
an approved partition table: H616 bootloader placement, exact partition
alignment and peak RAW staging usage must be verified first. The current card
must not be repartitioned or system-written without a separate operator action.

## Acceptance rule

No row becomes complete from source presence, an HTTP status alone, an old
receipt, or an empty comparison surface. Closure requires a fresh explicit
success result tied to the reviewed release, plus physical observation where
the requirement is inherently hardware-visible. Unknown hardware facts remain
open rather than being inferred from a related board or older image.
