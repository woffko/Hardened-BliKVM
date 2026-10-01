# QUIC transport v1

Status: design for `docs/baselines/work-plan-20261001-quic-transport.md`,
written in its phase Q0. Decision of Q0: **go** (the evidence is in "Phase Q0"
below). Nothing here is implemented yet.

The operator's request: a QUIC transport modelled on the RustAdmin client's,
chosen in Settings next to today's transport ("Standard": HTTPS for the page,
the API, MJPEG and every HID report, WebRTC through Janus for VP8 and
H.264), which stays the default.

## Reference

RustAdmin (`woffko/rustadmin`, branch `experimental`):
`docs/QUIC_PROTOCOL_V1.md`, `docs/QUIC_PROTOCOL_V2.md`,
`docs/QUIC_VPN_TRANSPORT.md`,
`src/quic_transport.rs` and the transport of its `hbb_common`. It is AGPL;
this project is GPL-3.0-only. Behaviour and numbers are taken over, code is
not.

Taken over:

| From the reference | Here |
| --- | --- |
| Independent reliable streams for control and input | the control and the input stream |
| Delta frames as DATAGRAMs, never retransmitted; key frames reliable | the same; each key frame on a stream of its own |
| Fixed-width headers, big-endian, validated before memory is taken; unknown types, flags and sizes refused | the frame headers below |
| Fragments out of order and exact duplicates accepted, conflicting ones refused, pending frames bounded | the receiver below |
| Reassembly: 120 ms without progress, 1 s hard lifetime for a delta frame; the receive time taken after the datagram is read | the same numbers |
| One outstanding key-frame request per connection, retries after 1, 2 and 4 s, then 10 s without one; a newer key frame clears it | the same schedule |
| The request names the stream and the last accepted frame; the host makes one key frame and restarts nothing; a key frame on its way answers further requests | the same |
| A delta frame is held back until the key frame it refers to has arrived | the reference id in every header |
| TLS 1.3 only, no 0-RTT, a pinned certificate, application authentication on top | TLS 1.3, no 0-RTT, the certificate hash, the session token |
| Fallback only for an unreachable path or a timeout, never after a certificate, authentication or protocol error | the failure table below |
| Datagram size from the path's live limit with a ceiling that survives a VPN | the live limit, at most 1200 bytes |
| Connect timeout 5 s, keep-alive 5 s | the same |
| Transport chosen in the settings; path, protocol, round-trip time, loss and key-frame requests shown | Settings > Network > Transport and its status line |

Not taken over: rendezvous, relay and NAT traversal; device pairing and
device keys (the web login is the identity); audio, clipboard and file
transfer; several protocol versions side by side (the page and the board are
updated together: one version, checked at connect); mouse movement as
datagrams (HID reports stay ordered on the input stream under the lease, see
"Input").

## Phase Q0: what was proven

### Crates

Chosen: **`wtransport` 0.7.2** (MIT OR Apache-2.0), the WebTransport server
over HTTP/3, on `quinn` 0.11.12, `rustls` 0.23.45 with `ring` 0.17.14, and
`rcgen` 0.14.10 for the certificate.

| | `wtransport` | `h3` + `h3-quinn` + `h3-webtransport` + `rcgen` |
| --- | --- | --- |
| Crates built for aarch64 | 110 | 67 |
| Of them new to the workspace | 68 | 28 |
| Last release | 2026-08-11 (three releases in 2026) | 2025-05-06, called experimental by its authors |
| Server API | sessions, streams, datagrams, address validation, request refusal, configuration reload | lower level; sessions and datagrams assembled by the caller |
| Client for tests | yes, with certificate hashes | no |

`wtransport` costs 40 more crates (URL parsing with the Unicode tables, an
X.509 parser, the native certificate store) that a server does not need and
its features cannot leave out. It is chosen all the same: browsers changed
their WebTransport handshake during 2025 and 2026 (Safari arrived in March
2026) and only this library has kept up, and its narrower API leaves less of
the protocol to this project. The whole stack is linked only into the
gateway, a process of its own with nothing but its UDP port.

