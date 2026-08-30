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

The state root is held under a non-blocking exclusive `flock` for the manager
lifetime. The journal is bounded to 64 KiB, rejects unknown or inconsistent
fields, is written through a mode-0600 temporary file, and is fsynced before
atomic replacement and parent-directory fsync. The current pointer is always a
single relative validated slot ID. It is replaced with `renameat2` relative to
a pinned slots-root descriptor and followed by directory fsync.

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
plaintext credential. The future supervisor must generate and validate the
actual one-use health credential, then pass its digest to this library; a
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

`bli-kvm-update-supervisor` exposes only fixed local commands. Slot IDs and a
root-owned public-key configuration may appear in argv; a health token may
enter only as exactly 32 raw stdin bytes for `confirm-stdin`. JSON receipts
never contain the token, credential path, or public key. The packaged watchdog
timer and Unix confirmation socket run under a distinct update identity with
no network or device authority. There is deliberately no network activation
socket and no production signing key or enabled configuration in the bundle.

The supervisor and systemd templates have host-side subprocess and static unit
tests, but they are not installed or enabled on H616. Candidate-process
restart ordering, systemd credential delivery, abandoned-staging cleanup, slot
garbage collection, journal backup, hardware-backed rollback counters, and
live interruption tests remain separate gates.
