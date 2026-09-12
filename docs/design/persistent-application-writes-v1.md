# Preparing persistent GUI application updates

Status: preparation requested; persistent device writes are not enabled.
The accepted baseline is slot 8, floor 8, generation 40, previous slot 7,
read-only status enabled and both write transports disabled. See
`../baselines/application-http-seq8-acceptance.md`.

## Configuration boundary

Keep the application/package defaults closed. Add one owned
`90-application-update-writes.conf` to each existing current/candidate server
drop-in directory, preserving the accepted status drop-in byte-for-byte.
The new file supplies the existing three explicit write settings; it does not
alter account, TLS, trust, Nginx, signed slots, update limits, or network ports.

Enable the two installed write sockets through exact owned
`sockets.target.wants` links. Server drop-ins contain `After` ordering but no
`Wants` or `Requires` dependency on write sockets: an apply/confirmed restart
must not reopen admission during shutdown or recovery. The status dependency
remains unchanged. A failed updater transport must not prevent the core KVM
server from starting; capability evidence is not worker-health evidence.

While uncommitted, four separate `89-write-policy-transaction.conf` drop-ins
add `Requires` and `After` on the transaction's boot-guard service to both
servers and both write sockets. Ordering alone cannot fail closed when the
guard fails. This dependency is on the guard, never on a write socket. A failed
uncommitted guard intentionally blocks those units until recovery succeeds.
The boot guard remains active after successful execution. Initial arming may
complete without restoration only after proving the same boot, prepared phase,
and still-absent write policy; otherwise it performs boot restoration.

The controller must reject pre-existing/foreign files and enable links, retain
their original absence in a durable backup manifest, and validate every owned
path, inode type, ownership, mode and content before removal. The manifest and
recovery code are root-private. No private configuration contents or secrets
belong in public receipts, source, command arguments or environment.

## Transaction and recovery

1. Verify the accepted installed package, slot/status, transport units,
   protected runtime and absence of conflicting guards. Save exact original
   configuration ownership and hashes before any configuration change.
2. Install and verify an independent deadline and an early-boot recovery unit
   before writing enable links or server overrides.
3. Install the owned configuration, reload systemd, start write sockets and
   restart only the confirmed server. Verify loaded/running settings, endpoint
   identity, existing status, and unchanged protected state.
4. Keep the transaction uncommitted through the required acceptance gates.
   Browser evidence must bind the tested configuration and device state; a
   generic `passed` flag or old happy-path receipt is insufficient.
5. Persist a verified commit receipt before disarming recovery. A late deadline
   or boot action must not undo an already committed policy.

Every gate receipt binds the transaction ID, device fingerprint, exact
configuration hash, baseline slot/floor/generation and receipt content hash.
The root adapter must read and validate the actual receipt before constructing
the typed evidence; accepting arbitrary supplied hashes is not verification.
After a durable commit, remove all four temporary guard dependencies and reload
the manager before stopping/disarming the guard. During rollback, remove write
configuration and enable links before removing guard dependencies. Never leave
unguarded writable configuration in an uncommitted crash window.

Commit and deadline compete to publish one `decision.json` through an exclusive
hard link to a fully written, synced proposal. The winner is immutable and
authoritative even when the secondary phase marker has not been updated. A
revoked winner prevents commit; a committed winner prevents a late deadline
from closing admission. This avoids the check-then-close race between reading
a phase and publishing the commit. Runtime revocation also needs exact owned
socket masks plus stop/cancel, so a delayed preparer cannot reopen admission.
The privileged backend must verify ownership and removal of this fence, not
merely trust a boolean. Arbitrary host PIDs and activation workers must not be
killed for installer recovery.

Runtime recovery first closes admission independently of the controller lock.
It then waits for upload, activation and confirmation workers, apply jobs,
health token and apply receipt to settle naturally. Only then may it remove
the owned configuration/enable links, reload systemd and restart the confirmed
server. It must preserve any newly observed confirmed floor and never restore
an old journal/pointer or delete a signed slot. A timeout keeps admission closed
and recovery available; it is not permission to kill an activation worker.
An already-restored invocation verifies closed state and disarms any remaining
retry timer without another server restart. An already-committed invocation
must not close accepted admission.

Uncommitted boot recovery runs before both write sockets, `sockets.target` and
both servers. It cancels queued write-socket starts non-blockingly, removes
only the owned policy, and reloads systemd. It must not synchronously wait for
or restart a downstream server while that server is ordered after the boot
guard. Post-boot verification/cleanup is separate from early-boot restoration.
The early-boot action retains the admission fence and recovery metadata for
that later verification; it never stops its own running boot-guard service.

`tools/persistent_write_contract.py` records these pure decisions and renders
the proposed units. It is not an installer and performs no device operations.
The companion file adapter records original absence, preserves status files,
installs transaction dependencies before policy files/enable links, validates
all owned targets before removal, and can detach only guard dependencies after
commit. Its filesystem tests run solely in temporary host directories; a
privileged device acceptance has not yet been performed.

The native admission adapter now closes only the two sockets, installs owned
runtime masks, reloads systemd, and repeats closure. Early-boot stops are
non-blocking. A failed first stop still attempts masking but remains a failure;
an unsuccessful release reinstalls the fence. Release requires the exact owned
preparer unit to have no process, control process, job or remaining cgroup, and
requires all owned write-policy paths to be absent. The preparer is rendered
with a bounded, control-group-scoped lifetime; it never owns update workers.
These adapter tests mock systemd and do not prove installed recovery.

