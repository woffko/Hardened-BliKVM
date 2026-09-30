# Native application recovery protocol

Status: implementation design from corrective P5 source review. Recovery
inspection, pre-teardown rejection intent and quiescence-gated selection
restoration are host-implemented. A host restoration coordinator now drives
startup and caller-supplied identity-fenced probes, manager acknowledgement and
exact challenge cleanup. Production probes, next-operation handoff and service
integration remain incomplete; this is not an installed
configuration or release acceptance.

## Existing evidence and the gap

- `update-manager::inspect_recovery` now provides read-only journal/pointer
  classification with independent confirmed-anchor and system-compatibility
  verification. It tolerates an invalid candidate without repairing anything.
  It does not decide rollback reason, persist intent or authorize services.

- `update-runtime::UpdateSupervisor::activate_native_candidate` retains manager
  locks through admission, quiescence, startup and identity-fenced probes. Its
  failure path retains the challenge and pending transaction; it does not confirm.
- `update-manager::reject_health` authenticates operation/binding and immediately
  calls `rollback_document`. `recover` can also call that function for interrupted
  preparation, boot change, timeout, pointer mismatch or invalid target.
- `rollback_document` validates the previous slot, switches the pointer, records
  the completed rollback and clears pending. It has no service quiescence or
  restored-readiness callback.
- `update-runtime::watchdog_once` invokes manager recovery before managing any
  services. It publishes a confirmed-apply request after manager rollback and
  uses the challenge as an interrupted-publication anchor.
- Packaged confirmed-apply dispatch restarts only the server. It does not restore
  owner, proxy and boardd executable identities or verify their readiness.

Thus neither a restored pointer nor an existing confirmed-apply request proves
native recovery. Calling the legacy watchdog inside a native rollback flow can
change selection while candidate device owners remain alive.

## Required durable state changes

### Pre-admission credential crash window

For already-admitted schema-3 operations, manager
`begin_native_recovery_with_credentials` and runtime
`begin_native_credential_recovery` now persist HealthChallengeLost for exact
partial/absent credentials, after higher-priority recovery reasons. Mismatched
or unsafe surviving evidence is an error. The decision preserves selection,
floor and pending evidence; restored readiness and exact cleanup remain required.
Historical pre-admission orphans still require the ownership resolution below.

The former native runtime called `HealthChallengeStore::issue` before
`admit_native_activation`, permitting orphan credentials after a crash. Native
issuance now uses `issue_after_admission`: generate identity/token in memory,
persist Prepared with that operation and binding, then publish token and state.
No credential file is published if admission returns an error. An ambiguous
admission error may still leave Prepared; recovery must inspect the journal.
Publication failures retain its exact authority, and Prepared recovery takes
precedence over missing-credential classification. Legacy issuance retains its
existing semantics. Historical orphan files from an earlier implementation are
still refused by the empty-runtime precondition rather than deleted implicitly.

Manager state/slot locks are held by `ApplicationUpdateManager`, so a future
cleanup operation must use that existing authority, revalidate native idle,
generation and confirmed selection, and distinguish pre-admission orphans from
terminal-history cleanup and a pending activation. Do not invoke legacy
`discard` merely because the current journal has no pending operation.

Runtime ownership is a material integration prerequisite: the observed installed
configuration uses v2 slot/state directories with the unsuffixed runtime directory.
Absence from one journal alone does not prove that credentials are unowned by
another configuration sharing that runtime root. Before enabling orphan cleanup,
establish exclusive runtime-to-manager ownership, including migration and legacy
entry points. Cover token-only, state-only, full-pair, ambiguous persistence,
newer credentials, mismatched history and shared-runtime configurations in crash
tests. This remains an open requirement, not implemented cleanup authority.

Concurrent access through the current implementation is now fenced separately:
`HealthChallengeStore::open` takes a nonblocking exclusive flock on its validated
directory descriptor for the store lifetime. Competing stores sharing that inode
receive `RuntimeBusy`, including managers with different slot/state locks; drop
releases the lock. No lock file is created, and credentials are neither rewritten
nor removed on contention. This is advisory exclusion for cooperating current
binaries, not a durable runtime-to-journal ownership binding, protection against
older/noncooperating writers, or authority to delete historical orphan files.
The deployment profile still needs to exclude those writers and establish the
runtime mapping. All 10 challenge tests and 30 supervisor tests pass after this
change; runtime/supervisor all-target Clippy passes with warnings denied.

Runtime contention is reported as CLI exit 66 (busy) and activation protocol
`Busy`, preserving retry semantics rather than returning generic Unavailable.
A cross-process CLI regression proves refusal without journal/pointer/credential
mutation and successful activation after lock release. `inspect-recovery` remains
usable while another process owns the runtime because inspection does not open
the challenge store. With this regression, all 31 supervisor tests pass.

### Production boot graph integration checklist

Read-only inspection of the packaged base units confirms that service integration
must be a complete native profile, not a replacement recover command:

- Both current and candidate servers require and start after update-recover.
  Update-recover is a oneshot ordered before both. A synchronous coordinator
  running inside that unit cannot start either server and wait for readiness:
  the server start is ordered after the coordinator's own completion.
- Board owner, video proxy and boardd execute `/usr/libexec/hardened-blikvm`
  binaries, whereas both servers execute the selected slot binary. The three
  base units are independently installable under `multi-user.target`. Native
  boot must prevent independent early starts and bind all four executable paths
  to the same verified selection, while retaining each component's sandbox and
  reviewed hardware configuration.
- The candidate server also requires the legacy confirm socket and invokes the
  legacy health reporter in ExecStartPost. Native preflight intentionally rejects
  this hook; removing the hook alone does not remove the socket dependency or
  disable competing confirmation authority.
