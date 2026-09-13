# Platform control protocol v2 (HID leases and ATX confirmation)

Status: design for the HID/ATX mutation path. No capability is enabled by this
document. Protocol v1 remains the denial-only contract, and shipped builds keep
HID, ATX and virtual media unavailable until every gate passes.

## Framing, versions and identity

v2 reuses the v1 framing: exactly one four-byte big-endian length prefix
followed by at most 4096 bytes of JSON. `schemaVersion` must be exactly `2`;
a v1 endpoint rejects v2 messages and a v2 endpoint rejects v1 messages.
Unknown fields, unknown variants, malformed frames, duplicate JSON keys,
non-finite numbers and request id 0 are rejected. `operationId` stays a
non-zero 32-character lowercase-hex replay identifier for every mutation.

## Attestation authority

The broker never trusts caller-supplied session or target bindings. Mutating
HID and ATX requests require a bounded, expiring target attestation:

- `target_attest { session_binding, target_binding, focus_binding,
  ttl_seconds }` establishes one attestation for the authenticated connection.
  Bindings are non-zero (64-bit session, 256-bit target, 128-bit focus) and
  `ttl_seconds` is bounded to 5..60.
- Authority: the network server submits the attestation over the privileged
  control connection only after an explicit operator safe-target/focus
  confirmation for that authenticated session. The broker verifies the peer
  identity (dedicated server UID/GID through `SO_PEERCRED`) and the request
  shape; it never derives a binding from a HID report or HTTP field. The
  server derives the three bindings with `bli-kvm-control-focus`, which mixes a
  domain-separated SHA-256 over the authenticated session identity, the hashed
  target identity, a fresh focus nonce, and the operator confirmation time.
- One attestation per session: a new `target_attest` replaces the previous one,
  `target_release` drops it, and both expire at `ttl_seconds`. An expired or
  replaced attestation fails closed.

## HID leases

- `hid_lease_acquire { session_binding, target_binding, ttl_seconds }`
  returns one opaque non-zero 128-bit `lease_id`; `ttl_seconds` is at most 30.
  Acquisition requires a current attestation matching the exact bindings.
- `hid_lease_renew { lease_id, session_binding, target_binding, ttl_seconds }`
  requires the exact same identity; the lease is never transferable.
- `hid_report { lease_id, session_binding, target_binding, report }` uses the
  existing fixed `HidReport` shapes and the pinned translator table. Releases
  are emitted before new presses, unchanged reports emit nothing, and an
  invalid report, identity mismatch, expiry, disconnect or backend failure
  revokes the lease and emits release-all for every held key and button.
- `hid_lease_release { lease_id, session_binding, target_binding }` is
  idempotent for the same identity and never affects another session.
- Exactly one lease exists device-wide. The `hid-transition` release ledger is
  mandatory: the union of old and new release sets is persisted before a
  held-state transition, the exact new set after backend success, and on
  startup the ledger is replayed as a pending-release queue before any new
  lease is granted.

## ATX confirmation

The ATX request keeps the fixed `power_short`, `power_long` and `reset`
actions and adds the persisted-policy fields: a non-zero operation id, the
session and confirmation bindings, and the confirmation timestamp. The broker
state machine is exactly the reviewed `atx-policy` sequence:

1. persist Pending before any backend call;
2. call only the fixed KVMD waiting routes (`power`, `power_long`, `reset`);
3. persist the completion and return the persisted result.

Duplicates and conflicts return the persisted result without another backend
call. Cooldowns are five seconds for `power_short` and thirty seconds for
`power_long` and `reset`. Startup reconciliation compares read-only LED/state
evidence before recording a Pending outcome and never retries automatically.

## Responses

A response is `applied` or one of the fixed codes: `capability_disabled`,
`not_armed`, `not_attested`, `lease_conflict`, `lease_expired`,
`cooldown_active`, `confirmation_stale`, `duplicate`, `pending_unreconciled`,
`invalid_state`, `backend_unavailable`, `backend_rejected`, `internal`.
Backend messages, paths, credentials and raw payloads are never returned. As
in v1, the broker may make one non-blocking diagnostic report after the
response write, and diagnostics can never change a control response.

## Dispositions and required gates

- Shipped builds keep HID, ATX and virtual media compile-time disabled. A
  separate test profile may enable one capability at a time for device gates.
- Before HID can be advertised: dedicated transition credential and systemd
  sandbox separate from the network server; server-mediated safe-target/focus
  flow and capability advertisement; GUI key/button set aligned to the
  translator table; H616 startup-ledger replay and disconnect/expiry gates;
  keyboard and mouse modes, rollover, 10,000-event soak, reconnect, BliKVM
  reboot and stuck-input recovery.
- Before ATX can be advertised: reviewed eligibility constructor, broker state
  machine, startup reconciliation, and live pulse timing, busy, duplicate,
  cooldown, restart and recovery gates with physically disconnected Power and
  Reset.

## Boundaries

- No generic method, command, path, URL, filename, device name, GPIO number,
  duration or shell field is added.
- No device state is polled by the protocol; ATX reconciliation uses the
  existing read-only LED/state evidence outside the control messages.
- Virtual media is unchanged in v2; its mutation path remains a separate slice.
