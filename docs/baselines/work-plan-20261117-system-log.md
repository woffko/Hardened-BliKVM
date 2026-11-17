# Work plan: a system log that survives a hang, and logout on the toolbar

The test board hung between 2026-11-14 02:14 and 2026-11-15 00:00 (board
time) and ran again only after its power was cut on 2026-11-16. Nothing of
that boot's log survived, and nothing reset the board:

- `/var/log` is Armbian's ramlog in zram, synced to the card once a day;
  after each sync `armbian-ramlog` sends journald back to `/run`
  (`journalctl --relinquish-var`), so the journal lives in memory;
- busybox syslogd runs with `-C128`, a ring of 128 KiB in memory, fed by
  journald;
- the SoC watchdog (sunxi-wdt) was never opened: systemd's
  `RuntimeWatchdogSec` was 0, and `kernel.panic` was 0;
- the System Log settings page, ported from NanoKVM, is read-only and reads
  NanoKVM's files (`/tmp/hardened-syslog/messages`, `/var/log/messages`,
  `/var/log/kern.log`), none of which the board has, so it shows nothing.

The ext4 recovery at the next boot confirms an unclean stop. With the LED
socket held open for ten minutes (20 owner reads a second), the server, the
board owner and boardd did not grow in memory, threads or descriptors.

Done by hand on the test board on 2026-11-17 at the operator's word, until
the next RAW update: systemd's watchdog (`RuntimeWatchdogSec=15s`, set to
16 s by the driver), `kernel.panic = 10`, and netconsole to the operator's
PC until the next boot.

Its items are L1, L2, ..., so as not to be confused with earlier plans.

## Rules

The rules of the earlier plans hold: English repository text; secrets only
through `project-memory-exec`; one commit per item, starting with a test that
fails on the code as it is; `RUST_TEST_THREADS=16 sh scripts/check.sh` before
each phase's last commit and every build; consent for every install; nothing
on the board that changes its settings without the operator's word; nothing
sent to the target.

## Decisions

1. **The local log.** A file on the card that busybox syslogd writes
   (`/var/lib/hardened-blikvm-syslog/messages`, outside the ramlog, bounded
   by the page's buffer and rotations: at most 1 MiB and four rotations)
   instead of the ring in memory (recommended; it survives a hang, at the
   cost of the card's writes). The alternative keeps the ring and relies on
   the remote copy.
   **Changed by the operator on 2026-11-17**, the card to be written as
   little as possible: busybox syslogd holds only what programs send through
   `syslog(3)` (sshd, cron; none of the board's services and no kernel line
   was in its ring), and takes `/dev/log` from journald. So the ramlog and
   the journal in memory stay; busybox syslogd and klogd are switched off,
   `/dev/log` going back to journald; a small forwarder keeps on the card
   only the messages of priority err and above (`/var/lib/hardened-blikvm-syslog`,
   256 KiB and one rotation, a mark at each boot, crit and above synced at
   once). This board's journal held no err line in seven hours.
2. **The remote copy.** UDP syslog to a host and port the operator sets,
   off by default, the local log kept beside it (`-R host:port -L`), as the
   page offers (recommended).
   **Changed with decision 1:** the forwarder sends the copy, the messages
   up to the page's priority, while the remote host is set.
3. **Release.** An application release with L2 to L5 and a system release
   (card image and RAW sequence 12) with L1, at the operator's word
   (recommended); or the application now and L1 with the next system.
4. **netconsole** stays a diagnostic made by hand, not a setting
   (recommended).

The operator took decisions 1 to 4 as written on 2026-11-17 and added an
item: a logout button on the toolbar (L5).

## Phase A: the system (L1)

L2's system part (its unit, busybox syslogd masked) joins the system with
L1's files.

- **L1. The watchdog and the panic reboot in the system.** The image and the
  system's staging carry `/etc/systemd/system.conf.d/60-hardened-blikvm-watchdog.conf`
  (`RuntimeWatchdogSec=15s`, `RebootWatchdogSec=10min`) and
  `/etc/sysctl.d/60-hardened-blikvm-panic.conf` (`kernel.panic = 10`), the
  way `packaging/sysctl/60-hardened-blikvm-stream.conf` reaches it; a RAW
  update otherwise brings a system without them. Tests: the image's and the
  staging's checks name both files; on the board, after the RAW install,
  systemd reports the hardware watchdog in use.

## Phase B: the application (L2 to L5)

- **L2. The forwarder.** A small program, `bli-kvm-syslog-forward`, in the
  application slot, run by a unit of the system that the system's
  `syslog.socket` starts (journald's `ForwardToSyslog`): without privileges
  but `CAP_SYSLOG` for `/dev/kmsg`, which journald does not forward; it reads
  the forwarded messages and the kernel's, appends err and above to the
  file on the card (crit and above synced at once, a mark with the boot's
  identity first, 256 KiB and one rotation), and, while
  `/etc/hardened-blikvm/syslog.json` names a remote host, sends the
  messages up to its priority there over UDP (RFC 3164 lines, the board's
  name added). The system's part, carried by the image like L1:
  the unit, busybox syslogd and klogd masked. Tests: the filter and the
  file's bound and rotation, the boot mark, the remote lines, the kernel
  lines' priority, the unit's sandbox in the image's check.
- **L3. The board owner and the server.** Typed requests carry the page's
  settings (remote on, host, port, priority 1 to 8, the kernel's lines on
  or off), validated as the server validates them today; the owner writes
  `/etc/hardened-blikvm/syslog.json`, restarts the forwarder and reads the
  settings back, and a failed write leaves the previous file. A read
  request returns, bounded, this boot's journal (`system`), its kernel lines
  (`kernel`), the board's own units (`backend`) or the errors kept on the
  card (`saved`). The server's `POST /api/system-log/config` (CSRF, a
  signed-in session) and `GET` go through the owner, `GET
  /api/system-log/messages` too, `POST /api/system-log/test` logs a test
  line. The capability `system_log` is `read_write` where the system has the
  forwarder's unit, `read_only` otherwise. Tests: the owner's requests with a
  played journal and file; each route with a played owner; no session, no
  answer; the route counts follow.
- **L4. The System Log page.** The remote host, port, priority and the
  kernel's switch work when the capability is `read_write`; busybox's own
  fields (buffer, rotations, small output, stripped timestamps, console
  level) are not shown on this board; the kinds include the errors kept on
  the card. English and Russian. Tests: a contract through the built page (a
  change saved and read back, a host required when remote is on, the four
  kinds), the GUI source test.
- **L5. Logout on the toolbar.** `web/src/pages/desktop/menu`: a logout
  button after Settings, asking as Settings > Account does ("Are you sure
  you want to logout?") and logging out the same way (the session ended on
  the board, remote control with it, then the login page); shown by
  default and hidden or shown like the other toolbar items in Appearance's
  list. English and Russian. Tests: a contract through the built page (the
  button asks; confirmed, one `POST /api/auth/logout` and the login page;
  cancelled, nothing; hidden by the toolbar setting; today there is no
  button), the GUI source test.

Then the slot is built and installed at the operator's word, the probes run,
and the operator sets a remote host (this PC, for instance) and sees the
board's messages arrive.

## Phase W: the record

The review note `docs/baselines/audit-20261117-system-log.md`: each item's
commit and test, what the board showed. With decision 3, the releases follow
the recipe at the operator's word.

## Operator steps

- The decisions above.
- Consent for each install; a remote host to send to, if wanted.
- Whether and when to publish and to merge.