- The proxy requires ustreamer. The native composition does not replace that
  external capture owner; the new graph must retain its explicit dependency and
  avoid accidentally creating a second capture process.

Native systemd preflight now reads effective `After`, `Requires`, `Wants`,
`Requisite` and `BindsTo` along with the existing startup-hook properties. It
rejects direct edges to legacy update-recover, update-confirm.socket and the two
legacy apply services, even if a drop-in removes ExecStartPost. Missing or
duplicate queried properties also fail closed. The ustreamer dependency remains
permitted. Tests verify rejection before start dispatch for every listed property
and legacy unit. This is bounded direct-edge inspection, not transitive graph or
installed-enablements verification. A native profile must provide its own
non-cyclic inspection/start arrangement rather than depend on legacy recovery.

The implementation must separate a non-serving selection/inspection boot gate
from the coordinator that starts and observes services, or give the coordinator
sole startup ownership with managed units that do not wait for its completion.
Whichever graph is selected, test effective merged units and installed enablement
links, not just base files. Cover cold boot, restart, candidate admission, failed
startup and restored startup. Reject legacy apply/confirm dispatch in native mode
and prove that no server can become available before its full composition is
verified. These are required integration gates, not changes already deployed.

### Production integration review disposition (2026-09-23)

#### Verified production privilege and dispatch constraints (2026-09-24)

Read-only review while the aggregate gate was running identified these concrete
integration constraints in the current packaged units:

- `hardened-blikvm-update-activation@.service` runs as the update user, has a
  15-second TimeoutStartSec, AF_UNIX-only networking, ProtectProc=invisible and
  empty capability sets. It cannot simply host a 45-second composition attempt
  with loopback TCP probes and cross-user executable observations.
- Its `OnSuccess` starts both legacy apply services. A native handler returning
  success must not inherit this dispatch, even if managed-server preflight passes.
  Native packaging must remove the worker's legacy completion triggers as well
  as candidate health hooks and dependencies.
- `update-recover.service` has the same unprivileged user, 15-second limit and
  networking/proc restrictions, and remains ordered Before both servers.
- The legacy apply-candidate unit has no User setting and executes root systemctl
  restart. This does not establish that the unprivileged activation worker is
  authorized to manage units itself.
- `update-manager/src/journal.rs::validate_directory` requires state-root owner
  to equal effective UID. Moving journal ownership into a root daemon would reject
  the existing update-user state directory; root is not an implicit ownership
  exception. Do not solve this by broadly changing installed state ownership or
  weakening that validation.

The production boundary must retain one update-user journal owner and explicitly
provide narrowly authorized service execution and process observation, or design
and verify a separate compatible ownership migration. The already suggested
daemon/worker split is not sufficient without this concrete UID/lock/sandbox
contract. Effective merged unit tests must include activation OnSuccess, service
timeouts, address families and process visibility in addition to the managed
units' dependency graph. No unit or device mutation was made by this review.

#### Privileged execution boundary review (2026-09-24)

Independent advisory review completed in job
`0d7a4387ff4c4888a2d1f96fe63cec7a` (exit 0). Its recommendation for one
authenticated local execution/observation helper is a useful direction, but its
proposed allowlist/path-only authorization is rejected. The packaged board owner
explicitly runs as root with CAP_CHOWN/CAP_NET_ADMIN, GPIO/HID device access and
writable configfs/network configuration. A sandbox does not make arbitrary code
in that unit safe. A path under an update-owned slot, or a caller-supplied inode,
does not establish trusted executable contents. CAP_SYS_ADMIN is also not an
implicit systemd authorization grant.

The selected design direction retains the update-user coordinator as the sole
journal owner and uses one local privileged boundary, subject to these required
implementation conditions (none is claimed implemented here):

- Authenticate peer UID on a root-controlled socket directory. Use closed unit
  enums and bounded frames; derive observed PIDs from the allowlisted unit,
  rather than accepting arbitrary process inspection requests.
- Independently verify the signed composition against root-controlled trust and
  target policy before authorizing privileged startup. Do not open another
  ApplicationUpdateManager while the coordinator holds its locks. Signature and
  payload verification must be factored as read-only artifact verification, not
  a second journal mutation authority.
- Copy verified payload bytes into a private root-owned execution tree, checking
  the destination bytes against the authenticated manifest before publication.
  Protect every directory component, selection link and executable from updater
  writes. Holding a source descriptor or checking its inode alone does not stop
  same-inode writes; chmod on updater-owned files is not an immutable boundary.
  Effective unit ExecStart must resolve only to that protected tree. Account for
  the extra composition copy in peak staging space and crash cleanup.
- Keep operation/generation and anti-rollback decisions with the journal owner;
  explicitly document that independent signature verification does not itself
  authenticate those decisions. Before deployment, settle whether the threat
  model requires independent rollback-floor enforcement at the privileged
  boundary. Do not infer a trusted generation from an IPC field.
- Retain protected executable handles/identities across startup and readiness.
  Adapt the coordinator's identity contract to the execution copy explicitly;
  the original staging inode will differ. Observe unit MainPID, start ticks and
  executable identity again after probes. A pathname-prefix test is insufficient.
- Enforce a helper-side monotonic attempt deadline, finite request/output sizes
  and single composition session. Disconnect stops further dispatch but cannot
  undo systemd jobs already accepted. Reconcile outstanding jobs and actual unit
  state before allowing a new attempt; do not equate EOF with cancelled mutation.
- Remove legacy worker OnSuccess triggers and recovery dependency cycles in the
  native profile. Place loopback probes in a boundary with explicitly permitted
  networking; AF_UNIX-only activation workers cannot perform those probes.

