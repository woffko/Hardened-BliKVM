# Reference-driven parity work plan (v2)

Status: supersedes the E0-E5 ordering for the remaining work. The reference is
the stand at `https://<reference-address>`; its interface surface was extracted from the
served JavaScript bundle and is the source of truth for what parity means.

## Reference surface (evidence)

Settings sections: `about, account, appearance, device, network, system,
systemLog, tailscale, update`.

- `device`: `advanced, autostart, disk, hdmi, hidOnly, mdns, mouseJiggler,
  network`.
- `network`: `dns, ipv, tls, wifi`.
- `system.firewall`: `backend, effectiveMode, enabledPort, enableHttpsFirst,
  baseline.{active,allows,apply,confirmTitle,confirmDesc}, cancel, disabled`.
- `systemLog`: `kernel, local, remote, level, save, test`.
- `screen`: `fps, quality, qualityHigh, qualityLossless`.
- `update`: `title, application, system, available, isLatest, preview,
  previewDesc, previewTip, paranoidBlocked, paranoidBlockedDesc, offline,
  confirm, cancel, updating, updateFailed, queryFailed`.
- `tailscale`: `account, enable, install, login, loginSuccess, deviceIP,
  deviceName, download, installing, failed, loading, cancelBtn`.

Board facts read on 2026-09-18: `NetworkManager` is active; `nft` is not
installed; the `tailscale` package is absent from the configured repositories;
the composite gadget is not created after a reboot, so HID and media are dead
until the first media attach.

## P0 - stability (blocks every physical behaviour)

1. **Gadget lifecycle.** The composite gadget is created and bound when the
   owner starts and lazily on the first HID or media request, with a read-back
   and a documented recovery after a reboot. Device gates: the HID probe
   (keyboard and mouse against an operator-observed target) and the media
   attach gate.

   Host side is implemented in `3b49bb4`: the owner binds a HID-only gadget
   before serving and before every HID lease, and the media slice changes a
   live gadget in place (`replace_mass_storage` on attach, the new
   `remove_mass_storage` on eject) so HID survives an eject. Device window
   `gadget-lifecycle-acceptance-20260918-a.json` (private) passed on the board:
   the restarted owner logged the startup bind, the gadget came up bound with
   `hid.usb0/1/2` and `/dev/hidg0..2`, the media probe
   (`virtual-media-acceptance-20260918-a.json`) applied select, attach and
   eject, and the post-eject state kept the HID functions bound with the
   mass-storage function gone. Keyboard and mouse on the target and behaviour
   across a reboot were not part of that window.
2. **Boot reliability.** Persistent journal, an audit of the serving unit's
   ordering and drop-ins, and a board server that starts after a reboot. Gate:
   reboot, then six of six units active, `443` answering and `/api/health` at
   `200`.

   The device audit found the board keeps `/var/log` on a RAM disk and points
   `/var/log/journal` at `/var/log.hdd/journal`, so an early journald start
   writes into a dangling symlink and loses the first boot records; the drop-in
   shipped in `packaging/systemd/systemd-journald.service.d` now orders
   journald after that mount. The owner is `Type=simple`, so a boot-time start
   of the server can race the control socket and leave the unit dead behind the
   nginx 502; the unit now waits for the socket with a bounded `ExecStartPre`.
   Running `scripts/check.sh` here also exposed that the source-boundary secret
   and private-address scans had never run because `ripgrep` was absent, so the
   committed tree still carried the operator signing-key directory and board
   addresses; the key is no longer tracked and the addresses are sanitized.
   The reboot gate itself still needs its own approval.