Persistent settlement has a separate policy from the fixed 7-to-8 acceptance
script. It durably records both floor and generation increases even while
workers are busy, rejects regressions, and requires two identical idle terminal
observations. Later confirmed slots are preserved, not reset to slot eight.
The candidate privileged coordinator now exposes `prepare`, cgroup-bound
`wire`, `restore`, and `cleanup` commands. It stages root-private pinned code and backups,
arms boot/deadline recovery before policy installation, and leaves dormant
guard definitions and recovery metadata after restoration for explicit
inspection/cleanup. Initial preparation is restricted to the accepted
slot-eight baseline; recovery preserves later monotonic state. Host tests
cover import closure, boot arming/restoration routing, write failure closure,
counter persistence, and snapshot preservation. Its installed idle rehearsal
has now passed; boot and pending-work recovery still need qualification.
Cleanup first requires verified restoration, absent
enable links/masks, idle preparer/recovery units, and no loaded server dependency
on the boot guard. It records a durable cleanup marker before removing owned
unit definitions, then re-verifies closed runtime and archives the root-private
code/backup instead of deleting it. An interrupted unit cleanup can resume;
foreign data is left for inspection. Full gate receipt qualification remains
pending. There is no permanent-retention CLI.

The host driver now has separate `preflight` and `rehearse` actions. Both use
pinned package/code inputs and a reserved private receipt before SSH staging.
Preflight reads the closed seq8 state only. Rehearsal submits preparation once,
restores admission to closed, cleans up, and independently re-reads the seq8
state on the same boot. It sends no application archive and invokes no upload,
activation, reboot or target-power operation. Ambiguous preparation triggers
recovery, not another preparation; ambiguous cleanup requires inspection of
the reserved root/archive instead of replay. Its installed idle rehearsal now
passed; see `../baselines/persistent-write-rehearsal.md`. Pending-work, boot and
browser acceptance remain separate; the rehearsal restored admission to closed.

A separate `browser` action passed its bounded installed stage-only scenario
on September 12 after system maintenance and Nginx executable refresh; see
`../baselines/persistent-write-rehearsal.md`.
It uses two pages of one authenticated session: one keeps the upload mounted,
the other invokes the real Account/Logout control. A single-tab Account switch
would first unmount/cancel the upload and could not isolate session revocation.
The scenario checks an invalid archive, partial-progress cancellation, retry of
the exact already-present seq8 archive, and termination of the original upload
after logout. It accepts either an observed HTTP 401 or a request-specific
`ERR_ABORTED` after Logout was requested, while the browser remains open.
Both pages must reach Login and protected status must return HTTP 401; a
successful upload response, a pre-logout abort, or browser teardown cannot
pass. The receipt distinguishes HTTP rejection from client cancellation, and
does not measure native worker stop latency. It forbids activation and restores/cleans the native policy after
the browser phase. Its paired SSH/GUI credentials are supplied only on stdin.

If plan/decision I/O or runtime recovery fails, the controller attempts an
emergency owned fence under its common lock and still reports failure. It
preserves damaged metadata and does not manufacture a restoration receipt.
A readable committed winner is never closed. Any future native retention
implementation must hold that same lock continuously from runtime/evidence
verification through decision publication and detachment; a check before
taking the lock would race emergency closure.

Offline systemd verification accepts the transaction dependency graph and
rejects an injected `basic.target`/`sysinit.target` cycle. This proves static
ordering only, not actual boot recovery. A local real-component browser test
now observes intermediate progress (6% in the verified run), cancels a partial
upload, retries it, and invokes the actual Logout component while verification
is pending. It proves unmount-driven XHR cancellation and the synthetic session
transition; it does not establish installed-worker logout acceptance.

## Acceptance before retention

`tools/persistent_write_pending.py` now defines the host-tested ordering for the
next held-upload restoration gate: reserve restoration before submission,
observe the owned fence while the same worker/spool is live, release only the
client heartbeat, require natural exit code 50 rather than a signal, then verify
native restoration and preserved terminal state. Errors close only the fixture
client and never submit a second restore. `tools/installed_pending_write_gate.py`
now provides a host-tested native adapter using the existing idle/held IPC probe,
exact worker/spool inspection and real restore command. Integration into the
host rehearsal driver and the installed run remain pending; these tests are
not installed pending-work evidence.

Existing evidence and remaining checks are deliberately separate:

| Gate                     | Current evidence                                                                    | Work still required for persistent policy                                |
| ------------------------ | ----------------------------------------------------------------------------------- | ------------------------------------------------------------------------ |
| Host safety              | Server/client authorization, length, quota and cancellation tests                   | Persistent controller ownership, phase and interruption tests            |
| Native upload/activation | Installed native worker gates and confirmed slot 8                                  | Preserve bindings while configuring permanent admission                  |
| GUI happy path           | Actual upload, reviewed activation, confirmation and restored desktop/mobile status | Verify the prospective persistent configuration                          |
| Failure and retry        | Installed invalid archive rejection and already-present seq8 retry                  | Bind qualified receipt to prospective retained transaction               |
| Cancel and progress      | Installed cancellation at 1% and successful retry                                   | Pending-worker restoration remains a separate gate                       |
| Logout                   | Two-page installed logout, original request abort, both Login screens, status 401   | Native worker stop latency and retention-bound evidence                  |
| Boot recovery            | Accepted persistent status recovery; temporary write recovery                       | Uncommitted persistent-write boot restore and committed-policy survival  |
| Disable/drain            | Temporary seq8 controller settled a real activation before restore                  | Owned permanent policy disable with pending work and failure injection   |

Negative upload tests must not activate another slot or infer permission to
publish sequence 9. Preserve all eight retained slots; quota-boundary retry is
idempotent, and uploads never perform automatic garbage collection. Permanent
retention requires all exact gate receipts plus explicit approval to retain the
policy. Preparing this controller does not itself provide that approval.
