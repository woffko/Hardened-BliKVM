# QUIC transport v1

Status: design for `docs/baselines/work-plan-20261001-quic-transport.md`,
written in its phase Q0. Decision of Q0: **go** (the evidence is in "Phase Q0"
below). Since 2026-10-09 the picture travels by protocol 2, a group of
pictures on a stream of its own ("Protocol 2: groups of pictures" below):
"Messages", "Receiver", "Gateway, sending" and "The bitrate" describe it,
and the phases record protocol 1 as it was measured.

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
| Delta frames as DATAGRAMs, never retransmitted; key frames reliable | protocol 1: the same, each key frame on a stream of its own; protocol 2: every frame reliable, a group of pictures on a stream of its own |
| Its protocol v4's reference barrier: a delta frame that arrives before its key frame waits for it, within 120 frames, 16 MiB and 2 s | the receiver below |
| Its video QoS (`video_qos.rs`): the bitrate lowered on a bad path and raised again slowly | the share of the bitrate below, by the page's lag |
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
  never from one sample. (Q3 found one rate too little as well: the host's
  clock changes its rate within seconds. See "Receiver".)
- The session request carries `Origin`; the gateway can and will refuse a
  foreign one.

Not checked in Q0: Firefox and Safari. This note first said that neither was
installed on the development host. That was wrong for Firefox (a Firefox
snap had been there since 2026-09-28); it was checked in Q4, see "Browsers".
Safari is not at hand.

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
| Anyone who reaches UDP 443 | handshake floods, amplification, many connections | off by default; QUIC address validation before any state is kept; at most 8 connections; 5 s to present a token; the unit's memory limit; admitted only from the local ranges in the Moderate and Restricted firewall modes, closed in Paranoid |
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
- **Frame socket**: a Unix socket of the H.264 encoder unit that serves the
  gateway's user and nobody else.
- Crate **`bli-kvm-quic-protocol`**: the messages of the page, of the server
  link and of the frame socket, with their limits; no dependency beyond
  `serde`. The gateway, the server and the encoder use it.

## Packaging and build

- The update-runtime package `0.1.0~dev35` installs the gateway and its unit
  (`/lib/systemd/system/hardened-blikvm-quic.service`), disabled. The
  application slot does not carry them: its closed set of executables is part
  of the update trust path, and the slot's package refuses both by name.
  `0.1.0~dev37` carries the gateway of protocol 2 and dev36's files, and
  `0.1.0~dev38` one that leaves a silent page unjudged. The server and the
  gateway say the same protocol number on the link, so a slot of protocol 2
  and such a package go onto a board together: between the two the server
  refuses the older gateway, and pages use the standard transport.
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
   number (2; 1 before 2026-10-09) and the token. The gateway redeems the token over the server
   link and gets an opaque session reference. Without a redeemed token after
   5 s the connection is closed.
4. The session has the web session's rights and ends with it: logout,
   expiry, session lock, password change, the switch going off, the server
   restarting.

Limits: 8 connections, 2 per web session; 64 tokens waiting, the oldest
giving way; idle timeout 30 s; keep-alive 5 s; the page pings every 2 s,
and every 250 ms while it watches the picture (for the board's clock, see
"Receiver"). A gateway that connects anew replaces the one before it, and
the sessions and tokens of that one are forgotten.

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
  is lost. A page that was frozen (a phone's browser with another app in
  front, a hidden tab whose timers were held back) asked nothing in that
  time: when its ping timer fires more than two intervals late, the time
  does not count against the session's answers (2026-10-10). The page tries again after 1, 2 and 4 s; attempts that find the
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
| page | `hello` (protocol, token); `ping` (id, page time); `received` (the capture time of a frame of a group received whole, as it arrives; not answered); `video` (subscribe or not); `keyframe` (stream, last accepted frame, frames dropped); `bye` |
| gateway | `welcome` (protocol, largest datagram) or `refused` (reason); `pong` (id, page time, board time); `video` (started with stream and size, stopped, or unavailable with the reason); `counters` (round-trip time, packets lost, frames sent and dropped, key frames sent, the share of the chosen bitrate that the path carries in percent), every 2 s; `closing` (reason) |

The gateway answers a `video` that unsubscribes with `video` stopped. A
view that mounts a new canvas (upscaling switched on or off) unsubscribes
and subscribes at once, and that answer comes after the new subscription:
the page counts its unsubscriptions and passes over the stopped that
answers each, which it once took for the picture being unavailable
(2026-10-09).

**Input stream** (opened by the page): each message is a request of the HID
lease API as the page sends it to `/api/hid/lease` today (acquire, renew,
report, release) with a sequence number, and each answer is that API's
answer with the same number.

