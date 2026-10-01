# Work plan: QUIC transport with a switch in Settings (2026-10-01)

The operator asked for a QUIC transport modelled on the one in the RustAdmin
client, and for a switch in Settings between it and today's transport, which
stays the default. This plan fixes what that means for a KVM whose client is
a browser, the order of the work and what counts as done.

Rules as in the earlier plans: capability-gated surfaces enforced by the
server and the board owner, privileged work only in the owner or a dedicated
helper, English repository text, secrets only through `project-memory-exec`
(never in arguments, logs or receipts, and never a program on its standard
input under it), every change shipped as a signed slot or the update-runtime
package and verified on the test board, audit and design documents updated
with evidence under `evidence/private`. Additions for this plan:

- One commit per item. An item's own tests run before its commit; the full
  `scripts/check.sh` runs before the last commit of each phase and before
  every slot or package build (a failure under host load is rerun and the
  flaky test named in the commit that fixes it). The branch is pushed after
  each phase.
- Phase Q0 is a gate. If it ends in "no-go", the plan ends there with the
  findings recorded; nothing of Q1-Q4 is started.
- The reference is read, never ported: RustAdmin is AGPL and this project is
  GPL-3.0-only. Behaviour and numbers may be taken over, code may not.
- Nothing here takes the HID lease on the test board or sends input to its
  target. Input over QUIC is proven on the host against the fake board
  backend; on the board only its refusal without control is observed, and the
  operator's own check is listed at the end.

## Where the work starts

- Slot 60 `0.1.0-alpha.5` (source `c613f18`), update runtime `0.1.0~dev34`.
- Today's transport, called **Standard** below:
  - the page, the API and MJPEG over HTTPS: nginx on TCP 443 in front of the
    server on loopback (`packaging/nginx/hardened-blikvm.conf`);
  - input: every HID report is an HTTPS request under the HID lease
    (`/api/hid/lease`; `web/src/hooks/useHidLease.ts`,
    `crates/bli-kvm-server/src/hid.rs`), mouse movement coalesced to one
    report in flight;
  - VP8 and H.264 through the Janus WebRTC gateway (UDP, a media port range,
    closed in the Restricted and Paranoid firewall modes). The encoder
    (`crates/bli-kvm-video-encoder`) sends RTP to Janus on loopback and makes
    key frames periodically, because Janus cannot pass a viewer's key-frame
    request on to an RTP source.
- The workspace has no TLS or QUIC code (nginx terminates TLS). Release
  builds abort on a panic. The server unit may bind only `tcp:18080` and
  reach only loopback
  (`packaging/systemd/hardened-blikvm-server-current.service`).
- The board's certificate is self-signed.

## What "like RustAdmin" means here

Reference: repository `woffko/rustadmin`, branch `experimental`:
`docs/QUIC_PROTOCOL_V1.md`, `docs/QUIC_PROTOCOL_V2.md`,
`docs/QUIC_VPN_TRANSPORT.md`, `src/quic_transport.rs`, and the transport
module of its `hbb_common`.

Taken over:

1. Independent channels: control and input on their own reliable streams, so
   neither waits for the other or for video.
2. Delta frames as QUIC DATAGRAMs that are never retransmitted; key frames on
   a reliable stream of their own.
3. Everything bounded: datagrams sized to the path, send queues where the
   newest frame wins, reassembly with a no-progress deadline (120 ms) and a
   hard lifetime (1 s for a delta frame), the receive time taken after the
   datagram is read, not before the wait for it.
4. Recovery without loops: one outstanding key-frame request per connection,
   retried after 1, 2 and 4 s and then held back for 10 s; a newer key frame
   clears it; the request names the stream and the last accepted frame; the
   encoder answers with one key frame and nothing is restarted; a key frame
   already on its way answers further requests; a delta frame never reaches
   the decoder before the key frame it refers to.
5. Security bound to the application: TLS 1.3, a pinned certificate, the
   session's authentication on top; a certificate, authentication or protocol
   error is shown and never becomes a silent fallback. Only an unreachable
   path falls back, and visibly.
