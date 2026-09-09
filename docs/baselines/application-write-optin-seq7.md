# Application write opt-in: sequence seven

This candidate adds the explicit server write opt-in but is to be installed
with the retained status-only configuration. Native upload/activation and GUI
write acceptance remain separate subsequent gates. No system-image or target
power action is included.

## Reproducible candidate

- Source and package builder: `7c813a7844b75774fea451516fc2b82d6a579d13`.
- Independent clean AArch64 builds: byte-identical prototype archives,
  SHA-256 `e3d730a2559391f48eac4efeb4bd95fec7bc286a10b03668f3b5f7446008d142`.
- Independent inert runtime packages: byte-identical,
  SHA-256 `da51a1990e39cbc7889cd369a599ddd5a62b1b2544237cb96d30bf2287fd67fb`.
- Independent signed application archives: byte-identical, 8,724,480 bytes,
  SHA-256 `ad36542bf330440717bf5b2ac0279258dfbd98e32834a6cf2a47f39722045a0f`.
- Application version `0.1.0-dev6`, sequence `7`, minimum system sequence `1`.
- Slot `app-00000000000000000007-f2262e8f9929dda83b2baf1005b5ae9262aa075b7ddaa16f4ad6200b5daf0140`.
- Server SHA-256 `b2e1350e997988af816d2c710323b3220b2d1a88ad742873d700e012ae2bb324`.

The host Rust supervisor verified publication, idempotent repeat staging,
immutable payload hashes/modes, initial state, rejection of repeat
initialization, payload tamper and wrong key, and unchanged state after the
negative checks. No payload was executed or device contacted. Private receipt:
`application-write-optin-seq7-host-verification.json`.

## Installation boundary

The `seq6-to-seq7-write-optin` controller profile binds the existing sequence-six
source slot, the signed archive above, and the separately retained dev9 updater.
It checks both write sockets/workers remain inert and all write environment
settings are absent from current/candidate units and the serving process.
Persistent read-only status wiring is preserved. The guarded runner has an
explicit profile, phase-bound browser receipts, no automatic activation retry,
and a nondecreasing observed recovery floor. Its deadline and boot recovery
execute the same bound profile.

Installed health-timeout rehearsal, confirmed rollout, browser checks, and
guard cleanup have not yet been performed for this candidate. Passing host
checks does not imply those gates passed or writable services were enabled.