3. **Armed reboot.** A clean dispatch while the server is healthy plus the
   owner-side acceptance until the gate reports `passed`; the reviewed wrapper
   keeps `systemctl --no-block`.

   The P0.2 boot gate passed on the board: after a reviewed reboot the boot
   identity changed, six of six units were active without help, `443` answered,
   `/api/health` returned `200`, `systemctl --failed` was empty, the owner
   logged the startup HID bind, and the journald drop-in ordered the journal
   after `/var/log.hdd` so the first boot records survived. The armed-reboot
   half then reproduced the known first-run regression: the dispatch was
   accepted (`system-action-gate-20260918-b.json`, `dispatching`) and the board
   never returned (no ICMP, ports 22/80/443 unreachable after about six
   minutes), which needs a physical power cycle and then the post-boot
   acceptance. The cause is not yet known; the persistent journal is the
   post-mortem source.

   The post-boot acceptance then found the store still holding the empty
   initialized record, and two defects followed from it. The owner deferred the
   journal write to the first dispatch, and when the store already held copies
   that `initialize` failed with a generation gap while the failure was
   swallowed, so the reboot was dispatched with no record and could never be
   accepted. The owner now initializes the journal as it loads a store with no
   valid copy and refuses the action when a write fails (fail-closed). That
   refusal then exposed the second defect: the owner runs with the reviewed
   drop-in group that keeps the control socket reachable, so its journal copies
   were group-owned by that group and the store rejected them as unsafe. The
   store now sets the ownership of every copy it creates from its own reviewed
   identity, the owner keeps `CAP_CHOWN` for exactly that, and the previously
   device-only socket-group drop-in is shipped from the repository.

   With both fixed the armed cycle passed
   (`system-action-gate-20260918-i.json`: the dispatch applied, the post-boot
   acceptance applied, the store reported the record accepted and a replay was
   rejected). One observation stays open and is recorded rather than explained:
   the board was unreachable for about six minutes after each armed reboot (the
   first one needed a physical power cycle) while the same reviewed wrapper
   returned the board promptly when the operator ran the boot gate instead.

## P1 - video menu

`fps`, `quality`, `qualityHigh` and `qualityLossless` in the toolbar menu, with
the parameters applied to `bli-kvm-video-proxy`/`ustreamer` and read back.

## P2 - settings sections

- `device`: `autostart`, `hidOnly`, `mouseJiggler`, `mdns` and the `network`
  link; `hdmi`, `disk` and `advanced` stay hidden with a documented reason.
- `network`: `dns` and `ipv6` (the APIs exist), `tls` (certificate view and
  replacement) and `wifi` through NetworkManager.
- `system.firewall`: the reference baseline model over the owner and nftables.
  The package is installed only with the operator's approval; until then the
  capability stays hidden and fail-closed. The gate writes and restores.
- `systemLog`: `kernel` and `local` through the existing API; `remote`, `save`
  and `test` where the hardware supports them.
- Beyond the reference: cards for the thermal curve, the panel and its
  backlight, the activity LED, the ATX power and HDD LEDs, the RTC and the
  session lock, all fed by existing observations.

## P3 - HID and inputs

Keyboard and mouse menus, the leader key, shortcuts and the mobile keyboard are
compared against the reference, with an end-to-end gate on an observed target.

## P4 - update mechanism and Tailscale

- **Application update**: restore the mechanism, not only the page - the signed
  production bundle (seq7, `1b8281cd…`) is published as a candidate, its health
  is checked, it is confirmed into `current`, activation and the watchdog return
  and the `settings.update.application` page exists in both the desktop and the
  mobile interface.
- **System update**: a separate track around `bli-kvm-system-boot-control`
  (storage adapter and selector), starting with the design and host tests; the
  page stays hidden with a documented reason.
- **Tailscale**: the package is installed once the operator provides a source;
  management is refused by default and the surface follows
  `settings.tailscale.*`.

## P5 - acceptance

A parity run over authenticated captures of both interfaces (desktop and
mobile), the release-stage physical re-verification of the fan, LED, panel and
ATX buttons, a soak and reboot acceptance, the final report, the manifest and
the baselines. The earlier claim that the reference has no firewall is
corrected.

## Invariants

Host tests and a green `scripts/check.sh` for every step; English for everything
that reaches GitHub; private evidence and secrets never published and secrets
only through `project-memory-exec`; device mutations only with the operator's
explicit confirmation in the session; a reboot only with its own explicit
confirmation; historical receipts are never rewritten and checks are never
weakened; one owner per gadget, ATX and video line; every change reversible.
