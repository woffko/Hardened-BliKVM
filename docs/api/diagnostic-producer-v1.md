# Diagnostic producer protocol v1

This contract defines one fixed local producer event and its mandatory
collector response. The codec is implemented but unlinked. It does not create a
collector process, socket, service, public API, log reader, or capability.

## Trust boundary

The producer frame never carries source role, UID, PID, epoch, sequence,
timestamp, path, unit, message, request data, or arbitrary bytes. A future
collector must obtain `SO_PEERCRED` from the accepted Unix stream and map its
UID to exactly one configured process role before reading a frame.

Current sysusers allocate five distinct non-root service accounts dynamically,
but all services run with shared group `hardened-blikvm`. Group access can limit
socket admission but cannot prove role. Startup must resolve the five exact
accounts, require five distinct nonzero UIDs, and reject an unknown, duplicate,
root, or collector UID. The kernel peer UID, not a producer field, selects the
source. Only one active connection per role is permitted.

The separate unlinked `bli-kvm-diagnostic-peer-policy` now enforces the numeric
portion of this boundary: six distinct nonzero UIDs, exact nonzero shared GID,
positive bounded PID, root/collector/unknown rejection, and source-only output.
User/group resolution and safe kernel credential retrieval remain adapter work.

The unlinked `bli-kvm-diagnostic-peer-adapter` now implements that narrow
adapter with fixed names, safe nix lookup, current effective-UID validation, and
Tokio `UnixStream::peer_cred`. It returns only the source and has no listener,
runtime, logging, configuration, or fallback identity.

The collector assigns lifecycle `Started`/`Stopped`, epoch, sequence, and
relative monotonic offset at accept, event acceptance, and EOF. A producer
cannot encode lifecycle events. The four long-running services should retain
one connection for their process lifetime; the update supervisor is a oneshot
source and receives a fresh collector-owned epoch per invocation.

## Event frame

Every event frame is exactly eight bytes:

| Offset | Meaning |
| --- | --- |
| 0 | schema version, exactly `1` |
| 1 | event tag |
| 2 | argument A |
| 3 | argument B |
| 4..7 | reserved, all exactly zero |

Tags and canonical arguments:

| Tag | Event | Argument A | Argument B |
| --- | --- | --- | --- |
| 1 | ready | 0 | 0 |
| 2 | unavailable | 0 | 0 |
| 3 | request rejected | reject reason 1..7 | 0 |
| 4 | retry scheduled | attempt 1..16 | 0 |
| 5 | operation finished | result 1..4 | 0 |
| 6 | resource pressure | resource 1..4 | level 1..2 |
| 7 | journal observed | health 1..3 | 0 |

Reject reasons are unauthorized, capability disabled, backend unavailable,
busy, invalid input, rate limited, and conflict in that order. Operation results
are applied, rejected, noop, and rolled back. Resources are memory, storage,
temperature, and sessions. Pressure levels are elevated and critical. Journal
health values are healthy, degraded, and recovery required.

All other tags, arguments, lengths, schema values, and nonzero reserved bytes
are invalid. The collector closes an invalid connection without returning a
response. It never scans forward for another apparent frame.

## Mandatory response

The producer sends only one event at a time and waits for one exact four-byte
response before sending the next event:

| Offset | Meaning |
| --- | --- |
| 0 | schema version, exactly `1` |
| 1 | response tag: accepted `1`, rejected `2` |
| 2 | accepted: zero; rejected: reason 1..3 |
| 3 | reserved, exactly zero |

Rejection reasons are event not allowed, backpressure, and collector
unavailable. `Accepted` may be sent only after the collector policy has
committed the event and advanced state. A rejection means the event was not
accepted and state did not advance. Pipelining is forbidden, so no producer
request identifier or correlation field is needed.

Protocol-invalid frames are connection-fatal and receive no response. A future
runtime must bound read/write time, close partial frames, reject a second active
peer for the same role, and never turn protocol text into a log message.

## Unavailable runtime

This protocol does not encode window rollover or persistence. The separate
unlinked `bli-kvm-diagnostic-window-policy` now defines a 32-record live window,
lifecycle reservation, one sealed-window handoff, exact acknowledgement,
backpressure, visible incomplete disconnects, manager restart, and monotonic
clock rollback. A runtime must still supply peer credentials, a fresh instance
binding, real monotonic time, bounded I/O, and concurrency without weakening
that policy. No raw vendor log adapter is permitted.
