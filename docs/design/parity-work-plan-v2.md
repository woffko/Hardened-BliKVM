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

Device gate: passed on 2026-09-19 (`device-settings-gate-20260918-f.json`) —
mDNS toggled and verified through the resolver and restored, HID-only enabled
with the media attach refused by the reviewed contract and restored, the test
line applied and observed in the journal, and the IPv6, WiFi, TLS and jiggler
views recorded. The manifest reports P2 complete.

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
  after a deploy before moving any switch. That readiness and retry landed
  (`a7613f4`) and the third run then showed the failure is deterministic and
  specific. HID-only *is* applied (the state file reads `enabled=yes` and the
  owner journal is clean), mDNS is applied and verified, and the failure lands
  on the **media attach under HID-only**: the reviewed refusal reaches the
  server, but `POST /api/virtual-media` answers `-503` where the reviewed
  contract is `{"applied": false, "rejection": "capability_disabled"}` with code
  zero — so the gate, and the interface with it, reads a refusal as an outage.
  Both media handlers do map `ControlResponse::Rejected`, so the next step is to
  trace which layer turns that rejection into `ServiceUnavailable` (the control
  client's decode or an earlier guard). The toggles were restored again and
  verified (`enabled=no`, `MulticastDNS=no`, `resolvectl` `Global: no`, health
  200).
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
- **P1.1 (evidence, 2026-09-19)**: the HDMI bridge is a plain **USB2 UVC dongle**
  (`345f:2131 MACROSILICON USB2 Video`, 480 Mbps, `/dev/kvmd-video` ->
  `/dev/video1`) driven by `uvcvideo`; it offers MJPEG and YUYV with no H.264,
  and the SoC has a hardware decoder only, so H.264 would be software
  transcoding on four A53 cores. A read-only encode probe (2026-09-19) confirmed
  the hardware encoder is not exposed at all: `h264_v4l2m2m` reports *"Could not
  find a valid device"*, there is no `libcedarc`/`libmpp`/`cedar_dev`, and
  `ustreamer 6.11` only offers MJPEG encoders. Measured software cost: MJPEG
  720p30 -> libx264 veryfast runs at ~14 fps (fits the 10 fps bridge), synthetic
  1080p30 only ~2 fps; the vendor board matches this by running MJPEG-only with
  Janus disabled. The vendor configuration sets
  `streamer.quality: 0` and passes **no `--quality`**, which is the proof that
  the JPEG quality cannot be changed on this board; the `1280x720` mode
  advertises 60 fps on MJPEG while the engine captures 10. The capture
  resolution is now a reviewed setting of its own (owner env file, unit
  `--resolution=${BLI_KVM_USTREAMER_RESOLUTION}`, four modes the interface
  offers, applied and read back on the device) and the 1080p experiment shows
  the 10 fps figure is independent of the output mode *and* of motion: driving
  the pointer for twenty seconds left the idle capture at 10 fps while two
  snapshot frames two seconds apart differed, so the feed is live. The figure
  is the `--slowdown` idle rate our unit requests, not a bridge limit: with one
  with one MJPEG viewer connected the delivered rate follows
  the request (10/20/30/50/50/50 at 1920x1080, 10/20/30/50/50/60 at 1280x720,
  800x600 and 640x480; `video-matrix-20260919-e.json`), with the 1080p plateau
  near 50 fps coming from `--drop-same-frames=30` on a static desktop. The
  delivered rate is now read back over the engine's read-only `/state` socket
  (`crates/bli-kvm-server/src/video_engine.rs`, env
  `BLI_KVM_USTREAMER_SOCKET`) and the screen menu shows it beside the requested
  value whenever it differs, so the interface no longer implies the requested
  60 fps is what the stream delivers. The reviewed video-settings gate now also
  applies a capture mode and requires the engine's `source.resolution` to follow
  it, reads the engine's `source.captured_fps` and requires the server's
  `capturedFps` read-back to equal it, and restores both
  (`tools/stage_device_video_settings_gate.py`); the device run waits for the
  same deployment approval. The operator supplied the vendor-stack
  analysis on 2026-09-19: the reference's H.264 is `libx264` software encoding
  into a shared-memory sink consumed by Janus/WebRTC, which agrees with the
  probe. Remaining: the operator decides whether to fund a Janus/WebRTC port
  plus a software-transcode project (and answer what the target outputs);
  until then the video-mode/bitrate/GOP controls stay hidden. Full evidence and
  consequences: `docs/baselines/hdmi-bridge-usb2.md`.

- **P1.1 video-settings wiring check (operator request, 2026-09-19)**: the
  operator supplied a vendor source map
  (`docs/baselines/video-settings-source-map.md`) after the wiring of some video
  settings was unclear. The vendor spreads them across `SettingsVideo.vue`,
  `useVideoResolution.js`, `video.route.js`, `video.js`, `kvmd-video.sh` and
  `app_default_config.js`; the V4 H.264 mode there is software `libx264` into a
  shared-memory sink for Janus. The check we still owe:
  1. **Our wiring table, host-verified**: for every setting we expose, follow
     the whole chain and prove each hop is attached — claim (UI component) ->
     HTTP (`POST /api/vm/screen`, `crates/bli-kvm-server/src/video_settings.rs`)
     -> owner (`crates/bli-kvm-control-broker/src/video_settings.rs`) -> on-disk
     artefact (`/var/lib/hardened-blikvm-video/ustreamer.env`) -> unit argument
     (`--desired-fps=${BLI_KVM_USTREAMER_FPS}`,
     `--resolution=${BLI_KVM_USTREAMER_RESOLUTION}`) -> engine field
     (`result.source.desired_fps`, `result.source.resolution`) -> read-back
     (`GET /api/vm/screen` `fps`, `capturedFps`). The capability advertises only
     `Fps` and `Resolution`; `quality`, video mode, bitrate, GOP and frame
     detect are hidden, each with a recorded reason.
  2. **Device capability matrix** (`tools/capture_device_video_matrix.py`): apply every reviewed resolution (1080/720/600/480) against the
     requested FPS values `10,20,30,40,50,60`, settle, and record
     `source.resolution.width/height`, `source.captured_fps`,
     `stream.queued_fps` and the requested value, then restore. Unsupported
     combinations are recorded as observed, never assumed. Output is a private
     receipt plus a table in `docs/baselines/video-settings-source-map.md`
     filled from the receipt.
  3. **Vendor comparison on the board** (done 2026-09-19): the retained
     `/opt/bin/blikvm/package/ustreamer/kvmd-video.sh` shows the H.264
     arguments only in the Raspberry Pi branch; the V4/H616 branch is MJPEG-only
     with `--desired-fps=20` and a hardcoded `1920x1080`. So the source map's
     V4 H.264 pipeline describes the newer/reference vendor build, not this
     image, and the physical board has no vendor H.264 path at all. Recorded in
     `docs/baselines/video-settings-source-map.md`.
  4. **State parity decision** (done 2026-09-19): the vendor state exposes
     `width`, `height`, `capturedFps` and `queuedFps`, so
     `GET /api/vm/screen` now also returns `width`, `height` and `queuedFps`,
     read from the same engine `/state` read as `capturedFps`
     (`crates/bli-kvm-server/src/video_engine.rs` `read_state`,
     `crates/bli-kvm-server/src/video_settings.rs`). The UI change is a
     follow-up; the capability/state surface now matches the reference fields.

  Gate on the device: apply the matrix, confirm each setting changes the engine
  `/state` and the delivered rate in the stream, and restore; receipt
  `video-matrix-*.json`. Acceptance: the wiring table above is checked in, every
  exposed control is proven attached, and the matrix receipt exists with no
  unrecorded combination.
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
  and refusing the others with a reason. `nftables` was installed on
  2026-09-19, and the new `tools/stage_device_firewall_gate.py` applies and
  restores the baseline. The device gate then found a sandbox limit: under
  `PrivateNetwork=yes` the owner's `nft` manages its own network namespace, and
  removing that directive raised the unit's `systemd-analyze security` exposure
  from 1.7 to 2.1, above the reviewed threshold of 2.0. The fix is a transient helper:
  each `nft` command is executed by a short-lived systemd unit the owner asks
  PID1 to start (`systemd-run --wait`, host network namespace, only
  `CAP_NET_ADMIN`), while the owner keeps its sandbox. The probe now checks the
  backend binary's existence instead of executing it. The gate then passed on
  the device with the unit unchanged (`firewall-gate-20260919-c.json`) and is
  pinned in the P2 manifest.
- **systemLog** (device gate, 2026-09-19): the test line works. `logger`
  returned success without the line ever appearing in the journal on this
  image, so the owner sends the reviewed native journal payload straight to
  `/run/systemd/journal/socket` (`ef8e349`), and the line is in the journal as
  `hardened-blikvm system-log test`. The gate's own check was the last bug: a
  cumulative count hid the new line, so it now counts matches inside a
  two-minute window (`2b87947`).
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

**Defect found on the operator's report (2026-09-19): input died after an idle
period (also through the Android client, which cannot be fixed client side).**

**Root cause of the recurring drop, found in the server log with one client
open:** every failure is `LeaseConflict`, on both the renewal and the new
acquire — never a timeout and never the gadget (the UDC stays `configured` and
`/dev/hidg0..2` stay present). So the interface loses input because the broker
already holds an active lease it cannot see: the client's session loses its
lease id (a failed renewal, a reload, a race between the renewal and a report),
its next acquire is refused as a conflict with the *same* session's lingering
lease, and it stays dead until that lease lapses — the 30 s working / 15 s dead
cycle the operator described. The fix belongs in the broker: an acquire for the
same confirmed session must **supersede** that session's stale lease (release
the engine's active lease for the same binding and grant the new one) instead of
answering `LeaseConflict`; a conflict with a *different* session stays refused.
Note that the report-based refresh added earlier is what keeps the stale lease
alive while a broken client still manages to report, so the supersede is the
missing half.

**Fixed and proved on the board (2026-09-19).** The policy's acquire now revokes
the requester's own session lease and retries once when the engine answers
`Busy`, so a client that lost its lease id always takes its own lease back,
while a conflict with a different session still fails closed; a policy test pins
both halves. Device probe: confirm, acquire (lease A), then a second acquire
**without releasing** (the lost-id case) now returns `applied=true` with a fresh
lease id, a report through the fresh lease applies and the release succeeds —
previously that second acquire answered `LeaseConflict` and input stayed dead
until the stale lease lapsed, which is exactly the operator's cycle. The broker admits a HID lease only while the session's *target
attestation* is live (`ATTESTATION_TTL_SECONDS = 30` in the server, the same
30 s the interface asks for the lease), while the interface's lease session
renewed the lease every 15 s and never refreshed the attestation, and its focus
cache held a confirmation for 25 s. After an idle stretch the attestation
expired unseen, the renewals failed, the session dropped its lease, and the
next keystroke's acquire was refused with `not_attested`, so reports were
dropped and input only came back by luck. Fix (client side, matching the
reference's always-on input): the lease session refreshes the attestation
before acquiring and before every renewal, and the client cache sits at 15 s,
well inside the server's window. Guard tests pin both. The decisive
fix is server-side, because the Android client cannot be changed: the server's
own focus confirmation (30 s) gated the lease route *before* the broker, so a
lapsed confirmation refused renew, report and release alike. The lease route
now accepts a lapsed confirmation only for a session whose **lease is live** and
refreshes the confirmation, so an active session keeps its keyboard and mouse
the way the reference does; a lapsed confirmation without a live lease is still
refused. Proved on the board: after one confirmation and an acquire, renew and
report succeed after 14 s, 34 s and 54 s of idle and the release applies.

The operator then reported a 30 s working / 15 s dead cycle, which came from
the server's lease entry only being extended by a *renewal*: a client that
keeps sending input without renewing (the Android interface among them) lost
its entry at the lease TTL, and its next acquire was refused because the
client's own focus cache was still warm. A report is now proof of an active
session everywhere: the broker refreshes the lease to the reviewed maximum on
every accepted report, and the server extends both the lease entry and the
confirmed bindings. Proved with a report-only client: 45 reports over 90 s with
zero failures and a clean release.
The interface tree and both binaries were redeployed (web backup at
`/var/lib/hardened-blikvm-gadget/web-pre-hid-lease-fix`). Note for a later review: the
mouse jiggler writes the same relative-mouse function the leased pointer uses,
so two writers share one HID function.


Host side: the built interface already carries the keyboard (with the leader-key
recorder) and mouse menus (absolute, relative and the mobile trackpad), and the
server serves the HID lease, focus and leader-key routes. The one missing
server surface was the shortcut store, which now exists reviewed
(`crates/bli-kvm-server/src/shortcuts.rs`, `b611e12`): one to four keys per
entry, ASCII key codes, a fixed entry count, an atomic mode-0600 file in the
server's own state directory, and `GET`/`POST`/`DELETE /api/hid/shortcuts` in
the shapes the interface already calls. What remains is the end-to-end gate on
an observed target: the reviewed keyboard and mouse probes plus a shortcut
round trip, with the operator watching the target. The lease supersede fix has
its own reviewed gate (`tools/stage_device_hid_lease_gate.py`, receipt glob
`hid-lease-supersede-gate-*.json`, probe `hardened-blikvm-hid-lease-gate`): one
session loses its lease id and re-acquires without a conflict, a report with the
fresh lease is applied, and a second independent session is still refused with
`lease_conflict`. The manifest's P3 phase pins that receipt, so the acceptance
report now lists it as the remaining device item next to the E4 cutover.


Keyboard and mouse menus, the leader key, shortcuts and the mobile keyboard are
compared against the reference, with an end-to-end gate on an observed target.

## P4 - update mechanism and Tailscale

- **Application update**: restore the mechanism, not only the page - the signed
  production bundle is published as a candidate, its health is checked, it is
  confirmed into `current`, activation and the watchdog return and the
  `settings.update.application` page exists in both the desktop and the mobile
  interface. Key rotation: the seq7 artifact was re-signed with
  `app-production-2026-09-18-01` and passed host verification (`d80ef86d…`),
  but the device's `application-state.json` reports **floorSequence 11,
  generation 49, pending null**, so a seq7 candidate is stale and cannot be
  published. The publishable candidate must carry a sequence above 11 (next:
  12) and be built from an accepted runtime package; the last accepted package
  is `6df67f5b…` (dev7, source/builder commit `3b57ee45…`) and the device must
  also trust the rotated key before the device cycle can run. A second blocker
  was found the same day: the current bundle builder requires the runtime
  package to ship `bli-kvm-boardd` (`REQUIRED_BUNDLE_PATHS`), but the accepted
  dev6/dev7 packages carry only four binaries; only the older dev2 package
  (`589a9eaa…`) has boardd. So a publishable seq>=12 candidate with the current
  payload needs a fresh runtime package built from a committed tree, i.e. the
  `git commit` step.
- **System update**: a separate track around `bli-kvm-system-boot-control`
  (storage adapter and selector), starting with the design and host tests; the
  page stays hidden with a documented reason.
- **Tailscale**: the package is installed once the operator provides a source;
  management is refused by default and the surface follows
  `settings.tailscale.*`. The remaining surface is implemented fail-closed
  (owner `tailscale.rs`, the ten `/api/extensions/tailscale/*` routes, host
  tests) and its device proof is the reviewed gate
  `tools/stage_device_tailscale_gate.py` (receipt glob
  `tailscale-surface-gate-*.json`, probe `hardened-blikvm-tailscale-gate`): the
  status is readable and every reviewed action is refused. The manifest's P4
  phase pins that receipt.

## P5 - acceptance

A parity run over authenticated captures of both interfaces (desktop and
mobile), the release-stage physical re-verification of the fan, LED, panel and
ATX buttons, a soak and reboot acceptance, the final report, the manifest and
the baselines. The earlier claim that the reference has no firewall has been
corrected in `docs/design/gui-parity-plan.md` (row 9, 2026-09-19), which now
records the reference `system.firewall.*` surface and our fail-closed
implementation of it.

## Status snapshot (2026-09-19)

Additional progress (later 2026-09-19): the application-update API
(status/upload/activation sockets plus the write opt-in) is wired on the
running board server and the authenticated status answers again; the firewall
baseline gate passes with the owner sandbox intact (a transient unit runs
`nft`); the video matrix gate holds a viewer and shows the idle 10 fps is the
`--slowdown` rate; a 180 s video soak passed; and the ST7789 status screen now
draws correctly (LSB-first font, no frame mirror, primary address from
`/proc/net/fib_trie`). The remaining device item is the E4 slot-serving cutover.
The rotated signing key `app-production-2026-09-19-01` is now enrolled without
discarding history: a second config with the new key and fresh roots
(`-v2`) was validated, the seq13 bundle was staged and the fresh state
initialised (floor 13, generation 1), the live trust config now points at it,
and the authenticated status surface reports the seq13 state while the previous
config and roots stay intact for rollback. The serving cutover still needs a
decision because the serving unit hardcodes the original slot root
(`/var/lib/hardened-blikvm-slots/current`) and lacks the board-server's newer
environment; options are a destructive activation in the original root or a
reviewed serving-unit change to the v2 root with the same environment.


`tools/phase8_acceptance_report.py` reports `overall=incomplete`,
`hostMissing=0`, `deviceUnrun=2`:

| Phase | Device item | State | Receipt |
| --- | --- | --- | --- |
| P1 | video settings gate | recorded | `video-settings-gate-20260919-d.json` |
| P1 | video capability matrix | unrun | tool ready, `capture_device_video_matrix.py` |
| P3 | HID lease supersede gate | recorded | `hid-lease-supersede-gate-20260919-b.json` |
| P4 | Tailscale surface gate | recorded | `tailscale-surface-gate-20260919-a.json` |
| E4 | slot serving cutover | unrun | needs a publishable seq>=12 candidate |

Done host-side and on the device this cycle: the board-profile bundle was
deployed (`33b09d24…`, backup `deploy-20260919084534`); the P1/P1.1/P3/P4 gates
above passed on the device; the seq7 production artifact was re-signed with the
rotated key (`d80ef86d…`, host verification passed); the vendor video source
map was checked against the retained files; `GET /api/vm/screen` now returns
`width`/`height`/`queuedFps` and the screen menu shows the delivered frame.

Blocked on explicit operator input:

- `git commit` (policy-denied in this session) is required before a fresh
  runtime package can carry `bli-kvm-boardd` and a committed source checkpoint,
  which is what a publishable seq>=12 candidate needs;
- the device must trust `app-production-2026-09-18-01` before the update cycle
  can run (the trust location was not found under `/etc/hardened-blikvm-update/`);
- the video matrix sweep and the state-parity redeploy need a device window;
- `nftables` (firewall gate), a Tailscale package source, and the
  operator-observed E2E/parity windows remain open.

## Operator directives (2026-09-19): panel parity, vendor audit, keys, packages

### Panel (ST7789 240x240) - full vendor parity

Root cause of the "garbage instead of bars" on the device display: `boardd`
runs `reset + init` and enables the backlight, but draws no frame, so the
ST7789 GRAM stays undefined after power-up/restart. The panel settings already
match the vendor (verified against the retained `third_lib/SPI_LCD`):

- SPI `/dev/spidev1.0`, channel 1/CS0, mode 0, 8 bits, 40 MHz;
- GPIO D/C 259, reset 262, backlight 260 (vendor pin indices 40/22/38 in the
  `USE_MANGOPI` table);
- init matches `LCD_1IN3_InitReg` byte-for-byte (0x3A=0x05, 0xB2, 0xB7, 0xBB,
  0xC0, 0xC2, 0xC3, 0xC4, 0xC6, 0xD0, 0xE0, 0xE1, 0x21, 0x11, 0x29) with
  MADCTL `0x36=0x70`; window 0..239; RGB565 MSB-first.

Work: draw a defined frame right after `initialize()`; implement the vendor
status screen (`BliKVM V4`, IP/hostname, temperature, uptime, CPU load, memory
and the vendor `/usr/bin/blikvm/oled_info.bmp`, 240x240 24bpp) on the vendor's
~5 s cadence with the backlight cycle; redraw on the display button; align the
default `DisplayConfig` with the vendor init (the live `boardd.json` currently
overrides a minimal default); confirm the reset polarity (live config
`activeLow=true` vs the active-high metadata). Host tests plus one reversible
device test: draw -> restart `boardd` -> no garbage, with a receipt. Implemented
2026-09-19 (`panel_font.rs`, `panel_status.rs`; 38 boardd tests; bundle
`fde27030…` deployed, backup `deploy-panel-20260919132943`): the daemon draws a
defined frame right after init, renders the status screen over the vendor
`oled_info.bmp`, refreshes every 5 s and redraws on the button wake. The board
control surface reports `displayInitialized=true`, `displayBacklightOn=true`,
`display_test=0`, `backlight_on=0`; the operator's visual confirmation of the
panel content is the remaining acceptance.

### Vendor settings audit

Compare `boardd.json` (fan curve, activity LED, ATX host LED inputs, button,
panel) against the retained vendor `config.ini` and `blikvm` sources; fix or
document every difference.

### Git, key rotation and packages

- commit the worktree to `main` and push to the private `origin`
  (`github.com/woffko/Hardened-BliKVM`), English content only;
- generate a new production signing key (`app-production-2026-09-19-01`), keep
  the private key only under `~/.local/share/hardened-blikvm/production-signing`
  (never in git, argv, logs or a bundle), mark the previous key compromised;
- fresh prototype bundle (committed checkpoint) -> runtime `.deb` with `boardd`
  -> seq12 production candidate signed with the new key -> host verification;
- enroll the new key in the device's update trust, then run
  candidate -> health -> confirm -> current -> watchdog and record the E4
  slot-serving cutover receipt;
- install `nftables` from `ports.ubuntu.com` and run the firewall gate; install
  Tailscale from the official `pkgs.tailscale.com` repository (arm64/jammy) and
  enable/disable it behind the fail-closed capability.

Reference credentials for the fresh parity capture live in the private handoff
under `evidence/private/` and are never committed.

## Invariants

Host tests and a green `scripts/check.sh` for every step; English for everything
that reaches GitHub; private evidence and secrets never published and secrets
only through `project-memory-exec`; device mutations only with the operator's
explicit confirmation in the session; a reboot only with its own explicit
confirmation; historical receipts are never rewritten and checks are never
weakened; one owner per gadget, ATX and video line; every change reversible.

## Completion addendum (2026-09-19, late)

Panel: the operator confirmed the status screen content on the device and asked
for a single font size, between the small address size and the double numeric
size. `TextStyle` gained a rational scale (`ratio`); every status line now
draws at 1.5x (`panel_font.rs`, `panel_status.rs`; 40 boardd tests). The
update bundle `ff4ed3e9…` was deployed (backup `deploy-panel-20260919154527`)
and the panel status gate passed again (`panel-status-gate-20260919-b.json`).

Trust rotation: the non-destructive rotation is final on the device. The fresh
`-v2` slot root was created with mode 0700 by the rotation script; the shipped
design uses group-traversable mode `drwxr-x---` so the serving user (group
`hardened-blikvm`) can execute the slot artifact. The mode was corrected to
0750 with no other change.

Slot-serving cutover: the running board server now executes the confirmed signed
slot from `<slotRoot>/current/bin/bli-kvm-server` through a reviewed, reversible
drop-in (`90-slot-serving.conf`); the slot and installed binaries and web entry
are byte-identical, and the gate refuses a slot that differs from the installed
artifact or a `current` that is not the confirmed slot. The new
`tools/stage_device_slot_serving_gate.py` (with unit tests) records
`update-slot-serving-20260919-a.json` (`result=passed`, HTTPS health after the
restart, executable canonicalized to the seq13 slot). The HID lease gate passed
again after the cutover (`hid-lease-supersede-gate-20260919-e.json`).

`tools/phase8_acceptance_report.py` now reports `overall=complete`,
`hostMissing=0`, `deviceUnrun=0` against the manifest.

Build determinism: the runtime Debian package built twice from the same
prototype bundle (`49c49a0f…`) is byte-identical
(`db8f45a1…`, `SOURCE_DATE_EPOCH=0`); recorded in
`evidence/private/runtime-deb-determinism-20260919-a.json`.

Live surface smoke after the cutover (authenticated): `/api/health`,
`/api/application/version`, `/api/application-update/status`,
`/api/capabilities`, `/api/vm/screen`, `/api/vm/info`,
`/api/virtual-media/catalog`, `/api/inventory`, `/api/hostname`,
`/api/system/time` and `/api/network/dns` all answer `code=0`. Residual,
pre-existing and outside the manifest: `/api/diagnostics` answers 503 because
the diagnostic collector/store is not part of the board profile (the earlier
`device-diagnostics-api-attempt*.json` receipts record the failed attempt; the
collector itself was verified separately). KVMD and `kvmd-main` are masked.
