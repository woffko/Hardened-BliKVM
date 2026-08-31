# Diagnostic producer protocol v1

This contract defines one fixed local producer event and its mandatory
collector response. The codec and its in-memory runtime composition are
implemented but unlinked. They do not create a collector process, socket,
service, public API, log reader, or capability.

## Trust boundary

The producer frame never carries source role, UID, PID, epoch, sequence,
timestamp, path, unit, message, request data, or arbitrary bytes. The transient
collector runtime obtains `SO_PEERCRED` from the accepted Unix stream and maps
its UID to exactly one configured process role before reading a frame.

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
Tokio `UnixStream::peer_cred`. It returns only an opaque authenticated-source
token and has no configuration or fallback identity. The token can be consumed
only once by the same crate's unlinked runtime core, which creates an opaque
instance-bound connection handle and never returns numeric credentials.

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

The unlinked `bli-kvm-diagnostic-producer-client` implements this exchange on
one serialized persistent connection to the fixed collector socket. It returns
invalid event/connect failures as definitely not sent. Any failure after frame
writing begins is delivery-ambiguous, discards the stream, and is never retried
automatically. A new explicit call may reconnect, but cannot claim whether the
prior ambiguous event was accepted. See
`../baselines/diagnostic-producer-client.md`.

The same unshipped crate now provides a bounded reporter for callers that may
not wait on diagnostics. It uses an eight-event fixed ring, immediate typed
drop receipts, one ordered no-retry worker, saturating semantic counters, and a
three-second shutdown ceiling. See
`../baselines/diagnostic-producer-reporter.md`.

Protocol-invalid frames are connection-fatal and receive no response. The
transient runtime bounds a started partial frame and response write to five
seconds, closes partial frames, rejects a second active peer for the same role,
and never turns protocol bytes into a log message. An idle long-running peer
may retain its one fixed role slot for its process lifetime.

## Unshipped process runtime

This protocol does not encode window rollover or persistence. The separate
unlinked `bli-kvm-diagnostic-window-policy` now defines a 32-record live window,
lifecycle reservation, one sealed-window handoff, exact acknowledgement,
backpressure, visible incomplete disconnects, manager restart, and monotonic
clock rollback. `DiagnosticCollectorRuntime` now composes a mapped peer, one
request/response at a time, connection termination, OS-random instance binding,
real monotonic time, concurrent role sessions, periodic ticks, and graceful
cleanup around a caller-owned pre-bound Unix listener. A process runtime must
own the fixed socket path, mode, account, and service lifecycle without
weakening that policy. The unshipped collector now implements and has passed
that isolated H616 boundary; see
`../baselines/device-diagnostic-collector.md`. Producer integration, export
handoff, packaging, and an authenticated API/UI remain absent. No raw vendor
log adapter is permitted.
