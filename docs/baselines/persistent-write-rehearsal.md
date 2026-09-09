# Persistent write-policy rehearsal

Date: 2026-09-09. Source: `8bafb6078e773df7a6afef5d3a195ef66b3389de`.
This is a successful same-boot wiring/restoration rehearsal, not approval to
retain HTTP write admission and not completion of the update acceptance plan.

## Evidence

The complete host check passed in job
`ce15bc30ab124afa9bfc378a99fb182d` (104.112 seconds). The installed read-only
preflight passed in job `7979b2ef5de542a6b5fa602d8d241ad0` (40.047 seconds).
The installed prepare/restore/cleanup rehearsal passed in job
`759d687be4be42ff8f844c2478cf54be` (76.079 seconds).

Private receipts, excluded from Git:

| Receipt                                              | SHA-256                                                            |
| ---------------------------------------------------- | ------------------------------------------------------------------ |
| `evidence/private/persistent-write-preflight-a.json` | `b4cb96ff19d2a4c7a90420ac06cd486e67a77dc85d630ecd76f1b87472f0179b` |
| `evidence/private/persistent-write-rehearsal-a.json` | `dd5abc018af6902aca34714f889a29d4d0c2f6c041ec305eb3681ee3361dcfb4` |

The controller verified the retained dev9 package, prepared exactly one owned
policy transaction, armed its independent deadline and boot guard, installed
the two server write overrides and socket-enable links, and verified loaded
and running configuration. It then closed and masked write admission, observed
natural pipeline settlement, removed only its owned policy, restarted the
confirmed server, verified preserved runtime/state, released the masks, and
disarmed recovery.

Cleanup verified idle owned services and absent loaded guard dependencies,
removed the dormant owned guard unit definitions, and archived the root-private
code and original-absence backup under the restored-transaction namespace.
The private receipt records its exact archive path. No backup or signed slot
was deleted. Temporary SSH staging was removed.

Independent observations before and after reported the same boot, selected and
confirmed slot eight, floor 8, generation 40, previous slot seven, and no pending
or rollback state. Both write transports and loaded/running write settings were
closed after restoration. The controller's preservation checks compared all
retained immutable state entries and protected account/TLS/trust/Nginx/peer
facts against the preparation snapshot.

## Boundaries

No application archive was sent, no application activation was invoked, and no
reboot, target-power operation, garbage collection or permanent retention was
requested. This rehearsal proves installed idle wiring/restoration and cleanup;
it does not prove restoration with active workers, power-loss/boot recovery,
browser failure/retry/cancel/logout behavior, or committed-policy survival.
Those remain distinct acceptance gates. Permanent HTTP writes remain disabled.
