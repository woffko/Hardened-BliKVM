# QUIC transport: outcome of the plan of 2026-10-01

The plan is `docs/baselines/work-plan-20261001-quic-transport.md`. The
design, every change against the plan and every measurement are in
`docs/design/quic-transport-v1.md`; this note says, item by item, what was
done and where its evidence is, what was found on the way, and what is left
to the operator. Receipts named here are under `evidence/private`, which is
not in Git.

## Outcome

Q0 ended with "go"; Q1 to Q4 are done. The test board runs slot 64
`0.1.0-alpha.9` (source `5a43c90`), the update runtime `0.1.0~dev35` with
the QUIC gateway, and the H.264 encoder with the frame socket. The board's
switch is off, and a browser uses the standard transport until its operator
chooses QUIC in Settings > Network > Transport.

## Items

| Item | Commits | Outcome and evidence |
| --- | --- | --- |
| Plan | f87bfd5 | Committed as written. |
| Q0.1 | 76733a5 | What is taken over from the reference transport and what is not ("Reference"). |
| Q0.2 | 58374f7 | `wtransport` 0.7.2 on `quinn`, `rustls` with `ring`; licence inventory without a missing licence; static aarch64 build ("Crates"). |
| Q0.3 | a1f1a92 | A throw-away echo endpoint on the board, removed afterwards; a session from the board's own page with the certificate's hash in Chromium; 60 s of datagrams at 8 Mbit/s with 0 and 0.004% lost, 9.6% of a core, 2.4 MiB (`quic-spike-20261001*.json`). |
| Q0.4 | 6d9d5ca | The design note. |
| Q0.5 | e008b21 | Go: every condition met ("Decision"). |
| Q1.1 | 9f4602b | `bli-kvm-quic-protocol`; 100000 random and damaged inputs without a panic. |
| Q1.2 | 11a5ad8 | `bli-kvm-quic-gateway`: endpoint, certificate in memory, limits, control stream, server link. |
| Q1.3 | aa22a19 | The server's link with the peer check, tokens, session end, `GET /api/transport`. |
| Q1.4 | 50262d4 | The owner's switch as the unit's enablement; `quicBlocked`. |
| Q1.5 | 644eb95, 2e6d240 | Unit, system user in a sysusers fragment of its own, update runtime `0.1.0~dev35`, the SD image builder. |
| Q1.6 | d376b2b | The Transport section, the browser's choice, the page's session, English and Russian. |
| Q1.7 | 802dbaa | The page's contract against the real gateway (the plan's failure table), the switch enforced twice; `check.sh` green. |
| Q1.8 | 17ef10b, 318d6c4 | Slot 61 and the package on the board; the device probe twice (235 of 235 pings answered, round trip 2.8 to 3.3 ms, logout ends the session, the port silent after the switch went off); GUI walks (`quic-transport-device-20261001-*.json`, `gui-walk-seq61-*.json`). |
| Q2.1 | f26b9a8 | Lease requests of a QUIC session served by the code behind `/api/hid/lease`; a session's end gives the lease back. |
| Q2.2 | dcc8f2f | The gateway's input stream: order, bounded queue, newest position. |
| Q2.3 | a3d218a | The lease session over HTTPS or QUIC, chosen when control is enabled. |
| Q2.4 | 58a8f79, 114179f | Contracts on the host, the whole stack with a stand-in owner, the relays of `tools/net`, the latency table (`quic-input-latency-host-20261002-a.json`). |
| Q2.5 | 671c211, 02ad3a2 | Slot 62; a report without a confirmation is refused in the words HTTPS gives, 2.2 to 2.5 ms against 15.3 to 15.7 ms; the HID ledger did not change (`quic-input-device-20261002-*.json`). |
| Q3.1 | ab42336, 1a5be3e | The encoder's frame socket; key frame on request, at most every 250 ms; a still screen answers at once; the unit's task limit. |
| Q3.2 | 6f275cb, 277541b | A watching session holds the encoder's demand and the quality; the demand returns with an encoder that was started again. |
| Q3.3 | 428ce20 | Key frames on streams, delta frames as datagrams, newest wins, requests, counters. |
| Q3.4 | 9b193a5, 134c32e | Receiver and decoder in a worker, the canvas, "H.264 (QUIC)" in the Screen menu, fallback to WebRTC with the reason, the counters. |
| Q3.5 | b41a1b9, 134c32e | Receiver tests with 100000 damaged cases; the contract in which Chromium decodes the encoder's own stream through the gateway and recovers from loss; `check.sh` green. |
| Q3.6 | 9fff9cc, 5a43c90, a890661 | Slot 64 and the encoder on the board; the device probe direct, with 1% and 3% loss and with 50 ms; H.264 over WebRTC beside it; CPU and memory (`quic-video-device-20261002-seq64-*.json`, `h264-webrtc-seq64-*.json`). A moving picture, which the board's target did not give, on the host (`quic-video-host-20261002-a.json`). |
| Q4.1 | 1d4fcf9 | The comparison "Standard against QUIC"; the firewall contract, the administration features note, the video design note, `docs/licensing.md` and `docs/third-party-rust.md`. |
| Q4.2 | c74703c | GUI walk desktop and mobile, VP8 and H.264 over WebRTC, the touch reach probe: all passed on slot 64 (`gui-walk-seq64-*.json`, `vp8-webrtc-seq64-*.json`, `h264-webrtc-seq64-*.json`, `touch-reach-seq64-*.json`). Firefox 156 checked (`quic-firefox-device-20261002-seq64.json`); Safari not at hand. |
| Q4.3 | this note | The end state below. |