Licences of the 68 new crates, all declared: MIT OR Apache-2.0 (and its
spellings) 37, Unicode-3.0 18, MIT 5, ISC 2, Apache-2.0 OR ISC OR MIT 2,
BSD-2-Clause 1, Apache-2.0 AND ISC 1 (`ring`), MIT OR Apache-2.0 OR Zlib 1,
Zlib OR Apache-2.0 OR MIT 1. `tools/license_inventory.py` reports no package
without a licence.

Direct dependencies the gateway will add to the workspace:

| Crate | Licence | Used for |
| --- | --- | --- |
| `wtransport` (features `quinn`, `ring`, `self-signed`) | MIT OR Apache-2.0 | the WebTransport endpoint; it re-exports `quinn` for the transport settings and counters, and makes the certificate through `rcgen` |

The proof endpoint built for the host (2.7 MiB) and, with the project's
recipe for C code (clang against musl 1.2.5 headers, `rust-lld`), statically
for aarch64 (2.3 MiB, accepted by `scripts/verify-aarch64-elf.sh`).

### Proof on the test board

A throw-away echo endpoint (one stream echoed, datagrams echoed, a datagram
source) ran on the board as a transient unit with a dynamic user and only
`CAP_NET_BIND_SERVICE`, and was removed afterwards (unit gone, file deleted,
port closed). Headless Chromium 149 on the development host loaded the
board's own page and opened the session with `serverCertificateHashes`.

It ran on **UDP 443**, not on a high port as the plan said: the page's
Content-Security-Policy allows connections to `'self'` only, and
`https://<board>/` on UDP 443 is the page's own origin, while any other port
is not. This is also why the plan's choice of UDP 443 stays: no policy
changes.

| Run (60 s, datagrams of 1100 bytes) | Sent | Lost | Out of order | Delay variation, median / 95% / 99% | Endpoint CPU | Peak memory |
| --- | --- | --- | --- | --- | --- | --- |
| 8 Mbit/s, 30 bursts a second | 54560 | 0 | 0 | 1.0 / 3.8 / 5.5 ms | 9.6% of a core | 2.4 MiB |
| 8 Mbit/s, 30 bursts a second, again | 54560 | 0 | 0 | 0.8 / 3.7 / 5.9 ms | 9.2% | 2.4 MiB |
| 8 Mbit/s, each datagram on its own timer | 54546 | 2 (0.004%) | 0 | 0.7 / 3.7 / 7.6 ms | 20.3% | 2.4 MiB |
| 2.5 Mbit/s, 30 bursts a second | 17050 | 22 (0.129%) | 0 | 0.4 / 1.3 / 9.7 ms | 3.7% | 2.4 MiB |
| 2.5 Mbit/s, again | 17050 | 10 (0.059%) | 0 | 0.2 / 0.5 / 4.4 ms | 3.7% | 2.4 MiB |
| 2.5 Mbit/s, again | 17050 | 0 | 0 | 0.2 / 0.5 / 1.7 ms | 3.7% | 2.4 MiB |

The session was ready 22-25 ms after the request; a stream echo took 1.4 ms
(median) and a datagram echo 1.0 ms. Receipts:
`evidence/private/quic-spike-20261001*.json`.

What the runs say beyond the numbers:

- A frame's datagrams must leave together. Sent as 30 bursts a second, the
  way encoded frames arrive, 8 Mbit/s costs 9.6% of a core; woken once per
  datagram it costs 20.3%. The gateway therefore hands a frame's fragments
  to the endpoint in one go and runs on one thread (on the host a pool of
  four workers took 9.0% where one thread took 7.7%).
- Loss comes in bursts, a few times a minute at most on this network, and
  not at all in some minutes. The recovery path is needed even on a local
  network.