6. The operator chooses the transport in the settings, and the page shows
   which one is in use, with its counters.

Not taken over: rendezvous, relay and NAT traversal (the browser connects to
the board), device pairing (the web login is the identity), audio, clipboard
and file transfer (none of them exists here or they stay on HTTPS), and
compatibility between protocol versions (the page and the board come from the
same slot and runtime package; one version, checked at connect).

In a browser QUIC is reachable only as **WebTransport** (HTTP/3), and the
picture is decoded with **WebCodecs**. WebTransport exists in Chrome and Edge
97+, Firefox 114+ and Safari 26.4+; a self-signed board also needs its
`serverCertificateHashes`, which Chrome has. The target for "go" is Chrome,
on the desktop and on Android. Firefox and Safari (WebTransport with
certificate hashes, and H.264 in WebCodecs) are checked where a browser is
at hand and recorded; where they lack a piece, the page stays on Standard.

## Design

Each decision below may be changed in the design note
(`docs/design/quic-transport-v1.md`, written in Q0) with the reason recorded.

### Processes

- **`bli-kvm-quic-gateway`**, a new unprivileged service
  (`hardened-blikvm-quic.service`, its own system user), is the QUIC and TLS
  front the way nginx is the HTTPS front. It is a process of its own because
  a panic aborts the process in release builds and the web server may not
  receive from the network. Sandbox as strict as the video proxy's, except
  that it may bind one UDP port and talk to the network on it; no writable
  path, a memory limit, no access to the owner's or the broker's sockets.
- It is installed by the update-runtime package in
  `/usr/libexec/hardened-blikvm` like the video proxy, and its unit runs only
  while the board's switch is on.
- Gateway and server talk over a Unix socket of the server that accepts only
  the gateway's user (peer credentials), with a closed, versioned request
  protocol (crate `bli-kvm-quic-protocol`). A version the server does not
  know makes QUIC "unavailable" with that reason; nothing else changes.
- Gateway and H.264 encoder talk over a local socket of the encoder unit that
  hands out encoded frames (below).

### Port

One UDP port, 443 by default (the number most networks let through),
configurable. Bound without root: a socket unit hands the socket over, or
the service has `CAP_NET_BIND_SERVICE` and nothing else. IPv4 and IPv6.

### Certificate

An ECDSA P-256 certificate made in memory when the gateway starts, valid
for 13 days from one hour before the start, replaced by a restart of the
gateway's endpoint at least every 7 days. No private key reaches a disk. Its
SHA-256 goes to the server over the local socket and from there to the page.
The page sends its clock with the session request; if the board's clock and
the page's differ so far that the certificate would not be valid for the
browser, the server answers "clock" instead of a token.

### Authentication

- `POST /api/transport/quic/session` (authenticated, CSRF) answers with the
  port, the certificate hash and a token: 256 random bits, single use, 30 s,
  bound to the web session.
- The page opens the WebTransport session and sends the token as the first
  message on the control stream. The gateway redeems it with the server. A
  connection that has not done so within 5 s is closed.
- The QUIC session has the web session's rights and ends with it (logout,
  expiry, session lock, password change, the board's switch going off).
- Limits: address validation before any state is kept, at most 8 connections
  and 2 per web session, idle timeout 30 s with a keep-alive, bounded stream
  counts, message sizes and queues. Tokens, hashes of them and report
  contents never reach a log.

### Channels

| Channel | Carrier | Content |
| --- | --- | --- |
| control | bidirectional stream, opened by the page | hello with token and protocol number, ping and pong (round-trip time and clock offset), video subscribe and unsubscribe, key-frame request, counters, close reason |
| input | bidirectional stream, opened by the page | the requests and answers of the HID lease API unchanged: acquire, renew, report, release |
| key frames | unidirectional stream, opened by the gateway | complete key frames |
| delta frames | DATAGRAMs | fragments of delta frames |

### Input