Required negative tests include modified same-inode payloads, writable ancestor
or selection links, invalid signatures/targets, replayed session handles,
unauthorized peers/units, PID reuse, disconnect after systemd acceptance, and
failed protected-tree publication. Existing locks, inventory tests and mock
readiness tests do not satisfy these production-boundary checks.

The stager now provides `ApplicationSlotStager::open_protected` for a separate
execution store. It requires effective UID 0 and walks from `/`, rejecting any
non-root-owned, group/other-writable, symlink or non-directory component. This
walk is repeated by the stager's root-validation gates, alongside its existing
held-directory identity check. The existing reader APIs can then create a private
snapshot, independently verify the archive/signature and publish root-owned
payload copies without touching manager state. Normal `open` retains existing
update-user behavior. This is a storage primitive only: no helper, protected
selection link, effective ExecStart mapping, compatibility/floor policy or native
coordinator call site has been connected yet. Existing root-owned writers remain
trusted; the directory walk is not protection from a malicious root process.

Host verification: 23 stager tests pass (7 unit, 16 integration), including new
negative cases for mutable ancestors, sticky public directories, symlinks,
non-directories, traversal and unauthorized callers. All-target Clippy passes
with warnings denied. Ancestor-walk positive tests use isolated unprivileged
fixtures; a production root-owned execution-store deployment remains untested.

#### Protected composition preparation implementation (2026-09-24)

`update-runtime::native_execution::NativeExecutionStore` now connects the
protected stager to native inventory validation. Production construction uses
only `open_protected`, with trust, system sequence, minimum application sequence
and quota supplied by trusted local configuration. `prepare` independently
verifies the archive and applies these checks before extraction/publication or
idempotent acceptance. It then opens the five native destination identities and
returns `PreparedNativeExecution`, borrowing the store so its lock remains held.
An exclusive preparation borrow serializes sessions. No manager is opened.

The underlying `stage_reader_with_policy` callback only sees a fully verified
archive. Policy rejection neither publishes a new slot nor accepts an existing
one, and preserves existing receipts. Wrong signatures and tampered payloads
fail before the callback. Ordinary staging retains its existing behavior.

Verification: all 25 stager tests passed; two new runtime tests cover version,
system compatibility, missing/empty/wrong-mode native members, quota shape, and
the actual signed archive preparation pipeline. The latter checks refusal
without publication, successful held destination files and independence from
subsequent source-buffer mutation. It uses an unprivileged store fixture to test
the pipeline, not to claim a root deployment. Stager/runtime all-target Clippy
passed after fixing a test expression; a fixture key-domain/missing legacy
platform-broker issue and SHA formatting incompatibility were corrected before
the final passing targeted test. Dependencies were promoted from existing
dev-dependencies; no new dependency version or lockfile change was needed.

This is artifact preparation, not service execution authority. Authenticated
helper IPC, protected selection links, effective ExecStart mappings, coordinator
identity adaptation, trusted policy loading and durable floor synchronization
remain required before enabling native production startup. The policy floor is
explicitly a supplied snapshot, not an independently maintained rollback journal.

#### Inspection CLI and runtime dispatch

The supervisor CLI now exposes:

```text
bli-kvm-update-supervisor --config /etc/hardened-blikvm/update-supervisor.json inspect-recovery
```

This acquires the normal manager locks and calls `inspect_recovery` directly,
before constructing a challenge store or runtime supervisor. It verifies the
confirmed anchor and reports generation, selection classification, pending state,
rollback intent and historical completion through an explicit field allowlist.
It omits completion health bindings. Exit success means inspection succeeded;
it does not mean that selection is safe to start or that services are healthy.
The command does not mutate the journal, selection or credentials, but normal
manager opening/locking still requires the configured filesystem permissions.
It does not select a production profile or replace the installed recover unit.

Supervisor all-target tests pass (9 library, 15 binary, 5 CLI, 1 upload contract).
The CLI regression verifies that inspection works without creating the runtime
directory, tolerates a damaged pending candidate while validating the confirmed
anchor, and preserves journal, pointer and challenge/token bytes. The binary
receipt test verifies completion binding omission and strict argument parsing.
All-target Clippy with warnings denied, formatting and diff checks pass.

Review job `e928b83a23bb46639db62a4c2474ea86` completed successfully. Its
daemon/worker split is a design suggestion, not an implemented interface. Source
inspection confirms that `UpdateSupervisor::activate_native_candidate` already
accepts the expected generation, a `NativeStartController`, a deadline, an
admission-delivery callback and identity-bound readiness probes. It retains the
manager locks across admission, EOF delivery, quiescence and startup. Production
dispatch must reuse that ordering, followed by `confirm_native_candidate`, rather
than reproduce it with independent manager instances.

At review time, `boot_native_composition`, `serve_quiescence_requests` and
`request_quiescence` were proposed pseudocode, not available runtime APIs.
The bounded runtime boot dispatcher described below is now implemented; the two
IPC APIs remain unimplemented suggestions.
A privileged helper must not try to reopen the manager while a requesting worker
holds its exclusive locks. Choose and test a single journal owner; if execution
is delegated, keep the helper protocol bounded to authenticated, exact operations
without transferring independent journal or confirmation authority. A capability
such as `CAP_SYS_ADMIN` alone is not proof of systemd D-Bus authorization.

The next coherent production profile must cover these paths together:

