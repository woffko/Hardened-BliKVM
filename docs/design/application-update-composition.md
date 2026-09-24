# Native application update composition

Status: corrective P5 audit findings and required implementation contract.
This document does not certify an installed device or a release.

## Observed source contract

`tools/build_production_update_bundle.py` selects five binaries from the runtime
Debian package plus the web tree. It requires all five binaries and
`share/web/index.html`. The signed application payload does not include the
systemd templates, drop-ins, Nginx configuration or provisioning state.

| Payload component | Packaged execution path | Activation coverage today |
| --- | --- | --- |
| `bin/bli-kvm-server` | `slots/current/bin/bli-kvm-server` | Candidate and confirmed server units select this path. |
| `share/web` | `slots/current/share/web` | Both server units select this tree. |
| `libexec/bli-kvm-control-broker` | `/usr/libexec/hardened-blikvm/bli-kvm-control-broker` | Board owner remains outside the selected slot. |
| `libexec/bli-kvm-video-proxy` | `/usr/libexec/hardened-blikvm/bli-kvm-video-proxy` | Board video proxy remains outside the selected slot. |
| `libexec/bli-kvm-boardd` | `/usr/libexec/hardened-blikvm/bli-kvm-boardd` | Board daemon remains outside the selected slot. |
| `libexec/bli-kvm-platform-broker` | `/usr/libexec/hardened-blikvm/bli-kvm-platform-broker` | Legacy template requires `kvmd.service`; it is not a native board deployment recipe. |

The slot prefix above is `/var/lib/hardened-blikvm-slots`.
The source evidence is in the corresponding files under `packaging/systemd/`:
`hardened-blikvm-server-{candidate,current}.service`,
`hardened-blikvm-board-owner.service`,
`hardened-blikvm-board-video-proxy.service`, `hardened-blikvm-boardd.service`,
and `hardened-blikvm-platform-broker.service`.

`hardened-blikvm-update-apply-candidate.service` restarts only the candidate
server. `hardened-blikvm-update-apply-confirmed.service` restarts only the current
server before acknowledging the apply request. Dependencies on an already
running owner/proxy do not restart those processes or select the slot binaries.

The health reporter checks HTTP 200 and JSON `code=0`, `status=ok`,
`backend=rust`, `platformId=bli-h616-pikvm`, then submits its one-use credential.
These checks do not establish owner/proxy/boardd executable identity, their
protocol compatibility, or successful web/video/device operation. Its response
size and deadline tests are transport evidence only.

The reviewed `94-application-update.conf` is now staged from one canonical source
for board-server, candidate and current roles by both bundle recipes through
`scripts/stage-application-update-profile.sh`. Update-runtime package dev10 ships
these three profiles under its documentation examples directory; it does not
activate the write opt-in on installation. Earlier package file maps are retained.
The status/upload/activation socket settings and write opt-in still require
explicit system provisioning for all three roles. Candidate/current base units
remain inert. Temporary rehearsal overrides in `tools/application_write_contract.py`
are not persistent release provisioning. Effective device overrides and production
native activation remain unverified.

## Required transaction contract

The concrete native rollback state/ordering gap and implementation checklist are
recorded in [Native application recovery protocol](native-application-recovery.md).
That protocol is a design, not an implemented or accepted recovery path.

1. **Closed native composition.** Define the native runtime component set and
   each component's executable, service, protocol and readiness evidence. Do not
   start the legacy KVMD-dependent broker to satisfy a payload inventory check.
   Select native inventory functionality explicitly. Keep uStreamer/Janus and
   other system dependencies version-bound through the system release contract.
2. **System prerequisites.** Provision reviewed service templates, socket
   permissions, environment, writable roots and update admission consistently
   for candidate and confirmed operation. Bind their compatible revision to the
   signed minimum-system requirement. Application slots must not rewrite live
   system units or grant arbitrary commands from payload metadata.
3. **One coordinated owner transition.** Persist intent before quiescing input
   and stopping affected components. Stop the previous owner completely before
   starting the candidate owner. Preserve gadget/media/ATX journals and enforce
   release/reconciliation. A second live owner is never a health probe.
4. **Coherent selection and recovery.** Bind every active native application
   executable to one immutable verified slot identity. Record the transition
   phase durably; recovery must converge after each interrupted phase, including
   pointer change, component stop/start, health confirmation and acknowledgement.
   An already running process does not change identity when a symlink changes.
5. **Composition health before confirmation.** Require bounded evidence for the
   selected executable identities, native IPC compatibility, authenticated API
   and web assets, and available video/board services. Keep read-only readiness
   separate from operator-observed HID/ATX and full release acceptance. Do not
   advance the confirmed floor based solely on the server's generic health JSON.
