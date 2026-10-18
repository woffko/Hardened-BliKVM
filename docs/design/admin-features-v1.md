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

## The QUIC transport's switch (plan of 2026-10-01)

"QUIC on this device" (Settings > Network > Transport,
`docs/design/quic-transport-v1.md`) is a switch of the same kind and is off
by default, but it is not a key of the switch file above. Like the SSH
switch it is the enablement of a unit: on enables and starts
`hardened-blikvm-quic.service`, off stops and disables it, and the state is
read back from systemd, so one state survives a reboot and the switch file,
which refuses unknown keys, keeps its format for older slots.

- Only an authenticated session with CSRF may change it
  (`POST /api/transport/quic/enable`, `.../disable`); switching it on asks
  first and names the UDP port.
- It is enforced twice, below the web server: while it is off the unit does
  not run and nothing listens on the port, and the server hands out no
  session token because no gateway is on its link
  (`the_switch_is_enforced_twice`).
- The gateway is a process of its own under its own user, with one
  capability (binding the port) and no authority: it redeems the page's
  token at the server and passes requests on. What a session may do is
  decided by the server for the web session behind it.

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
  same binary with `--download`, running with no capabilities as
  `hardened-blikvm-download`, an account of its own in no group another
  account shares; the web server is the one other member of its group. It
  ran as the web-server account until the security review of 2026-10-18,
  N1):
  - it is the only unit with outbound network access (the web server may
    reach localhost alone); it may not reach the board's own loopback,
    link-local or multicast addresses, except a local DNS stub, nor the
    board owner's, the admin helper's or the update services' sockets;
  - it accepts only `http`/`https` URLs without credentials, with an ASCII
    host name and none of `[ ] { }` anywhere (so no IPv6 literal host),
    whose last path segment is an `.iso` name the store accepts, one
    download at a time (a lock next to the status file, 0640, which the web
    server probes);
  - before forwarding a download, the web server empties the drop directory
    `.downloads` in the image store (the server's, 0750, the helper's group)
    and prepares in it one empty file for the image (the server's, 0660, the
    helper's group). The unit binds that directory at
    `/run/hardened-blikvm-download/drop` and can write nothing else of the
    store, the catalog or `/etc/hardened-blikvm-gadget`; it passes through
    `/var/lib/hardened-blikvm-gadget` only to read the switches (an ACL
    entry `u:hardened-blikvm-download:--x`);
  - `curl` runs with a cleared environment, URL globbing off and the
    reference limits (HTTP(S) only, 2 h, a stall limit), except that a file
    may be as large as an upload (64 GiB, the reference's is 8 GiB), into the
    prepared file; progress goes to `/run/hardened-blikvm-download/status.json`
    in the reference shape;
  - the helper follows redirects itself, up to 5: it resolves each hop's
    host, pins curl to those addresses (`--resolve`) and reads the next hop
    from curl's `--write-out`. Once a hop was on a public address, a hop into
    a private, CGNAT, unique-local, loopback or link-local one is refused
    before anything is sent to it; the operator's own URL may name a private
    address (security review of 2026-10-18, MED-05, N8);
  - a finished file must carry the ISO 9660 magic, and the helper reports it
    `finished` (read as idle by the page); a failure empties the file and
    reports `failed` for 30 seconds;
  - the web server then checks the file again (still its own, one link,
    within the size bound, the ISO 9660 magic), hashes it, looks at the
    switch and at the attached image once more, and publishes it as it does
    an upload: renamed into the store (0644) and recorded in the media
    catalog as a CD-ROM or read-only removable image. Until then the status
    reads `in_progress` at 100 %; a refused publish reads `failed` for 30
    seconds. A server restarted during a download takes the publish up again
    from the helper's status;
  - it re-reads the switch while downloading and stops when it is turned off.
  The application slot that prepares and publishes comes with the system
  that runs the helper under its own account: an older system's helper
  publishes on its own and leaves the prepared file empty, which the server
  removes; an older slot on the newer system finds its downloads refused
  ("the web server prepared no file for this download").
  Local image upload stays in the web server (`POST /api/download/file`) and
  needs no switch; the Download menu that carries both is shown whenever the
  image store is configured.
- **PicoClaw** (the reference's AI assistant, runtime v0.2.8):
  - install: the download helper fetches the pinned upstream release archive
    (`picoclaw_Linux_arm64.tar.gz` of sipeed/picoclaw v0.2.8) and accepts it
    only when its SHA-256 matches the digest pinned in the source (the
    reference fetches its checksum from the same CDN as the archive); only
    the `picoclaw` member is unpacked into
    `/var/lib/hardened-blikvm-picoclaw-runtime` (the download helper's
    account, 0755),
    so the runtime cannot modify its own binary. Uninstall removes the binary
    and the runtime's home;
  - runtime: `hardened-blikvm-picoclaw.service` runs `picoclaw gateway` as
    the dedicated `hardened-blikvm-picoclaw` user with a sandbox in which only
    its home (`/var/lib/hardened-blikvm-picoclaw`) is writable. It has network
    access for the model API but not to link-local or multicast addresses. Its
    agent tools (shell, files) act inside that sandbox, not as root. A root
    pre-start check refuses to start while the switch is off, and the admin
    helper starts and stops the unit (and stops it when the switch is turned
    off);
  - configuration: a helper running as the runtime's user
    (`hardened-blikvm-picoclaw-api@.service`, no network) writes the
    runtime's `config.json` with the reference defaults, the model entry and
    the agent profile, and owns `.security.yml` (the model API key and the
    gateway token), which it renders from its own 0600 store. The API key is
    written only there, because the runtime reads it; it is never returned to
    the web server or the GUI. The same helper lists, reads and deletes chat
    sessions with bounded sizes;
  - KVM access: the runtime's MCP client calls
    `http://127.0.0.1:18080/internal/picoclaw/mcp` (screenshot and HID
    actions) with an internal token the server generates and keeps in its
    state directory. The server accepts those calls only directly on
    loopback (not through the TLS proxy, which always adds
    `X-Forwarded-For`), with the token and the switch on. Actions are sent as
    HID reports through the HID lease of the operator session that opened the
    assistant, so the assistant can only act while that operator's KVM page
    holds the keyboard and mouse. While the assistant session holds its lock
    the server drops manual HID input, as the reference does;
  - chat: the browser's `/api/picoclaw/gateway/ws` is relayed to the local
    gateway (`ws://127.0.0.1:18790/pico/ws`) with the gateway token, one
    assistant session at a time (30-minute lock renewed by traffic).

## Other helper duties (plan phase B)

- **Tailscale** (admin helper, root, no switch as in the reference): the
  package is installed from Tailscale's apt repository through a private
  source list and list directory, signed-by the repository key we ship
  (fingerprint 2596A99EAAB33821893C0A79458CA832957F5868), so the system's apt
  configuration is untouched; start and stop enable or disable
  `tailscaled`, up passes `--accept-dns=false`, and the login URL is read
  from the daemon's status while a detached `tailscale login` waits for the
  operator. The memory limit and swap settings of the reference are not
  offered: they exist for its 256 MB board.

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
