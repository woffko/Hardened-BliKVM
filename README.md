# BLi-KVM

BLi-KVM is a security-focused KVM-over-IP stack for BliKVM v4 H616 hardware.
It is being built as a GPL-3.0-only Rust control plane with a React/TypeScript
web interface derived from the proven Hardened NanoKVM product shape.

The target is not a themed copy of the existing BliKVM application. BLi-KVM
uses a platform-neutral core and explicit hardware adapters so that security,
API, update, and recovery invariants remain testable across platforms.

The canonical GitHub repository is
[`woffko/Hardened-BliKVM`](https://github.com/woffko/Hardened-BliKVM).
Private device credentials, runtime configuration and hardware receipts are
excluded from Git. This source repository is not a complete device image.

## Status

Beta. A pre-release carries an SD card image for the BliKVM v4 (Allwinner
H616), a signed application bundle for a board that already runs one, and
(since 0.1.0-beta.3) a signed RAW system bundle that gives such a board the
image's system; see the
[releases](https://github.com/woffko/Hardened-BliKVM/releases) for the files,
how to write the card or update a board, the first boot and the known
limitations.
Everything was verified on one board. Expect rough edges and report what you
find.

## What it does

- **Remote desktop**: keyboard and mouse (absolute and relative; on a target
  whose desktop spans several monitors, the absolute mouse kept on the
  captured one by a layout set in the Mouse menu,
  [`docs/design/screen-area-v1.md`](docs/design/screen-area-v1.md)), a touch
  layout for phones with an on-screen keyboard and a row of extra keys, the
  ATX power buttons, and virtual media (an ISO as CD-ROM, disk images as mass
  storage).
- **Video**, chosen in the Screen menu: MJPEG (the capture passed through),
  H.264 and VP8 (libvpx) over WebRTC with a choice of quality, and H.264
  over the QUIC transport. H.264 is encoded by the H616's video engine
  (H.264 HW) or by x264 (H.264 SW), one choice for the whole board; where
  the engine cannot be had, x264 encodes and the menu says why
  ([`docs/design/h616-ve-h264-encoder.md`](docs/design/h616-ve-h264-encoder.md)).
  The encoders work only while someone watches.
- **Upscaling**, in the Screen menu (Auto by default, or Off, and a
  sharpness): where the browser draws with a GPU, a picture shown larger
  than its size is drawn sharply (a deringing pass for the capture's JPEG
  and SGSR 1) in every view, filling the window or at the target's
  resolution, a pinch on a phone kept smooth; a watchdog turns it off where
  frames are lost to it, and a browser that draws in software draws as
  before ([`docs/design/screen-upscaling-v1.md`](docs/design/screen-upscaling-v1.md)).
- **Quality monitor**: QM on the toolbar opens a window over the picture
  with what arrives (speed, delay, frames a second, codec, bitrate), and in
  its Extended level the transport's, the board's encoder's and the
  browser's decoder's numbers, the upscaler's and how much of a second the
  page's own picture code takes; on a phone it spans the screen
  ([`docs/design/h264-engine-choice-and-quality-monitor-v1.md`](docs/design/h264-engine-choice-and-quality-monitor-v1.md)).
- **Transport**: the standard one (HTTPS, WebSocket, WebRTC), or QUIC
  (WebTransport on UDP 443) for the keyboard, the mouse and the H.264
  picture: a group of pictures on a stream of its own, what the path loses
  sent again, and the H.264 bitrate fitted to the narrowest path of the
  sessions that watch. QUIC has a switch on the board, off by default, and a
  choice in each browser (Settings > System > Network > Transport,
  [`docs/design/quic-transport-v1.md`](docs/design/quic-transport-v1.md)).
- **Settings** as in Hardened NanoKVM: device, appearance (light, dark and
  system theme, accent colours), network (IPv4, IPv6, DNS, Wi-Fi with a choice
  of the main connection), a firewall with four modes and a confirmation that
  rolls back by itself, time, system log, Tailscale, account, session lock,
  and the board's display going dark after a time without activity (its
  button lights it again).
- **Updates**: signed application slots with a health check and automatic
  rollback, and signed RAW system updates behind their own switch, which
  keep the board's accounts, keys, network settings and switches and write
  the old system back if the new one does not start.
- **Target agent** (Windows, off by default; Settings > Device): with its
  switch on, the board's mouse device carries a vendor collection that a
  small agent on the target talks to, and a read-only drive carries the
  agent. The page then knows the target's Windows, its monitors (the
  absolute mouse kept on the captured one without a layout set by hand) and
  its keyboard layouts, and at the operator's action under remote control
  switches the layout (also to the letters typed on a phone), changes the
  monitors' mode (undone after 15 s unless kept), passes clipboard text both
  ways and locks the session. "Start the agent" types its start line on the
  target, also as "Update the agent" over an agent that is not the board's
  version, which then leaves; the agent's own menu can start it at
  sign-in, from a copy that follows the agent started last. Only the agent
  of the session at the console serves the board. The quality monitor's
  Agent row says its state
  ([`docs/design/target-agent-v1.md`](docs/design/target-agent-v1.md)).
