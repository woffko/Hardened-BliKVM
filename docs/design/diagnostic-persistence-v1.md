# Diagnostic persistence v1 design

Status: default-off, unlinked concrete adapter library. This document does not
authorize a credential source, external floor source, executable, service,
broker feature, HTTP route, UI, package, or enablement.

## Fixed composition boundary

`DiagnosticPersistenceRuntime::new` accepts only two already validated
fixed-boundary objects: `LoadedDiagnosticJournalKeys` and
`DiagnosticJournalStore`. It creates only the production
`DiagnosticExportClient`, whose socket path and timeouts are compile-time
constants. There is no constructor or argument for a path, raw key, credential
name, socket, timeout, retry policy, generation, or floor source.

The adapter deliberately does not call the key loader or open the store. A
later executable may do so only from a separately reviewed fixed systemd unit
after a trusted external rollback-floor source exists. The current key loader
therefore remains authorized only for the accepted empty init/inspect units.

The owned fixed repository obtains a short-lived typed key-ring view for every
operation. Loads use `load_read_only`, so a crash temporary or degraded copy is
not repaired or removed implicitly. Persistence uses the store's exact atomic
`update`, checks its returned generation, and then the coordinator performs a
separate clean authenticated re-selection and complete typed-journal equality
check.

## One-step runtime

Each explicit `poll(floor)` performs no loop and no retry:

1. if a prior acknowledgement future was cancelled, execute one bounded
   coordinator recovery step and return without collector I/O;
2. otherwise issue exactly one read through the fixed export client;
3. map its typed result into one coordinator observation;
4. return immediately after idle, unavailable, read-failure, persistence, floor
   advance, or explicit reread outcomes; and
5. only for a same-binding durable pending entry at the exact external floor,
   perform the coordinator's second selection, obtain one linear permit, and
   issue exactly one acknowledgement.

New sealed state is always persisted first and returns `AdvanceFloor`; it
cannot be acknowledged in that call. The caller must durably advance its
independent floor and invoke another poll, which necessarily performs a fresh
collector read.

Read connect failure is `ReadNotSent`; post-connect read failure is
`ReadRetryable`. Both return without store access or an internal retry.
Collector-unavailable read state is checked against the store but never
acknowledged.

Acknowledgement outcomes map exactly:

- acknowledged becomes coordinator `Applied`;
- no sealed becomes `NoSealed` and therefore durable `orphaned`;
- mismatch requests one later explicit read without mutation;
- a definitely not-sent error leaves durable pending state and consumes
  readiness; and
- response-level collector unavailability, timeout, EOF, I/O, malformed
  response, wrong binding, or any other delivery-ambiguous error first becomes
  durable `ack_ambiguous`.

## Async cancellation

Cancellation during the read drops only a non-mutating client connection. A
later poll is an explicit new read.

The acknowledgement permit is held across the one acknowledgement future.
Cancellation at any point after that future begins drops the armed permit,
sets the coordinator's in-process recovery marker, and causes the next poll to
perform no collector read. Matching pending state is persisted as ambiguous;
already ambiguous state requests a later read; an authenticated terminal write
is reported; missing/conflicting state fails closed. A one-copy interrupted
write remains repair-required, while a two-copy write that returned an error
is discovered without repeating the acknowledgement.

Release builds use `panic=abort`, so a process crash may lose the in-memory
marker. It cannot erase the already durable pending journal; startup must use a
fresh collector read and the accepted coordinator reconciliation rules.

## Errors and privacy

The public result exposes only closed states and journal generations. Errors
retain only already redacted store/journal categories and bounded generations.
No bundle bytes, collector binding, key ID/material, nonce, credential, path,
numeric identity, raw I/O detail, vendor log, or free-form text enters
`Debug`, `Display`, metrics, or receipts. The 8 KiB typed snapshot remains
inline to avoid adding a heap-allocation failure at the local trust boundary;
the fake-transport `poll` future has a test-enforced 16 KiB maximum and the
exact export-client instantiation has a 32 KiB maximum.

## Deferred gates

The current H616 has no trusted production floor that survives full-filesystem
rollback and no accepted persistent key custody. The adapter remains absent
from the default Platform Broker and prototype bundle. Before any runtime
enablement it needs committed host/RustSec/reproducible AArch64 evidence, then a
synthetic fixed-key/floor rollback-gated H616 test that provisions no production
secret and invokes no Power/Reset action.

Only after that gate may a separate authenticated read-only API and localized
dedicated Diagnostics tab expose temperature and other allowlisted typed health
metrics. Raw vendor logs, hardware actions, identity fields, and generic system
control remain outside the product boundary.
