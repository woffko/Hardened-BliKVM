# Installed production-key application update gate

Prerequisite: retained dev4, enabled idle watchdog/socket and the confirmed
sequence-one production slot. This test changes only application-update state
and the selected server service; the payload is identical to the already
accepted application. No HID, media or Power/Reset action is involved.

Sequence-two bundle SHA-256:
`fe3680eb1607b2ec5c957916b0b343c55390171e873ad978f0f521b0c2d42778`.
Manifest SHA-256:
`5df0c19a152415011fe2fa3f9510fbd1d4c8d02d79c7db8a6619fa225cb41f53`.
Slot: `app-00000000000000000002-5df0c19a152415011fe2fa3f9510fbd1d4c8d02d79c7db8a6619fa225cb41f53`.

Both builds use the accepted runtime package from `531f5f1`, the backed-up
`app-production-2026-09-05-01` key and the explicitly acknowledged passwordless
policy. Archives match byte-for-byte (8,499,200 bytes). The 27 payload files
match the existing application; only the signed sequence/manifest changes.
The private key never leaves its host store. Artifacts are beneath the approved
build root in `production-application-531f5f1-seq2-{a,b}/`.

Host receipt `evidence/private/production-bundle-seq2-host-verification.json`
proves Rust signature/tree verification, staging idempotency, owner/read-only
metadata, rejection of altered payload and wrong public key, and disposable
root cleanup without executing payloads. This host test initializes a fresh
sequence-two state; it does not claim to exercise a live one-to-two update.

Required live sequence:

1. Verify the exact installed updater tree, enabled services, current signed
   slot/floor one, absent pending requests and unchanged TLS/accounts/trust.
   Capture actual Nginx/broker/video PIDs and serving identity. Stage the bound
   sequence-two bundle under updater identity without changing `current`.
2. Arm bounded durable recovery before any temporary unit override or service
   activation. For the timeout rehearsal only, use a fixed runtime candidate
   drop-in that replaces the health-reporting post command with `/usr/bin/false`.
   Preserve the signed candidate payload and never forge health confirmation.
3. Invoke the actual activation template. Expect temporary GUI unavailability,
   followed by the real watchdog's health timeout, restored slot one and floor
   one. Journal generation/history legitimately change; never compare them to
   the pre-test generation or restore an older journal copy.
4. Remove only the exact temporary override, reload definitions, independently
   verify recovery and exercise GUI login, diagnostics, video/reconnect/logout.
5. Activate the same staged sequence-two payload normally. Verify candidate
   health confirmation, consumed one-use token, floor two, matching
   active/confirmed pointer, acknowledged apply request, stable clean ticks,
   preserved Nginx/peers/configuration and real browser behavior.
6. Remove guard/staging after independent verification. Once floor two is
   confirmed, do not undo it if a later GUI check fails: preserve authoritative
   state and repair forward. Recovery must respect the manager's current
   journal, not a stale host expectation or a forced pointer/journal rollback.

New status queries use `enable_update_services.current_status`, which does not
claim generation one and does not own/delete the watchdog runtime directory.
Idle-only verifiers remain useful solely before this transaction. Reboot and
interruption acceptance, including boot ordering, are separate required gates;
an isolated process test or persistent guard file does not prove them.
