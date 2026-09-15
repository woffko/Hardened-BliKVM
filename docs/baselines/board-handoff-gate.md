# Board-owner handoff gate (real device)

Status: passed 2026-09-15 on the H616 board (`10.0.77.118`, Armbian Jammy).

## What the gate proves

The reversible gate stops KVMD, starts the Rust board owner from the verified
test bundle, and verifies that the owner serves its control socket. The
composite gadget is created on demand by the first attach, so gadget, media and
HID state are asserted by the later media/HID gates, not here.

## Procedure

1. `tools/capture_device_handoff_preflight.py` -> `handoffReady=true`
   (`evidence/private/handoff-preflight-20260915-a.json`; KVMD and vendor daemon
   present, `kvmd` gadget bound, configfs mounted).
2. Copy the verified bundle to
   `/var/lib/hardened-blikvm-gadget/gate/bundle.tar.gz`
   (`sha256 6873ac4836f82d2237d2e3babd26c92c0c34ac62e20f3e71f6ecb43944b05c20`).
3. `tools/stage_device_board_profile_gate.py --confirm-handoff`
   (`evidence/private/board-handoff-gate-20260915-e.json`): `kvmd_was_active=yes`,
   `kvmd_stopped=yes`, `owner_started=yes`, `socket_present=yes`.
4. `--teardown` (`evidence/private/board-handoff-teardown-20260915-a.json`):
   `owner_stopped=yes`, `hardened_gadget_removed=yes`, `runtime_removed=yes`,
   `kvmd_restarted=yes`; KVMD back `active`, `kvmd` gadget rebound, UDC present,
   the gate directory and control socket removed.

## Fixes found by the real device run

- `/run` is mounted `noexec`, so the broker cannot execute from there; the gate
  stages under `/var/lib/hardened-blikvm-gadget/gate`.
- A crashed owner leaves a stale control socket; the gate clears it on start and
  teardown.
- Stopping KVMD leaves its `kvmd` gadget directory, so the gate looks for the
  `hardened` gadget explicitly.

## Not covered here

- ATX pulse timings are operator facts and were not exercised; the gate used
  placeholder values inside its own runtime directory.
- Media attach/eject and HID reports through the owner; ATX pulse timing.
