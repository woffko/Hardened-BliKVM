# Health-gated application activation v1

This contract selects only application slots already published and fully
validated by `bli-kvm-update-stager`. It cannot stage an archive, write a
system partition, authorize a hardware action, or activate a full-system
image.

## Persistent layout

The manager uses two existing canonical absolute directories owned by its
dedicated identity. Neither may be group- or world-writable, they must be
distinct and non-nested, and both must be on the same filesystem.

```text
slots-root/
  app-<20-digit-sequence>-<manifest-sha256>/...
  current -> app-...
  .current-next                 temporary symlink only

state-root/
  application-state.json       mode 0600
```

The state root is held under an exclusive `flock` for the manager lifetime.
Acquisition uses 40 non-blocking attempts separated by 25 milliseconds and
then returns `ManagerBusy`; only `EAGAIN`/`EWOULDBLOCK` is retried. This absorbs
short systemd command overlap without allowing an unbounded waiter. The journal
is bounded to 64 KiB, rejects unknown or inconsistent fields, is written
through a mode-0600 temporary file, and is fsynced before atomic replacement
and parent-directory fsync. The current pointer is always a single relative
validated slot ID. It is replaced with `renameat2` relative to a pinned
slots-root descriptor and followed by directory fsync.

## Activation transaction

```text
confirmed active slot
        |
        v
persist Prepared journal
        |
        v
atomically switch current symlink
        |
        v
persist AwaitingHealth journal
        |                         |
        | valid bound health      | timeout, reboot, failure,
        v                         | pointer/proof mismatch
persist Confirmed                v
raise anti-rollback floor    switch previous + persist RolledBack
```

Every referenced slot is reverified from its retained Ed25519 manifest and
signature, exact tree, file hashes, modes, ownership, and link counts. A target
whose `minimumSystemSequence` exceeds the running system sequence is rejected.

The anti-rollback floor is the highest confirmed application sequence. It is
raised only after health confirmation, never after staging or pointer switch.
A new candidate must be strictly newer than the floor; an alternate artifact
at the same sequence is rejected. Failed candidates do not raise the floor.

## Health binding

Each activation has a non-zero 128-bit operation ID and a non-zero 256-bit
health-binding digest. The journal stores only those opaque bindings, not a
plaintext credential. The local supervisor generates and validates the actual
one-use health credential, then passes its digest to this library; a
network request must never call confirmation directly.

The health window is bound to a kernel boot ID and a monotonic boot-time clock.
The default is 60 seconds, with a hard accepted range of 10 seconds through
10 minutes. A different boot ID, expired deadline, explicit health failure,
invalid target proof, or pointer mismatch rolls back to the separately
reverified previous slot. A corrupt candidate cannot block rollback. A corrupt
non-authoritative previous candidate is dropped from the journal rather than
blocking the confirmed active slot.

## Crash recovery

| Crash point | Durable state | Possible pointer | Recovery |
| --- | --- | --- | --- |
| Initial journal before first pointer | confirmed slot | missing | recreate `current` from journal |
| After Prepared journal | old slot active | old or target | force old pointer and record interrupted rollback |
| After pointer switch, before AwaitingHealth | Prepared | target | force old pointer and record interrupted rollback |
| During health window | AwaitingHealth | target | resume only on the same boot before deadline |
| During rollback after pointer switch | AwaitingHealth | old | finish the persisted rollback record |
| After confirmation | confirmed target | target | clean, idempotent state |

The current symlink is never followed to decide policy. The signed journal slot
reference is authoritative, and recovery overwrites an owned malformed symlink
with the verified active reference. A regular file, directory, foreign owner,
unsafe state file, corrupt journal, or missing rollback anchor is a hard stop.

Operation history is persisted and bounded to 128 records. Reusing an ID with
different target or health binding is rejected; exact confirmed, rolled-back,
pending, and no-op retries are deterministic. History is never silently
pruned. Exhaustion is explicit and requires a future reviewed rollover format.

## Current implementation boundary

`bli-kvm-update-manager` implements the journal, pointer, health, replay,
anti-rollback, and recovery core. `bli-kvm-update-runtime` reads the kernel boot
ID twice around `CLOCK_BOOTTIME`, generates one-use 32-byte health credentials,
stores them only in a mode-0400 runtime file, compares responses in constant
time, zeroizes token buffers, and reconciles challenge state with the manager.