## Found on the way

1. A gateway that stopped kept its place on the server's link while it let
   its connections drain, so the server handed out tokens for an endpoint
   that no longer answered. It now leaves the link first (Q1.7's contract
   found it).
2. The runtime's sysusers file is pinned by hash; the gateway's user went
   into a fragment of its own instead of changing it.
3. On the board x264 could not be opened for the first QUIC viewer: an
   encoder for a new size was made while the old one lived, and the unit's
   16 tasks were not enough for both. The kernel's log had the same refusal
   once before this plan.
4. After the encoder had been started again, its viewers' demand was gone
   with the old process, and a QUIC viewer waited for a picture without end.
   The server now holds the demand again, and the page gives a picture up
   after ten seconds without a frame.
5. The board's target showed a still screen during the picture's probes. A
   still screen gave a new viewer its first picture up to 2 s late; the
   capture loop now lets a frame through for it. It also meant that the
   board's numbers say nothing about a moving picture under loss: the host's
   do.
6. The development host's clock changes its rate every few seconds. A
   frame's age is therefore read between two answered pings and not from a
   line through many of them.
7. The design note said in Q0 that no Firefox was installed on the
   development host. One was; it is checked now and the note is corrected.
8. The encoder binaries deployed to the board before this plan carried 19
   paths of the build machine in their panic messages. The build now maps
   them away like the other binaries' and passes `verify-aarch64-elf.sh`.
9. Delta frames are never sent again, as in the reference. On a path that
   loses 1% of its datagrams a moving picture of 30 frames a second keeps
   23, with 3% lost 18. This is the main argument for leaving Standard the
   default.

## Not shown

- A moving picture on the board, clean and under loss: the target stood
  still, and the probes may not touch the keyboard or the mouse.
- Control over QUIC with a real lease on the board: by the plan's rule the
  probes never take one. The lease path is tested on the host against the
  real server and gateway.
- The restrictive firewall modes on the board: the transport ran in
  Baseline only; `quicBlocked` and the refusal are tested on the host.
- Safari, and phones.

## Board end state (2026-10-02)

As found before Q3 and compared field by field afterwards
(`quic-q3-state-before-20261002.json`, `quic-q3-state-after-20261002.json`):
the QUIC switch off (unit disabled and inactive, nothing on UDP 443), the
terminal, scripts, remote download and PicoClaw switches off, the switch for
RAW system updates on as the operator left it, firewall Baseline, video at
60 frames a second, quality 100, 1920x1080, WebRTC quality 2.5 Mbit/s,
session lock 3600 s, SSH enabled, the operator's two images in the catalog,
no drive attached. The HID ledger is the file it was. No unit, file or port
of a probe is left.

## Left to the operator

- A check from the phone (Chrome on Android) with control enabled: typing
  and the mouse over QUIC, and the H.264 picture over QUIC with something
  moving on the target.
- A check from an iPhone if it matters; Firefox on a phone likewise.
- Whether QUIC becomes the default, and whether Restricted should admit UDP
  443 from the local ranges.