| Entry point | Required dispatch and evidence |
| --- | --- |
| Cold boot / coordinator restart | Distinguish idle confirmed, current completion, live pending, and recovery intent; fresh complete-composition readiness, never historical receipt alone |
| Admission | Existing native candidate pipeline; flush and EOF before quiescence; exact generation and full signed inventory |
| Confirmation | Existing native identity-fenced confirmation and exact cleanup; disable generic reporter/socket authority in this profile |
| Watchdog / failed activation | Credential-aware durable intent, full quiescence, restoration and readiness-gated acknowledgement |
| Status / stage / GC / legacy apply | Explicit schema/profile dispatch; no legacy recovery side effects or independent server restart |
| Installation / rollback | Effective merged unit graph, enablement links, selected-slot binaries for all four components, actual configured roots, bounded authorization and retained sandboxes |

Do not replace the legacy base units globally or enable schema migration from a
boolean configuration flag alone. Test the complete opt-in profile and rollback
against the observed v2 composition first. Native idle boot dispatch, production
probe configuration, exclusive runtime ownership and the privileged execution
boundary remain implementation work. The runtime idle-start primitive described
below is now implemented; production CLI/service wiring is still absent.

`UpdateSupervisor::boot_native_composition` now dispatches under the held manager
locks using the schema version exposed by `RecoveryInspection` (also reported by
CLI `inspect-recovery`). It rejects legacy schema 1 and stale generations without
migration or service calls. Current completion uses `start_completed_composition`
with fresh startup/probes. Pending/intent uses credential-aware recovery decision
and `restore_native_composition`; interrupted Prepared admission therefore restores
through acknowledgement and exact cleanup. A still-live pending transaction
returns `NativeBootOutcome::AwaitingHealth` with its unchanged pending metadata;
it does not start services, run readiness probes or renew the health window.
Callers must continue native confirmation/watchdog handling instead of reporting
this outcome as Ready. Idle schema 3 uses `start_idle_composition`.

Legacy confirmed-apply evidence is rejected before native recovery decision,
including reason branches that do not invoke the credential-inspection callback.
Direct restoration repeats this guard before any controller calls. Checking only
at completion cleanup was too late: it could leave competing legacy authority
present during quiescence and startup. Rejection preserves the legacy request,
pending credentials and journal rather than deleting evidence to make progress.
The targeted boot test covers both entry points and subsequent normal recovery
after test-fixture conflict removal; credential-loss regression also passes.

The boot regression covers legacy refusal, idle readiness without journal change,
failed admission delivery through recovery, completion replay with fresh service
startup, and intact live pending without service calls/deadline changes. Targeted
runtime test passes; all 39 manager tests and all 30 supervisor tests pass after
the inspection schema field addition. All three crates pass all-target Clippy
with warnings denied. Production controller/probe configuration and effective
unit graph remain required before exposing mutation dispatch.

Additional boot fault evidence: an expired attempt leaves journal and services
untouched. Failure of restored readiness retains the same pending transaction,
Restoring intent, unchanged floor and credential bytes. A later successful retry
completes that exact intent and only then removes credentials. Board readiness
also checks that a reported failsafe latch has maximum (100 percent) reported
duty, as required by the fan-supervisor state machine. This checks response
consistency; it is not a physical waveform or RPM measurement.

Web delivery readiness now requires exactly one appropriate Content-Type for
root/HTML and lowercase .js/.mjs/.css/.wasm/.svg routes, in addition to signed
size/hash validation. MIME parameters and type case are supported; other data
resources retain their signed-byte checks. Six web-related runtime tests pass,
including complete signed-tree delivery and malformed MIME rejection. Actual
browser execution, authenticated API readiness and endpoint-to-process binding
remain required production gates.

`UpdateSupervisor::start_idle_composition` provides the schema-3 idle/confirmed
branch without migrating or writing the journal. It rejects pending operations,
recovery markers, stale generations and leftover runtime credentials before
service mutation. It preflights the complete signed composition, quiesces all
managed units, revalidates selection, starts owner/proxy/boardd/current server,
and requires fresh identity-fenced probes within a maximum 45-second attempt.
Selection and empty-runtime checks bracket callbacks. Probe failure is an error,
not persisted health or permission to fall back to legacy apply. Existing exact
completion/confirmation cleanup remains a prerequisite when credentials survive.

The targeted idle-boot regression passes: schema-1 refusal, stale-generation
refusal, full startup/probe ordering, failed readiness, unchanged journal bytes,
and preserved orphan credentials with no service calls. Runtime all-target Clippy
passes with warnings denied. This is host evidence only; the full aggregate has
not been rerun after this addition.

The first implemented transition is `begin_native_rejection`: exact generation
and operation fences, independently verified rollback anchor, then an atomic
schema-2 journal write containing `nativeRollback` (operation, intent generation,
explicit-health-failure reason). Pointer, floor and original activation phase
remain unchanged. The marker represents `RollbackPending`; it survives reopen.
Legacy journal entrypoints refuse schema 2, including watchdog recovery and
generic confirmation/rejection. `inspect_recovery` remains available. Existing
schema-1 journals omit the new field and retain their serialized contract.
`restore_native_selection` requires the exact generation and persisted intent,
checks the confirmed anchor before and after a trusted quiescence callback, then
switches selection and persists `Restoring`. Pending activation and history remain
pending; the floor does not advance. Re-entry requires quiescence again, including
after interruption between pointer replacement and journal persistence. This is
selection restoration only. Runtime `restore_native_composition` now binds the
exact generation/intent to confirmed inventory preflight, quiescence, selection,
`start_restored_and_observe` and `check_readiness`, under one deadline of at most
45 seconds. The restored startup guard requires `Restoring` and a confirmed
pointer; ordinary Current startup still rejects pending state. Successful probes
must match startup process identities. The coordinator now runs those probes
inside `acknowledge_native_restoration`, persists completion only on success,
then cleans the exact challenge. No production path invokes these opt-in APIs.
Release/system compatibility provisioning remains mandatory.

