# Platform control protocol v2 (HID leases and ATX confirmation)

Status: design for the HID/ATX mutation path. No capability is enabled by this
document. Protocol v1 remains the denial-only contract, and shipped builds keep
HID, ATX and virtual media unavailable until every gate passes. The control
broker now decodes v2 frames and routes them through a compile-time-disabled
`ControlV2State`, which rejects every v2 request with `capability_disabled` and
reports a fixed rejection after the response write.

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
  The implemented operator flow is `GET /api/hid/focus` (session-bound,
  one-time, 30-second challenge) followed by `POST /api/hid/focus` with the
  challenge, target identity, operation ID and explicit confirmation. The
  endpoint requires the authenticated session and CSRF token, and reports the
  broker's fixed rejection unchanged; the capability stays unavailable.
  `POST /api/hid/lease` then carries `acquire`, `renew`, `report` and `release`
  using only the bindings remembered from a successful confirmation; a lease
  request without a current confirmation is rejected before any broker call,
  and an acquire returns the broker-minted lease ID. The response echoes
  `ttlSeconds` for acquire and renew so the client can renew before expiry; a
  report or release carries no TTL.
- One attestation per session: a new `target_attest` replaces the previous one,
  `target_release` drops it, and both expire at `ttl_seconds`. An expired or
  replaced attestation fails closed.

## HID leases

- `hid_lease_acquire { session_binding, target_binding, ttl_seconds }` is
  answered with `lease_granted { lease_id }`, one opaque non-zero 128-bit
  identifier minted by the broker; `ttl_seconds` is at most 30. Acquisition
  requires a current attestation matching the exact bindings.
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
The broker can attach the persisted state machine only under the
`atx-transition-profile` feature, using the reviewed eligibility constructor
with explicit backend and physical-readiness inputs; the shipped broker
rejects every ATX request with `backend_unavailable`. The server exposes
`POST /api/application-power` with a typed action and operation ID; it uses
only the session/target bindings remembered from a successful safe-target/focus
confirmation, requires a current confirmation, and returns `applied` or the
fixed broker rejection so the interface can show indeterminate and cooldown
states.

## Virtual media request

The v2 protocol now carries a typed virtual-media request:

```
VirtualMedia { request: MediaRequest }
MediaRequest {
  command: VirtualMediaCommand,   // select { imageId, mode, writable }
                                  // | mount { imageId, mode, writable }
                                  // | set_connected { connected }
                                  // | reconnect
  sessionBinding: u64,
  targetBinding: [u8; 32],
  issuedAtUnixMs: u64,
}
VirtualMediaObserve { request: MediaObserveRequest }
MediaObserveRequest {
  sessionBinding: u64,
  targetBinding: [u8; 32],
  issuedAtUnixMs: u64,
}
```

Validation requires non-zero session and target bindings, a non-zero issued
timestamp and a valid command; there is no path, URL or filename field. A
`mount` selects the image and attaches it as one recorded operation, so the
reference single-call mount does not trip the per-operation cooldown;
`reconnect` re-enumerates the gadget without changing the selection. Shipped
builds reject both requests with `backend_unavailable`; the feature-gated media
profiles attach the persisted media engine and the board owner executes them on
the Rust gadget (the KVMD profile maps `mount` onto its two fixed routes and
rejects `reconnect` as unsupported).

## Responses

A response is `applied`, `lease_granted { lease_id }` with a non-zero
128-bit identifier, `media_state { file, connected, cdrom, writable }` for a
`VirtualMediaObserve` request (the file is a bare image name, never a path), or
one of the fixed codes: `capability_disabled`,
`not_armed`, `not_attested`, `lease_conflict`, `lease_expired`,
`cooldown_active`, `confirmation_stale`, `duplicate`, `pending_unreconciled`,
`invalid_state`, `backend_unavailable`, `backend_rejected`, `internal`.
Backend messages, paths, credentials and raw payloads are never returned. As
in v1, the broker may make one non-blocking diagnostic report after the
response write, and diagnostics can never change a control response.

## Dispositions and required gates

- Shipped builds keep HID, ATX and virtual media compile-time disabled. A
  separate test profile may enable one capability at a time for device gates.
  The server advertises HID and power `read_write` only in a
  `restricted-control-surface` build with an explicit opt-in and a configured
  control socket; every other build keeps them unavailable, so advertising
  cannot be turned on by configuration alone.
  The broker can attach a HID control policy only under the
  `hid-transition-profile` feature; even there each acquire, renew and report
  requires a current target attestation, a release stays unconditional for
  safety, and an acquire mints the returned lease identifier.
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

## Test-profile environment

The restricted-control test profile is a separate build, never a shipped or
released artifact. It compiles `restricted-control-surface`,
`hid-transition-profile` and `atx-transition-profile` together and selects the
backends from fixed environment keys:

- application server: `BLI_KVM_CONTROL_SOCKET` (absolute control socket) and
  `BLI_KVM_RESTRICTED_CONTROLS=1` (explicit opt-in). Without both, HID and
  power stay `not_exposed` even in a feature build. Virtual-media writes need
  the separate `BLI_KVM_MEDIA_CONTROLS=1` opt-in; the read-only catalog uses
  the absolute `BLI_KVM_MEDIA_CATALOG` path.
- control broker HID backend: `BLI_KVM_HID_PROFILE=1`, `BLI_KVM_HID_LEDGER`
  and `BLI_KVM_HID_KVMD_SOCKET`, all absolute.
- control broker ATX backend: `BLI_KVM_ATX_PROFILE=1`, `BLI_KVM_ATX_JOURNAL`
  and `BLI_KVM_ATX_KVMD_SOCKET`, all absolute. The optional absolute
  `BLI_KVM_ATX_RECONCILIATIONS` selects a reviewed startup reconciliation file
  whose format is documented in `docs/api/atx-transition.md`.
- KVMD credential: delivered by systemd through `CREDENTIALS_DIRECTORY` as
  `kvmd-user` and `kvmd-password`; every attached backend reads its own
  instance and the broker refuses to start without the directory.

When both profile selects are present the broker serves HID and ATX from one
state: HID leases still require a current target attestation and ATX actions
still require a reviewed confirmation. Reconciliation input for the test
profile is empty by design; a pending operation is reported as
`pending_unreconciled` until it is reviewed. The profile bundle is labelled
`restricted-control-test` with `releaseAccepted` and `deviceAccepted` false.

## Boundaries

- No generic method, command, path, URL, filename, device name, GPIO number,
  duration or shell field is added.
- No device state is polled by the protocol; ATX reconciliation uses the
  existing read-only LED/state evidence outside the control messages.
- Virtual media is unchanged in v2; its mutation path remains a separate slice.

## Read-only VM info

`GET /api/vm/info` and `GET /api/vm/hardware` are read-only. `info` reports the
mDNS name, image version, application version and uptime; `ips` and `deviceKey`
are intentionally empty because the unprivileged network process does not
enumerate link addresses and the hardened build does not expose a unique device
identifier. Operators read addresses from the authenticated inventory.