**Frames.** A group of pictures, a key frame and the delta frames that
refer to it, travels on a unidirectional stream of its own, opened by the
gateway: each frame whole, its 32-byte header and its bytes, one after
another. The stream is finished with its group, or reset when the gateway
gives the group up ("Gateway, sending"). Protocol 1 sent a key frame alone
on a stream and a delta frame as datagrams; the page still reads datagrams,
and the gateway sends none. The header, integers big-endian:

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 1 | version (1) |
| 1 | 1 | kind: 1 key frame, 2 delta frame (on a group's stream whole: fragment 0 of 1) |
| 2 | 1 | codec: 1 H.264, Annex B |
| 3 | 1 | reserved, zero |
| 4 | 2 | fragment index (zero for a key frame) |
| 6 | 2 | fragment count (one for a key frame) |
| 8 | 4 | stream id: new after an encoder restart or a change of size |
| 12 | 4 | frame id, rising by one within a stream |
| 16 | 4 | reference: the id of the key frame this frame depends on (its own for a key frame) |
| 20 | 4 | size of the complete frame |
| 24 | 8 | capture time, microseconds of the board's monotonic clock |

A datagram of protocol 1 was at most 1200 bytes and never more than the
path's live limit.

## Receiver (the page's decoder worker)

- Fragments in any order; an exact duplicate is ignored; a fragment that
  contradicts what is known of its frame discards the frame. At most 4
  frames and 2 MiB being put together, and 120 frames and 16 MiB waiting in
  all.
- A frame being put together without a new fragment for 120 ms, or older
  than 1 s, is given up. A complete frame waits up to 2 s for its key frame
  and the frames before it (the reference's barrier): on a slow path the
  frames after a key frame can be whole before it.
- A frame is decoded only if it continues the chain: its stream is the
  current one, its reference is the last decoded key frame, and its id
  follows the last decoded frame. Otherwise it waits for its key frame or is
  dropped.
- A gap, a frame given up or a decoder error starts recovery: one `keyframe`
  request, repeated after 1, 2 and 4 s, then none for 10 s; a key frame newer
  than the gap ends it.
- WebCodecs `VideoDecoder`, tuned for latency, draws to the `#screen` canvas;
  the picture's size follows the stream. Since 2026-10-06 the decoder is told
  the board's colour space (the capture's JPEG YCbCr: full range, the BT.601
  matrix, sRGB), so that a stream of an older encoder, which does not say it,
  is not drawn as the narrow range
  (`docs/baselines/audit-20261006-colour-range.md`).
- **The age of a frame** is the time from its capture to its drawing, by
  the board's clock. The page learns that clock from its pings: an answer
  carries the board's time, which is taken for the middle of the round
  trip. Only the pings with the shortest round trips count (within 2 ms of
  the fastest of the last twenty), and they are kept as points. The board's
  time at a frame's drawing is read between the two points around it, so a
  frame's age is known once the next ping has been answered, and neither
  the offset between the two clocks nor their rates need to be known. While
  the picture is watched the page pings four times a second.

  The first version drew one line through two minutes of pings, which is
  right for a steady clock. The development host's is not: its monotonic
  clock ran at 0.970 to 0.978 of the true rate and changed its rate every
  8 to 16 s (measured against `CLOCK_MONOTONIC_RAW`), which puts such a line
  off by tens of milliseconds. Between two pings 250 ms apart the same clock
  is off by less than 1 ms (`tools/test_quic_board_clock.mjs` plays it).

  The pings are answered on the page's thread, and the frames drawn in its
  worker: the page hands each answer over with the time it arrived and the
  time it is sent, both by the page's `performance.now()`, and the worker
  places them on its own clock by the quickest of those messages
  (`PageClock`). The two count from their own origins on one monotonic
  clock; the origins themselves are the wall clock when each context
  began, and the first version placed the page's times by them. On slot 84
  the first board probe after the install read every frame 4.05 s old,
  slots 81 to 83 54 to 68 ms: the development host's wall clock is kept by
  steps of about a second against its slow monotonic clock, and a worker
  begun a few steps after its page put every time the page handed over
  that far off (workers begun one by one in a page read 3, 929 and then
  1858 ms apart from it within 80 s). The picture was not late. The video
  contract now gives the worker an origin 4 s off: 6 ms with the change,
  4006 ms without it (2026-10-11).

**In the page** (`web/src/lib/quic`):

- `video-receiver.ts` holds the rules above and nothing else: no clock, no
  decoder, no stream. The caller gives the time with every call and decodes
  what comes back, so the rules are tested without a browser.
- `video.worker.ts` is the worker: the receiver, the decoder and the canvas
  the page handed over. The decoder is configured from the sequence
  parameter set of each key frame (`h264.ts`), so the profile and the level
  are the encoder's own. A decoder that reports an error, or has not come
  back with a frame handed to it 1 s ago (or has 120 before it), is closed
  and started again from a key frame that is asked for: a picture that falls
  behind is not shown late. Frames that come together, a group's frames
  after a pause on the path, are not that, however many; protocol 1 counted
  12 frames before the decoder, which a burst of them passed.
- The session (`session.ts`) reads the groups' streams and hands their
  frames to the worker as each is whole (`group.ts`): the first must be a
  key frame and the rest whole delta frames, or the stream is refused. A
  stream that ends in a reset was given up by the gateway. It tells the
  gateway each frame as it arrives (`received`, its capture time): by an
  event and not by a timer, which the browser slows down on a hidden page
  (Chromium to once a minute after five minutes), and which would have
  looked like a queue. It still reads datagrams, keeping 4096 for the page
  to read, as protocol 1 needed.
- The worker reports once a second: frames drawn, frames in the last
  second and frames a second over the last five (a still screen gives one
  frame every 2 s, and Settings shows the latter), the age of the frames
  whose age became known in the last five seconds as median and 95th
  percentile, the page's own share (from a frame's arrival, with its first
  fragment or its key frame's stream, to its drawing), frames given up,
  key-frame requests made, decoder restarts. The
  canvas carries them as `data-quic-*` attributes, and the status line
  carries what the gateway counts for the session (`data-frames-sent`,
  `data-frames-dropped`, `data-key-frames`, `data-packets-lost`).
- For a probe that measures a span of its own the worker keeps counts since
  it began (`quicVideoProbe` on its global): the frames drawn, their ages
  by the millisecond, the page's share by the tenth of a millisecond, and
  the capture time of the newest frame, which gives the span by the board's
  clock. A probe reads them twice and takes the difference.

## Gateway, sending

- **One connection to the encoder** (`src/frames.rs`), held while any
  session watches; its records go to every session that does. An encoder
  that restarts is found again, and the sessions notice no more than a new
  stream. One that sends nothing for 3 s, connection after connection, is
  not waited for: the sessions are told `stopped`, and the server's demand
  is let go.
- **A session that watches** (`src/video.rs`): the server is asked first and
  holds the encoder's demand; then the session takes its place among the
  watchers, which asks the encoder for a key frame. The page is told
  `started`, with the stream and the picture's size, when the first key
  frame of a stream goes out, and `unavailable` with the reason when the
  server refuses or the encoder cannot be reached.
- **A group of pictures** travels on a unidirectional stream of its own,
  below the control and the input stream in priority, written by a task of
  its own so that the encoder's frames are never held up by the path. The
  transport sends again what the path loses: a lost packet delays the group
  by a round trip instead of breaking the chain of delta frames until the
  next key frame. The congestion controller is BBR, which a path that loses
  packets at random does not slow down as it does Cubic.
- **The page's lag.** The transport takes what it is given into a buffer of
  its own long before the path carries it, so what is on its way is judged
  by the page's word: the newest frame it says it received whole. How long
  the oldest frame it has not received has been on its way is its lag.
- **The next key frame.** When it is ready, the group before is finished,
  and what of it is on its way goes on; but if a queue has built in front of
  the page ("The bitrate"), it is reset with every earlier group still on
  its way, and the page goes on from the new key frame.
- **A slow key frame**, one the page has not received half a second after
  it was sent, shows a path that takes long over key frames: the next key
  frame would only wait behind it, and is left out with the frames that
  refer to it, until the one after. Four seconds without it, its group is
  given up.
- **A page too far behind**, more than 1.5 s with more on its way than a
  key frame, has every group reset, and the picture starts again from a key
  frame that is asked for: the page shows the present instead of a picture
  that grows older on a path that cannot carry it. With less on its way
  than a key frame, waiting is the shorter way.
- **A page that gives the picture up** has its groups' streams reset: what
  the transport still holds of them is not sent for nobody.
- **A silent page**, one that has said nothing for a second (a ping, a
  frame received), is frozen in the background or gone with its session
  still open until the transport gives it up. What it has not received is
  no evidence of its path: no group is given up or left out for it, and no
  key frame asked for. Once it speaks again its path is judged afresh.
- **After a loss** (a frame that found the session's queue of 8 full, or
  the page's `keyframe`) the session gets no delta frame until the next key
  frame, and the encoder is asked for one. One key frame answers every
  session that waits for one.
- **Counters**, every 2 s with the path's round trip and lost packets:
  frames sent, frames dropped, key frames sent, and the share of the
  bitrate.
- **The board's time** in a `pong` is the system's monotonic clock, the
  clock of a frame's capture time.

## The bitrate

The board has one H.264 encoder for every viewer of H.264, over WebRTC and
over QUIC alike, and the operator chooses its bitrate in the Screen menu. A
path narrower than that bitrate builds a queue in front of its page: the
picture grows older until the gateway gives it up and starts again from a
key frame, which on such a path is late in its turn. So each session asks
for the share of the chosen bitrate that its path carries
(`crates/bli-kvm-quic-gateway/src/share.rs`), and the server runs the
encoder at the chosen bitrate times the least share of the sessions that
watch, never below a quarter of it.

- **The queue.** The page says each frame as it arrives, and the least lag
  of each second stands for the path in that second: the moments when
  frames are on their way count for nothing. The least of those over half a
  minute is the path without a queue, its round trip and the page's own
  delay in saying. A second more than 300 ms above it has a queue; one no
  more than 100 ms above it keeps up.
- **Down** by a quarter when a second has a queue that does not drain (its
  least lag no shorter than the second's before), and when a group is given
  up; at most once in 2 s, in which the lower bitrate shows, and not in a
  picture's first 3 s, whose first key frame tells little.
- **Up** by a fifth once the page has kept up for 10 s, and again after as
  long: from a quarter to the whole in eight steps.
- **A silent page** holds nobody's bitrate down: its share is whole until
  it speaks again. On slot 81 the operator's phone left two sessions
  behind, its page frozen with another app in front, and each, still open
  at the gateway for the transport's 30 s, took the encoder to a quarter of
  the chosen bitrate within 9 s for every viewer (2026-10-10).
- **The server** keeps each session's share, as the gateway last said it
  (`video_share` on the link), within a quarter and the whole, and counts
  it while the session watches. The H.264 encoder's demand connections, a
  WebRTC viewer's as well, are sent `kbps` with the chosen bitrate times the
  least share, and again whenever it changes. Nothing is stored: the
  operator's choice stays as it is, and the whole bitrate comes back when
  the session that asked for less gives the picture up, ends, or asks for
  more. The VP8 encoder is not touched.
- **The page** shows the share in the quality monitor ("Bitrate this path
  carries") and in the status line's `data-bitrate-share`.

## Encoder

The H.264 encoder unit gains the frame socket
(`/run/hardened-blikvm-h264/frames.sock`, `src/frames.rs`).

- **Who is served.** Any local process may connect; the encoder reads the
  peer's user from the socket and serves only the gateway's
  (`BLI_KVM_ENCODER_FRAME_USER`). On a board without that user the socket
  stays closed. At most four clients are served at once.
- **Records.** A client receives records from the next key frame on, each a
  32-byte header and the whole frame. The header is the frame header above
  with the picture's width and height (two bytes each) where the fragment
  fields are.
- **Ids.** A stream id is random, never zero, and new with every encoder: a
  restart, or another size. A frame's id rises by one with every encoded
  frame; a delta frame names the last key frame as its reference.
- **Capture time** is the moment the capture frame was read, on the
  system's monotonic clock, which every process of the board shares.
- **A queue per client**: at most 90 records and 8 MiB. A client that does
  not read loses its queue and starts again from a key frame, which is then
  asked for; it never holds the encoder up, and one that takes no bytes for
  5 s is dropped.
- **Key frames.** A client asks for one by writing a byte. A key frame on
  request is made at most once in 250 ms; a request inside that time waits
  for its end, and a key frame made for any other reason answers it. A new
  client counts as a request.

  250 ms is the reference's number. The first choice here was 500 ms; the
  time sets the frame rate on a path that loses datagrams, because a
  picture that lost a frame stands still until its key frame. On the host,
  1920x1080 at 30 frames a second and 2.5 Mbit/s over 15 s: with 1% of the
  datagrams lost 19 frames a second at 500 ms and 24 at 250 ms, with 3% lost
  7 and 17.
- **A still screen** gives the encoder a frame only every 2 s: it compares
  the capture's frames and passes on what changed. For a key frame that is
  due, and for a viewer that has just come, the capture loop lets its next
  frame through, changed or not; the answer would otherwise wait up to 2 s
  for the periodic frame.
- **Threads.** The unit may run 40 tasks (it was 16). The process has six
  threads of its own and x264 six; every viewer's demand takes one and
  every client of the frame socket two. An encoder for another size is now
  made after the old one is gone: made before, it doubled x264's threads
  for a moment, the limit of 16 refused them on the board
  (`fork rejected by pids controller` in the kernel's log), x264 could not
  be opened and the process ended, to be started again by systemd.
- Janus keeps its RTP and its periodic key frames: one at least every 2 s
  while the picture changes. The encoder's own key frames at its interval
  now count as such, so two no longer follow each other when the two clocks
  meet.
- The server holds the demand and sets the quality for a QUIC viewer as for
  a WebRTC viewer: when a session asks for the picture over the link, the
  server opens a demand connection of the H.264 encoder for it (the codec's
  name, then the operator's quality, then every change of it) and keeps it
  until the session gives the picture up, its connection closes, its web
  session ends or the link goes. A session that asks twice holds one
  demand. The answer names why there is no picture: `session` (it is not one
  of a live web session), `unavailable` (this board has no H.264 encoder
  with a demand socket) or `encoder` (it could not be reached).
- **An encoder that ends** (its unit starts it again 2 s later) takes its
  demand connections along. The server notices the end of the connection
  and asks again every 500 ms until the encoder is back, for a QUIC viewer
  and a WebRTC viewer alike; before, a viewer went on waiting for an encoder
  that had nobody to work for. The gateway finds the frame socket again by
  itself.

## Input

The server stays the only authority: a lease request from the link is served
by the code behind `/api/hid/lease`, for the web session the token belonged
to. Order is the stream's. The page keeps one mouse report in flight and
never drops a keyboard report, as today. A QUIC session that ends releases
the lease like a client that vanished.

- **The answer** is the API's own: its status and its body, as
  `{"status": 200, "body": {"code": 0, "msg": "success", "data": {...}}}`. A
  request without a current confirmation of the target gets the words HTTPS
  gives (`a current focus confirmation is required`); what is no lease
  request gets 422; a session whose web session has ended gets 401.
- **The web session** is checked for every request, and the request counts
  as its activity for the idle lock, as a request over HTTPS does. The
  confirmation of the target stays on HTTPS (`/api/hid/focus`).
- **The lease** is the web session's, whichever transport took it. The
  server remembers which QUIC session acquired it and gives it back when
  that session's connection closes, when the link goes, or when another
  gateway takes the place; it gives back nothing the session did not take
  and nothing that a later acquire has replaced.
- **Order**: the server works through the lease requests of a link one at a
  time, in the order they arrived, and a session's release on its end waits
  behind the requests it made. A request that has reached the owner is
  finished and accounted for even if the link ends meanwhile.
- **The page** chooses the lease's transport when remote control is
  enabled: the QUIC session if one is open, HTTPS otherwise
  (`web/src/hooks/useHidLease.ts`, `web/src/lib/quic/lease.ts`). Mouse,
  keyboard, the phone keyboard and its extra keys, the on-screen keyboard,
  paste and shortcuts all send through that one lease, so nothing else
  changes: one mouse report in flight, movement merged behind it, keyboard
  reports in order. A request waits 8 s for its answer, as over HTTPS. A
  session that is lost fails the lease's requests, which turns control off
  like any failure; enabled again, control takes HTTPS while QUIC is away.
  Control that was enabled over HTTPS stays there when a session opens, and
  Settings say which of the two carries the input.
- **The gateway** serves one input stream per session: the stream the page
  opens after its control stream. It passes one request to the server at a
  time and writes the server's answer back under the page's number; if the
  server gives none within 3 s the page is told `503`. At most 32 requests
  wait; beyond that the stream is not read, and the page is held by the
  stream's window. A pointer movement that waits and is superseded before it
  was sent is merged into the newer one (the newest absolute position, the
  sum of relative movements while it fits a report), and the answer to the
  one that was sent answers every request it stood for. Keyboard reports,
  buttons, the wheel and reports under another lease are never merged and
  never dropped. What is not a request ends the session with `protocol`.
  What the page wrote before it closed the stream is still passed on; the
  page may open the stream again.

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
  | `timeout`, `lost`, `unavailable` | the session | notice with "Retry"; Standard until a session opens again |
  | `sessions` | the gateway | notice with the reason; Standard until a session opens again |
  | `handshake`, `token`, `protocol`, `hello` | the gateway | notice with the reason; no retry by itself |
  | `clock` | the session request | notice with the reason |

  A notice is raised once per failure; Settings shows the same reason in the
  status line, with "Retry" unless the page could never have a session.
  After `timeout`, `lost`, `unavailable` and `sessions` the page also tries
  again by itself (2026-10-09): when it is looked at again (a phone's
  screen that was off) and every 35 s while it is, without raising the
  notice again. A refusal for too many sessions is among them because a
  session the page lost on a bad path still counts until the gateway lets it
  go, 30 s after its last packet.
- **The picture.** With QUIC chosen and H.264 the Screen menu's mode, the
  picture comes over the session and the menu's entry reads "H.264 (QUIC)".
  The page does not start WebRTC first and switch: it waits for the session,
  which is asked for as soon as the page is there. H.264 stays on WebRTC,
  and the entry reads "H.264 (WebRTC)", when
  - this browser has no H.264 decoder in WebCodecs (asked once, with
    `VideoDecoder.isConfigSupported`);
  - the session failed, until a session is open again (the page's own
    attempts or "Retry"): the picture then comes back over it, as it does
    after a stall;
  - the gateway has no picture for the session: `unavailable` with the
    server's reason (`unavailable`: no H.264 encoder on this board;
    `encoder`: it could not be reached), or `stopped` after an encoder that
    could not be reached for 3 s;
  - no frame was drawn for 10 s while the session was open (`stalled`): even
    a still screen gives one every 2 s, so a picture that shows nothing for
    that long is not coming, whatever holds it up.

  Settings says which of them it is under the status line, with the
  picture's size, its frames per second and the age of a frame while it
  comes over the session, and with "Retry" when the gateway had no picture.
  MJPEG and VP8 keep their paths whatever the choice. The status line's
  key-frame requests are the page's own count.

  Where the firewall mode closes WebRTC and admits the session's port
  (Restricted), H.264 has no WebRTC to stay on. The Screen menu then offers
  it only while the session can serve it: the entry reads "H.264 (QUIC)" and
  can be chosen with QUIC this browser's choice, and with Standard it is
  closed and says that QUIC would open it. A picture that is given up there
  takes the mode to MJPEG once the menu is opened, as a WebRTC mode under a
  closing firewall mode always did. A browser without WebRTC still gets
  neither WebRTC mode: the page's start takes it to MJPEG before a session
  exists.
- **Firewall**: Baseline and Moderate need no rule. Restricted admits the
  port from the local ranges by a rule of its own, beside HTTPS and SSH (the
  operator's decision of 2026-10-02; the plan had left it closed), and H.264
  then comes over the session while WebRTC stays closed. Paranoid keeps the
  port closed, and `quicBlocked` in the firewall status is true there only
  (`docs/design/firewall-reference-contract.md`).
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

## Phase Q2: input

What is tested:

- **Server** (`src/tests/quic_transport.rs`): a lease request over the link
  is served like the API's (forty key reports reach the owner in order,
  under the bindings of the page's own confirmation, and HTTPS reports under
  the lease the link acquired); without a confirmation the link refuses in
  the words HTTPS gives, for every kind of request; what is no lease request
  gets 422 and an unknown session 401; a session that ends gives its lease
  back (its connection closes, the link goes), gives nothing back twice and
  nothing that HTTPS took; a request over the link keeps the idle lock away
  while a login that does nothing is locked.
- **Gateway** (`src/input.rs`, `tests/sessions.rs`): the queue's rules by
  themselves (order, keys never merged, positions and relative movements
  merged, buttons, the wheel and another lease kept apart, the bounds), and
  against a page and a server: sixty key reports written at once arrive in
  order; a busy server gets the newest position only and its answer answers
  all twenty; what was written before the stream closed is passed on, and
  the stream can be opened again; what is no request ends the session; a
  server that does not answer costs one request 3 s.
- **Page** (`tools/playwright/quic_input_contract.mjs`, the built page with
  the real gateway and a played server): the lease goes over the session and
  nothing of it over HTTPS, only the confirmation of the target; a burst of
  typing arrives whole and in order; a refusal turns control off; control
  enabled over HTTPS stays there when a session opens and moves when it is
  enabled again; a lost session turns control off and HTTPS serves the next
  time; an ended web session sends the page to the login. The contract of
  the standard path (`restricted_control_lifecycle.mjs`) is unchanged.
- **Everything of our own on the way**
  (`tools/playwright/quic_input_stack_contract.mjs`): a page of the server's
  origin, the real gateway, the real web server and a stand-in for the board
  owner that takes 20 ms over a report. Sixty key reports written at once
  reach the owner in order; of twenty positions between two keys fewer reach
  it, the newest last, and all twenty-two requests are answered; a session
  that closes, and a gateway that stops, give the lease back; a request
  without a confirmation gets the same answer over both transports.

### A report and its answer, on the host

`tools/playwright/quic_input_latency.mjs`: headless Chromium 149 on the
development host, the optimized builds of the server and the gateway, the
stand-in owner answering at once, 400 key reports one after the other for
each transport and path, after 10 that are not counted. The path is the
loopback interface, directly and through the relays of `tools/net`. The
standard path here is HTTP/1.1 to the server, without the TLS and the nginx
of a board. Receipt: `evidence/private/quic-input-latency-host-20261002-a.json`.

| Path | Standard: median, 95th, 99th, longest | QUIC: median, 95th, 99th, longest |
| --- | --- | --- |
| direct | 4.0, 4.9, 6.1, 6.9 ms | 1.0, 1.5, 1.7, 3.2 ms |
| through the relays, nothing added | 7.1, 8.0, 9.9, 10.7 ms | 1.2, 1.6, 1.9, 4.5 ms |
| 50 ms each way | 103.5, 104.8, 105.3, 105.5 ms | 102.6, 103.8, 104.1, 104.3 ms |
| 50 ms each way, 2% loss | 103.6, 105.3, 504.5, 504.9 ms | 102.7, 104.5, 335.2, 357.2 ms |

- On a path without delay a report over QUIC is answered in a quarter of the
  time: the page writes one small message to an open stream instead of
  making an HTTP request. With the delay of a real distance the two differ
  by a millisecond: both need one round trip.
- Loss: the UDP relay dropped 2% of the datagrams each way, so the QUIC
  numbers are measured: a lost packet costs about 250 ms at this distance.
  The TCP relay cannot lose a segment; it stalls a direction for one
  retransmission instead, by default what Linux takes to repair the loss of
  the only segment in flight (twice the round trip plus 200 ms, 400 ms
  here), so the standard path's numbers under loss are that model's. In both
  cases one report in about twenty-five is hit, which is why the 95th
  percentile does not show it and the 99th does.
- Input over QUIC is one ordered stream: a lost packet holds the reports
  behind it, as TCP does. What QUIC changes for input is the cost of a
  report on a good path and of a loss on a bad one; it does not remove the
  wait for a retransmission.

### On the board, without remote control (Q2.5, 2026-10-02)

Slot 62 (`0.1.0-alpha.7`) and the update-runtime package rebuilt with the
gateway of this phase. `tools/playwright/quic_input_device_probe.mjs` never
confirms a target and never asks for a lease: it sends reports under a lease
that nobody was granted. Receipts:
`evidence/private/quic-input-device-20261002-b.json` and `-c.json`.

- The input stream opens: the first answer came 3.4 ms after the stream was
  opened.
- A report is refused over QUIC with the answer HTTPS gives, word for word:
  status 200, `{"code": -1, "msg": "a current focus confirmation is
  required"}`. What is no lease request gets 422 over both.
- The page's Transport section says that keyboard and mouse use the session
  once remote control is enabled.
- The HID ledger did not change: the same bytes and the same time of
  writing before the first run and after the last, with 836 refused
  requests over HTTPS and as many over QUIC between them. No focus request
  was made.
- The switch was turned on through the API and is off again; nothing
  listens on UDP 443.

The refusal is made by the web server alone, so its time shows what the two
transports cost on the real network, without the owner and the HID gadget:

| Refused report to its answer | HTTPS (TLS, nginx): median, 95th, longest | QUIC: median, 95th, longest |
| --- | --- | --- |
| 200 each, taking turns | 15.7, 26.5, 74.9 ms | 2.5, 17.1, 24.0 ms |
| 400 each, taking turns | 15.3, 25.8, 47.7 ms | 2.2, 16.3, 24.0 ms |

On the local network a report over QUIC is answered in about a seventh of
the time HTTPS takes. An applied report adds the owner's work to both
alike.

## Phase Q3: the picture

What is tested:

- **Frame format** (`bli-kvm-quic-protocol`, `frame.rs`): the header and the
  record header keep the documented layout; what cannot be is refused; a
  frame is cut into equal fragments and a rest, and a fragment whose length
  does not fit its frame is recognised.
- **Encoder** (`src/frames.rs`, `tests/frame_socket.rs`): a client gets
  records from the next key frame on; a key frame on request is made once
  per cooldown; a client that does not read loses its queue and holds nobody
  up; only the admitted user is served, and no more than four clients; the
  frames are numbered from the stream's first key frame; and the encoder's
  own process, fed a capture stream, serves numbered H.264 frames and a key
  frame on request, and on a still picture makes the key frame when it is
  asked for, not 2 s later.
- **Server** (`src/tests/quic_transport.rs`): a subscription over the link
  holds the encoder's demand and its end lets it go, as does the end of the
  session; without an encoder there is no picture, with the reason; the
  quality reaches the encoder over a QUIC session's demand; a demand is
  held again when the encoder comes back.
- **Gateway** (`tests/sessions.rs`): the picture is key frames on streams
  and delta frames as datagrams; a page that lost a frame gets a key frame
  and nothing before it; a newer key frame overtakes one that is still on
  its way; a frame without room is dropped whole; the page is told why
  there is no picture; two sessions share the encoder's frames.
- **Receiver** (`tools/test_quic_video_receiver.mjs`, the page's module
  without a browser): the header as the gateway writes it; fragments in any
  order and duplicates; frames held for the one before them and for their
  key frame; a contradicting fragment; a frame that stops arriving; a pause
  longer than the deadline before a fragmented frame does not count against
  it, one inside it does; the recovery schedule; a failing decoder; the
  bounds in frames and bytes; old frames, old key frames and another
  stream; and 100000 random and damaged datagrams and streams, after each
  of which the bounds hold and no frame has left its chain. The board's
  clock (`tools/test_quic_board_clock.mjs`) and the codec string
  (`tools/test_quic_h264.mjs`) have their own.
- **Page** (`tools/playwright/quic_video_contract.mjs`): the built page, the
  real gateway, a played server, the encoder's own x264 engine behind a
  frame socket (`examples/h264_frame_source.rs`: a picture that carries its
  frame's number) and a UDP relay between the page and the gateway.
  Headless Chromium decodes the stream: the number read back from the
  canvas moves on at the source's rate, nothing is given up or asked for on
  a clean path, and the menu and Settings say what serves the picture. With
  3% of the datagrams lost each way for 8 s the picture goes on, no picture
  is older than the one before it, and key frames are asked for no faster
  than the encoder makes them (four a second); once the path is clean the
  picture is whole again and nothing more is asked for. The counters the
  worker keeps for the probes agree with what was drawn. A gateway that is
  started again gets a new session and a new subscription. A server without
  H.264 for the session, an encoder that goes away, an encoder that is
  there and sends nothing for ten seconds, and a browser without an H.264
  decoder leave H.264 on WebRTC with the reason in Settings, and "Retry" has
  the picture back where another attempt can help. MJPEG is not touched by
  the choice.

  Eight runs of the lossy part on the development host (640x360, 30 frames
  a second): 12 to 17 datagrams lost towards the page, 174 to 215 of 240
  frames drawn, 20 to 32 given up, 4 to 8 key-frame requests; the picture
  was whole again, with a second in which nothing was given up or asked
  for, 2.3 to 2.5 s after the loss ended.

### A moving picture, on the host

`tools/playwright/quic_video_measure.mjs`: headless Chromium 149, the
optimized builds of the gateway and of the frame source (the encoder's x264
engine, ultrafast, on a picture that changes with every frame), 1920x1080 at
30 frames a second and 2.5 Mbit/s, a key frame every 2 s and on request, the
UDP relay between the page and the gateway, 60 s on each path after a
warm-up. A frame's age runs from the moment the source took the picture to
its drawing; the source, the gateway and the page share the host's clock
here. A board's target gives such a picture only while somebody works on it.

| Path | Frames a second | Age, median and 95th percentile | The page's share of it | Frames given up | Key-frame requests |
| --- | --- | --- | --- | --- | --- |
| the relay without impairment | 30.0 | 17 and 21 ms | 1.3 and 2.1 ms | 0 | 0 |
| 1% of the datagrams lost each way | 23.0 (8 in the worst second) | 17 and 22 ms | 1.3 and 1.8 ms | 321 | 73 |
| 3% lost each way | 18.3 (8 in the worst second) | 16 and 21 ms | 1.2 and 1.8 ms | 500 | 118 |
| 50 ms added each way | 30.0 | 67 and 74 ms | 1.3 and 3.9 ms | 0 | 0 |

- On a clean path every frame arrives, 17 ms after it was taken; the page
  needs 1.3 ms of that from the frame's first datagram to its drawing, with
  no jitter buffer in between.
- Delay is added once: 50 ms each way make 67 ms.
- Loss costs frames, not delay. A frame of about 10 KB travels as nine
  datagrams, none is sent twice, and a frame that lost one is given up
  together with the frames after it until a key frame comes: with 1% lost
  the picture kept 23 of its 30 frames a second, with 3% lost 18. What is
  shown is as fresh as on a clean path. This is the reference's behaviour
  and decision 2 of the plan. A path that loses datagrams all the time is
  served better by a transport that sends them again, as WebRTC does; one
  parity fragment per frame would be the remedy within this design and is
  not part of this plan.

### On the board (Q3.6, 2026-10-02)

Slot 64 `0.1.0-alpha.9` (source `5a43c90`), update-runtime `0.1.0~dev35`
with the gateway of Q3.3, and the encoder of the same source deployed with
`tools/device/deploy_vp8.sh`. Headless Chromium 149 on the development host,
the board on the local network, `tools/playwright/quic_video_device_probe.mjs`
for 60 s after a warm-up, at the source's 1920x1080 and the operator's
2.5 Mbit/s. Remote control stayed off; the probe asks for no lease, and the
HID ledger was the same file before and after.

**The target's screen stood still during the probes.** The encoder then
passes on one frame every 2 s, by turns a key frame on its stream and a
delta frame of one datagram. These runs therefore show the picture's way
through the board, the network and the page, and its timing; what a moving
picture does under loss is what the host's table above shows. The relay
(`tools/net`) ran on the development host, between the page and the board.

| Path | Frames | Age, median and 95th percentile | The page's share | Given up | Key-frame requests | Round trip, packets the gateway lost |
| --- | --- | --- | --- | --- | --- | --- |
| direct | 32 in 64 s | 72 and 110 ms | 1.1 and 1.5 ms | 0 | 0 | 1.7 ms, 0 |
| direct, again | 32 in 64 s | 93 and 112 ms | 1.1 and 1.6 ms | 0 | 0 | 1.9 ms, 0 |
| relay, 1% lost each way | 31 in 62 s | 90 and 111 ms | 1.2 and 1.4 ms | 0 | 0 | 2.2 ms, 4 |
| relay, 3% lost each way | 31 in 62 s | 91 and 115 ms | 1.1 and 1.5 ms | 0 | 0 | 2.6 ms, 17 |
| relay, 50 ms each way | 31 in 62 s | 128 and 163 ms | 1.1 and 1.4 ms | 0 | 0 | 102.9 ms, 0 |
| direct, a WebRTC viewer beside it | 25 in 49 s | 69 and 105 ms | 1.2 and 1.4 ms | 0 | 0 | 1.7 ms, 0 |

- A frame is drawn 57 to 65 ms after the board took it when it is a delta
  frame and 93 to 112 ms when it is a key frame (the second direct run,
  whose receipt has every value: 15 and 17 frames). Half of the frames
  being key frames, the median falls on one kind or the other. The time is
  the encoder's preparation and encoding, as measured before for WebRTC
  (`docs/design/h616-software-video-encoder.md`), and 1 ms in the page.
- Lost packets cost nothing here: the gateway counted 4 and 17 of them, a
  key frame's stream sent them again, and no delta frame happened to be
  among them in a minute.
- The first picture came 1.3 s after the page was opened while the encoder
  was still on the capture stream, and 3.1 s when it had left it for want of
  viewers and had to return.
- **H.264 over WebRTC**, the same minute and the same still screen
  (`vp8_webrtc_probe.mjs`): 32 frames decoded, none lost; a frame waited
  26 ms in the jitter buffer and was decoded 28 ms after its first packet
  arrived. Over QUIC the page draws it 1.1 ms after its first datagram: no
  jitter buffer.
- **Both viewers at once**: the QUIC page drew 25 frames in 49 s and the
  WebRTC page decoded 16 in 30 s; the encoder was not started again and its
  task limit was not reached.
- **CPU and memory with one QUIC viewer** over 40 s of the direct run
  (systemd's accounting, the board's clock): the gateway 0.5% of a core and
  3.1 MiB (one thread), the H.264 encoder 10.7% of a core and 30.7 MiB, the
  capture 6.5%, the board 95% idle. With a still screen the gateway has
  almost nothing to send; for a stream of 2.5 Mbit/s its endpoint took 3.7%
  of a core in Q0, and the encoder's load with a moving picture is the one
  the video note has for WebRTC, since it is the same encoder.

What the board showed that the host had not (fixed before these numbers):

- x264 could not be opened when the first QUIC viewer came, and the encoder
  ended: its unit's task limit (see "Encoder", threads).
- After that the viewer waited for a picture that nobody made: the server's
  demand had gone with the old process (see "Encoder", an encoder that
  ends), and the page had no rule for a picture that shows nothing (see
  "Switches, Settings, failures, firewall", `stalled`).
- On the still screen the first picture and every key frame on request
  waited up to 2 s (see "Encoder", a still screen), and Settings showed a
  frame rate of 0 or 1 and an age only in every other second.

Receipts: `evidence/private/quic-video-device-20261002-seq64-*.json`,
`h264-webrtc-seq64-*.json`, `quic-q3-state-*.json`.

### After the plan: slot 65 (2026-10-02)

Slot 65 `0.1.0-alpha.10` carries the firewall rule for Restricted. On the
board, with the same probes and without a lease:

- **The firewall modes** (`tools/playwright/quic_firewall_device_probe.mjs`).
  In Restricted the status said that WebRTC is blocked and QUIC is not, the
  owned table listed the rule, a session opened in 43 ms and its ten pings
  were answered, and the page drew H.264 from the session, with
  "H.264 (QUIC)" offered and VP8 closed. In Paranoid the status said both
  are blocked, the table had no such rule, the server refused a session with
  `blocked`, and a connection made with the certificate's hash from before
  got no answer in 4 s. Baseline was applied again, and a session opened.
- **A picture that changes.** The target's screen changed about ten times a
  second during this run, and the operator's quality was 8 Mbit/s. In 66 s
  the page drew 695 frames at 1920x1080, a frame 94 ms after the board took
  it in the median and 158 ms at the 95th percentile, and added 2.3 ms of
  its own (7.5 ms at the 95th percentile). No frame was given up and no key
  frame asked for; the gateway sent 788 frames, 36 of them key frames, and
  lost no packet. The encoder's own lines of that minute explain the age:
  at 8 Mbit/s x264 takes 47 ms for a frame, where it takes 28 ms at 2.5.
- H.264 and VP8 over WebRTC, the GUI walks on the desktop and on a phone's
  size, and the touch reach probe passed on the same slot.

Receipts: `evidence/private/quic-firewall-device-seq65-20261002.json`,
`quic-video-device-seq65-20261002.json`, `h264-webrtc-seq65-20261002.json`,
`vp8-webrtc-seq65-20261002.json`, `gui-walk-seq65-*.json`,
`touch-reach-seq65-20261002.json`.

### Browsers (Q4.2)

| Browser | WebTransport | Session with the certificate's hash | H.264 in WebCodecs | The page with QUIC chosen |
| --- | --- | --- | --- | --- |
| Chromium 149 (headless, Linux) | yes | yes | yes | input and the picture over the session: every probe and contract above |
| Firefox 156.0.1 (headless, Linux, through its geckodriver) | yes | yes, ready after 34 ms | yes | the session is open, the picture is drawn from it at 1920x1080 (`tools/device/quic_firefox_probe.mjs`, on the board) |
| Safari | not checked: none at hand | | | |
| Chrome on Android, Safari on iOS | not checked: left to the operator | | | |

## Protocol 2: groups of pictures (2026-10-09)

Over slot 80 the operator found the QUIC picture less smooth than before,
the quality monitor counting decoder restarts, and asked that the transport
work at least as the reference's `experimental` branch does. That branch
(its protocols v2 to v4 and `video_qos.rs`) keeps delta frames as
datagrams, sends key frames reliably with a refresh scoped to the frame
that was lost, holds a delta frame that arrives before its key frame (v4's
reference barrier: 120 frames, 16 MiB, 2 s), and lowers the bitrate by an
eighth when 3 % of the packets are lost over 10 s or the datagram queue
fills. The operator chose a stream for each group of pictures, the barrier,
and a bitrate fitted to the path (2026-10-09).

On the host, through the relay, the test source's 1280x720 (x264 makes
about 940 kbit/s of its picture), plain drawing, 25 s each:

| Path, each way | Frames a second | Age, 95th percentile | Given up | Key frames asked for |
| --- | --- | --- | --- | --- |
| clean | 30.0 | 20 ms | 0 | 0 |
| 40 ms, 3 % lost | 30.0 | 151 ms | 0 | 0 |
| 40 ms, 10 % lost | 29.8 | 197 ms | 0 | 0 |
| 100 ms, 3 % lost | 30.3 | 564 ms | 0 | 0 |

Protocol 1 with the barrier drew 7.5 frames a second on the 40 ms, 3 % path
(the source at 800 kbit/s), gave up 455 frames and asked for 54 key frames:
every lost datagram broke the chain until a key frame. The groups'
streams with Cubic drew 5.9 frames a second there, the newest frame 10 s
old: a loss-based controller carries about 0.85 Mbit/s at an 80 ms round
trip and 3 % loss (MSS / RTT x 1.22 / sqrt(p)), less than the picture. BBR
paces at the bandwidth it measures and does not take random loss for a
full path.

**A narrow path.** The first version finished a group when the transport
had taken all of it, which it does into its own buffer at once: on a path
half the picture's rate the finished groups queued behind each other, the
page drew nothing for 11 s and gave the session's picture up. A key frame
that was reset by the next one before it arrived starved the picture the
same way, and BBR keeps the rate it measured for about ten round trips
after a path narrows at once, so the relay's queue overflowed (15 to 20 %
of the datagrams). Hence the rules in "Gateway, sending": what is on its way
by the page's word, a slow key frame waited for, a group given up only for
more than a key frame's worth. The test source's x264 does not go below
about 630 kbit/s at 1280x720 whatever it is asked, so a narrow path is
measured with a chosen 800 kbit/s, which it follows
(`tools/playwright/quic_share_contract.mjs`): narrowed to half the
picture's rate (326 of 652 kbit/s), the session asked for 75, 56, 42 and
31 % within 8 s, and the page kept up after about 26 s at 29.8 frames a
second, its oldest frame 250 to 450 ms old; wide again, the share climbed.

## Standard against QUIC

What the measurements of this note say, side by side:

| | Standard | QUIC |
| --- | --- | --- |
| A key report and its answer, host, direct, median | 4.0 ms (HTTP/1.1, a request each) | 1.0 ms (one stream) |
| The same, 50 ms each way, median | 103.5 ms | 102.6 ms |
| The same, 50 ms each way and 2% loss, 99th percentile | 504.5 ms (a retransmission, modelled) | 335.2 ms (measured) |
| A refused report on the board, median | 15.3 to 15.7 ms (TLS, nginx, the server) | 2.2 to 2.5 ms |
| A frame from its first packet to the screen, board | 28 ms (WebRTC: 26 ms of jitter buffer) | 1.1 ms |
| A moving picture on a path that loses 1% of its datagrams | WebRTC sends lost packets again (not measured here) | protocol 1: 23 of 30 frames a second (host) |
| The same with 3% lost | | protocol 1: 18 of 30 frames a second; protocol 2, 40 ms each way: 30 of 30, 151 ms at the 95th percentile (host) |
| What must be open | TCP 443, and for WebRTC a UDP range with ICE | UDP 443 beside TCP 443 |

- Input gains most where there is delay and loss: a report no longer waits
  behind a stalled TCP connection, and on the board it saves the HTTPS
  round through nginx.
- The picture arrives sooner by the jitter buffer, about 27 ms. Protocol 1
  showed fewer frames on a path that loses datagrams all the time than a
  transport that sends them again would; protocol 2 sends them again, a
  round trip late.
- This is why the browser's default stayed Standard: QUIC was the better
  choice on a clean or merely slow path, and where a single UDP port is
  easier to pass than a WebRTC negotiation. Whether it becomes the default
  is the operator's decision.

## Changes against the plan

| Plan | Now | Why |
| --- | --- | --- |
| Q0.3 on a high UDP port | on UDP 443 | the page's policy admits only its own origin |
| Key frames on one stream | one stream per key frame | a newer key frame can replace one that is still in transit |
| Delta frames as datagrams (protocol 1) | a group of pictures on a stream of its own (protocol 2, 2026-10-09) | a lost datagram broke the chain until the next key frame; over slot 80 the operator saw the picture stand and the decoder start again |
| 15% of a core at 8 Mbit/s | holds for frames sent as bursts | the cost is per wake-up, not per byte; measured both ways |
| The switch as a key of the administration switch file | the gateway unit's enablement, like the SSH switch | one state that survives a reboot and is read from systemd; the switch file, which refuses unknown keys, keeps its format for older slots |
| The frame socket open to the gateway's group | open to connect to, serving the gateway's user only | the gateway is in no group the other units share; the encoder checks who connects, as the server does on its link |
| The page receives in the existing decoder worker (`direct.worker.ts`) | a worker of its own, `web/src/lib/quic/video.worker.ts` | the existing worker belongs to the Direct mode, which this board does not offer, and speaks that mode's WebSocket records with a fixed codec; reassembly, the chain, recovery and the board's clock are another protocol, and the Direct mode stays as it was |
| Q3.6: the picture on the board at 1% and 3% loss | on the board with the still screen its target showed; a moving picture under the same loss on the host | the probes may not touch the target's keyboard or mouse, and nothing moved on its screen: one frame every 2 s, half of them on a stream, loses nothing in a minute |
| Firefox checked in Q0.3 | checked in Q4.2 | Q0 took it for absent; it was installed |
| The port stays closed in Restricted and Paranoid | Restricted admits it from the local ranges; Paranoid keeps it closed | the plan left the question to the operator, who decided on 2026-10-02 that UDP 443 is to be admitted |