`begin_native_recovery` now provides the non-disruptive manager decision for
InterruptedPrepare, BootChanged, HealthTimeout, PointerMismatch and TargetInvalid.
It independently verifies the confirmed anchor, requires the exact generation,
and persists native intent before any pointer or service mutation. Existing
intent is returned unchanged, preserving its original reason. A live candidate
inside its original health window is left untouched; same-boot reversed time is
rejected. No pending operation returns no new intent, so callers must inspect the
journal to distinguish idle from completed recovery. Schema 3 now also accepts
HealthChallengeLost through runtime-sourced exact credential inspection, as
described above; schema 2 retains its earlier reason restrictions. The original
26-test recovery checkpoint has been superseded by 39 manager and 69 runtime
tests. These host results do not establish production service integration.

Extend the manager protocol with explicit rollback intent and restoration state.
The names below are proposed; existing serialized enums must not be changed until
their journal compatibility and system prerequisite contract are implemented.

| Phase | Durable meaning | Permitted native action |
| --- | --- | --- |
| `Prepared` | Candidate admission, previous selection retained | Deliver receipt, then bounded quiescence |
| `AwaitingHealth` | Candidate selected; not confirmed | Candidate startup and health checks |
| `RollbackPending` | Operation rejected; reason and exact previous anchor retained | Quiesce the entire composition, then restore selection |
| `Restoring` | Previous slot selected; service restoration not acknowledged | Start and verify the complete previous composition |
| Completed rollback record | Previous composition passed the required restoration gate | Publish/acknowledge exact recovery completion and clean obsolete challenge |

Rollback intent must bind the operation, generation, target, previous/confirmed
anchor, reason and health binding. A failed candidate may be unreadable: validate
the rollback anchor independently rather than requiring the invalid target to
pass a complete status query. Do not accept a replacement anchor from the caller.
An explicit health failure must not be relabeled as a lost challenge.

The rollback floor remains unchanged throughout restoration. The operation must
not be recorded as fully recovered merely because the pointer was restored.

## Coordinator ordering

1. Acquire the existing exclusive manager/slot locks. Validate the exact operation
   and recovery authority. Read and verify the durable previous anchor without
   repairing the pointer as a side effect. Preflight the required native unit
   contracts and complete signed payload before disruption when possible.
2. Persist `RollbackPending` before the first stop request. If persistence fails,
   dispatch no stop or pointer operation. A boot-time recovery must honor this
   intent rather than renew candidate health or resume candidate startup.
3. Stop producers, then backends with existing clean-stop observations. Require
   no main/control process, job or remaining cgroup and a normal successful exit.
   A timed-out systemctl client is not cancellation of its queued job. Any
   uncertain stop retains intent and prevents pointer restoration/startup.
4. Restore the verified previous selection under the held locks, then persist
   `Restoring`. A crash between pointer replacement and journal persistence must
   be recognized from the retained intent and its exact two authorized slot IDs.
   No arbitrary observed pointer becomes trusted recovery state.
5. Start the selected previous owner, proxy, boardd and current server in the
   closed dependency order. Update the current-server selection guard to allow
   exactly this authenticated restoration state, not arbitrary pending updates.
6. Require complete restored executable identity, IPC/backend health,
   authenticated API and signed web delivery. Bind observations before and after
   probes and to the same journal generation. This is a bounded point-in-time
   readiness gate, not an atomic guarantee that processes can never fail later.
7. Persist the completed rollback only after that gate. Then acknowledge the exact
   recovery request and remove the obsolete challenge. If publication/cleanup
   fails or the process crashes, replay must retain enough evidence to reconcile
   the exact operation without authorizing another candidate or an older request.

On recovery re-entry, never infer that a unit is still stopped solely from a
durable phase: systemd may have restarted it after the previous process exited.
Reobserve/quiesce before any pointer repair or owner replacement. Recovery uses
its own bounded attempt budget; it must not resurrect or extend candidate health.

## Compatibility and boot provisioning

`NativeSlotIdentity::start_completed_and_observe` and supervisor
`start_completed_composition` now provide an opt-in completed-state startup path.
They require the exact completion receipt/generation and confirmed selection,
validate cleanup and the complete signed inventory, preflight all four units,
quiesce the full composition, and perform fresh identity-fenced startup/readiness
under a new attempt budget of at most 45 seconds. The historical receipt is not
rewritten or promoted into current health. Ordinary legacy Current startup is
unchanged. After renewed user authorization, runtime Longrun job
`7c0b525ea0f945b49f1069b4f044d4ea` passed all 62 library tests, including the new
startup scenarios. All-target Clippy also passes. No packaged boot unit invokes
this API yet.

### Completion and cleanup constraints from the current journal

#### Native lifecycle implementation (manager admission and idle migration)

Runtime idle migration is now bound to actual credential storage through
`UpdateSupervisor::migrate_native_idle`. It requires the exact generation and a
complete verified confirmed native inventory. A current completion authorizes
only its exact existing cleanup; otherwise the runtime must contain neither
challenge file nor a legacy confirmed-apply request. Empty-runtime checks also
run on already-migrated replay. Orphan full/state-only/token-only credentials,
legacy apply requests and incomplete signed payloads are refused without deletion.
Three focused runtime tests and runtime all-target Clippy passed. The preceding
62-test runtime baseline passed in job `e89f9bf916424a6a9bb745122a1b9c9e` (exit 0,
29.043 seconds); those 62 tests predate this runtime migration addition.
Activation/startup/confirmation dispatch still needs coherent schema-3 wiring.

`confirm_native_activation` now completes the manager-side schema-3 success path.
It fences generation, operation, target and health binding before the trusted
whole-composition readiness callback. It validates time, signed target and
confirmed anchor, then repeats those checks with fresh kernel time and an unchanged
full journal after readiness. Only success records Confirmed, clears pending and
raises the floor. Confirmed replay performs no readiness callback or journal write;
it is historical operation evidence, not authority to clean a newer operation.