- The server stays the only authority: the gateway passes each lease request
  to the same code that serves `/api/hid/lease`, with the web session's
  identity. Focus confirmation, lease, ledger, cooldowns and "release all"
  are those of today.
- The stream keeps the order. Keyboard reports are never dropped; mouse
  movement is coalesced to the newest while the board is slower, as today.
- A QUIC session that ends releases the lease like a client that vanished.
  The page then says so and the operator enables control again.

### Video

- H.264 only in this plan (the x264 encoder; VP8 stays on WebRTC, MJPEG on
  HTTPS).
- The encoder gains a frame socket: each client receives encoded frames from
  the next key frame on, with stream id (new after a restart or a change of
  size), frame id, key flag, the id of the key frame a delta refers to, and
  the capture time. A client may ask for a key frame; the encoder makes one,
  at most one per cooldown, and one key frame answers all who asked. A client
  that falls behind loses frames, not the others, and starts again from a key
  frame. Janus keeps its RTP and its periodic key frames.
- The server holds the encoder's demand and passes the quality (bitrate) for
  a QUIC viewer exactly as for a WebRTC viewer.
- The gateway sends key frames on their stream and delta frames as datagrams
  cut to the path's datagram size; a frame that cannot be sent before the
  next one arrives is dropped and counted.
- The page receives in the existing decoder worker
  (`web/src/pages/desktop/screen/direct.worker.ts`): reassembly with the
  deadlines above, the reference rule, the recovery schedule, WebCodecs
  decoding tuned for latency, drawing to the `#screen` canvas the pointer
  code already knows. A decoder error resets the decoder and asks for a key
  frame.

### Switches and Settings

- **Board switch** `quic_transport`, an administration feature like the
  terminal's (`docs/design/admin-features-v1.md`): default off, kept by the
  board owner, enforced below the web server (the unit does not run while it
  is off, and the server refuses to issue tokens). The owner writes its key
  only while it is on, so a slot older than this plan still reads the file.
- **Settings > Network > Transport**:
  - "QUIC on this device": the board switch, with a confirmation that names
    the UDP port;
  - "This browser uses": Standard (default) or QUIC, kept in this browser;
    QUIC can be chosen only when the board's switch is on and the browser has
    WebTransport, otherwise the reason is shown;
  - a status line: the transport in use, and for QUIC the round-trip time,
    lost datagrams and key-frame requests.
- With QUIC chosen, input and control use the QUIC session, and the Screen
  menu's H.264 mode is served over it (and says so). MJPEG and VP8 keep their
  paths. A browser without an H.264 decoder in WebCodecs keeps H.264 on
  WebRTC and Settings says so.
- English and Russian.

### When QUIC does not work

| Situation | The page |
| --- | --- |
| Switch off, firewall mode closes the port, no WebTransport | offers only Standard and says why |
| No answer within 5 s, or the session is lost and 3 reconnects (1, 2, 4 s) fail | shows a notice, uses Standard for this page load, offers "Retry" |
| Certificate hash does not match (after one fresh attempt), token refused, protocol unknown | shows the error with its reason and stays on Standard; no automatic retry |
| Board clock too far from the browser's | says so; Standard |

Standard always remains usable: a KVM must stay reachable.

### Firewall

No rule is added. Baseline and Moderate already admit the port from the
sources they admit. Restricted and Paranoid keep it closed, as they keep
WebRTC closed (`docs/design/firewall-reference-contract.md`); the firewall
status reports `quicBlocked` so the page can say why.

### Dependencies

A Rust QUIC stack from crates.io, pinned in `Cargo.lock`: `quinn`, `rustls`
with `ring`, and a WebTransport layer (`wtransport` preferred; `h3` with
`h3-webtransport` as the alternative), chosen in Q0. Conditions: the licence
inventory passes (`tools/license_inventory.py`, `tools/test_crate_licenses.py`,
`docs/licensing.md`), a static aarch64 build works with the existing cross
toolchain, and every new direct dependency is named in the design note with
its licence and what it is used for. They are linked only into the gateway.

## Phases

### Q0: proof and design (at most one working day; gate)

