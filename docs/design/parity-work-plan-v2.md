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
   the board answered no probe for about six minutes after each armed reboot
   (the first one needed a physical power cycle), while the same reviewed
   wrapper returned the board promptly when the boot gate ran it. The operator
   confirmed the second boot recovered on its own. The boot itself is not slow
   in the journal (userspace reaches `graphical.target` in 27 s and
   `NetworkManager` starts within seconds), so the dark window is consistent
   with the board's Ethernet link or DHCP taking minutes after a warm reboot
   rather than with a stalled unit; the boot gate's own reboot does not show it.

## P1 - video menu

`fps`, `quality`, `qualityHigh` and `qualityLossless` in the toolbar menu, with
the parameters applied to `bli-kvm-video-proxy`/`ustreamer` and read back.

Reconnaissance: the built interface already ships the whole screen menu
(`web/src/pages/desktop/menu/screen/` with video mode, resolution, quality,
fps, gop, frame detect, scale and reset) and posts to `/api/vm/screen`, but it
shows only `Scale` because the menu entries are gated on
`capabilities.video.status.availability === 'read_write'` and the server does
not implement the route or advertise that capability. The work is therefore
server-side: implement `/api/vm/screen`, apply the settings in
`bli-kvm-video-proxy`/`ustreamer`, read them back, and advertise the
capability only when the proxy can enforce them.

Implemented: the retained `ustreamer` is start-up-configured, so the privileged
board owner applies a change by writing the reviewed environment file the
streamer unit reads and restarting that unit (restoring the previous file when
the restart fails). The core crate owns the bounded, tamper-tolerant settings
model; the server maps `{type,value}` onto it, forwards one request over the
control socket, answers unsupported entries explicitly, and serves the
read-back over `GET /api/vm/screen`. The manifest carries the reviewed setting
list, so the menu offers the frame rate and the JPEG quality (the
`qualityHigh` and `qualityLossless` entries are values 80 and 100) and hides
the reference's video-mode, resolution, GOP and frame-detect switches, which
the MJPEG engine cannot apply. The device gate then passed
(`video-settings-gate-20260918-a.json`): the owner restarts the streamer from
its sandbox, the API read-back reports the applied frame rate and quality, the
engine's own `/state` moves from 40 to 24 desired fps and the gate restores 40
and 80 afterwards. The hardware encoder reports quality `0` in `/state`, so the
gate requires the engine to agree on the frame rate and verifies the quality
through the owner's file and the API read-back.

## P2 - settings sections

Inventory taken on 2026-09-18: the built interface already ships every section
the reference has (`about, account, appearance, device, network, system,
system-log, tailscale, update` under `web/src/pages/desktop/menu/settings/`),
including the firewall page, the mDNS and mouse-jiggler toggles and the panel
(`oled`) switch, so P2 is again mostly server-side.

- **device** (device-gate note, 2026-09-19): the first gate run found that the
  owner's settings directory is read-only inside its sandbox, so the reviewed
  configuration must be rewritten in place (as the resolver writer already
  did); the second run applied and verified mDNS, then lost the HID-only call
  to a `-503` immediately after the owner restart that the deploy performs, and
  both toggles were restored by hand and verified (`enabled=no`,
  `MulticastDNS=no`, `resolvectl mdns` `Global: no`). The gate still has to
  pass in one run: it should wait for the owner socket and a healthy front
  after a deploy before moving any switch.
- **device**: `mdns` is implemented end to end (owner flips the reviewed
  `systemd-resolved` key and restarts the unit; `GET /api/vm/mdns` plus the
  enable/disable routes and a server test). `mouseJiggler` is implemented too:
  the owner persists the accepted mode and the serve loop sends a one-pixel
  relative move plus its release on every interval, the absolute mode is
  refused because the target never reports the pointer position, and
  `/api/vm/mouse-jiggler` reports what the owner holds. Still to do: `hidOnly`
  (the owner refuses media attach and the media capability follows),
  `hidOnly` (the owner persists the switch and refuses every virtual-media
  command except a detach while it is on, with `GET`/`POST /api/vm/hid-only`
  and a switch in the device section). The read-only `autostart` list is
  surfaced with the network link (both live in the device section and the link
  switches the shared settings tab to `system`), and the `hdmi` entry is gone
  with the reason recorded: the board captures HDMI and has no HDMI output or
  hotplug control to toggle.
- **network**: `dns` (read and the reviewed write), `interfaces` and `wol` are
  implemented, and `ipv6` now speaks the reference shape: the read derives the
  mode from the primary interface's kernel state and the write accepts either
  `{mode}` or the reviewed `{interface, disabled}` toggle, resolving an omitted
  interface from the default route and refusing `manual` instead of silently
  downgrading it. `wifi` is implemented: the board really has
  `wlan0` with an active NetworkManager, so the owner reads the device state
  with `nmcli` and connects by writing a mode-0600 NetworkManager keyfile and
  bringing that profile up, which keeps the password out of argv, the
  environment and every log; a profile that fails to come up is removed again
  and only an owner-created profile can be disconnected. The server exposes
  the reference routes and refuses the access-point mode with a reason. `tls` is implemented: the owner toggles the
  reviewed front configuration with a marker block inside the port-80 server
  (test the configuration, reload the unit, restore the file on failure) and
  serves the public certificate view through `openssl`, with `GET`/`POST
  /api/vm/tls` on the server. The network section is complete; the remaining
  P2 work is the firewall (fail-closed while `nft` is absent, baseline model
  and write/restore gate with the operator's approval), systemLog
  `remote`/`save`/`test`, the extra cards (fan curve, activity LED, ATX LEDs)
  and one reviewed device window for the new toggles.
- **system.firewall**: implemented fail-closed. The owner probes the reviewed
  backends (`nft`, `iptables`, `ip6tables`), reports the baseline as
  unavailable and refuses every request while `nft` is absent, and applies the
  one reviewed ruleset with `nft -f` when a backend exists, saving the previous
  ruleset and restoring it when the apply fails. The server exposes the
  reference status, mode and confirm routes, offering only the baseline mode
  and refusing the others with a reason. Running the write/restore on the
  device needs `nft` installed, which remains a separate operator approval.
- **systemLog**: `/system-log/config` and `/system-log/messages` already serve
  the configuration, kernel and local views. `test` is implemented: the owner
  writes exactly one bounded line to the journal with `logger`, which is the
  surface the local view reads, and `POST /api/system-log/test` fails closed
  when the owner refuses. `remote` stays unavailable by design (this board has
  no vendor log service to forward through) and `save` is the existing
  download surface; the configuration write is the remaining item, because the
  reviewed settings live in our own persistent-journal drop-in rather than in
  the reference's log service, so the interface's save button has nothing
  reviewed to apply yet.
- Beyond the reference: the panel switch (`oled`), session lock and the
  thermal/RTC views already exist in the interface; the fan curve, activity
  LED and ATX LED cards are the remaining additions.


Reference mapping, kept here for the sections that are still open: `device`
carries `autostart`, `hidOnly`, `mouseJiggler`, `mdns` and the `network` link,
while `hdmi`, `disk` and `advanced` stay hidden with a documented reason;
`network` carries `dns`, `ipv6`, `tls` and `wifi`; `system.firewall` is the
reference baseline model over the owner and nftables, installed only with the
operator's approval and hidden and fail-closed until then, with the gate
writing and restoring; `systemLog` carries `kernel`, `local`, `remote`, `save`
and `test` where the hardware supports them; and beyond the reference the
thermal curve, the panel and its backlight, the activity LED, the ATX power and
HDD LEDs, the RTC and the session lock are fed by existing observations.

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
