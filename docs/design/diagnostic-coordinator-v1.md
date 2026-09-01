# Diagnostic coordinator v1 design

Status: design and unlinked `no_std` implementation boundary. This layer has
no concrete filesystem adapter, key loader, collector client, runtime, timer,
retry loop, service, HTTP route, UI, package, or enabled capability.

## Safety objective

The coordinator closes the gap between the accepted pure journal planner and a
future live collector: an exact sealed window may be acknowledged only after
its `pending_ack` entry is durably written, authenticated, re-selected from two
clean copies, and covered by a nonzero generation floor retained outside the
diagnostic filesystem.

The caller supplies exactly one fresh collector observation per `observe`
call. The coordinator performs no collector I/O and never loops or retries. A
repository call is likewise bounded: one initial load, at most one persistence
operation, and one authenticated post-persistence load.

## Repository contract

`DiagnosticJournalRepository` exposes only `load(minimum_generation)` and
`persist(minimum_generation, mutation)`. It accepts the typed mutation emitted
by the journal planner and returns a `SelectedDiagnosticJournal`, so arbitrary
or merely prepared state cannot authorize an acknowledgement.

The repository must finish its file/readback/directory sync protocol before
`persist` returns. The coordinator nevertheless loads again and requires all
of the following before reporting progress:

- the selected journal is not degraded;
- the selected generation equals the mutation generation; and
- the complete typed journal equals the planned journal.

A degraded load always stops with an explicit-repair requirement. The
coordinator never repairs a copy as a side effect of observation or
acknowledgement recovery.

## External rollback floor

Every public step receives `DiagnosticGenerationFloor`, which cannot represent
zero. A selected generation below the floor is a hard rollback violation. A
selected generation above the supplied floor permits no reconciliation or
acknowledgement; the only result is `AdvanceFloor`.

Every successful mutation similarly returns `AdvanceFloor` for its new
generation. The caller must durably advance the independently retained floor
and obtain a fresh collector observation. The coordinator does not silently
advance or retain the external floor. No suitable hardware-backed or otherwise
rollback-independent production floor exists on the current H616 device, so
this unlinked model does not enable production diagnostics.

## Readiness and linear acknowledgement permit

An acknowledgement requires two explicit calls:

1. `observe` accepts a fresh sealed snapshot, loads a clean journal at the
   exact external floor, and records only in-memory readiness when the journal
   planner returns the same durable `pending_ack` binding.
2. `prepare_acknowledgement` authenticates and re-selects the journal again,
   verifies the same pending binding and generation, consumes readiness, and
   returns one linear `DiagnosticAcknowledgementPermit`.

The readiness result exposes only the generation, not the collector binding.
The binding is available through the permit for one exact transport call. A
`NotSent` completion leaves the durable entry pending but consumes readiness;
another attempt requires a fresh collector observation. `Mismatch` requests a
fresh read without mutation. `Applied` and `NoSealed` persist terminal states.
Both an explicit delivery-ambiguous error and an acknowledgement response that
reports collector unavailability persist `ack_ambiguous` conservatively before
any reread.

## Cancellation and crash recovery

Dropping an armed permit, including async-task cancellation after the caller
starts transport polling, records an in-process cancelled binding. All normal
observations then fail with `AcknowledgementRecoveryRequired`.

`recover_cancelled_acknowledgement` loads a clean journal at the external
floor and behaves as follows:

- matching `pending_ack` is first persisted as `ack_ambiguous`;
- matching `ack_ambiguous` requests a collector reread;
- matching `acknowledged` or `orphaned` proves that a prior write completed and
  clears the in-memory marker; and
- a missing or different entry fails closed and retains the marker.

If persistence returned an error after both copies were already committed,
recovery discovers the authenticated terminal state. If only the first copy
advanced, recovery stops on degradation until a separately authorized repair.

A full process crash necessarily loses the in-memory cancellation marker, but
the durable entry remains `pending_ack`. On restart, a fresh collector read is
authoritative: the same exact sealed binding may be permitted again, while no
sealed window or a different binding first makes the uncertain old entry
`orphaned`. The coordinator never claims an unobserved acknowledgement.

## Privacy and logging

The crate accepts only typed diagnostic bundles and closed state enums. It has
no path, hostname, address, account, device-identifier, raw-log, wall-clock, or
free-form-message input. Coordinator error `Debug` and `Display` suppress the
repository payload. Permit `Debug` omits the collector binding and journal
contents. Production callers must keep repository errors equally bounded and
must not log bundle bytes or binding values.

## Deferred integration

A later default-off adapter must map the existing export client exactly:

- read `NoSealed`/`Sealed`/`CollectorUnavailable` to one observation;
- acknowledgement `Acknowledged`/`NoSealed`/`Mismatch` to the matching
  completion;
- acknowledgement response `CollectorUnavailable` to conservative ambiguity;
- acknowledgement `NotSent` errors to `NotSent`; and
- every delivery-ambiguous transport error or cancelled future to ambiguity
  recovery, never an automatic acknowledgement retry.

That adapter must use the fixed encrypted store and fixed systemd credential,
remain default-off, and pass host, independent AArch64, synthetic crash, and
rollback-gated H616 tests before broker linkage. Production key/floor custody,
authenticated read-only HTTP, the localized dedicated Diagnostics tab,
packaging, privacy review, soak, reboot persistence, and release acceptance are
separate later gates.
