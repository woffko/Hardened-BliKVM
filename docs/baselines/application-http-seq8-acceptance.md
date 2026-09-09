# Installed HTTP/GUI application update acceptance

Accepted on 2026-09-09 on the enrolled H616 test device. This is the
same-boot, temporary-write happy path, not permanent write enablement or a
complete release/physical acceptance matrix.

## Exact inputs

- Acceptance source: `dc848f5` (controller foundation `48012b1`).
- Retained updater: `0.1.0~dev9`; no updater package was installed in this gate.
- Application archive: `hardened-blikvm-application-0.1.0-dev6-seq8.tar`.
- Archive size: 8,724,480 bytes.
- Archive SHA-256:
  `d2acf270fa2aa9cb924c362066ece50f4b0fbd4c4c8b2975648dab5ac0b4f849`.
- Manifest SHA-256:
  `375f9800a147b8f273073349af1613e3fb0cf9f7b97af914d7a75e9446ea694b`.
- Signed slot:
  `app-00000000000000000008-375f9800a147b8f273073349af1613e3fb0cf9f7b97af914d7a75e9446ea694b`.

The archive had already been published through the installed native upload
worker. This gate expected `already_present`, not a new ninth slot.

## Observed result

The confirmed baseline was sequence 7, floor 7, generation 37. The controller
verified installed package bindings, all eight immutable slots, account/TLS/
trust/configuration hashes, Nginx and peer identities, and persistent status
wiring. It armed its independent recovery timer before installing only the
owned `/run` write-environment overrides and starting the two write sockets.

The actual GUI performed one login, one upload, and one activation request.
The browser selected and SHA-256-verified the real archive File, received the
exact signed `already_present` receipt, displayed the current/target slots and
generation 37 in the confirmation dialog, and received HTTP 202 with a valid
`awaiting_health` receipt. There were no blocked extra mutations or page errors.

The root controller separately established confirmed sequence 8, floor 8,
generation 40, previous slot 7, no pending operation and no rollback. It closed
write admission, allowed workers and apply jobs to settle naturally, removed
the exact owned overrides, and restarted the confirmed server. Only the
selected symlink and authoritative journal were permitted to change; all
eight signed trees, other persistent state, configuration and peer identities
were preserved. No slot was deleted, and the confirmed floor was not lowered.

A fresh browser login after restoration verified three exact status reads,
desktop and mobile panels, and two Refresh checks. No upload/activation
controls or requests remained in that restored interface. The root cleanup
then removed its validated guard and temporary controller staging.

An independent read-only `seq8` observation subsequently rechecked the package,
confirmed status, rollback anchor, persistent status wiring, absent write
environment in both loaded units and the running server, inert write sockets,
and absence of HTTP guard files. Its temporary probe staging was removed.
Neither this observation nor the acceptance requested a reboot or target
Power/Reset action.

## Evidence

Private receipts are kept under `evidence/private`; credentials and raw browser
headers/bodies are not published.

- `installed-application-http-gate-attempt5-20260909.json`, SHA-256
  `770beefaa3169b0aeec0c86cd87972242c7280e615f4f9329546170ab63d6079`.
- `application-http-seq8-independent-retained-20260909.json`, SHA-256
  `d114360c02cf0907a890573efd3bf5112b902b0c5b555184bba11c18f51e0ca4`.

The complete host check passed again after the independent seq8 observer was
added: Rust checks/tests/Clippy, Python tests and type checks, browser contracts,
and web build/lint/format gates. This is separate from the device evidence.

Earlier probe runs stopped before activation and each restored sequence 7
and closed writes, followed by independently verified cleanup. They exposed
test-observer issues: the login title precedes its lazy form, File-backed XHR
post data is opaque to interception, and Chromium can evict the response from
inspector cache after a large upload. Local real-browser reproductions and
regression tests addressed these without changing application or proxy logic.
The final probe observes only update-route XHR `loadend` responses; it does not
rewrite requests/responses or capture login data.

## Remaining boundaries

HTTP writes are **disabled** in the retained deployment; read-only status stays
enabled. This result does not establish permanent write-policy acceptance,
HTTP negative/fault-injection coverage, deadline expiry during an in-flight
activation, sequence-eight reboot persistence, resource bounds, another
hardware sample, or a recovery/system-image release.
