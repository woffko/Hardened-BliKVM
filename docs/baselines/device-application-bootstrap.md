# Persistent application bootstrap on H616

The initial signed application slot and generation-one update journal were
retained on the test H616 using source `b251241`. Full host verification job
`cf252406885a44699bd9a04c0cd6fe34` passed before device use.

Read-only preflight `6f8ababbfb344db58cc16998da308f00` verified the installed
updater dev2 package, enrolled trust, absent state/slots/runtime, inert updater
services, sufficient free space, clean guards and healthy serving runtime.

Rehearsal `b2a46dd4e6c94842aaf888e6b44e3bc3` then published the two roots,
staged the production-signed bundle, initialized generation one and verified
it in an independent SSH transaction. Explicit rollback moved both exact
directories into a private quarantine and restored absent production roots.
The quarantine remains at
`/var/lib/hardened-blikvm-state-quarantine-e7bba4f9364d5948874b60ca`.
Its data was retained, not deleted. The serving runtime and trust were unchanged.

Retaining job `ade55063f2e643deb8e4f882e5572e1b` repeated preparation and
inspection, committed the roots, removed early-boot and deadline protection,
and performed another status verification after guard cleanup. Its final state:

- updater package: `hardened-blikvm-update-runtime 0.1.0~dev2`;
- public trust: `app-production-2026-09-05-01`;
- immutable slot:
  `app-00000000000000000001-7a088d20f639fabf5ed4f59b7223cd34b8f39cb9fe2c5764acff6df1789fbc6f`;
- slot root: `/var/lib/hardened-blikvm-slots`, mode 0750, UID 984 / GID 989;
- state root: `/var/lib/hardened-blikvm-update`, mode 0700, UID 984 / GID 989;
- generation and floor sequence: one;
- active, confirmed and `current`: the same initial slot;
- previous slot, pending activation and last rollback: absent;
- temporary runtime, controller staging and active guard files: removed; and
- updater/current/candidate units: still inactive and disabled.

The bundle is byte-equivalent to the accepted serving application payload.
Initialization selected the updater's baseline pointer only; the GUI continues
to run from the package-backed `hardened-blikvm-server.service`. Broker, video,
server and Nginx processes, account and TLS remained unchanged. No attached
target Power/Reset action occurred.

Private evidence is recorded in:

- `evidence/private/device-bootstrap-preflight.json`;
- `evidence/private/device-bootstrap-rollback-rehearsal.json`; and
- `evidence/private/device-bootstrap-retained.json`.

Next is reversible transfer of serving responsibility to the signed current
slot, followed by browser verification and candidate/confirmation/update API
work. Nginx currently requires the package-backed server; that dependency must
be changed before stopping the old server, and must not bind Nginx lifetime to
either current or candidate during later updates. Bootstrap and unused-trust
removal tools now correctly reject this initialized device state.
