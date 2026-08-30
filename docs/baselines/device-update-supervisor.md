# H616 isolated application update supervisor gate

The test-only BliKVM H616 device passed an isolated `/tmp` application-update
gate with the exact AArch64 static-PIE supervisor:

```text
dba225bd9b4040a7aa8a32d670c2f16a35c78f280b65e54af96df131a158e65e
```

The gate used an ephemeral Ed25519 key generated only in host process memory.
Only its public key and three 20,480-byte signed application fixtures were
uploaded. No private key was serialized, transferred, logged, or retained.

## Accepted behavior

- the release-linked supervisor executed natively on AArch64;
- three signed sequences were verified and published as immutable slots;
- restaging the identical sequence-1 artifact returned the idempotent existing
  slot rather than overwriting it;
- sequence 2 became active only after the exact one-use 32-byte stdin health
  credential was confirmed;
- confirmation raised the persisted anti-rollback floor to sequence 2;
- a later attempt to activate sequence 1 was rejected;
- an all-zero wrong health response was rejected without consuming the real
  credential;
- removing the sequence-3 credential caused watchdog rollback to the confirmed
  sequence-2 slot with reason `health_challenge_lost`;
- the resulting pointer, confirmed slot, active slot, floor, pending state,
  and clean watchdog result all matched;
- the same clean watchdog path passed inside a transient systemd sandbox with
  no-new-privileges, private devices, protected system/kernel settings,
  Unix-only address families, IP denial, and one exact writable staging tree.
- after rollback, explicit `gc 0` deleted only unreferenced sequence 1 while
  preserving active/confirmed sequence 2 and protected previous rollback
  anchor sequence 3; no deletion marker or tombstone remained.

## Preserved device state

The gate began and ended with KVMD and kvmd-nginx active. It did not restart
either service, install or enable a unit, create an account, alter the existing
GUI, open a port, invoke HID/media/ATX, or perform any Power/Reset action. The
root filesystem remained `rw` as requested. The marker-protected staging tree,
health token, and transient unit were absent after cleanup. The private receipt
is mode 0600 and contains only allowlisted booleans, hashes, sizes, and coarse
state.

## Remaining gates

This proves the verifier, stager, journal, raw `SYS_renameat2`, kernel boot-time
sampling, one-use credential, watchdog rollback, transactional slot deletion,
and transient sandbox on the target kernel and userspace. It does not yet
install the packaged socket/timer, restart a real candidate server from the
`current` pointer, deliver the token through systemd credentials, interrupt
real storage writes, reboot/power-cut the device, or establish production
signing keys.