6. **Complete rollback.** Restore all affected component selections and restart
   them in dependency order. Verify restored readiness before acknowledging the
   recovery request. Retain recoverable intent when stop/start/readiness fails;
   do not erase pending evidence merely because a pointer was restored.

## Evidence required to close P5

The update-runtime composition kernel and systemd stop adapter are host-tested
foundations. Production activation still uses the legacy path. The adapter
checks main/control PID, job and exit state with bounded subprocess capture;
integration must additionally establish effective unit kill policy/cgroup
cleanup, producer termination behavior and dependency ordering on the target.
A timed-out systemctl client may have queued a stop job: recovery must reconcile
service state rather than assume that killing the client undoes the operation.
The adapter's maximum 45-second budget is a host-side ceiling, not a measured
target stop time or an extension of the signed transaction health deadline.

### Activation wiring constraints found in source

Manager foundations now expose separate `admit_activation` and
`execute_prepared_activation` operations, with intent preserved across reopen
and exact identity/clock validation before and after quiescence. They do not
yet change runtime admission or service execution. In particular, current
recovery treats Prepared as interrupted: handoff must be serialized with the
watchdog or explicitly represented in durable runtime state before network
admission can use this split. Prepared must not be mislabeled AwaitingHealth.

The runtime now offers a serialized alternative: a generation-bound admission
callback executes after durable Prepared while retaining manager locks, followed
by quiescence and selection in the same supervisor lifetime. A real socket caller
must deliver the Prepared receipt and close its activation stream within a fixed
deadline before the callback succeeds. Delivery failure retains recovery intent
without quiescing. The callback is host-tested but not yet wired to the network
worker; worker authorization and timeout requirements below still apply.

- `activation_socket::serve_stdin` waits for `activate` before sending the
  response. Quiescing the requesting server inside that call can deadlock a
  graceful HTTP drain: the server waits for the activation response while the
  worker waits for the server to exit. Admission/receipt delivery and durable
  execution must be coordinated explicitly rather than inserting a blocking
  stop into the current socket handler.
- `hardened-blikvm-update-activation@.service` runs as
  `hardened-blikvm-update` with empty capabilities and a 15-second start limit.
  It does not establish systemd stop authorization or fit the adapter ceiling.
  The execution design must preserve narrow privileges and bounded timing;
  changing this worker to unrestricted root is not an implementation contract.
- Server `main.rs` now handles SIGTERM/SIGINT with shared admission shutdown,
  HTTP drain, tracked HID upgrades and MJPEG cancellation under a 20-second
  deadline. Host library regressions pass; real-process signal, live WebSocket
  release, stalled-request and target systemd evidence remain outstanding.
  This does not resolve the activation request/drain dependency cycle above.
- The stop adapter now additionally requires effective `KillMode=control-group`,
  `SendSIGKILL=yes`, and an empty `ControlGroup` observation. Zero main/control
  PIDs with a retained group are not clean stop evidence. Target behavior and
  effective overrides remain unverified.

- Native start now reads effective `Id`, `LoadState`, `Type`, `ExecCondition`,
  `ExecStartPre` and `ExecStartPost` before dispatch. It requires the exact loaded
  simple service with no startup hooks. The existing candidate template has a
  generic health reporter in `ExecStartPost`, so it is deliberately incompatible
  with native coordinated startup. That reporter can wait on the same manager
  locks and cannot establish full composition health. Native provisioning must
  remove the legacy hook and leave confirmation with the coordinator. The guard
  is per-unit startup evidence, not proof of complete trusted dependencies or
  an implementation of native confirmation/recovery.

- `activate_native_candidate` now preflights all four candidate startup contracts
  before issuing a challenge or delivering Prepared admission. The generation
  fence precedes these read-only controller calls. A known incompatible last
  unit therefore cannot trigger quiescence of the running composition. Each
  actual systemd start still repeats its effective-contract check; preflight is
  not permission to ignore later unit configuration changes.

- Host transaction regressions: candidate component mismatch/unavailability,
  quiesce failure, stop/start failures, timeout, recovery after every durable
  boundary, failed rollback and retry. Assert one owner and one coherent slot.
- Clean-install artifact checks: all required templates/drop-ins/env/socket/root
  settings derive from the release recipe, with no rehearsal-only override.
- Reproducible application/system artifacts with exact dependency and source
  bindings; full `scripts/check.sh` through the authorized Longrun mechanism.
- Fresh target evidence: complete native component identity before update,
  candidate readiness, confirmation, failure rollback, reboot recovery and
  continued access to the update UI/API. Physical effects require the relevant
  observed-operator gate.

These requirements are additive to P6 RAW image/update/recovery and P7 strict
release-bound acceptance. Server/web slot success cannot substitute for them.
