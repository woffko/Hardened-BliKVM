# Persistent write seq10 acceptance cycle

Accepted on 2026-09-13 on the enrolled H616 test device. This is temporary
guarded-write acceptance on the installed signed seq10 application, not
permanent write enablement, a release candidate, or a physical/hardware matrix.

## Exact inputs

- Baseline source: `07a0b39`; boot-observer counter fix: `feedadf`.
- Signed application archive: `hardened-blikvm-application-0.1.0-dev7-seq10.tar`,
  8,724,480 bytes, SHA-256
  `85e06d1b68e54480bab89c3cdc5e0d9f80cc3ee007e7ba1e425432af4b250f1f`,
  manifest SHA-256
  `8a30162d0b620c2f7a6679061d2c9c4bf863aa8ac1be2d2e6dfa77caacc6ae01`.
- Signed slot:
  `app-00000000000000000010-8a30162d0b620c2f7a6679061d2c9c4bf863aa8ac1be2d2e6dfa77caacc6ae01`.
- Retained updater: dev9; guarded old/new packages were the reviewed dev7
  (`updater-dev7-4d47974-a`) and dev9 (`updater-dev9-77192d3-deb-a`) runtimes.
- Rehearsal runner SHA-256:
  `db7f88e6ffe992afe7822f595da73f08af403d793a6e213ba23d6a738645e572`.
- Starting state: confirmed seq10, floor 10, generation 46, previous seq9,
  no pending operation, no rollback, HTTP write admission closed, guard and
  staging cleaned.

## Observed results

### Negative browser gate

The installed seq10 browser gate rejected the wrong archive, cancelled a
partial upload, retried the exact signed archive, and logged out during the
in-flight upload. The revoked upload terminated through a request-specific
abort (`net::ERR_ABORTED`) instead of an upstream HTTP 502; upload requests
returned HTTP 200/200. No activation was requested. After restoration the
device remained at seq10/floor10/generation46 with closed writes, and the
owned guard and controller staging were removed. Transaction
`15403252a10eb12ef27e3a3e`; receipt `persistent-write-seq10-browser-20260913-a.json`.

### Closed GUI before and after reboot

Two read-only runs used the `seq9-to10` write profile and verified three
repeated status reads, the desktop and mobile update panels, and two refresh
checks with zero upload or activation requests, zero blocked mutations, zero
page errors, secure cookies and revoked session. Receipts
`persistent-write-seq10-gui-closed-20260913-a.json` and
`persistent-write-seq10-gui-closed-20260913-b.json`.

### Pending native gate

The held-upload fixture submitted one incomplete native IPC body, one restore
submission and a socket fence while the worker was live. The worker exited
naturally (code 1, status 50), the terminal state was preserved, the probe
source was removed, and before/after status remained seq10/floor10/generation46
with closed writes and a cleaned guard. Transaction `54e5229bd8e38b40ac7de60a`;
receipt `persistent-write-seq10-pending-20260913-a.json`.

### Boot gate

The first attempt (transaction `72c9f710dc2d75d84998e25f`) requested one
software reboot and recovered early boot correctly, but the retained observer
compared the settled slot against a hardcoded seq8 counter pair and failed the
runtime proof. Independent inspection confirmed the device had revoked the
guard, masked admission and retained the 10/46 baseline. The retained guard was
completed with the corrected plan-derived counters: early-boot proof, runtime
restore, cleanup, staging removal, and a closed post-cleanup observation.
Receipts `persistent-write-seq10-boot-20260913-a.json` (failed attempt) and
`persistent-write-seq10-boot-recovery-20260913-a.json` (verified recovery).

The clean end-to-end rerun at fix `feedadf` (transaction
`653fc9b98d799392c43053b5`) passed: one software reboot was requested and
accepted, the boot identity changed from
`0d33022d851e610ab9afc09546bed91c513261d857a0d34c587d90e5e4fc58ef` to
`beb32478c0dc1d30be976e11ab063709c9f96832c8d42ee7e51e09c8fb252c70`, the early
boot guard exited successfully (code 1, status 0) before the server started,
the deadline recovery never executed, no workers or queued jobs remained, and
restore plus cleanup retained the 10/46 baseline with the guard and staging
removed. Receipt `persistent-write-seq10-boot-20260913-b.json`. The subsequent
closed GUI run passed.

## Boundaries

- Permanent write admission remains disabled; every gate ended closed with the
  owned guard and controller staging removed.
- No target Power/Reset action was requested or performed; only BliKVM software
  reboots were used.
- No second device, recovery card, long soak, or full-system update is covered.
- Private receipts, endpoints, accounts, and credentials remain outside Git.
