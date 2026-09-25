# Native execution helper v1

Status: implementation design for the corrective P5 privileged boundary. It
refines the "Privileged execution boundary review" in
[Native application recovery protocol](native-application-recovery.md). Nothing
in this document is deployed or accepted on a device.

## Problem

The native coordinator must stop, start and observe the board owner, video proxy,
board daemon and server as one composition. The coordinator is the single
journal owner and runs as the unprivileged update user; the manager refuses a
state root whose owner differs from the effective UID. The board owner runs as
root with CAP_CHOWN/CAP_NET_ADMIN, GPIO/HID devices and writable configfs and
network configuration. Update-owned slot files are writable by the update user
(same inode, even when read-only mode bits are set), so a unit that executes an
update-owned path is not protected against that user.

The update user can therefore not be given systemd control of these units, and
root units must not execute update-owned bytes.

## Roles

| Role | Identity | Authority |
| --- | --- | --- |
| Coordinator | `hardened-blikvm-update`, existing supervisor | Journal, operation/generation, candidate/rollback decision, health confirmation |
| Execution helper | root, socket-activated, one session at a time | Protected copies, protected selection link, closed-unit systemd operations, process identity, readiness probes |

The helper never opens the manager, journal, pointer or health credentials. The
coordinator never receives a privileged operation other than the closed requests
below.

## Transport and session

- Socket `/run/hardened-blikvm-exec-api/exec.sock`; directory root-owned, not
  writable by the update user. Accept only peers whose `SO_PEERCRED` UID equals
  the configured update-user UID; other peers are closed before reading.
- One connection is one session. The helper admits one session at a time and
  holds the protected store lock for its lifetime.
- Fixed binary frames: one request frame at most 128 bytes, one response frame at
  most 512 bytes; unknown tags, trailing bytes, oversize frames and non-canonical
  identifiers are refused and end the session.
- Helper-side monotonic session deadline: at most 90 seconds from accept, and at
  most 64 requests. Every systemctl call is bounded by the remaining time.
- Disconnect or deadline ends dispatch. It does not cancel systemd jobs that were
  already accepted. A new session first cancels queued jobs of the managed units
  and then requires their observed state before any other request; if a job
  cannot be cancelled within the deadline, the session answers `Busy`. A stuck
  job therefore cannot lock recovery out permanently.

## Requests

All unit arguments are the closed `ApplicationUnit`/`NativeStartUnit` enums; the
helper maps them to fixed unit names. No path, unit name, command or policy
value is accepted from the peer.

| Request | Helper behavior |
| --- | --- |
| `Prepare { slot_id }` | Open `<updater slot root>/<slot_id>` from root configuration. Rebuild the canonical archive stream from the signed manifest, signature and payload files (see below), and pass it to `NativeExecutionStore::prepare`, which snapshots, independently verifies and publishes a root-owned copy. The published slot ID must equal the request. Idempotent. |
| `Select { slot_id }` | Require that every managed unit was observed cleanly stopped earlier in this session. Require a prepared protected slot that satisfies the local policy and the selection ledger, reverify its signed inventory, then atomically replace the root-owned `selected` link and sync the directory. Idempotent. |
| `Stop`, `ObserveStop`, `ValidateStart`, `Start` | The existing `SystemdController` contract, executed by root under the session deadline. Start is refused unless the selected protected slot has been verified in this session. |
| `ObserveIdentity { unit }` | Bind the unit's MainPID to the executable of the currently selected protected slot, using `observe_process_identity` with the held protected file. Returns PID, start ticks, device/inode, the selected slot ID and the SHA-256 recorded in the signed manifest for that role. |
| `Probe { component }` | Run the existing owner/proxy/board/web readiness probe from the helper, which has the required socket access and loopback networking. The web probe is checked against the protected web tree. |

## Rebuilding the archive from an updater slot

The updater slot is writable by the update user, so the helper treats it as
untrusted input:

1. Open the slot directory with `O_DIRECTORY | O_NOFOLLOW` and hold it.
2. Read the manifest and signature with fixed size limits, relative to the held
   directory, without following links. Verify the signature against the root
   trust configuration before opening any payload file. Manifest path validation
   is the verifier's existing validation.
3. Open each payload file component by component relative to the held
   directory, without following links; accept only regular files whose size
   equals the signed size.
