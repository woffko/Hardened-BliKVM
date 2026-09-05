# Reboot during unconfirmed application activation

Use the retained dev5 installation and confirmed sequence-two application.
The bound sequence-three archive is
`9b8fa7781e9b453fa96cef385aa135e7e35d127276dbe22fe87364e4ead37bee`,
manifest `4bef35dff43b8c06b7a2c8965acb0c9b106e605881fcc955fe24bbc5c3c03d6a`.
It uses the same accepted payload and backed-up production application key.
Two independent archives matched; the host Rust signature/tree/tamper checks
passed in `evidence/private/production-bundle-seq3-host-verification.json`.
No private key or health credential is uploaded or logged by the test driver.

Stage the archive without changing active state. A marker-owned temporary
runtime candidate drop-in replaces only its health post-command with a fixed
90-second sleep. Verify the loaded command, activate the candidate through the
installed unit and capture a real `awaiting_health` pending operation with at
least 30 seconds remaining. Do not forge confirmation or alter signed files.

Request exactly one software reboot. Its successful acceptance requires a
new boot ID, `lastRollback=boot_changed`, floor two, active/confirmed slot two,
no pending operation, a newer journal generation, and the rejected candidate
as previous slot. Verify original and candidate signed payload metadata/hashes,
preserved account/TLS/trust, recovery-before-server ordering, stable serving
and subsequent browser behavior. Legitimate rollback changes to journal and
pointer inodes must not be mistaken for immutable payload changes.

The runtime override and temporary source disappear on reboot. Independent
cleanup also handles a rejected/lost reboot request, removes only marker-owned
override/source artifacts and waits for the installed watchdog if needed. It
never restores a journal or decreases a confirmed floor. Without its ownership
marker it must leave any pre-existing candidate override untouched.

This is a health-window interruption by orderly software reboot, not an abrupt
power-cut or storage-write interruption test. Those claims require separate
evidence and are not inferred from this gate.