- **Administration features** that stay off until they are switched on: web
  terminal, scripts, remote ISO download, PicoClaw, RAW system updates. Each
  switch is enforced below the web server
  ([`docs/design/admin-features-v1.md`](docs/design/admin-features-v1.md)).

Not there: audio, and updates fetched from a feed (they are uploaded). The
time, NTP, remote system log and autostart script settings are shown
read-only.

## How it is put together

The vendor's KVMD services are masked or disabled; the board runs these
processes instead, each as its own systemd unit with a sandbox:

| Process | Runs as | What it does |
| --- | --- | --- |
| nginx | root, workers as `www-data` | TLS on TCP 443 in front of the web server |
| `bli-kvm-server` | `hardened-blikvm-server`, from the active signed slot | login, sessions, the API and its policy, the web interface, WebRTC signalling, the QUIC gateway's link; no device access of its own |
| board owner (`bli-kvm-control-broker`) | root | the USB gadget (keyboard, mouse, mass storage), the target agent's channel and drive, the ATX lines, network and firewall changes, the feature switches; it takes only fixed, typed requests over a Unix socket |
| `bli-kvm-boardd` | root | the fan and the board's peripherals |
| uStreamer and the MJPEG proxy | `hardened-blikvm-video` | the capture, and its MJPEG stream for the server |
| video encoders (VP8, H.264) | dynamic users | encode for Janus and for the QUIC gateway while someone watches; H.264 with the video engine or with x264 |
| encoder driver (`hardened-blikvm-ve-enc`) | kernel module, loaded by its own unit | gives the H.264 encoder the video engine, to one opener at a time |
| Janus | dynamic user | WebRTC media, streaming plugin only |
| QUIC gateway (`bli-kvm-quic-gateway`) | `hardened-blikvm-quic` | QUIC and TLS on UDP 443 while its switch is on; it holds no authority and passes on what the server allows |
| update supervisor and its helpers | `hardened-blikvm-update` | verify, stage and activate slots, check their health, roll back |
| administration and download helpers | socket-activated, per request | the features behind the switches |

```text
Browser
  |  HTTPS, WebSocket (TCP 443)      WebRTC media (UDP)      QUIC (UDP 443, optional)
  v                                       ^                        ^
nginx --> bli-kvm-server ---------> Janus <-- RTP --+              |
              |      ^------------------------------|--- link ---- QUIC gateway
              |                                     |                  ^ frames
              v  typed requests                video encoders ---------+
        board owner                                 ^ MJPEG
   (USB gadget, ATX, network, firewall)        uStreamer (capture)
```

On the target, the optional agent (`hardened-blikvm-agent.exe`, built
reproducibly from `crates/bli-kvm-target-agent` by
`scripts/build-target-agent.sh`) runs as the signed-in user, finds the
board's collection and answers only the calls the protocol names
(`crates/bli-kvm-agent-protocol`); it opens no network connection and runs
no command.

Design notes are under [`docs/design`](docs/design): the QUIC transport, the
firewall contract, the administration features, the video encoders (software,
and H.264 on the video engine with its driver), the engine choice and the
quality monitor, the RAW system update, the screen's upscaling, the target
agent. Plans, audits and what was shown on the test board are under
[`docs/baselines`](docs/baselines); the most recent are
[`audit-20261017-review.md`](docs/baselines/audit-20261017-review.md),
[`audit-20261014-target-agent.md`](docs/baselines/audit-20261014-target-agent.md),
[`audit-20261009-upscaling-phases.md`](docs/baselines/audit-20261009-upscaling-phases.md),
[`audit-20261008-upscaling-gate0.md`](docs/baselines/audit-20261008-upscaling-gate0.md),
[`audit-20261004-review.md`](docs/baselines/audit-20261004-review.md)
and [`audit-20261004-h264-choice-quality-monitor.md`](docs/baselines/audit-20261004-h264-choice-quality-monitor.md).
[`docs/security-boundaries.md`](docs/security-boundaries.md) and
[`docs/licensing.md`](docs/licensing.md) hold the security model and the
licences. [`docs/architecture.md`](docs/architecture.md),
[`docs/implementation-plan.md`](docs/implementation-plan.md) and
[`docs/feature-matrix.md`](docs/feature-matrix.md) were written during the
transition from the vendor's software and describe that stage; where they
differ from the design notes, the design notes are newer.

## Development

```sh
./scripts/check.sh
cargo run -p bli-kvm-server
scripts/build-aarch64.sh
```

The check runner uses `/mnt/d/debug/blikvm/hardened-blikvm/pnpm-store` for
large temporary frontend dependencies. Override it with the task-specific
`BLI_KVM_PNPM_STORE` variable when that path is unavailable.

The development server binds to `127.0.0.1:18080` and refuses non-loopback
binds. Device exposure must use a separately hardened TLS reverse proxy. Do not
place device images, credentials, private evidence, or extracted root
filesystems in this repository.
