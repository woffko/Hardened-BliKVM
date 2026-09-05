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
from stdin using `ack-confirmed-apply`. Under the manager lock it removes the
runtime request only if generation, active/confirmed slot, no-pending status and
the actual queued request still match. A stale acknowledgement preserves newer
work. If restart fails, ExecStartPost does not acknowledge the request.

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