The separate `NativeConfirmationOutcome::RecoveryPending` reports durable native
intent for an invalid target, mismatched pointer, changed boot, expired deadline
or failed readiness. It does not claim rollback completion and does not switch
the pointer, stop services, clear pending or advance the floor. Invalid bindings,
stale generation, reversed same-boot time and callback document substitution are
refused. Clock/persistence errors require durable-state inspection by the caller.
Three additional host regressions cover successful confirm and another admission,
replay, failed readiness, timeout before/after probes, boot change, damaged target,
reversed clock and changed journal. All 37 manager tests, all-target Clippy and
runtime all-target compilation passed. Runtime binding of probes/credentials and
production native dispatch remain incomplete.

Native admission/execution is now host-implemented too:
`admit_native_activation` accepts only schema 3 with an exact generation and no
current rollback/completion, and persists Prepared without switching selection.
`execute_native_prepared_activation` requires that generation, exact request and
valid time, invokes trusted bounded quiescence after the caller has delivered
admission EOF, then checks fresh time and unchanged full document before selection.
Legacy admission/execution still reject native journals. Closed common helpers
reuse target verification, replay, anti-rollback and Prepared validation without
relaxing either public loader.

Before native Prepared persistence, the manager serializes prospective Prepared,
AwaitingHealth, Confirmed, RollbackPending, Restoring, completion and archival
documents against the 64 KiB limit using maximum-width generations and the longest
supported rollback reason. It also requires generation space for the five normal
remaining recovery transitions. This is a serialization bound, not disk-space or
unlimited-retry assurance. Pointer selection now validates the prospective journal
before switching, while retaining the existing crash-recovery ordering.

New host tests exercise reopen/execution, blocked legacy dispatch, replay, complete
recovery followed by a different admission with the old archive retained, and
post-quiescence expiry/document substitution. All 34 manager tests, manager
all-target Clippy and runtime all-target compilation passed at that milestone.
Runtime credential/admission wiring and production dispatch remain incomplete;
these manager callbacks do not themselves prove delivery of network EOF.

The manager now accepts explicit schema 3 and exposes `migrate_native_idle`.
Migration requires exact generation, no pending activation or rollback intent,
verified referenced slots and a confirmed pointer. It validates the complete
prospective serialization before invoking a trusted bounded runtime cleanup hook,
then revalidates selection, anchors and the whole original journal before CAS.
Schema-2/3 completion is moved intact to its terminal history record; schema-1
idle has no receipt. Pointer, floor and terminal outcomes stay unchanged. A
schema-3 idle replay does not invoke cleanup, and stale generations are refused.
No production entrypoint invokes migration yet.

Archived receipts are schema-3-only and validated against the matching terminal
operation, target, binding, phase, reason and historical generations. They cannot
serve as current cleanup authority. Native intent/restoration/acknowledgement now
preserve schema 3. Legacy mutation loaders still accept schema 1 only. Four new
host regressions cover reopen/replay, unfinished-recovery refusal, callback
failure, changed pointer/document, stale cleanup, archive tampering, generation
exhaustion, schema-preserving restoration and the 64 KiB serialization limit.
The initial migration milestone passed 32 manager tests and all-target Clippy;
the follow-ups above extend that evidence. Runtime callback wiring remains to
implement; migrating a production device now would strand its legacy
update entrypoints and is not authorized by these host APIs.

Gemini review `f917c904f1914d47b175a1a6e2e60abe` finished exit 0. Its separate-idle
transition recommendation and crash/capacity concerns informed this change.
Automatic archive truncation and mandatory panic on directory-fsync failure were
not adopted. Retaining evidence and returning persistence errors requires callers
to re-inspect state rather than assume an error implies an uncommitted write.

Use an explicitly versioned native lifecycle rather than silently relaxing the
existing schema-2 meaning. Schema 3 can identify native ownership even when neither
rollback intent nor a completion receipt is present (initial native idle or a
successfully confirmed native update). Keep schema-1/2 readers and validators
compatible with their existing documents; every legacy mutation loader must
continue rejecting any non-legacy schema.

| Starting state | Required gate | Atomic next state |
| --- | --- | --- |
| Schema 1 idle, no pending | Exact generation, verified confirmed selection, compatible system profile and no conflicting runtime markers | Schema 3 native idle or native Prepared; no intervening legacy activation |
| Schema 2 rollback/restoration | Complete existing native restoration and readiness | Existing schema 2 completion; migration cannot erase unfinished recovery |
| Schema 2 completed | Exact receipt/generation cleanup under locks, then revalidate unchanged journal/anchor | Schema 3 with prior receipt archived against its terminal operation, optionally new Prepared in the same CAS |
| Schema 3 idle | Full target verification, operation replay checks, native admission handshake | Native Prepared with durable operation/binding/time/previous/target |
| Native Prepared | Delivered admission EOF, bounded full quiescence, fresh boot/time and generation checks | Native AwaitingHealth; pointer and journal crash replay retained |
| Native AwaitingHealth | Fresh whole-composition identity/readiness and exact operation/binding/deadline | Native idle, terminal Confirmed, floor advanced exactly once |
| Native AwaitingHealth failure | Durable reason before teardown | Native rollback intent; never call legacy rollback_document |

Retain the previous completion against its terminal history record before removing
it from the current cleanup position. Admission invalidates old cleanup authority
through the generation change, even when historical evidence remains readable.
Newly issued challenge files before Prepared persistence require exact orphan
reconciliation; their mere presence must not authorize a replacement operation.
No-op/replay paths must avoid issuing credentials or dispatching service changes.