`bli-kvm-update-supervisor` exposes only fixed local commands. `validate-config`
parses the public configuration without touching state, slots, or runtime.
Packaged unprivileged services receive the root-only source through systemd
`LoadCredential` and pass `/run/credentials/%n/update-supervisor-config` to the existing config
argument. Service-owned configuration is accepted only on the checked read-only
credential mount under root-controlled `/run/credentials`; direct root-owned
administrative configuration retains its existing validation.

`ack-confirmed-apply` reads a bounded public apply request from stdin and
acknowledges only the same queued generation/confirmed slot while no activation
is pending. It is used after the fixed confirmed-service action, not exposed
through the network or health-confirmation socket. A stale ACK preserves newer
work. Clean watchdog passes do not request repeated restarts; see
`../design/update-service-application-v1.md` for the event contract.

`stage` accepts
one absolute local archive path and can publish a verified slot but cannot
select it. `initialize`, `activate`, `status`, `gc`, and `watchdog-once` accept
only their bounded non-secret values. Slot IDs, archive/config paths, and a
root-owned public-key configuration may appear in argv; a health token may
enter only as exactly 32 raw stdin bytes for `confirm-stdin`. JSON receipts
never contain the token, credential path, or public key. The packaged watchdog
timer and Unix confirmation socket run under a distinct update identity with
no network or device authority. The bundle contains no production signing key
or enabled configuration. The newer guarded activation socket is described
below; adding its source does not enable network activation on a device.

### Generation-fenced activation socket source

The separate `activation-socket --peer-user USER` entrypoint authorizes the
kernel Unix peer before reading one closed, framed request and exact EOF.
Metadata is capped at 16 KiB with a two-second absolute reception deadline;
no journal/slot lock is held while receiving it. The only request is `activate`
with a validated slot ID and nonzero expected journal generation. It accepts
no path, URL, command, health token or confirmation instruction.

`UpdateSupervisor::activate_if_generation` holds the existing manager locks,
checks the generation, absence of a pending operation, non-current target and
its signed proof before watchdog housekeeping, then checks again after
housekeeping and before issuing the challenge. The existing local `activate`
semantics are unchanged. An old confirmation returns `stale_generation` and
cannot start a second activation. A caller must reread status and ask for a
new explicit confirmation, never silently replace the expected generation.

Only a new `AwaitingHealth` result produces an acceptance receipt containing
the operation ID and target slot ID. It is not a health confirmation. The
worker exits zero only for that result, even when response delivery fails;
rejected, stale, pending and already-active requests exit nonzero (97 after a
delivered rejection) and do not authorize apply dispatch. No credential path
or token appears in the receipt. Inert package dev9 adds a separate one-client
socket and a 15-second oneshot service with fixed `OnSuccess` candidate and
confirmed apply units. It has no success-exit override, condition-skip shortcut
or user-selected unit/command. Apply runs only after the worker exits and
releases manager locks. Dev1–dev8 package file maps remain unchanged.

Host tests cover generation fencing, repeated requests before/after health
confirmation, preserved challenge identity, typed errors, exact receipts and
success-for-dispatch after a lost reply. Offline package/systemd checks cover
the new templates. Installed identity, dispatch,
restart/health/rollback and browser acceptance remain required; these source
changes do not enable writable-update capability.

### Opt-in HTTP activation source

`POST /api/application-update/activate` is protected by the existing session,
origin and CSRF middleware before any body or IPC work. It is disabled unless
`BLI_KVM_UPDATE_ACTIVATION_SOCKET` is explicitly configured, and requires
separate configured status and upload sockets. Authenticated calls return 503
when disabled. Configuration alone still advertises read-only capability.

The closed `application/json` body contains only `slotId` and
`expectedGeneration`; the latter is a canonical positive decimal **string**,
not a JavaScript number. Unknown fields, noncanonical/overflowing counters,
invalid slot IDs, encoded bodies and trailers are rejected. Reading is bounded
to 16 KiB plus an overflow sentinel and a two-second absolute deadline; the
session is checked again before dispatch. The separate client allows one
in-flight activation and has a ten-second total deadline. It sends exactly
once, validates bounded framing, EOF, request ID and returned target identity.