- Q0.1 Read the reference documents and transport; write down in the design
  note what is taken over and what is not.
- Q0.2 Choose the crates: licence inventory, number of new crates, release
  build for the host and static for aarch64, binary size.
- Q0.3 Proof on the test board with a throw-away echo endpoint (one stream,
  datagrams, a datagram source at 2.5 and 8 Mbit/s), run as a transient
  unprivileged unit on a high UDP port and removed afterwards. From headless
  Chromium on the development host, with the page loaded from the board:
  the session with `serverCertificateHashes`, stream and datagram echo, 60 s
  of datagrams at 8 Mbit/s (loss, reordering, round-trip time), the
  endpoint's CPU and memory on the board. Firefox on the host if one is
  available; recorded either way.
- Q0.4 `docs/design/quic-transport-v1.md`: threat model of the new listener,
  the design above with what Q0 changed, limits, messages, failure rules,
  what stays on HTTPS.
- Q0.5 Decision, recorded with its evidence. **Go** needs all of: the session
  works from the board's own page with certificate hashes in Chromium;
  datagram loss on the local network below 0.1% at 8 Mbit/s; the endpoint
  within 15% of one core and 32 MiB at 8 Mbit/s; a static aarch64 build; the
  licence inventory passing. Otherwise **no-go**: record the findings and the
  alternatives (a trusted certificate, a WebRTC data channel for input) and
  stop.

### Q1: gateway, session, switch, Settings (control channel only)

- Q1.1 `bli-kvm-quic-protocol`: the messages of all channels and of the
  server link, with sizes bounded; encode and decode; tests with random and
  damaged input (at least 100000 cases, no panic, no unbounded allocation).
- Q1.2 `bli-kvm-quic-gateway`: endpoint, certificate, limits, control channel
  (hello, ping), server link, counters.
- Q1.3 Server: the gateway's socket with the peer check, token issue and
  redemption, session end, `GET /api/transport` (availability, switch,
  blocked, port). Route counts in `docs/design/gui-parity-plan.md` and
  `docs/baselines/phase8-acceptance.md` follow
  (`tools/test_plan_route_drift.py`).
- Q1.4 Owner: the `quic_transport` switch starts and stops the unit; the
  firewall status gains `quicBlocked`.
- Q1.5 Packaging: unit, system user, update-runtime package `0.1.0~dev35`;
  what the SD image builder needs for the new unit and user, recorded.
- Q1.6 Web: the Transport section of Settings > Network, the browser's
  choice, the QUIC session (connect, ping, status, the failure table above).
- Q1.7 Tests: units; a Playwright contract with the host's gateway and a
  fake server link (session, token refused, wrong hash, switch off, blocked,
  timeout with fallback notice, session ended by logout); the switch enforced
  twice; `check.sh`.
- Q1.8 Board: package and slot 61 installed; device probe (switch on,
  session, 60 s of ping with round-trip times, logout ends the session,
  switch off, the port answers nothing afterwards); GUI walk desktop and
  mobile; end state.

### Q2: input over QUIC

- Q2.1 Server: lease requests from the gateway link served by the code
  behind `/api/hid/lease`; a QUIC session's end releases the lease.
- Q2.2 Gateway: the input stream, order, the bounded queue, coalescing.
- Q2.3 Web: the lease session sends over HTTPS or QUIC, chosen when control
  is enabled (`web/src/lib/hid-lease.ts`, `web/src/hooks/useHidLease.ts`);
  mouse, keyboard, the mobile keyboard and its extra keys, the on-screen
  keyboard, paste and shortcuts all go through it.
- Q2.4 Tests on the host with the fake board backend: order, no keyboard
  report lost, coalescing, "release all" on disconnect, refusal without
  confirmation, the Standard path unchanged. The time from a report to its
  answer, median and 95th percentile, for HTTPS and QUIC: on a clean path and
  through a loss-and-delay proxy (2% loss, 100 ms round trip); a UDP proxy
  tool for QUIC and its TCP counterpart for HTTPS, both committed.
