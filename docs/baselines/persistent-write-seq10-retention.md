# Permanent write admission retained on the seq10 baseline

Accepted on 2026-09-13 on the enrolled H616 test device. The committed write
policy is intentionally retained (admission open) by explicit user decision;
the permanent disable workflow remains host-tested only.

## Exact inputs

- Baseline: confirmed seq10, floor 10, generation 46, previous seq9.
- Signed application archive: `hardened-blikvm-application-0.1.0-dev7-seq10.tar`,
  8,724,480 bytes, SHA-256
  `85e06d1b68e54480bab89c3cdc5e0d9f80cc3ee007e7ba1e425432af4b250f1f`,
  manifest SHA-256
  `8a30162d0b620c2f7a6679061d2c9c4bf863aa8ac1be2d2e6dfa77caacc6ae01`.
- Reviewed updater packages: dev7 (`updater-dev7-4d47974-a`) and dev9
  (`updater-dev9-77192d3-deb-a`).
- Historical activation receipt: `persistent-write-seq10-activation-20260913-a.json`
  (transaction `13a294fc66832ca78e160394`, baseline 9, terminal 10/46).
- Historical archive: `/var/lib/hardened-blikvm-write-restored-13a294fc66832ca78e160394`.

## Prerequisite transactions

1. `pending`: receipt `persistent-write-seq10-prereq-pending-20260913-a.json` —
   partial native IPC body, single restore submission, socket fence with a live
   worker, natural worker exit code 1 status 50, unchanged 10/46, guard cleaned.
2. `boot --reboot`: receipt `persistent-write-seq10-prereq-boot-20260913-a.json` —
   one software reboot, early-boot guard completed before serving with a passed
   qualification, boot `977798750c1a5f56...`, unchanged 10/46, guard cleaned.

## Commit transaction

`tools/run_persistent_write_commit_cycle.py`, receipt
`persistent-write-seq10-commit-20260913-d.json`:

- wired guard `write-retain-828cfb89528ce919111aeec7` prepared under the shipped
  controller;
- host gate passed against the pinned controller suite;
- native upload gate passed: `retry`, `invalid-archive`, `short`, `surplus` and
  `malformed` under the update server UID with the shared group, `unauthorized`
  under root, spool empty;
- in-transaction negative GUI gate passed: failure/retry, cancel, and logout
  with an observed HTTP 401 in-flight termination (`kind response`, status 401);
- evidence store seal
  `2419b433e0f9499a79f209b81c57904fec1ad86efedc7bbd3fb3355376576a08` with all
  nine receipts: five fresh gates plus `native_activation`/`gui_happy_path` from
  the accepted activation through compatibility, and `boot_restore`/
  `disable_drain` from the prerequisite transactions;
- native commit published with explicit retention approval, all nine gate hashes
  bound, guard retained, controller staging removed, admission open.

## Retained reboot survival

`tools/run_persistent_write_retained_reboot.py`, receipt
`persistent-write-seq10-retained-reboot-20260913-c.json`:

- read-only retained verification before the reboot (retained, approved);
- exactly one software reboot (reply 0);
- after the boot identity changed (`977798750c1a5f56...` to `82b576400cb8bc4f...`)
  the policy was still retained and approved and the retained snapshot
  comparisons passed.

## Boundaries

- The committed policy remains open; write admission is intentionally retained.
- The permanent disable workflow is covered by host tests but was not executed on
  the device in this cycle.
- No target Power/Reset action was used; only BliKVM software reboots.
- No soak, second device, recovery card or release acceptance is covered.
- Private receipts, endpoints, accounts and credentials remain outside Git.