An HTTP 202 response contains `data.phase = "awaiting_health"` and a `receipt`
with `operationId` and `targetSlotId`. It never claims health confirmation.
Stale/current/pending/busy rejections retain the API's existing conflict code;
transport or protocol failures return sanitized 503. A lost response or logout
after dispatch does not undo activation. The caller must reconcile via status,
never silently retry or substitute a newer generation. Host HTTP/client tests
cover authorization, closed bodies, exact u64 generation, receipt correlation,
bounded concurrency/timeouts, lost replies and absence of automatic retry.
These tests are not installed systemd dispatch or native browser acceptance.

### Capability-gated browser confirmation

The staged receipt exposes a separate review action only under writable
capability. Eligibility requires fresh status, no pending operation, a target
above the confirmed sequence floor and a compatible minimum system sequence.
The dialog snapshots the exact verified target, current confirmed slot and
decimal generation. It refreshes status; confirming stays disabled during
refresh/staleness and if target, generation or eligibility changes. A changed
intent must be closed and explicitly reviewed again; generation is not replaced
inside an existing dialog. Activation uses a synchronous in-flight guard and
never auto-resends, including after ambiguous HTTP errors.

The UI distinguishes sending, awaiting health, unknown outcome, rejection,
another pending operation and a currently confirmed target. A receipt cannot
display success. Reconciliation ignores old generations, correlates pending
operation/target when a receipt exists, and requires fresh status to report the
target as currently confirmed. This observation does not attribute confirmation
to a particular request. A changed terminal state with a different confirmed
target directs the user to inspect status and rollback reason. Unknown outcome
permits only an explicit new review, not an automatic retry. Unmount/capability
loss aborts transport and ignores late callbacks; it does not promise rollback.

Local Chromium acceptance uses the real component and HTTP adapter with
synthetic receipts and controlled status props. It covers mobile layout,
cancel without dispatch, stale-generation confirmation lockout, exact explicit
sends, receipt-before-health waiting, confirmed-target reconciliation and an
ambiguous 503 reconciled without resend. It is not an installed native test,
nor proof of real supervisor health or systemd apply dispatch. Writable
capability remains disabled until those gates pass.

Supervisor stderr remains deliberately generic. When a runtime operation
reaches the manager and fails, the local exit status identifies only the fixed
error variant and carries no slot ID, path, token, key, or journal contents:

| Code | Manager error |
| ---: | --- |
| 60-65 | invalid/unsafe slot or state roots, overlapping roots, or different filesystems |
| 66 | `ManagerBusy` |
| 67-70 | unsafe/invalid/oversized journal or concurrent journal change |
| 71-74 | initialization, pointer, or slot-ID state error |
| 75-78 | generation/history exhaustion or invalid clock/operation material |
| 79-81 | operation conflict, pending activation, or required recovery |
| 82-86 | system/anti-rollback policy, confirmation/proof mismatch, or invalid retention |
| 87 | nested `StagerBusy` |
| 88 | another nested stager/proof error |
| 89 | manager I/O error |

The supervisor also passed an isolated native H616 gate covering staging,
confirmation, floor enforcement, downgrade rejection, lost-token rollback,
cleanup, and a transient systemd sandbox. The packaged socket/timer remain
uninstalled and unenabled. Candidate-process restart ordering, systemd
credential delivery, journal backup, hardware-backed rollback counters, and
live interruption tests remain separate gates. Marker-safe staging cleanup and
transactional reference-aware slot garbage collection are implemented and
passed their isolated H616 gate.

The next packaged orchestration layer has separate confirmed and candidate
server units. Both execute only the signed `current` slot. The candidate unit
requires the one-use runtime token through systemd `LoadCredential`, waits for
the Rust server's strict loopback `/api/health` response, and then runs the
minimal `bli-kvm-update-health-reporter`. The reporter accepts only IPv4
loopback, validates the bounded HTTP status/content type/product JSON, reads
exactly 32 credential bytes, writes them only to the fixed Unix confirmation
socket, and zeroizes the buffer.

Local activation and watchdog units use fixed `OnSuccess` apply services.
After the originating command has completely exited, token present selects the
candidate service; token absent selects the confirmed service after rollback.
The root apply services invoke `systemctl restart` only for those two literal
unit names. No request-derived command, unit name, shell, TCP control socket,
or browser route is involved. The exact server, reporter, one-use systemd
credential, confirmation socket, failed candidate, timeout rollback, and
restored server passed a temporary isolated H616 process gate. The packaged
templates remain uninstalled and unenabled; production key enrollment, live
installation, interruption, reboot, and full-system recovery are separate
gates documented in
`../baselines/device-update-process-orchestration.md`.
