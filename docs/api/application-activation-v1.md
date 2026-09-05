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
`LoadCredential` and pass `%d/update-supervisor-config` to the existing config
argument. Service-owned configuration is accepted only on the checked read-only
credential mount under root-controlled `/run/credentials`; direct root-owned
administrative configuration retains its existing validation.

`stage` accepts
one absolute local archive path and can publish a verified slot but cannot
select it. `initialize`, `activate`, `status`, `gc`, and `watchdog-once` accept
only their bounded non-secret values. Slot IDs, archive/config paths, and a
root-owned public-key configuration may appear in argv; a health token may
enter only as exactly 32 raw stdin bytes for `confirm-stdin`. JSON receipts
never contain the token, credential path, or public key. The packaged watchdog
timer and Unix confirmation socket run under a distinct update identity with
no network or device authority. There is deliberately no network activation
socket and no production signing key or enabled configuration in the bundle.

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
