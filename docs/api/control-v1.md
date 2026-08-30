# Platform control protocol v1

This protocol is an internal Unix-socket contract for future HID, ATX, and
virtual-media brokers. It is not an HTTP API and its presence does not enable
any capability. The current H616 manifest keeps HID, ATX, and virtual media
unavailable.

## Framing and identity

Each message is exactly one four-byte big-endian length prefix followed by at
most 4096 bytes of JSON. Unknown fields, unknown variants, malformed frames,
schema versions other than 1, and request ID 0 are rejected.

Every request carries two independent identifiers:

- `requestId` correlates one connection and response;
- `operationId` is exactly 32 lowercase hexadecimal characters, is not all
  zeroes, and identifies the intended mutation for replay protection.

Requiring an operation ID is not itself idempotency. A runtime control broker
must persist or otherwise retain a bounded replay window and return the prior
result for a duplicate before any mutating backend can be enabled.

## Typed requests

The request union contains no generic method, command, path, URL, filename,
device name, GPIO number, duration, or shell field.

### HID

HID accepts one fixed report variant:

- keyboard: exactly eight bytes; byte 1 is reserved and must be zero; the six
  key slots reject rollover/error codes and duplicate non-zero keycodes;
- relative mouse: exactly four bytes; only five defined button bits are
  accepted;
- absolute mouse: exactly six bytes; only five defined button bits are
  accepted and both little-endian coordinates must be at most 32767.

Before a backend is enabled, the broker must require a short-lived HID lease
bound to one authenticated session and an explicitly safe target. Disconnect,
expiry, parser failure, or backend failure must release all pressed keys and
buttons. The current implementation has no such lease and therefore exposes no
HID route.

### ATX

ATX accepts only `power_short`, `power_long`, or `reset`. It does not accept a
caller-selected pulse duration or GPIO identifier. A runtime backend must add
explicit eligibility, confirmation, persisted idempotency, cooldown, monotonic
bounded timing, and inactive-level restoration. Physical Power and Reset are
excluded from the current test session.

### Virtual media

Media selection accepts only a non-zero opaque `imageId`, a `cd_rom` or
`removable_disk` mode, and a writable flag. Writable CD-ROM is invalid. The
broker must resolve the opaque ID through a private, verified catalog; it must
never treat it as a path. Connect/disconnect is a separate typed command.
Upload, remote download, path resolution, configfs mutation, and BIOS boot are
not implemented by protocol v1.

## Responses and errors

A response is either `applied` or a fixed rejection code:
`capability_disabled`, `not_armed`, `cooldown_active`, `invalid_state`,
`backend_unavailable`, `backend_rejected`, or `internal`. Backend messages,
paths, credentials, and raw payloads are never returned.

The first runtime gate for this protocol must be denial-only: all three request
families must receive `capability_disabled`, no backend socket or device may be
opened, and the existing read-only inventory and video paths must remain
unchanged.
