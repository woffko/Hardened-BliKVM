# Installed production-key application update acceptance

The actual installed dev4 updater completed the sequence-one-to-two
application flow on H616 on 2026-09-05. The signed payload is unchanged from
the accepted runtime; only the signed application sequence changes. Bundle
bindings are in `../design/installed-application-update-gate-v1.md`.

- Timeout rehearsal `e866f5a1093541748c1610668f1a060a`: a temporary fixed
  health-post-command failure caused the real watchdog to restore slot one.
  Floor stayed one, generation became four, `lastRollback=health_timeout`.
- Restored GUI browser job `5f5f849bc6074749a561a6b0d8777160` passed login,
  Diagnostics, decoded video, four reconnect responses and logout revocation.
- Normal activation `c53bb6498f0240859a56f690013f8b3b` confirmed slot two,
  consumed the health token, acknowledged apply and raised floor to two.
- Slot-two browser job `9500b73965b0434099b0c44ceda503a6` passed the same
  phase-bound browser checks, with zero control requests.
- Commit `e267099852d5466bbe157915ae1a03ab` and independent cleanup
  `713dc83cba05452a90357979714a5a78` verified the stable result and removed
  guard units, temporary override and controller/package staging.

Final active/confirmed slot:
`app-00000000000000000002-5df0c19a152415011fe2fa3f9510fbd1d4c8d02d79c7db8a6619fa225cb41f53`.
Floor two, generation seven, no pending operation, no last rollback. Previous
slot one remains intact. Serving PID 546989 stayed unchanged through the
post-confirmation browser, commit and independent cleanup. Nginx, broker/video,
accounts, TLS and enrolled public trust were preserved throughout. No target
Power/Reset control occurred. Private receipts use
`evidence/private/device-application-update-*.json`.

Host test suite passed in `900344a09bc2417eaf6d09e35f1abee5`. No private
signing key was copied to the device; the counter was never restored from a
journal backup. Confirmed floor two is now the authoritative baseline.

Remaining: mandatory boot-time recovery/verification ordering before signed
server execution, confirmed and interrupted-update reboot acceptance,
signed-update API/GUI and remaining planned product functionality. The old
generation-one/inert-only test helpers must not be used against this state.
