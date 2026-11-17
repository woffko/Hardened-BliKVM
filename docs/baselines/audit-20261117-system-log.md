# A system log that survives a hang, and logout on the toolbar, carried out

The record of `work-plan-20261117-system-log.md`. The test board hung between
2026-11-14 02:14 and 2026-11-15 00:00 (board time) and ran again only after
its power was cut on 2026-11-16; nothing of that boot's log survived, and
nothing reset the board. Each commit names the test that failed on the code
before it.

## Items

| Item | What | Commit | Test that failed before | State |
| --- | --- | --- | --- | --- |
| L1 | the card image carries systemd's watchdog (`RuntimeWatchdogSec=15s`, 16 s on this driver) and `kernel.panic = 10` | `04f0f8f` | `test_the_image_resets_a_hung_board` (the files were missing and the builder did not install them) | fixed |
| L2 | the system log forwarder: journald's forwarded messages and the kernel's, err and above kept on the card (crit synced, a mark per boot, 256 KiB rotated once, a burst bound), an optional UDP copy; update runtime `0.1.0~dev47`; the image enables it as `syslog.service` and masks busybox syslogd and klogd | `eb0d619` | the crate's tests (new), `test_dev47_carries_the_syslog_forwarder` (the package was dev46), `test_the_image_keeps_the_boards_errors_and_not_busybox_syslogd` (no links) | fixed |
| L3 | the board owner writes the forwarder's settings (the previous file back when the restart fails) and reads this boot's journal, its kernel lines, the board's units or the kept errors; the server's routes go through it; `system_log` is `read_write` where the server finds the forwarder | `1147d6f` | `the_system_log_settings_and_logs_go_through_the_board_owner` (no `writable`, and `POST /api/system-log/config` was not routed) | fixed |
| L4 | the System Log page names the kept errors, puts the priority and the kernel switch with the remote copy, leaves busybox's fields out, reads four logs, writes only where the system has the forwarder | `7d42ec1` | `tools/playwright/system_log_contract.mjs` (no card section) | fixed |
| L5 | a logout button on the toolbar, after Settings, asking as Settings > Account does | `bb77622` | `tools/playwright/logout_toolbar_contract.mjs` (no button) | fixed |

Found on the way: `check.sh` also tests the server without the board's
restricted control surface, where L3's route test read `read_only`; it is
run only with that feature (`fc30294`). The image builder would have put the
source board's remote host (`/etc/hardened-blikvm/syslog.env`) and kept
errors on every new card; it removes both and names the settings among the
source board's state (`1afb20a`, `test_the_source_boards_system_log_stays_with_it`).
Phase A's check stopped three times on timing tests while another session's
virtual machines loaded the host (`mjpeg_upscale_contract.mjs` drew 5.7 of 10
frames a second, `quic_share_contract.mjs` saw its QUIC session open again on
the narrowed path, an update supervisor upload test timed out); none touched
L1, and the check passed in one run on `fc30294` and on `1afb20a`.

Decisions 1 and 2 were changed by the operator on 2026-11-17, the card to be
written as little as possible (the plan records why); decisions 3 and 4 were
taken as written, and L5 was added by the operator.

## The hang

Found on 2026-11-17: the board stopped between 2026-11-14 02:14 and
2026-11-15 00:00 (board time). Armbian's ramlog synced `/var/log` to the card
each midnight up to 2026-11-14 and not on 2026-11-15, and nothing on the card
was written after 2026-11-14 02:14; the next boot's ext4 recovery on both
partitions confirms an unclean stop. Its cause is not known: no log of that
boot survived, the kernel has no lockup detector, and the board was down
until its power was cut. The LED socket of 0.1.0-beta.13 was ruled out as a
leak: held open for ten minutes (twenty reads of the board owner a second),
the server, the owner and boardd grew in neither memory, threads nor
descriptors.

## Gates

On the test board, at the operator's word.

- By hand on 2026-11-17, before the plan: systemd's watchdog and
  `kernel.panic = 10`, and netconsole to the operator's PC (until the next
  boot).
- **Slot 111** (0.1.0-beta.14) was built from a clean tree at `fc30294` after
  a green `scripts/check.sh` (the slot's tar `98198437…c82a`) and installed on
  the RAW system of sequence 11; the install was confirmed and the state was
  as found but for the slot.
- **The system staged by hand** (as for earlier system releases): update
  runtime `0.1.0~dev47` installed (its files are dev46's with the forwarder
  and its unit, and the update supervisor and execution helper rebuilt with
  `rustix` 1.1.6), the forwarder enabled as `syslog.service`, busybox syslogd
  and klogd masked, one reboot. After it: the forwarder active, no syslogd,
  `/dev/log` journald's again, `errors.log` on the card with the boot's mark
  and the boot's error lines, systemd's hardware watchdog at 16 s,
  `kernel.panic` 10, no failed unit.
- The watching probes passed on slot 111 and that system: the GUI walks on a
  desktop and a phone (the toolbar ending `settings, log-out, maximize`; the
  System Log section read without an error), WebRTC with H.264 and with VP8,
  QUIC video, touch reach. The board's API: `system_log` `read_write`, the
  settings writable, the four logs read through the owner.
- **The remote copy**, at the operator's word ("leave it on"), to the
  operator's PC on UDP 5514: the test line and the forwarder's own restart
  lines arrived as RFC 3164 lines with the board's name. It stays on.
- **The system release**: the card image 0.1.0-beta.14 was made from the
  board at `1afb20a` after a green `scripts/check.sh` (the image `c50889f3…`,
  its compressed copy `703e8c20…`, the root `15ec1a7a…`; the compressed copy
  unpacks to the image and its first partition is the root); the image
  carries the watchdog and panic files, the forwarder enabled as
  `syslog.service`, busybox masked, and neither the board's remote host nor
  its kept errors. The RAW bundle of sequence 12 (`2ca9a8c1…`) was signed and
  its signature verified.
- **The RAW install** of sequence 12 on the board, at the operator's word,
  root's home saved first: uploaded, written and booted in about 33
  minutes; the certificate, the host keys, the account, the slot, the host
  name, the session lock and SSH carried. The state as found but for the RAW
  phase, sequence and version, a temporary IPv6 address and the encoder's
  live state; the forwarder active from the image, no syslogd, systemd's
  hardware watchdog at 16 s, `kernel.panic` 10, the remote copy's settings
  carried (`/etc/hardened-blikvm`) and its lines arriving again after the
  boot. The errors kept before the update were on the old root and are gone:
  a RAW update writes a new root. Confirmed: the board keeps sequence 12;
  root's home put back and its copy deleted. The RAW switch was found on and
  left on; netconsole, loaded by hand, went with the reboot.

The test board ends on slot 111, update runtime dev47 and the RAW system of
sequence 12, its settings and switches as found but for the remote copy the
operator set.
