# Diagnostic sealed-window export protocol v1

This is a fixed local Unix-stream contract for one future authenticated
collector consumer. It is not an HTTP API, does not package the collector, and
does not expose live-window state. The current crate is a pure unlinked codec
with no socket, identity lookup, persistence, retry, allocation, or I/O.

## Request frame

Every request is exactly 16 bytes:

| Offset | Bytes | Meaning |
| ---: | ---: | --- |
| 0 | 1 | schema version, exactly `1` |
| 1 | 1 | operation tag |
| 2 | 2 | reserved, exactly zero |
| 4 | 8 | collector instance ID, unsigned big-endian |
| 12 | 4 | window ID, unsigned big-endian |

Operation tag `1` reads the current sealed window and requires both IDs to be
zero. Tag `2` acknowledges a sealed window and requires both IDs to be nonzero.
Unknown tags, wrong length, nonzero reserved bytes, and noncanonical bindings
are invalid and must become connection-fatal in the future adapter.

The instance ID is the collector process's nonzero OS-random binding. It
prevents a delayed acknowledgement for window `1` from a prior collector
process from clearing window `1` after a restart.

## Response frame

Every response has a 24-byte header followed by at most 8,192 typed export
bytes:

| Offset | Bytes | Meaning |
| ---: | ---: | --- |
| 0 | 1 | schema version, exactly `1` |
| 1 | 1 | response tag |
| 2 | 1 | omitted lifecycle-event count |
| 3 | 1 | record count, at most `32` |
| 4 | 8 | collector instance ID, unsigned big-endian |
| 12 | 4 | window ID, unsigned big-endian |
| 16 | 2 | payload length, unsigned big-endian |
| 18 | 6 | reserved, exactly zero |
| 24 | variable | typed deterministic diagnostic bundle |

Response tags are:

| Tag | Result | Canonical fields |
| ---: | --- | --- |
| 1 | no sealed window | zero IDs/counts and no payload |
| 2 | sealed window | nonzero IDs/count, exact payload, bounded omission count |
| 3 | acknowledgement applied | echoed nonzero binding and no payload |
| 4 | acknowledgement mismatch | echoed requested nonzero binding and no payload |
| 5 | collector unavailable | zero IDs/counts and no payload |

The public sealed-response encoder accepts only a `DiagnosticBundle`; callers
cannot pass an arbitrary byte slice. Decoding borrows the bounded payload and
validates every header field, exact total length, record-count bound, binding,
tag, and reserved byte. Raw request text, paths, identities, wall-clock values,
arbitrary filters, and user-controlled labels do not exist in this contract.

## Snapshot and acknowledgement semantics

Reading clones the current sealed snapshot and does not change collector state.
Acknowledgement clears state only when both the current instance ID and window
ID match. A mismatch or absence does not mutate the window.

The future consumer must durably retain a received bundle before acknowledging
it. If the acknowledgement response is lost, delivery is ambiguous: the
consumer must reconcile by reading again and must not assume success from a
transport failure. This v1 codec itself provides no retry, persistence, or
cross-restart durability claim.

The future adapter must authenticate an exact non-root consumer through kernel
peer credentials, bound partial-read/response-write time, permit only one
request and response per connection, and keep producer ingestion independent
of a stalled export consumer.