- Clocks must not be compared directly. The development host's clock ran
  8.3% slow during the runs (its time service was slewing it), so arrival
  times drifted 5 s against send times within a minute without any datagram
  being late (the board's clock was right; the times in the table are by
  the host's clock and so about 8% short). The delay variation above is what
  remains after the best straight line is taken out. The page therefore
  estimates the board's clock from its pings as an offset and a rate, and
  never from one sample.
- The session request carries `Origin`; the gateway can and will refuse a
  foreign one.

Not checked: Firefox and Safari (neither is installed on the development
host).

### Decision

| Condition of Q0.5 | Result |
| --- | --- |
| Session from the board's own page with certificate hashes in Chromium | yes |
| Datagram loss on the local network below 0.1% at 8 Mbit/s | 0, 0 and 0.004% |
| Endpoint within 15% of a core and 32 MiB at 8 Mbit/s | 9.6% and 2.4 MiB when frames are sent as bursts, which is what the gateway does; 20.3% when every datagram has its own timer, which it must not do |
| Static aarch64 build | yes |
| Licence inventory | passes |

**Go.**

## Threat model of the new listener

| Who | What they can try | What stops it |
| --- | --- | --- |
| Anyone who reaches UDP 443 | handshake floods, amplification, many connections | off by default; QUIC address validation before any state is kept; at most 8 connections; 5 s to present a token; the unit's memory limit; closed in the Restricted and Paranoid firewall modes |
| Anyone who reaches UDP 443 | malformed packets against the QUIC and TLS code | a memory-safe stack; a process of its own without files, devices or the owner's and broker's sockets; a crash ends QUIC only, HTTPS and the lease are untouched |
| A web page in the operator's browser | open a session to the board | it has no token: one is issued only to the authenticated session with its CSRF token, once, for 30 s; the gateway also refuses a foreign `Origin` |
| Someone on the path | pose as the board on UDP 443 | the certificate's hash comes over the HTTPS session; TLS 1.3 only |
| Someone who replays a captured token | reuse it | single use, 30 s, bound to the web session |
| A taken-over gateway process | act on the board | it can only speak the server link's closed protocol: redeem tokens it was given, pass lease requests of sessions it serves (the server applies the lease rules), read the frames every viewer gets. It cannot issue tokens, reach the owner or read a password |
| An authenticated page | exhaust the gateway or the encoder | bounded queues and message sizes; key-frame requests limited by the schedule and by the encoder's cooldown |

Tokens, their hashes, report contents and frame data never reach a log.

## Processes

- **`bli-kvm-quic-gateway`** (`hardened-blikvm-quic.service`, system user
  `hardened-blikvm-quic`), installed by the update-runtime package in
  `/usr/libexec/hardened-blikvm`. It runs only while the board's switch is
  on. One thread. Sandbox like the video proxy's, except that it may bind
  UDP 443 (`CAP_NET_BIND_SERVICE` as its only capability, and the unit
  admits no other port) and talk to the network on it.
- **Server link**: a Unix socket in the server's runtime directory that
  accepts only the gateway's user (peer credentials). The gateway connects
  and keeps one connection; it reconnects when the server restarts, and its
  QUIC sessions end with the server's.
- **Frame socket**: a Unix socket of the H.264 encoder unit, open to the
  gateway's group.
- Crate **`bli-kvm-quic-protocol`**: the messages of the page, of the server
  link and of the frame socket, with their limits; no dependency beyond
  `serde`. The gateway, the server and the encoder use it.

## Packaging and build

- The update-runtime package `0.1.0~dev35` installs the gateway and its unit
  (`/lib/systemd/system/hardened-blikvm-quic.service`), disabled. The
  application slot does not carry them: its closed set of executables is part
  of the update trust path, and the slot's package refuses both by name.
- The unit runs as `hardened-blikvm-quic`, a system user in no shared group.
  It has a sysusers fragment of its own
  (`packaging/sysusers.d/hardened-blikvm-quic.conf`, shipped with the
  package as an example and applied at installation), because the runtime's
  identities are a pinned set (`tools/runtime_provisioning_policy.py`).
  `SocketBindDeny=any` with
  `SocketBindAllow=udp:443` leaves it one port; 64 MiB and 16 tasks bound
  what it can take; the sockets of the owner, the broker and the helpers are
  out of its sight.
- The serving units (`hardened-blikvm-server-current` and `-candidate`, in
  both profiles) create the link's runtime directory
  `/run/hardened-blikvm-quic-link` and name the socket and the user that may
  connect. A server without these settings has no QUIC transport: the feature
  is absent from its manifest and Settings does not show the section. The
  server takes the directory away when it stops, so the gateway finds the
  socket again by path after every restart of the server.
- The gateway listens on `[::]:443` for IPv6 and IPv4; on a board without
  IPv6 it falls back to `0.0.0.0:443`.
- `ring` compiles C and assembly for the board. `scripts/build-aarch64.sh`
  gives it clang, the LLVM archiver and the headers of musl 1.2.5
  (`scripts/musl-headers.sh`, the release the encoder library scripts pin by
  hash); rustc links with `rust-lld` against the toolchain's own musl, as it
  does for every other binary. No C toolchain for the board is installed.
- **SD image.** `tools/image/build_sd_image.sh` copies a board's root file
  system and installs nothing of the application itself, so an image has the
  gateway, its unit and its user when the source board has update-runtime
  `0.1.0~dev35` and the user of its sysusers fragment. Three things follow:
  - the board's switch is the unit's enablement, a link under
    `/etc/systemd/system/multi-user.target.wants`; the builder removes it,
    because the switch of the source board does not belong to a new card;
  - the gateway keeps nothing on disk (its certificate lives in memory), so
    the builder's list of reviewed state under `/var/lib` needs no entry;
  - the builder's check of the paths that units need has nothing to find:
    the unit is not enabled in the image, and it names only paths under
    `/run`.

## Certificate

ECDSA P-256, made in memory at start, valid for 13 days from one hour
before. After 7 days the gateway makes a new one and reloads the endpoint's
configuration: sessions that are open go on, new ones get the new
certificate. No private key reaches a disk. The server learns the hash over
the link and gives it to the page with the token. The page sends its clock
with the request; if the certificate would not be valid by that clock, the
server answers `clock` instead.

## Session

1. `POST /api/transport/quic/session` with the page's clock (authenticated,
   CSRF). Answer: port, certificate hash, token (256 random bits, single
   use, 30 s, bound to the web session), or the reason there is none:
   `unavailable` (no gateway is on the link: the switch is off, or the
   gateway is not running or speaks another link protocol), `blocked` (the
   firewall mode) or `clock`. `GET /api/transport` tells the page which: the
   switch as the owner reads it, whether the gateway is on the link, whether
   the firewall mode closes the port, and the port.