The current StateStore validates a closed document, caps serialization at 64 KiB,
then writes/fsyncs a temporary file, atomically replaces the journal and syncs the
directory. Receipt archival must stay within that bound and the existing maximum
128 history records. Reject capacity exhaustion before issuing admission or
stopping services; never prune replay evidence merely to fit another update.
An interrupted final directory sync may leave a committed document despite an
error, so replay must use observed state rather than assume a failed return means
that persistence did not happen.

The second Gemini review recommended historical completion coexistence (option C),
but its proposed clearing on confirmation omitted native idle and initial native
activation semantics. This plan instead makes that mode explicit and preserves
the existing-format boundary. Before implementation, test conversion, stale cleanup,
every persistence boundary, journal-capacity exhaustion, both no-op and replay,
and confirmation failures that retain native recovery responsibility.

Journal validation now rejects orphan Pending history records in every pending
phase: there must be exactly one Pending record globally and it must match the
active pending operation. Host regression coverage includes Prepared,
AwaitingHealth, RollbackPending and Restoring; terminal history remains valid.

Next-operation handoff is still blocked deliberately: `prepare_activation` loads
through the legacy `load_required`, which rejects schema 2. Clearing a completion
receipt alone would also violate the current schema-2 exclusive intent/completion
invariant. A native admission transition must preserve the prior completion's
cleanup fence until cleanup is verified, retain terminal operation replay
history, and persist its new Prepared operation atomically in a native-only
journal state. Its executor and health-confirmation paths must understand that
state before admission is enabled. Do not temporarily downgrade to schema 1 or
broaden the legacy loader to make the next activation work.

The in-progress schema-2 validator requires both `nativeRollback` and its matching
pending activation/history record. Therefore clearing pending after probes alone
is not a valid completion transition. Returning the document to schema 1 would
also reopen legacy mutation paths before native cleanup and boot routing have
been established. Neither shortcut is an acceptable acknowledgement mechanism.

The completion implementation must retain durable, validated evidence linking
operation ID, intent/completion generations, rollback reason, target and confirmed
anchor to the completed history record. A completed native state must remain
distinguishable from an in-progress restoration and from legacy rollback. Its
validation must reject mixed pending/completed records and substituted anchors.
Recovery inspection must continue to work without verifying a damaged candidate.

Readiness must be checked under the held manager locks with the exact Restoring
generation/intent and confirmed pointer before and after the trusted callback.
Persist completion only after the bounded callback succeeds; a stale observation
object passed in by a caller is not a fresh readiness gate. Floor advancement is
never part of rollback completion.

`acknowledge_native_restoration` now implements this manager transition: validate
the exact Restoring generation/intent and confirmed selection, invoke a trusted
fresh readiness callback under the existing locks, then independently verify the
anchor/pointer again and require the complete journal to remain unchanged. An
atomic generation-checked write clears pending/intent, records RolledBack history
and lastRollback, and retains schema 2 with `nativeCompletion`. The receipt stores
the original intent, completion generation, failed target, confirmed slot and
health binding. The validator requires exactly one intent or completion, binds
completion to the matching terminal history, and rejects mixed or substituted
state. Legacy APIs remain blocked; the receipt is historical readiness evidence,
not current health after reboot. Next-operation handoff remains to be implemented.

Manager tests cover Prepared and AwaitingHealth origins, early/stale/failed
readiness, pointer replacement during the callback, no floor advance, reopen,
legacy refusal, replay rejection and eleven invalid receipt/journal mutations.
All 24 manager tests and manager/runtime all-target Clippy passed. Gemini supplied
an advisory design review; its boot/identity/cleanup fences were consistent with
the source review. Suggested ordered random operation IDs and environment labels
as identity proof were not adopted. No target recovery acceptance is implied.

Runtime cleanup must use that durable completed operation, rather than whichever
challenge happens to exist on restart. Legacy `confirm_from_reader` publishes a
confirmed-apply request before consuming the challenge; native restoration has
already started the complete composition and must not blindly reuse this
server-only restart path. Test crashes before and after completion persistence,
each exact cleanup operation and final acknowledgement. Repeated cleanup must be
idempotent without repeating a service restart or deleting a newer operation's
challenge/request. These are implementation requirements, not completed gates.

### Cleanup artifact inventory (2026-09-23 source inspection)

`update-runtime/src/challenge.rs` stores two separate files: a bounded
`health-challenge.json` containing operation, health binding, target, boot and
time bounds, and a 32-byte `health-token`. `consume` validates the complete pair,
then removes the token before the state document. A crash between those removals
leaves state-only evidence; replay cannot use `consume` because `load_required`
requires both files. Native cleanup needs a validated state-only branch tied to
the completed operation. A token-only file has no operation identifier: its hash
must match the completed operation's retained health binding before removal.
Consequently, the completion record must preserve that binding, not only operation
and slot IDs. The token itself must never be persisted in the completion receipt.

`clear_stale_after_rollback` removes either file based on presence and safe file
metadata, without checking operation identity. Native cleanup must not reuse this
method. Native LSP references identify its callers in the existing legacy
supervisor recovery/reconciliation paths; these paths still require separate
native routing. Absence of both challenge files can represent completed cleanup,
but malformed or mismatched surviving files must remain intact and fail closed.

The separate `confirmed-apply-request.json` schema contains only generation and
slot ID, not operation ID. Its existing acknowledgement checks exact request
equality. Native completion must not publish a new legacy apply request (which
would restart only the server), nor delete an observed request just because it
names the confirmed slot. Define and persist the authorized request identity, or
reject an unexpected existing request, before adding native cleanup of this file.
Every cleanup mutation must retain the manager/slot locks and exact completed
journal fence; `HealthChallengeStore` alone does not acquire those locks.

