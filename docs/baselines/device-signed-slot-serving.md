# Signed-slot GUI serving checkpoint

The test H616 now serves its GUI from the retained signed initial application
slot through `hardened-blikvm-server-current.service`. The package-backed
`hardened-blikvm-server.service` is disabled and inactive. The selected slot is
`app-00000000000000000001-7a088d20f639fabf5ed4f59b7223cd34b8f39cb9fe2c5764acff6df1789fbc6f`;
the journal remains generation/floor one, with no pending activation.

The local Nginx unit no longer requires the old server and is ordered after
current/candidate without coupling its lifetime to either backend. Its SHA-256
is `8eb92de0e770e6fc4375b86766ba35b2893825eacd8114bf2beb804a8c7ff3b7`.
Nginx kept its PID throughout both switches and the explicit rollback.
Account, TLS, public trust, broker and video services remained unchanged.

Full host job `55c0dae6de824f7aa3faedd8ecc0cd59` passed for the cutover
implementation. The first preflight exposed an invalid systemd query against
an empty `@.service` template; source `75026ef` checks enablement on templates
and activity on concrete instances instead. Preflight
`258d131392a44d3686f4c6ef2cb09246` then passed.

Rehearsal prepare `b2b170992b664b079bcb4b80de6b9736` started the signed server.
Browser job `2af9c9bd391d4ea88fc1aadcee0de086` passed login, inventory,
Diagnostics, decoded video, reconnect and logout stream revocation. Explicit
rollback `947dd0c32bea43bfb535d433cdd8e568` restored the original Nginx unit
and package-backed server; `165eafb9a2ea4047aed2e20f614ffa86` independently
verified restoration and removed the rehearsal guard.

Retaining prepare `2be2c3e180fd4bcd8c62b937ed57bedf` passed. Its first
browser attempt failed in the combined reconnect/decode/old-stream-release
phase. Source `f6f7534` now tracks the last active MJPEG response before reload
and identifies old-stream release as a separate phase. Browser retry
`fb57057979534354ac5ef846f3bb3018` passed with four MJPEG responses/reconnects,
decoded frames, secure cookies, logout revocation, cleared session, no control
requests and no WebSockets. The host commit gate binds browser acceptance to
the exact serving guard ID.

Commit `f53ef4bdd0dd4d43b6e0494a41e4ff37` retained the switch and disarmed
deadline recovery. Cleanup `d5141b5db5e84aefa51879770cc6ad93` repeated
verification in a new SSH transaction and removed the guard/controller staging.
The confirmed serving PID was 405007 and its executable SHA-256 was
`5f2da39a6563dff23503ec4ebe7c461775b579443dbae3e10c58821d22618fbc`.
Private receipts are `device-serving-*.json` under `evidence/private/`.

## Remaining updater boundary

The watchdog timer, confirmation socket and candidate infrastructure remain
disabled. Source inspection before enabling them found that watchdog
`OnSuccess` invokes fixed apply units whose commands unconditionally restart
the selected server. With the two-second timer, successful clean watchdog ticks
would repeatedly restart the GUI. Idempotent service reconciliation must be
implemented and tested before activating that timer. Candidate confirmation,
timeout rollback, signed update API/GUI, restart persistence and remaining KVM
control adapters are not accepted by this serving checkpoint.