2. The page opens `https://<host>:<port>/kvm` with the hash. The gateway
   validates the address before keeping state, refuses a foreign `Origin`
   and any other path.
3. The page opens the control stream and sends `hello` with the protocol
   number (1) and the token. The gateway redeems the token over the server
   link and gets an opaque session reference. Without a redeemed token after
   5 s the connection is closed.
4. The session has the web session's rights and ends with it: logout,
   expiry, session lock, password change, the switch going off, the server
   restarting.

Limits: 8 connections, 2 per web session; 64 tokens waiting, the oldest
giving way; idle timeout 30 s; keep-alive 5 s; the page pings every 2 s. A
gateway that connects anew replaces the one before it, and the sessions and
tokens of that one are forgotten.

A gateway that stops leaves the link before it closes its connections and
lets them drain (up to 2 s): from that moment the server answers
`unavailable`, instead of handing out tokens for an endpoint that no longer
answers.

**The page's side** (`web/src/lib/quic/session.ts`):

- One session per page while QUIC is this browser's choice and the board
  lists the feature. The standard transport is not touched by anything that
  happens to it.
- A connection that is turned down within 2 s was refused: the gateway may
  just have replaced its certificate, so the page tries once more with a
  fresh token and hash, and reports `handshake` if that fails too. One that
  fails later had nobody answering and counts as a timeout, whoever gave up
  first: Chromium ends an unanswered handshake after 4 s (five packets,
  measured), the page after 5 s.