`HealthChallengeStore::consume_exact` now implements the lower-level cleanup
mechanism for an expected operation, target and health binding. It validates all
surviving evidence before deleting either file, accepts the matching state-only
or token-only cases, and syncs the directory even on an empty replay. Eight
challenge tests cover these cases plus mismatched newer operations, corrupted
metadata/token/state and symlinks; the runtime library suite passed 62 tests.
`UpdateSupervisor::cleanup_native_completion` now binds this mechanism to an exact
generation and durable completion receipt under the held manager/slot locks. It
requires confirmed selection and refuses any legacy confirmed-apply request,
preserving that request rather than attributing it by slot alone. It rechecks the
recovery inspection after cleanup. `restore_native_composition` invokes it after
successful acknowledgement; cleanup failure leaves the completed journal intact.
Cleanup-only replay after reopen does not start services or rerun readiness.

The host coordinator regression covers real issued challenge binding, readiness
failure retaining pending, a completed journal with cleanup blocked by a legacy
request, stale/substituted receipts, reopen with only the state file remaining,
and repeated cleanup without service events. All 62 runtime library tests and
runtime all-target Clippy passed after wiring. Persistence fault injection and
production boot/next-operation routing remain outstanding.

New recovery phases are not backward-compatible with an old manager that does
not understand them. Bind support to the system/updater release and the signed
minimum-system requirement before native activation is enabled. Retained legacy
rehearsal profiles and receipts remain historical evidence, not migration logic.

The native system profile must route activation, boot recovery, watchdog and
confirmed-apply handling through the native coordinator. Merely changing the
interactive activation socket leaves legacy boot/watchdog pointer writes active.
Candidate ExecStartPost generic confirmation must be removed in that profile;
confirmation belongs to the complete composition gate. Unit dependency startup
must not reenter a manager while its locks are held. Slot-root owners must not be
enabled before reviewed boot recovery has established a valid selection.

Do not activate partial native drop-ins against the legacy updater. Inert example
files and host controller fixtures do not establish installed compatibility.

### Production dispatch findings (2026-09-23 read-only inspection)

`bli-kvm-update-supervisor/src/main.rs` dispatches `Activate` to
`supervisor.activate`, `WatchdogOnce` to `supervisor.watchdog_once`, and
`ConfirmStdin` to `supervisor.confirm_from_reader`. The activation socket has its
own early dispatch before this match. Integrating only a new boot branch would
leave the other entrypoints on legacy semantics; native configuration must select
one consistent implementation across all of them, including status and GC.

The packaged `hardened-blikvm-update-recover.service` invokes `watchdog-once`,
uses a 15-second start timeout, and runs as the unprivileged update user with
AF_UNIX-only networking, hidden unrelated proc entries and no capabilities.
Those effective constraints must be reviewed against systemd authority,
cross-user process identity observations and HTTP readiness probes before the
native coordinator can run there. Increasing the timeout alone is insufficient.

There is also a concrete startup-order trap: `server-current.service` both
Requires and is After `update-recover.service`, while recover is Before current
and candidate. Replacing recover's ExecStart with a coordinator that synchronously
starts current would wait for the very recover job that owns the coordinator.
The native unit graph must remove that self-dependency while ensuring components
cannot boot outside the reviewed coordinator. Merely running the existing
start-controller against these unit names does not establish a valid boot graph.

`update-apply-confirmed.service` performs a privileged restart of current-server
before calling its acknowledgement command. It neither restarts the complete
composition nor uses the new completion receipt before dispatch. Native mode
must not emit this request or inherit this path as its recovery executor. These
are source-verified integration gaps; no installed unit state was inspected here.

## Implementation-to-evidence checklist

| Deliverable | Current evidence / required next evidence |
| --- | --- |
| Ordered native quiescence | Host composition tests; target cgroup, device release and fan handoff still required |
| Durable rollback intent before teardown | `begin_native_rejection` persists schema-2 intent with generation/operation fences; coordinator and persistence-fault/stop-order integration still required |
| Read-only recovery planning | `inspect_recovery` verifies the confirmed anchor and classifies pointer without mutation; durable decision/coordinator integration still missing |
| Pointer restoration fenced by clean stop | Manager callback failure preserves pointer/journal; exact intent/generation and post-callback anchor verification pass host tests. Real controller integration and every stop-observation failure remain required |
| Restoration state across process restart | Host test reopens after pointer replacement before journal persistence and after `Restoring`, requires repeated quiescence and rejects stale replay; persistence-fault injection remains required |
| Full previous composition startup/readiness | Host coordinator and restored-role guard tested with controller/probe fixtures; real probes and production authority remain required |
| Recovery acknowledgement only after readiness | Manager receipt and host coordinator tests pass; real component/probe failures and production routing remain required |
| Runtime marker cleanup crash recovery | Exact receipt/generation cleanup and reopen/partial-file replay tested; persistence fault injection and production routing remain required |
| Boot/watchdog/apply authority and unit recipes | Missing native routing and compatibility provisioning; inspect effective packaged and installed units |
| Release-bound device recovery | Unrun; requires concrete artifacts, failure rehearsal and fresh strict acceptance |

Required crash cases include: before/after intent persistence; each stop dispatch
and observation; before/after pointer switch; before/after `Restoring` persistence;
each component start; each readiness probe; before/after rollback completion; and
before/after challenge/request cleanup. Assert one owner, no floor advance, no
early acknowledgement and no restoration from an unverified anchor in every case.

Full workspace checks and artifact builds remain subject to the Longrun contract.
Physical HID, GPIO/fan handoff and separate-media SD/RAW recovery retain their
operator/hardware gates. Native application recovery does not replace RAW recovery.
