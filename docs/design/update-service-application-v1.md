# Event-driven application of confirmed service state

The original watchdog unit scheduled both apply services on every successful
tick. Their unconditional restart commands would restart the serving GUI every
two seconds even when the manager reported clean state. The timer has remained
disabled on the designated device while this source correction is developed.

Updater dev3 changes that boundary:

- only explicit activation dispatches candidate application;
- watchdog and health confirmation may dispatch confirmed application;
- confirmed application requires both absent health-token and a pending
  `confirmed-apply-request.json` file; and
- clean watchdog passes with no request do not create one.

The private runtime request is closed JSON containing schema version one,
journal generation and the confirmed slot ID. Its maximum size is 512 bytes;
there is no path, command, unit, URL or token field. The request uses the same
owned-runtime checks as the health store and is written atomically and
synchronized, with mode-0600, single-link, no-follow file access. It is
published after successful confirmation or recovery and before clearing stale
health evidence, so an interrupted cleanup can reconstruct the required action.

PID 1 snapshots this public request with `LoadCredential`. The apply service
runs as the updater identity, while its single fixed `+`-prefixed systemctl
command restarts only `hardened-blikvm-server-current.service`. After successful
application, the ordinary unprivileged supervisor reads the bounded request
from its verified read-only credential mount using `ack-confirmed-apply-credential`.
The privileged systemctl action has null stdin; it does not need to enter the
credential namespace. A separate `ack-confirmed-apply` command retains the
bounded stdin interface for local tests. Under the manager lock it removes the
runtime request only if generation, active/confirmed slot, no-pending status and
the actual queued request still match. A stale acknowledgement preserves newer
work. If restart fails, ExecStartPost does not acknowledge the request.

On systemd 249, automatic credential mounting is confined to the first
`ExecStart`; the observed post-command namespace masks `/run/credentials` with
mode 000. `ExecStartPre` cannot initialize it either. The apply unit therefore
explicitly bind-mounts only `/run/credentials/%n` read-only at
`/run/hardened-blikvm-apply-credentials/%n` for post commands. Binding beneath
the masked original parent is insufficient because that parent stays mode 000.
The loader accepts this additional fixed root only with the same root-owned
ancestors, service-owned mode-0400 file, single-link and read-only mount checks.
The first main command still snapshots the request before restarting the
server; ACK must use that snapshot, not reload the possibly newer live request.
Native acceptance must verify this narrow bind mount without weakening the
credential loader's ownership, ancestor or read-only checks.

References: systemd v249 [credential namespace gating](https://github.com/systemd/systemd/blob/v249/src/core/execute.c#L2952-L2960)
and [main-command credential initialization](https://github.com/systemd/systemd/blob/v249/src/core/service.c#L1941-L1945).

The privileged command receives no request-selected argument and no general
shell. The acknowledgement can write only the update runtime directory and
does not write journal, slots or public trust. The confirmation health token
remains a separate one-use secret and is not included in this request.

Host tests cover repeated clean ticks without restart requests, health
confirmation, rollback, preservation until acknowledgement, stale ACK versus
newer generation, unsafe symlink/oversized/unknown request rejection, and the
CLI stdin flow. Real systemd delivery, repeated-tick PID stability, failed
application retry and dev3 package installation still require device acceptance.

The native acceptance runner is `tools/verify_apply_events.py`. It renders the
actual apply/watchdog units into an isolated `/var/lib` fixture (the device
does not permit executing the test binary from `/run`), retaining their
credential, privilege and sandbox settings while redirecting only paths and
the literal restarted service to a temporary sleep process. Three ephemeral
signed fixture bundles exercise confirmation and missing-health recovery.
Repeated real timer invocations must preserve the process PID between events;
an injected ExecStartPre failure must retain the request until a later retry
successfully applies and acknowledges it. A separate deadline cleans abandoned
fixture services, and the SSH wrapper verifies the production serving runtime
before and after the test.

The target parser rejected `%d` in ExecStart/ExecStartPost and StandardInput.
The packaged templates now use `/run/credentials/%n/...`, preserving the
actual credential mount while using the supported unit-name specifier. Earlier
transient credential tests supplied the expanded path directly and therefore
did not prove compatibility of `%d` in installed unit files.