- A session whose pings stay unanswered for 6 s, or whose connection closes,
  is lost. The page tries again after 1, 2 and 4 s; attempts that find the
  gateway absent, time out or are turned down count as tries, any other
  answer ends them with its reason. After the third the page reports `lost`.
- `closing` with the reason `session` ends the session for good: the page
  does not try again, and asks the server at once, whose answer sends it to
  the login.

## Messages

Control and input messages are JSON objects, each preceded by its length as
two bytes (at most 4096); unknown fields and types are refused.

**Control stream** (opened by the page):

| From | Message |
| --- | --- |
| page | `hello` (protocol, token); `ping` (id, page time); `video` (subscribe or not); `keyframe` (stream, last accepted frame, frames dropped); `bye` |
| gateway | `welcome` (protocol, largest datagram) or `refused` (reason); `pong` (id, page time, board time); `video` (started with stream and size, stopped, or unavailable with the reason); `counters` (round-trip time, packets lost, frames sent and dropped, key frames sent), every 2 s; `closing` (reason) |

**Input stream** (opened by the page): each message is a request of the HID
lease API as the page sends it to `/api/hid/lease` today (acquire, renew,
report, release) with a sequence number, and each answer is that API's
answer with the same number.

**Frames.** Every key frame travels on a unidirectional stream of its own,
opened by the gateway; if a newer key frame is ready before an older one is
through, the older stream is reset. A delta frame travels as datagrams. Both
begin with the same 32-byte header, integers big-endian:

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 1 | version (1) |
| 1 | 1 | kind: 1 key frame, 2 delta fragment |
| 2 | 1 | codec: 1 H.264, Annex B |
| 3 | 1 | reserved, zero |
| 4 | 2 | fragment index (zero for a key frame) |
| 6 | 2 | fragment count (one for a key frame) |
| 8 | 4 | stream id: new after an encoder restart or a change of size |
| 12 | 4 | frame id, rising by one within a stream |
| 16 | 4 | reference: the id of the key frame this frame depends on (its own for a key frame) |
| 20 | 4 | size of the complete frame |
| 24 | 8 | capture time, microseconds of the board's monotonic clock |

A datagram is at most 1200 bytes and never more than the path's live limit.

## Receiver (the page's decoder worker)

- Fragments in any order; an exact duplicate is ignored; a fragment that
  contradicts what is known of its frame discards the frame. At most 4
  frames and 2 MiB pending.
- A frame without a new fragment for 120 ms, or older than 1 s, is given up.
- A frame is decoded only if it continues the chain: its stream is the
  current one, its reference is the last decoded key frame, and its id
  follows the last decoded frame. Otherwise it waits for its key frame or is
  dropped.
- A gap, a frame given up or a decoder error starts recovery: one `keyframe`
  request, repeated after 1, 2 and 4 s, then none for 10 s; a key frame newer
  than the gap ends it.
- WebCodecs `VideoDecoder`, tuned for latency, draws to the `#screen` canvas;
  the picture's size follows the stream.
- The board's clock is estimated from the pings (offset and rate, from the
  samples with the shortest round trip); a frame's age is the time from its
  capture to its decoding by that estimate.

## Gateway, sending

- A frame's fragments are handed to the endpoint together. If the endpoint's
  datagram buffer has no room for the whole frame, the frame is dropped and
  counted, and the session then needs a key frame, like after a loss.
- The gateway asks the encoder for a key frame when a session subscribes,
  when it dropped a frame for it, or when the page asks; one key frame
  answers every session that waits for one.

## Encoder

The H.264 encoder unit gains the frame socket. A client receives records
(the header above without the fragment fields, then the frame) from the next
key frame on. A client that does not read loses its queue and starts again
from a key frame; it never holds the encoder up. A key frame on request is
made at most once per cooldown. Janus keeps its RTP and its periodic key
frames. The server holds the demand and sets the quality for a QUIC viewer
as for a WebRTC viewer.

## Input

