# System boot-control and recovery transaction v1

This contract defines state and evidence boundaries for a future H616 A/B
system updater. It does not define an H616 partition map, U-Boot environment,
device path, writer, reboot command, or recovery image.

## Authorities

Four authorities stay separate:

1. the system-image verifier authenticates fixed filesystem payloads and a
   layout ID but cannot write or select a slot;
2. a future recovery-owned writer maps that layout ID to one inactive slot,
   writes it, and proves complete readback;
3. a boot-selector adapter performs only fixed A/B/recovery selection and a
   bounded attempt decrement using platform evidence not supplied by a network
   request;
4. a boot-health agent observes the running slot and can confirm only the
   pending operation for the current kernel boot.

The network server has none of these authorities. There is no activation or
reboot socket.

## Persistent domains

The authoritative system journal and selector record must survive loss of
either root slot and must not live inside root A or root B. Their eventual
storage requires redundant, checksummed, generation-numbered writes with a
platform-specific power-failure guarantee.

The journal contains only bounded public state:

- schema and generation;
- accepted layout ID;
- highest confirmed system sequence;
- confirmed slot proof;
- one optional pending operation;
- bounded replay history and last rollback reason.

A slot proof contains the fixed slot enum, system sequence, signed manifest
SHA-256, layout ID, and exact component hashes/sizes. It contains no device
path, offset, mountpoint, secret, or signing key.

The selector record contains the operation ID, previous and candidate slot,
candidate manifest SHA-256, initial and remaining attempts, and its own
generation/checksum. The selector must decrement an attempt durably before
loading a candidate. The accepted initial attempt range is one through three.

Each journal and selector record has two canonical JSON copies. A
domain-separated SHA-256 checksum covers schema, generation, and payload. Two
equal generations must be byte-equivalent in meaning; divergence is split
brain. A one-generation difference selects the newer valid copy in degraded
mode so the older copy can be repaired. A larger gap is a hard stop. One torn
or invalid copy permits degraded read; no valid copy is a hard stop.

The checksum detects corruption and torn state. It is not authentication,
hardware anti-rollback, or permission to trust state restored by an attacker.

## Transaction phases

```text
confirmed A
   |
   v
WritePrepared(B)       journal durable; B is never selectable
   |
   | complete write + full readback + offline filesystem checks
   v
CandidateReady(B)      expected and observed component proofs match
   |
   v
BootPrepared(B)        journal durable before selector mutation
   |
   | selector atomically arms B with A fallback and bounded attempts
   v
BootArmed(B)
   |
   | selector decrements before boot; boot agent verifies B and boot ID
   v
AwaitingHealth(B)
   |                         |
   | bound health success    | timeout, attempts exhausted, invalid proof,
   v                         | wrong slot/boot, explicit failure
Confirmed(B), floor raised   v
                         selector A + journal RolledBack(B)
```

The system floor is raised only after `Confirmed`. Preparing, writing, arming,
or booting a candidate never raises it. Rollback from an unconfirmed candidate
returns to the previously confirmed sequence, which still equals the floor.

## Write transaction

Before touching an inactive slot, the recovery writer must:

1. validate `hardened-blikvm-system-v1` from a private source snapshot;
2. prove an exact accepted layout profile and identify active/inactive slots
   without caller-provided paths or offsets;
3. prove no update or boot operation is pending;
4. persist `WritePrepared` with the expected signed proof;
5. ensure every target component is unmounted and is not state, recovery, MSD,
   or the running/confirmed slot.

The writer streams only the expected number of bytes to the inactive fixed
target, flushes the device, then reads every byte back and compares SHA-256.
Offline filesystem validation is additional evidence, not a replacement for
readback. Any interruption leaves `WritePrepared`; recovery treats the target
as invalid and never selects it. V1 restarts a failed write from byte zero and
does not infer resumability from a partial prefix.

Only complete readback and filesystem checks can persist `CandidateReady`.

## Journal/selector crash reconciliation

Journal and selector storage cannot be assumed atomically writable together.
Recovery therefore compares both records and fails closed:

| Journal | Selector | Result |
| --- | --- | --- |
| no pending | confirmed slot | clean |
| `WritePrepared`/`CandidateReady` | confirmed slot | candidate remains unselected; explicit retry or discard |
| `BootPrepared` | confirmed slot | record interrupted arm; candidate remains unselected |
| `BootPrepared` | exact matching armed operation | finish journal transition to `BootArmed` |
| `BootArmed`/`AwaitingHealth` | exact matching candidate/fallback/counter | continue bounded attempt/health handling |
| pending | selector already on previous after exhaustion/failure | record rollback, floor unchanged |
| any state | mismatched operation, slot, digest, generation, or increasing attempt count | hard stop into recovery |

Recovery may repair only an exact, uniquely determined transition. It never
guesses from the currently mounted root or a partition number.

## Boot and health binding

Candidate boot observation must include:

- exact pending operation ID and slot;
- current kernel boot ID sampled around a monotonic boot-time clock read;
- selector evidence proving the attempt was decremented;
- a reverified signed slot proof;
- a one-use health binding unavailable through argv, environment, HTTP, or the
  browser.

Confirmation requires the same boot ID, unexpired monotonic deadline, exact
operation/health binding, selector evidence, and a compatible confirmed
application slot. Application confirmation that would require the candidate
system is forbidden while the system update is pending. This preserves the
ability to roll back to the previous system.

## Anti-rollback boundary

A journal-only floor can be rolled back by restoring old storage. Production
acceptance therefore requires either a hardware-backed monotonic counter or an
equivalent separately justified recovery-rooted mechanism. Until that exists,
the software floor is corruption/replay protection but not physical rollback
resistance.

Recovery has its own sequence and trust domain. A recovery image may boot to
repair a failed system but cannot silently lower the normal system floor.

## Recovery media

Release acceptance requires at least two independently written and verified
recovery cards. Recovery media must be independently bootable, read-only by
default, contain the exact public trust and layout profiles, and provide no
default credential. It must validate the on-device journal, selector, slot
proofs, and readback before offering a bounded repair.

The truncated field image is forbidden as recovery input. Neither vendor
first-boot partition expansion nor the Bli-PiKVM MSD recipe is an accepted A/B
layout.

## Current implementation boundary

`bli-kvm-system-boot-control` implements only canonical record validation,
dual-copy generation selection, journal/selector coherence, and pure
reconciliation outcomes. It opens no file, device, mount, boot environment, or
socket and cannot persist a record, decrement a real attempt, select a slot, or
reboot. A storage adapter must later prove atomic copy replacement and replay
the returned reconciliation action under recovery authority.
