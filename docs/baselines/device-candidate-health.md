# Native candidate reporter credential acceptance

On 2026-09-05 the actual Rust server and updated reporter passed the isolated
candidate service gate. Source `14894b5` reporter SHA-256:
`1c1db722b6250160349915202f6c081838ec656f663fa61fe07831ccdb06fc5c`.
The server binary remains the same as the retained signed-slot application.

The gate rendered the actual candidate template with a separate loopback port
and separate state directory, preserving sandbox settings. Without the
explicit read-only credential alias, startup failed and no token reached the
test listener. With the alias, strict Rust HTTP health passed and exactly the
expected 32 credential bytes arrived from the server's kernel-verified UID/GID.
Credential contents were neither logged nor stored in the result receipt.

Device job `82167d25cf70486e8b987c5a7d4629b8` and receipt
`evidence/private/device-candidate-health-isolated.json` prove successful
delivery, fixture cleanup and unchanged production serving runtime. Host
validation passed in `4b4aa806d2cd42a797e81be8ba665674`; ARM64 build passed in
`a0b5e77b4b754f7781bf81c74dc35c45`.

This listener checked delivery and peer identity; it did not confirm a real
manager transaction. Next prove the complete packaged-template chain with
isolated signed slots, socket activation, confirmed-service application and
timeout rollback. Then package/deploy the candidate fix and test the actual
production-key update flow. Installed updater remains dev3; its watchdog and
confirmation socket are disabled.
