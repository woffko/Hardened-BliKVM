# QUIC transport v1

Status: design for `docs/baselines/work-plan-20261001-quic-transport.md`,
written in its phase Q0, whose decision is still open. Nothing here is
implemented yet.

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

