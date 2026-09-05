# Interrupted application reboot acceptance

On 2026-09-05 job `d5202e51bca34baea7593829f0457c17` captured a real
sequence-three `awaiting_health` transaction with 59,402 milliseconds left,
then requested exactly one software reboot. The signed candidate was unchanged;
only a marker-owned runtime health-post-command delay was used.

After the new boot, the installed recovery path restored active/confirmed
sequence two, kept floor two, advanced journal generation from nine to ten and
recorded `lastRollback=boot_changed`. Pending state was absent and previous
slot was the rejected sequence-three candidate. Verification preceded server
startup; signed payload fingerprints and account/TLS/trust were preserved.
The runtime override, temporary source and persistent probe staging were removed.

Browser job `1f9c871085444e5cb946c4526a75db99` subsequently passed login,
Diagnostics, decoded MJPEG, four reconnect responses and session-clearing
logout, with zero control requests. Receipts are
`evidence/private/device-interrupted-update-reboot.json` and
`evidence/private/device-interrupted-update-reboot-browser.json`.

This is orderly software reboot during the health window, not abrupt power
loss or interruption of a storage write. No target Power/Reset action was used.
The authoritative baseline is now generation ten/floor two, serving slot two,
with dev5 and enabled updater services. Next expose the verified updater through
bounded local IPC and authenticated API/GUI; the signed-update engine's device
acceptance does not imply those network-facing features are already available.
