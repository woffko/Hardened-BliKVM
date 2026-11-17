# The system log

The test board hung on 2026-11-14 and ran again only after its power was cut
two days later; nothing of that boot's log survived
(`docs/baselines/work-plan-20261117-system-log.md`). Armbian keeps `/var/log`
in zram and syncs it to the card once a day, and after each sync it sends
journald back to `/run`, so the journal lives in memory. busybox syslogd kept
a ring of 128 KiB in memory, and it held only what programs send through
`syslog(3)`: it had taken `/dev/log` from journald, and neither the board's
services nor the kernel reached it.

The operator chose to write the card as little as possible: the ramlog and
the journal in memory stay, and only the errors are kept on the card.

## Parts

- **journald** keeps the whole log of the boot in memory, as before, and
  forwards each message to `syslog.socket` (`ForwardToSyslog`, its default).
- **busybox syslogd and klogd** are masked by the card image, so `/dev/log`
  is journald's again and sshd's and cron's messages are in the journal.
- **The forwarder** (`bli-kvm-syslog-forward`, unit
  `hardened-blikvm-syslog.service`, which is `syslog.service`) takes the
  socket from systemd and reads the kernel's records from `/dev/kmsg`, which
  journald does not forward. It:
  - appends the messages of priority err and above, kernel and services
    alike, to `/var/lib/hardened-blikvm-syslog/errors.log` on the card, at
    once, and syncs those of priority crit and above; a line with the boot's
    identity marks each boot, written once per boot; at 256 KiB the file is
    rotated once (`errors.log.1`), so the card holds at most 512 KiB of it;
    at most 100 lines at once and ten a second after, the rest counted in a
    line of its own, so a storm of errors cannot keep the card busy;
  - reads a boot's kernel records from its start the first time it runs in
    the boot, and from their end after a restart within it;
  - sends, while the settings name a remote host, the messages more urgent
    than the settings' priority (busybox's `-l`: 1 to 8, 8 sends all) to it
    over UDP as RFC 3164 lines with the board's name, the kernel's only if
    the settings say so; the host's name is resolved again every five
    minutes.
- **The settings** are `/etc/hardened-blikvm/syslog.env`
  (`HARDENED_BLIKVM_SYSLOG_REMOTE=host:port`, `_PRIORITY`, `_KERNEL`), which
  systemd reads for the unit; without the file nothing is sent and the
  errors are still kept; an invalid file sends nothing either. The board
  owner writes it from the System Log page (L3).

On this board's journal, seven hours held three warnings and no error: the
card sees a boot mark and the rare error.

## The page, the board owner and the server

The board owner writes the page's settings (`SystemLog`), whole and root's
alone, and restarts the forwarder; if the restart fails it puts the previous
file back and starts the forwarder with it. It reads them back
(`SystemLogState`, with whether the system has the forwarder's unit), and
reads a log (`SystemLogRead`): this boot's journal (`system`), its kernel
lines (`kernel`), the board's own units and nginx (`backend`), through
`journalctl` with fixed arguments, a 10 s deadline and a bound on its
output, or the errors kept on the card (`saved`); the answer is the tail,
at most 1000 lines and 16 KiB, within one control message. These requests
run on the owner's settings worker, not under the lock HID leases take.

The server's `GET` and `POST /api/system-log/config` and
`GET /api/system-log/messages` go through the owner; the capability
`system_log` is `read_write` where the server finds the forwarder's unit and
the owner answers, `read_only` elsewhere, and a write is refused there. The
test line (`POST /api/system-log/test`) goes to the journal as before and,
with a remote host set, on to it.

The kept errors live on the root (`/var/lib/private`), so a RAW system
update, which writes a new root, starts them afresh; the settings live in
`/etc/hardened-blikvm`, which the update carries.

## Sandbox

The forwarder runs as a dynamic user without capabilities
(`CapabilityBoundingSet=`), reads `/dev/kmsg` only (`DeviceAllow=/dev/kmsg r`;
`kernel.dmesg_restrict` is 0 on this system), writes only its state
directory (readable by root alone, through `/var/lib/private`), and reaches no address on the board itself
(`IPAddressDeny=localhost`), Janus's included. `systemd-analyze security`
rates its exposure 1.4.

## How it reaches a board

The forwarder and its unit are files of the update runtime from `0.1.0~dev47`
(a system part); the card image enables the unit as `syslog.service` and
masks busybox syslogd and klogd; a RAW system update brings both with the
image's root. An application slot on an older system finds no forwarder, and
the System Log page stays read-only there (L3).

## Tests

`crates/bli-kvm-syslog-forward` (the settings, the parsing of journald's
lines and the kernel's records, the remote lines and their filter, the file
on the card, its boot marks, rotation and bound against a storm),
`tools/test_build_debian_update_runtime_package.py`
`test_dev47_carries_the_syslog_forwarder`, `tools/test_build_sd_image.py`
`test_the_image_keeps_the_boards_errors_and_not_busybox_syslogd`,
`tools/test_packaging_boundaries.py` (no loopback),
`scripts/verify-systemd-units.sh` (the unit loads and its exposure is within
its limit).
