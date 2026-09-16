# Board serving cutover: KVMD retired, board profile on the main surface

Date: 2026-09-16. Operator approval: remove KVMD and serve our stack on the main
port. Receipt: `evidence/private/board-serving-cutover-20260916-a.json`.
Related: `evidence/private/kvmd-removal-20260916-a.json`,
`evidence/private/phase8-acceptance-20260916-c.json`.

## Result

The board's main surface (nginx on 443) now serves the
Rust board profile. KVMD is stopped, disabled and masked. The composite gadget
`hardened` (keyboard, absolute mouse, relative mouse, mass storage) is bound to
`musb-hdrc.2.auto` with the reviewed image attached. Ports:

- plain HTTP -> `308` redirect to HTTPS (kept: session cookies are
  `Secure`, so login over plain HTTP would be rejected);
- `https://<board>/` -> `200`, `<title>Hardened BliKVM</title>`;
- `/api/capabilities` -> HID `read_write`, Power `read_write`, virtual media
  `read_write`, platform `bli-h616-pikvm`, board `BliKVM v4 H616`.

Units:

- active/enabled: `hardened-blikvm-board-owner.service`,
  `hardened-blikvm-board-server.service`, `hardened-blikvm-nginx.service`;
- inactive/disabled: `hardened-blikvm-server.service` (vendor package-backed
  server), `hardened-blikvm-server-current.service`,
  `hardened-blikvm-platform-broker.service`,
  `hardened-blikvm-video-proxy.service`,
  `hardened-blikvm-update-watchdog.timer`;
- inactive/masked: `kvmd.service`, `kvmd-main.service`, `kvmd-otg.service`,
  `kvmd-webterm.service`.

## Defects the cutover exposed

1. `hardened-blikvm-server.service` requires
   `hardened-blikvm-platform-broker.service`, which requires `kvmd.service`.
   Starting the package-backed server therefore resurrected KVMD, and the
   enabled signed-slot server `Conflicts=`-stopped the board server. An empty
   `Requires=`/`Wants=` reset in a drop-in did not clear the dependency on this
   systemd, and a masked requirement failed the whole transaction. The board
   profile now uses `hardened-blikvm-board-server.service`, which requires only
   the board owner and is part of the board bundle and `verify-systemd-units`.
2. The owner creates its control socket with its own umask, so the unprivileged
   server user could not connect until the owner ran with
   `Group=hardened-blikvm` and `UMask=0007`.
3. The media engine enforces its command cooldown, so `select`/`set_connected`
   must be spaced the way the GUI spaces them.

## Retained update path

The signed-slot serving arrangement and the update watchdog timer are idled,
not removed: the update supervisor config, trust, sockets and slots stay in
place, and the unit files and enablement states are backed up under
`/var/lib/hardened-blikvm/backup-20260916/serving/`. Re-enabling them is in the
cutover receipt. The follow-up is to package the current board build as a
signed application slot so the retained model serves it again.