The server stays the only authority: a lease request from the link is served
by the code behind `/api/hid/lease`, for the web session the token belonged
to. Order is the stream's. The page keeps one mouse report in flight and
never drops a keyboard report, as today. A QUIC session that ends releases
the lease like a client that vanished.

## Switches, Settings, failures, firewall

- **The board's switch** is the gateway unit's enablement, applied by the
  owner like the SSH switch: on enables and starts
  `hardened-blikvm-quic.service`, off stops and disables it, and the state is
  read back from systemd. The unit is installed disabled, so the switch is
  off by default and nothing listens on the port until the operator turns it
  on. The server issues a token only while the gateway is on the link.
- **Settings > Network > Transport**, as in the plan: the switch (switching
  on asks first and names the UDP port), this browser's choice (Standard by
  default, kept in the browser) and the status line. Switching the board's
  switch off returns this browser to Standard.
- **Failures** as in the plan's table:

  | Reason | Where it comes from | The page |
  | --- | --- | --- |
  | switched off, `blocked`, no WebTransport | `GET /api/transport`, the browser | offers only Standard and says why |
  | the gateway is switched on but not on the link | `GET /api/transport` | the same, and looks again every 2 s |
  | not served from the gateway's port (a forwarded port) | the port in the answer | the same: the page's policy would not let it connect |
  | `timeout`, `lost`, `unavailable` | the session | notice with "Retry"; Standard for this page load |
  | `handshake`, `token`, `protocol`, `sessions`, `hello` | the gateway | notice with the reason; no retry by itself |
  | `clock` | the session request | notice with the reason |

  A notice is raised once per failure; Settings shows the same reason in the
  status line, with "Retry" unless the page could never have a session.
- **Firewall**: no rule; `quicBlocked` in the firewall status for Restricted
  and Paranoid.
- The Content-Security-Policy is unchanged.

## What stays on HTTPS

The page and its login, the whole API including the session request above,
settings, media and image transfers, the terminal, MJPEG, the WebRTC
signalling, and everything when the browser's choice is Standard.

## Phase Q1: what is tested

- **Messages** (`bli-kvm-quic-protocol`): round trips with their documented
  names, sizes at their limits, and 100000 damaged or random inputs for each
  of the page's messages, the frame header and the link's messages: no
  panic, no allocation beyond the bound. The page's own encoder and reader
  have the same cases (`tools/test_quic_messages.mjs`).
- **Gateway** (`tests/sessions.rs`, against a page made with wtransport's
  client and a server that speaks the link): welcome and pings; a token is
  redeemed once; what is not a proper hello is refused without asking the
  server; a silent page is dropped after the hello timeout; a foreign origin
  and another path are not admitted; the server ends a session; a lost link
  ends every session and nobody is served until it is back; connections are
  bounded; the certificate is replaced while open sessions go on; the IPv6
  wildcard serves IPv4 pages; a gateway that stops leaves the link first.
- **Server** (`src/tests/quic_transport.rs`): login and CSRF; the token's
  life (single use, 30 s, bound to the web session, at most two sessions);
  the clock answer; a logout ends the sessions; a gateway that leaves takes
  its sessions and tokens with it; another user's process and another link
  protocol are not served; the switch is the owner's. Two tests run the real
  gateway on the real link: a page's session from token to logout, and
  **the switch enforced twice**: off, nothing answers on the port for a page
  that holds the certificate's hash, the server issues no token, and a token
  from before opens nothing once the gateway is on again.
- **Owner**: the switch enables and starts, or stops and disables, the unit,
  and reports a unit that cannot be switched.
- **Page** (`tools/playwright/quic_transport_contract.mjs`): the built page
  under the shipped Content-Security-Policy against the host's build of the
  gateway, bound to the UDP port with the number of the page's TCP port; the
  server's side of the link and the transport API are played by the
  contract. It covers: off by default with the reason; the question that
  names the UDP port; a session with one token redeemed once; a reload; the
  return to Standard; a session ended with the web session; a phone; token
  refused; wrong certificate hash (one fresh attempt, no token reaches the
  gateway); another protocol version; too many sessions; the clock; the
  firewall mode; no WebTransport; a lost session that comes back; one lost
  for good (three attempts, 1, 2 and 4 s apart); a port where nothing
  answers; switching off (the gateway stops, the port answers nothing, the
  browser returns to Standard). Each fault put into the page on purpose
  (the retry schedule, the fresh attempt, QUIC always offered, no "Retry"
  after a timeout, an ended session tried again, no question to the server)
  made it fail.

