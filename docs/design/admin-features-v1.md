# Administration features v1 (terminal, scripts, image download, PicoClaw)

Status: design for plan phase A (`docs/baselines/work-plan-20260926.md`).

Hardened BliKVM administers local hardware, so the reference's administration
features are offered, but each risky one is switched off by default and the
switch is enforced below the web server. This note fixes the switch model, the
privilege boundary of every feature and the reference contract we keep.

## Feature switches

| Feature | Switch | Default | Reference control |
| --- | --- | --- | --- |
| Terminal (KVM shell, serial port via `picocom`) | `terminal` | off | `GET/POST /api/vm/terminal/enabled` |
| Scripts (list, upload, run, delete) | `scripts` | off | none in the reference (always on there) |
| Remote ISO download | `remote_download` | off | `GET/POST /api/download/image/remote/enabled` |
| PicoClaw assistant | `picoclaw` | off | runtime install/start in the reference |

- The board owner persists the switches in
  `/var/lib/hardened-blikvm-gadget/admin-features.json` (root, 0644, atomic
  replace, unknown keys refused); the directory is readable only by root and
  the `hardened-blikvm` group, which includes the web server. A missing or unreadable
  file means every switch is off.
- The owner exposes them through the control protocol v2
  (`AdminFeatureState`, `AdminFeatureUpdate`); only an authenticated session
  with CSRF may change them, and switching the terminal on needs the explicit
  confirmation the reference shows.
- The capability manifest advertises each feature as `read_write` when the
  build supports it, and the GUI reads the switch state separately, so a
  switched-off feature stays visible in Settings > Appearance but hidden in the
  menu bar, exactly as in the reference.
- Enforcement happens twice: the server refuses the feature's routes while its
  switch is off, and the helper that does the work re-reads the switch file
  and refuses on its own. A GUI that ignores the state gains nothing.

## Privilege boundaries

The network-facing server stays unprivileged. Privileged work runs in helpers
that authenticate the server by `SO_PEERCRED`, use a closed request protocol
and are socket-activated per connection.

- **Admin helper** (`hardened-blikvm-admin@.service`, root, per connection):
  - terminal: opens a PTY with `/bin/login -f root`-equivalent root shell
    (`/bin/bash -l`), relays data and window-size frames, ends with the web
    session or on disconnect; one terminal per operator session;
  - scripts: stores uploaded scripts under `/var/lib/hardened-blikvm-admin/scripts`
    (root, 0700; names `[A-Za-z0-9._-]{1,64}`, 256 KiB each, 64 scripts),
    lists, deletes, and runs one with a bounded runtime and output
    (foreground: 60 s, 256 KiB captured; background: detached transient
    unit with a 1-hour limit).
- **Image download helper** (`hardened-blikvm-image-download@.service`, the
  same binary with `--download`, running as the web-server account with no
  capabilities, write access only to the image store and the media catalog):
  - it is the only unit with outbound network access (the web server may
    reach localhost alone); it may not reach the board's own loopback,
    link-local or multicast addresses, except a local DNS stub;
  - it accepts only `http`/`https` URLs without credentials whose last path
    segment is an `.iso` name the store accepts, one download at a time
    (a lock next to the status file);
  - `curl` runs with a cleared environment and the reference limits (HTTP(S)
    redirects only, 5 redirects, 2 h, a stall limit and an 8 GiB file limit)
    into a hidden temporary file; progress goes to
    `/run/hardened-blikvm-download/status.json` in the reference shape;
  - a finished file must carry the ISO 9660 magic, then it is hashed,
    published atomically (0644) and recorded in the media catalog as a
    CD-ROM or read-only removable image; any failure removes the temporary
    file and reports `failed` for 30 seconds;
  - it re-reads the switch while downloading and stops when it is turned off.
  Local image upload stays in the web server (`POST /api/download/file`) and
  needs no switch; the Download menu that carries both is shown whenever the
  image store is configured.
- **PicoClaw**: the reference downloads and runs an agent runtime. On this
  board the runtime runs as its own unprivileged unit, reaches the KVM only
  through the web API with an operator-issued token, and its model API key is
  stored by the owner and never returned to the GUI. It is ported last.

## Reference contract kept

- Terminal: `POST /api/vm/terminal/unlock` (password re-entry, rate limited,
  one-time ticket), then `GET /api/vm/terminal?ticket=...` (WebSocket; text
  frames are keystrokes, binary frames carry the window size). The serial
  terminal page types a `picocom` command into the same shell, so `picocom` is
  installed on the board.
- Scripts: `GET /api/vm/script`, `POST /api/vm/script/upload`,
  `POST /api/vm/script/run`, `DELETE /api/vm/script`, plus the existing
  autostart views.
- Download: `POST /api/download/image`, `GET /api/download/image/status`,
  `GET /api/download/image/enabled`, `GET/POST /api/download/image/remote/enabled`.

## Tests and evidence

Unit tests cover switch persistence and refusal on both sides, the helper
protocols, name and size limits, and the ticket flow. Device evidence: each
feature refused with its switch off, working with it on, and off again after a
fresh image.