4. Emit entries in the verifier's canonical sorted order. The stream goes
   through the normal snapshot, signature and hash verification, so concurrent
   modification causes refusal, never publication of modified bytes.

## Identity binding in the coordinator

The coordinator keeps its existing `NativeStartController` and identity traits.
The helper client implements them. For `observe_identity(unit, expected)` the
client hashes `expected`, the verified updater slot file, and requires the
helper's reported slot ID and role SHA-256 to match. The pre-probe and post-probe
identities are compared as they are today. The coordinator thus binds the
running process to bytes that are signed and equal to its own verified slot,
while the executed inode is the protected copy. The updater slot file belongs to
the coordinator's own UID, so hashing it is a consistency check, not a boundary
against the coordinator; the helper's own checks are the security boundary.

## Selection and unit mapping

The native profile maps `ExecStart` of the owner, video proxy, board daemon and
both server roles to `/var/lib/hardened-blikvm-exec/selected/...`, and the web
root to `selected/share/web`. The `selected` link and every ancestor are
root-owned and not group/other writable, as enforced by `open_protected`.

`Select` is refused unless the managed units were observed stopped in the same
session, so an automatic `Restart=` cannot run a newly selected slot that the
coordinator has not started and observed. The coordinator changes the manager
pointer first, then calls `Select` before the first start. After a crash between these steps, the next attempt repeats
`Select` for the journal-selected slot, so the two cannot diverge while services
run. The helper never selects a slot by itself.

## Rollback floor decision

The helper enforces a root-owned static minimum application sequence and the
local system sequence from its own configuration. In addition it keeps a
root-owned selection ledger with the currently selected slot and the slot that
was selected before it. `Select` accepts a slot whose sequence is at least the
current one, or exactly the recorded previous slot. Selecting forward records
the old current slot as previous; selecting the previous slot clears it. Native
recovery can therefore restore the slot that ran before a candidate, while a
compromised update user cannot move more than one step back. The ledger is
written atomically before the link is replaced; after a crash between the two,
the next `Select` repeats the same idempotent transition. The helper does not
accept a floor from IPC and does not confirm health.

## Boot and service graph

- Base units stay unchanged for the legacy profile. The native profile is a
  separate drop-in set. It clears `OnSuccess` of the activation worker, removes
  `Requires`/`After` on update-recover and the confirm socket, removes candidate
  `ExecStartPost`, and removes `WantedBy=multi-user.target` from the four managed
  units so that nothing starts them outside the coordinator.
- A native boot unit runs the coordinator's `boot-native` command after the
  helper socket. It is not ordered before the managed units, which removes the
  current recover/server cycle.
- The activation worker keeps AF_UNIX-only networking and no capabilities,
  because privileged operations and probes are delegated. Its start timeout must
  cover the 45-second composition budget plus admission.

## Crash space and cleanup

The helper adds one protected copy per prepared slot. Its quota is separate from
the updater quota. Before each `Prepare`, the helper removes protected slots that
are neither the prepare target, the selection ledger's current or previous slot,
nor the target of the `current` link (update package 0.1.0~dev14; before that the
collection was not implemented and a full store refused every new activation). Abandoned snapshots are cleaned by the stager's existing recovery.

## Required negative tests

- Unauthenticated peer UID; oversize, truncated, trailing and unknown frames.
- Prepare with a modified, same-inode payload file, a bad signature, a slot ID
  mismatch, a system sequence above policy or an application sequence below it.
- Select of an unprepared, damaged or policy-rejected slot; writable ancestor or
  replaced `selected` link; select without observed quiescence; a second
  rollback step below the recorded previous slot.
- Start before select; start of a unit with legacy hooks or dependencies.
- Identity for an exited or reused PID, or for an executable other than the
  selected protected copy.
- Disconnect after systemd accepted a job, then a new session that must cancel it.
- Session deadline and request count exhaustion.

## Review

An advisory Gemini review of this design raised five points. Archive rebuilding
now verifies the signature before any payload file is opened. The static floor
alone was replaced by the one-step selection ledger; the suggested counter that
advances on every successful start was rejected because it would forbid the
required restoration of the previous slot. `Select` now requires observed
quiescence. Queued jobs are cancelled by the next session instead of blocking it.
Hashing the updater file stays a consistency check, as explained above.

