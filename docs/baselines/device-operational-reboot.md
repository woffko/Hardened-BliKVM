# Clean operational reboot acceptance

The H616 completed one software reboot with dev5 and the confirmed signed
sequence-two application on 2026-09-05. Preflight
`4723ee0f9738403db655cacf95649d5c` verified the exact operational baseline.
Reboot job `f33cbca3144843c88c669621f83c68cc` requested reboot once,
observed a changed kernel boot-ID hash and verified the recovered runtime.

The mandatory recovery oneshot exited successfully at boot monotonic
27,787,995 microseconds; the observed current server started at 41,959,567.
The required dependency was present and verification preceded serving.
Application state remained generation seven/floor two, active and confirmed
slot two, no pending transaction, previous slot one. The full persistent
state/slot fingerprints, Nginx unit and account/TLS/trust hashes matched.
The filesystem returned `rw` without remount remediation. A subsequent
stability probe preserved the server PID; probe staging was removed.

Browser job `36a3826241ce440c80d863dabbf6bd5d` then passed login,
Diagnostics, decoded video, four reconnect responses and session-clearing
logout, with zero control requests. Receipts:
`evidence/private/device-operational-reboot-{preflight,accepted,browser}.json`.

No attached-target Power/Reset action or abrupt power loss was tested. Next is
software reboot while a new signed candidate is awaiting health confirmation,
requiring `boot_changed` rollback rather than merely a timeout. API/GUI update
integration and remaining planned product work remain outstanding.
