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

Alpha. A pre-release carries an SD card image for the BliKVM v4 (Allwinner
H616) and a signed application bundle for a board that already runs one; see
the [releases](https://github.com/woffko/Hardened-BliKVM/releases) for the
files, how to write the card, the first boot and the known limitations.
Everything was verified on one board. Expect rough edges and report what you
find.

## What it does

- **Remote desktop**: keyboard and mouse (absolute and relative), a touch
  layout for phones with an on-screen keyboard and a row of extra keys, the
  ATX power buttons, and virtual media (an ISO as CD-ROM, disk images as mass
  storage).
- **Video**, chosen in the Screen menu: MJPEG (the capture passed through),
  H.264 (x264) and VP8 (libvpx) over WebRTC with a choice of quality, and
  H.264 over the QUIC transport. The encoders work only while someone
  watches.
- **Transport**: the standard one (HTTPS, WebSocket, WebRTC), or QUIC
  (WebTransport on UDP 443) for the keyboard, the mouse and the H.264
  picture. QUIC has a switch on the board, off by default, and a choice in
  each browser (Settings > System > Network > Transport,
  [`docs/design/quic-transport-v1.md`](docs/design/quic-transport-v1.md)).
- **Settings** as in Hardened NanoKVM: device, appearance (light, dark and
  system theme, accent colours), network (IPv4, IPv6, DNS, Wi-Fi with a choice
  of the main connection), a firewall with four modes and a confirmation that
  rolls back by itself, time, system log, Tailscale, account, session lock.
- **Updates**: signed application slots with a health check and automatic
  rollback, and signed RAW system updates behind their own switch.
- **Administration features** that stay off until they are switched on: web
  terminal, scripts, remote ISO download, PicoClaw, RAW system updates. Each
  switch is enforced below the web server
  ([`docs/design/admin-features-v1.md`](docs/design/admin-features-v1.md)).

Not there: audio, and updates fetched from a feed (they are uploaded). The
time, NTP, remote system log, autostart script and OLED settings are shown
read-only.

## How it is put together

The vendor's KVMD services are masked or disabled; the board runs these
processes instead, each as its own systemd unit with a sandbox:

| Process | Runs as | What it does |
| --- | --- | --- |
| nginx | root, workers as `www-data` | TLS on TCP 443 in front of the web server |
| `bli-kvm-server` | `hardened-blikvm-server`, from the active signed slot | login, sessions, the API and its policy, the web interface, WebRTC signalling, the QUIC gateway's link; no device access of its own |
| board owner (`bli-kvm-control-broker`) | root | the USB gadget (keyboard, mouse, mass storage), the ATX lines, network and firewall changes, the feature switches; it takes only fixed, typed requests over a Unix socket |
| `bli-kvm-boardd` | root | the fan and the board's peripherals |
| uStreamer and the MJPEG proxy | `hardened-blikvm-video` | the capture, and its MJPEG stream for the server |
| video encoders (VP8, H.264) | dynamic users | encode for Janus and for the QUIC gateway while someone watches |
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

Design notes are under [`docs/design`](docs/design): the QUIC transport, the
firewall contract, the administration features, the software video encoders,
the RAW system update. Plans, audits and what was shown on the test board are
under [`docs/baselines`](docs/baselines); the most recent are
[`audit-20261002-quic-transport.md`](docs/baselines/audit-20261002-quic-transport.md)
and [`audit-20260929-review.md`](docs/baselines/audit-20260929-review.md).
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