## Implementation slices

1. Closed protocol and frame codec with peer authentication.
2. Canonical archive rebuilding from a slot tree and `Prepare`/`Select` on the
   protected store.
3. Helper session server with an injected controller for host tests.
4. Coordinator client implementing the existing controller traits, with SHA-256
   identity binding.
5. Supervisor/helper CLI, native profile units and effective-unit checks.
6. Device installation and recovery rehearsal. This needs separate operator
   authorization.

### Implementation status

- Slice 1: `bli-kvm-update-protocol::execution` defines the closed request and
  response envelopes with a 1 KiB bound (three tests: closed variants, rejected
  path/unit/command/floor fields, slot and envelope validation, identity digest
  binding). Peer authentication belongs to the helper server in slice 3.
- Slice 2: `update-runtime::slot_archive::open_slot_archive` streams the
  canonical archive from an updater slot; the rebuilt stream is byte-identical
  to the builder output in the test fixture. Tests cover same-inode payload
  modification, truncation, symbolic links, foreign slot identifiers and
  signature verification before any payload file is opened.
  `NativeExecutionStore::prepare_from_slot` publishes distinct protected inodes;
  `NativeExecutionStore::select` applies the policy and the one-step
  `SelectionLedgerStore`, writes the ledger before the protected `current`
  link and replays idempotently after a crash between the two. Tests use
  unprivileged fixtures; a root-owned deployment is not yet tested.
- Slice 3: `update-runtime::native_exec_server` implements the bounded session
  (quiescence before `Select`, selection before `Start`, identities with the
  signed role digest, request budget and deadline) and `serve_connection`
  with peer UID authentication. `SystemdExecutionBackend` runs the closed unit
  operations and the existing readiness probes as root; the next session
  cancels and re-observes jobs queued by a disconnected coordinator.
- Slice 4: `update-runtime::native_exec_client::ExecutionClient` implements the
  native controller traits over one session and never replays an unanswered
  request. Native flows call `prepare` before disruption and `select` after
  observed quiescence.
- Slice 5: `bli-kvm-update-exec-helper` loads a protected root-owned
  configuration and refuses foreign peers before opening the store.
  Supervisor commands `migrate-native-idle` and `boot-native`, and
  `activation-socket --native-profile` for end-to-end admission and
  confirmation, use the helper at its fixed socket. The server requests
  admission only with `BLI_KVM_UPDATE_NATIVE_ADMISSION=1`. Native units are
  rendered by `tools/render_native_profile.py` and verified in their own
  synthetic systemd root; the update package 0.1.0~dev11 installs the helper
  units disabled and ships the replacements as examples.
- Update package 0.1.0~dev12: the status worker reads the journal through
  `observed_status`, so the GUI and API report status on a native (schema 3)
  journal instead of failing with RecoveryRequired.
- Update package 0.1.0~dev13, from the first native activation on the board
  (slot 16), which failed and left the board without a web server:
  - The native candidate unit kept `BindReadOnlyPaths=/run/credentials/%n:...`
    after its credential was removed, so systemd refused to start it
    (226/NAMESPACE). The renderer drops the bind and now rejects any unit that
    references `/run/credentials/` without loading a credential.
  - A unit whose last run failed was never observed as stopped, and
    `reset-failed` does not clear the exit status, so recovery was refused with
    `NotStopped(CandidateServer)` until an operator intervened. Stop
    observation now reports `StoppedAfterFailure` for a process-free terminal
    unit whose last run failed; quiescence, native start and identity
    observation accept it as stopped. A failed unit with a remaining process,
    job or control group is still refused.
  - After a native rollback the completion record remains until
    `migrate-native-idle` archives it; the status API reports the rolled-back
    confirmed state in the meantime, and new activations require the
    archive step.

### Deployment prerequisites (not done)

- Root-owned `/etc/hardened-blikvm/exec-helper.json` with the trust key, the
  coordinator UID, the updater slot root actually used on the device
  (`slots-v2`), static sequences and reviewed board requirements.
- Explicit `migrate-native-idle` on the device journal, which is a durable
  schema change; installing the native replacements and disabling the legacy
  recover/watchdog/confirm/apply units; a rollback plan to the legacy profile.
- These steps change the running device and need separate operator approval.
