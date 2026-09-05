# Persistent read-only status acceptance

The device retains exact root-owned current/candidate server drop-ins for the
installed local status socket. The socket remains disabled as a standalone
boot unit; the servers' Wants/After dependency starts it. The worker retains
its read-only sandbox and fixed peer-UID policy. No upload or activation
authority was added to the network process.

The accepted prerequisites are the complete reversible desktop/mobile cycle
in `status-api-seq5-acceptance.md`, the real corrected early-boot restoration
in `persistent-status-boot-attempt.md`, and the retained updater dev7 reboot
in `updater-dev7-package.md`. Earlier failures remain documented separately.

Retention preparation `00487692b1c244af829dd5226f907710`, enabled browser
`a1b1c226b5ec464a81f5d0d77917ce91`, commit
`ace7bb36f5f84a12be5a8cc196320234`, cleanup
`c8b8ec64c42e47778777a28bee2d4992`, and independent observation
`9ecb03e7430648f5868ea6c01c48f87b` passed. The first later reboot exposed an
initial-recovery startup failure, so that attempt was not accepted.
After the dev7 ordering correction, reboot
`ae63cbf258b146b0bbb2f4b0f7321224` and post-boot browser
`c0ae27a7ace24bf0a1dd50e51061fe0a` passed with no manual service start.

Current accepted application identity is sequence five, version `0.1.0-dev4`,
floor five and journal generation twenty-two. API counters remain decimal
strings; selected and confirmed metadata match; no update is pending. The
desktop/mobile Update pane shows read-only status and Refresh. Login, video,
Diagnostics and logout continue to work. Account, TLS, trust, application
contents and configuration hashes were preserved across the accepted reboot.
Transaction guards and observer staging were removed; build artifacts and
private test receipts remain available.

Longer polling/video resource soak, deliberate status unavailability behavior,
additional browser engines, signed upload and activation UI, system recovery,
physical hardware gates and release acceptance remain outside this checkpoint.