- Q2.5 Board, without control: the input channel opens; a report is refused
  with the reason HTTPS gives; the HID ledger does not change.

### Q3: video over QUIC

- Q3.1 Encoder: the frame socket, key frame on request with the cooldown,
  ids and the reference id, a queue per client; tests. Deployed with
  `tools/device/deploy_vp8.sh`.
- Q3.2 Server: video subscription from the gateway holds the demand and
  carries the quality.
- Q3.3 Gateway: key-frame stream, datagram fragments, newest-wins queue,
  request handling, counters.
- Q3.4 Web: receiver and decoder in the worker, the canvas, the Screen
  menu's label, fallback to WebRTC H.264, the status line's counters.
- Q3.5 Tests: reassembly (including a pause longer than the deadline before
  a fragmented frame), the reference rule, the recovery schedule; random and
  damaged datagrams and frames (at least 100000 cases); a Playwright
  contract in which headless Chromium decodes a stream made by the host's
  encoder through the gateway, and recovers from injected loss with a
  bounded number of key-frame requests; `check.sh`.
- Q3.6 Board: device probe for 60 s at the source's size (frames decoded,
  frames per second, age of a frame at decoding as median and 95th
  percentile, key-frame requests), the same through the proxy at 1% and 3%
  loss and with 50 ms delay; the H.264 WebRTC probe on the clean path for
  comparison; CPU and memory of the gateway and the encoder with one viewer.

### Q4: finish

- Q4.1 Documents: the numbers of Q2 and Q3 in the design note (Standard
  against QUIC), the firewall contract, the administration features note,
  the video design note, the licence inventory.
- Q4.2 Regression on the board: GUI walk desktop and mobile, the VP8 and
  H.264 WebRTC probes, the touch reach probe; Firefox and, if at hand, Safari
  noted as checked or not.
- Q4.3 End state: the QUIC switch off (unless the operator said to leave it
  on), every other administration switch off, firewall Baseline, the
  operator's quality and video settings as found. Audit and status documents
  and the memory note updated; the operator's steps listed once.

## What the operator should expect

- On the local network little changes: WebRTC already uses UDP and the
  software encoder sets the frame rate. The picture may arrive some tens of
  milliseconds sooner (no jitter buffer) and recover faster from a loss
  (a key frame when asked, not when the period ends).
- Over a VPN or a mobile network: one UDP port instead of a WebRTC
  negotiation, and input that no longer waits behind a stalled TCP
  connection. Q2 and Q3 measure both; the numbers decide whether QUIC should
  later become the default.

## Out of scope, possible follow-ups

QUIC as the default or an "Auto" choice; QUIC in the Restricted mode; VP8 or
MJPEG over QUIC; retiring Janus; HTTP/3 for the page itself; a publicly
trusted certificate; audio.

## Risks

| Risk | Handling |
| --- | --- |
| Certificate hashes unusable from the board's page in Chromium | Q0 gate |
| The QUIC stack's licences or its aarch64 build | Q0 gate |
| A new listener on the network | default-off switch, own sandboxed process, address validation, limits, token within 5 s, random-input tests, closed in the restrictive firewall modes |
| Board clock wrong, certificate not yet or no longer valid | detected at the session request, shown, Standard |
| The encoder shared by WebRTC and QUIC viewers | key frames on request limited by a cooldown; Janus path unchanged; measured with both viewers |
| Memory and CPU on the H616 | limits on the unit, measured in Q0 and Q3 |
| A slot rolled back below this plan | the gateway cannot redeem tokens and serves nobody; the switch key is written only while on |

## Operator

Decided in this plan, to be changed by saying so: the board's switch is off
by default; UDP 443; the port stays closed in Restricted and Paranoid; the
browser's default is Standard.

Left to the operator at the end: a check from the phone (Android Chrome)
with control enabled, typing and mouse over QUIC and the H.264 picture over
QUIC; a check from an iPhone or Firefox if they matter; whether QUIC becomes
the default; whether Restricted should admit it.
