# Retained application update to sequence 11

Accepted on 2026-09-13 on the enrolled H616 test device. The update was applied
through the permanently retained write admission; no rehearsal guard was used.
The committed policy, its guard and the evidence store remain in place.

## Exact inputs

- Signed archive: `hardened-blikvm-application-0.1.0-dev7-seq11.tar`,
  8,724,480 bytes, SHA-256
  `e113e4bc1d9091d294ea499e775a03e685f51f75f362c3cbb08c0fbaf005ec40`,
  manifest SHA-256
  `d6283a7be390423188d8931bae148d9555305a187f18049796cca6b793621d25`.
- Slot:
  `app-00000000000000000011-d6283a7be390423188d8931bae148d9555305a187f18049796cca6b793621d25`.
- Accepted runtime package SHA-256:
  `6df67f5bf124e5d7eaeb40adb770359662d22a609c654fdcdb6fb21be35bf289`;
  source and builder commit `3b57ee45463a7c90a06b61a2b7b874c7fd66df81`;
  enrollment request SHA-256
  `c6cab9580246a4782d3ab8185115423d9ea7870d0be5591c437529d492ac592e`;
  key `app-production-2026-09-05-01`.
- Starting state: confirmed seq10, floor 10, generation 46, eight slots
  seq3-seq10, retained guard `hardened-blikvm-write-retain-828cfb89528ce919111aeec7`.

## Host acceptance

Two independent builds produced byte-identical archives. Host verification
(`persistent-write-seq11-host-verification-20260913-a.json`) passed signature,
manifest, payload, idempotent staging and negative checks without contacting the
device or executing payload code; supervisor SHA-256 `5cb154f9...`.

## Capacity reclamation

The first upload attempt was rejected with `code -409` "upload staging quota
exceeded": the stager allows eight immutable slots and the device already held
seq3-seq10. The oldest unprotected slot was reclaimed with the committed
policy's own supervisor `gc` path:

- receipt `slot-reclamation-retained-seq3-20260913-a.json`;
- deleted only
  `app-00000000000000000003-4bef35dff43b8c06b7a2c8965acb0c9b106e605881fcc955fe24bbc5c3c03d6a`;
- seven slots remained, every retained tree hash was unchanged, authoritative
  status was unchanged, and the retained admission stayed active;
- the committed `decision.json` and `plan.json` were untouched.

## Retained GUI activation

`device_application_write.mjs activate` with profile `seq10-to11` passed
(`persistent-write-seq11-activation-20260913-b.json`): one login, one upload,
one activation request, zero blocked mutations, zero page errors, the real
archive SHA-256 was verified in the browser, and the updater returned HTTP 202
with a validated `awaiting_health` receipt (`stagingDisposition: published`,
`generationReviewed: 46`).

The retained updater then confirmed the candidate automatically. The
authoritative read-only observation
(`seq11-status-poll-20260913-1.json`) reported floor 11, generation 49 and no
pending operation, with active and confirmed both equal to the seq11 slot.

## Retained state after the update

A read-only check confirmed:

- slots are seq4-seq11 plus `current`, exactly eight immutable slots;
- the retained guard and evidence store directories are still present;
- `decision.json` is root-owned mode 0600;
- the upload and activation sockets are `active`.

## Boundaries

- The legacy `closed` GUI probe asserts read-only update controls; it no longer
  applies while admission is intentionally retained and was replaced by the
  activation probe plus the authoritative status observation. Mobile-panel
  coverage is therefore not part of this cycle.
- Permanent disable remains host-tested only.
- Slot reclamation stays a reviewed, single-oldest-slot operation; this cycle
  deleted seq3 only.
- No target Power/Reset action and no system-image update was performed.
- Private receipts, endpoints, account names and credentials remain outside Git.
