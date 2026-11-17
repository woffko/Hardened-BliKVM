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
2. **The remote copy.** UDP syslog to a host and port the operator sets,
   off by default, the local log kept beside it (`-R host:port -L`), as the
   page offers (recommended).
3. **Release.** An application release with L2 to L5 and a system release
   (card image and RAW sequence 12) with L1, at the operator's word
   (recommended); or the application now and L1 with the next system.
4. **netconsole** stays a diagnostic made by hand, not a setting
   (recommended).

The operator took decisions 1 to 4 as written on 2026-11-17 and added an
item: a logout button on the toolbar (L5).

## Phase A: the system (L1)

- **L1. The watchdog and the panic reboot in the system.** The image and the
  system's staging carry `/etc/systemd/system.conf.d/60-hardened-blikvm-watchdog.conf`
  (`RuntimeWatchdogSec=15s`, `RebootWatchdogSec=10min`) and
  `/etc/sysctl.d/60-hardened-blikvm-panic.conf` (`kernel.panic = 10`), the
  way `packaging/sysctl/60-hardened-blikvm-stream.conf` reaches it; a RAW
  update otherwise brings a system without them. Tests: the image's and the
  staging's checks name both files; on the board, after the RAW install,
  systemd reports the hardware watchdog in use.

## Phase B: the application (L2 to L5)

- **L2. The board owner keeps the system log.** A typed request carries the
  page's settings (remote on, host, port, priority 1 to 8, buffer 16 to
  1024 KiB, rotations 0 to 4, small output, stripped timestamps, the kernel
  log and its console level), validated as the server validates them
  today. The owner writes them as busybox syslogd and klogd options
  (`/etc/default/busybox-syslogd`), the local log on the card per decision
  1, restarts the two services and reads the options back; a write that
  fails leaves the previous options. Tests: the options for each field;
  invalid values refused; a failed write or restart leaves the file as it
  was.
- **L3. The server's routes.** `POST /api/system-log/config` (CSRF, a
  signed-in session) through the owner, `GET` from the owner;
  `GET /api/system-log/messages` reads, through the owner, the local log
  (`system`), its kernel lines (`kernel`) and the journal of the board's own
  units (`backend`), bounded as today; `POST /api/system-log/test` sends a
  test message. The capability `system_log` becomes `read_write`. Tests: each
  route with a played owner; no session, no answer; the route count follows.
- **L4. The System Log page.** The switches and fields work when the capability is
  `read_write`; the page says where the local log is kept. English and
  Russian. Tests: a contract through the built page (a change saved and read
  back, a remote host required when remote is on, the three kinds shown), the
  GUI source test.

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