### On the board (Q1.8, 2026-10-01)

Update-runtime `0.1.0~dev35` and slot 61 (`0.1.0-alpha.6`), firewall
Baseline, headless Chromium 149 on the development host, wired network.
`tools/playwright/quic_transport_device_probe.mjs` ran twice; the receipts
are `evidence/private/quic-transport-device-20261001-a.json` and `-b.json`.

| What | First run | Second run |
| --- | --- | --- |
| Switching on in Settings until QUIC is offered: the owner enables and starts the unit, the gateway reaches the link | 3.1 s | 3.0 s |
| The page's session, from choosing QUIC to the first answered ping | 366 ms | 400 ms |
| A session by itself: session request, connection ready, welcome | 224, 31, 25 ms | 189, 27, 5 ms |
| Pings answered in 60 s, four a second | 235 of 235 | 235 of 235 |
| Round trip by the page's clock: median, 95th percentile, longest | 2.0, 23.1, 28.1 ms | 3.4, 24.6, 32.5 ms |
| Round trip as the gateway's QUIC stack measures it on the board: median, 95th percentile, longest | 2.8, 17.8, 20.4 ms | 3.3, 5.4, 5.6 ms |
| Packets lost | 0 | 0 |
| The page's own session, looked at every second | connected 60 of 60 | connected 60 of 60 |
| From the logout until the sessions are told they end | 0.70 s | 0.81 s |
| Switching off until the state reads off | 2.1 s | 2.1 s |
| The port afterwards, for a page that holds the certificate's hash | no answer | no answer |
| A session request afterwards | `unavailable` | `unavailable` |

- The gateway with two sessions pinging: 2.9 MiB resident (0.75 MiB charged
  to its unit), one thread, 0.66 s of processor time in 45 s (1.5% of a
  core). Its only capability is `CAP_NET_BIND_SERVICE`, the system call
  filter is on, and it listens on UDP 443 for IPv6 and IPv4.
- Most of opening a session is the session request, about 0.2 s: the server
  asks the owner for the firewall mode each time. The QUIC and TLS handshake
  with the WebTransport request takes about 30 ms.
- The page was sent to the login 0.7 to 0.8 s after the logout: the server
  looks at its sessions once a second.
- The development host's monotonic clock ran at 0.970 of real time during
  the probes, so the spans taken by the page's clock read 3% short; the
  gateway's own round trips are not affected.
- The GUI walk passed on the desktop and the phone layout (21 steps each,
  `GET /api/transport` among them, no failed request, no page error).
- Afterwards: the unit disabled and stopped, nothing listening on UDP 443,
  the administration switches, the firewall mode, the video settings and the
  WebRTC quality as they were found.

A lesson for every test and probe that measures time on the development
host: its wall clock is stepped forward by about 3.6 s every half minute
(and its monotonic clock runs about 10% slow), so a span taken from the wall
clock is sometimes 3.6 s too long. Spans are taken from the monotonic clock,
and numbers that matter come from the board's own clock.

## Changes against the plan

| Plan | Now | Why |
| --- | --- | --- |
| Q0.3 on a high UDP port | on UDP 443 | the page's policy admits only its own origin |
| Key frames on one stream | one stream per key frame | a newer key frame can replace one that is still in transit |
| 15% of a core at 8 Mbit/s | holds for frames sent as bursts | the cost is per wake-up, not per byte; measured both ways |
| The switch as a key of the administration switch file | the gateway unit's enablement, like the SSH switch | one state that survives a reboot and is read from systemd; the switch file, which refuses unknown keys, keeps its format for older slots |
