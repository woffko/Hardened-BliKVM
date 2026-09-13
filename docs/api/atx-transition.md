# ATX transition boundary

The ATX transition source is intentionally split into read-only LED evidence,
a persisted policy journal, and a fixed KVMD client. None of these components
is connected to the control broker or network server, and the ATX action
capability remains unavailable.

## Read-only LED state

The authenticated H616 baseline returned an enabled, idle ATX subsystem and a
power LED value through `GET /atx`. Sanitized inventory already carries that
boolean. The capability manifest therefore advertises only `power.ledState` as
`read_only` with runtime evidence; `power.status` remains `unavailable`, so the
GUI exposes no Power or Reset action.

## Persisted policy

An ATX operation contains a non-zero 128-bit operation ID, opaque session
binding, opaque 256-bit confirmation binding, confirmation timestamp, and one
enum action: `power_short`, `power_long`, or `reset`.

The policy persists a private, atomically replaced and fsynced journal before
returning an internal permit. It provides these invariants:

- the public eligibility constructor is disabled; shipped builds can only
  create an ineligible policy, and the reviewed constructor exists behind the
  `atx-reviewed-profile` feature after a recent non-zero session/confirmation
  binding and explicit backend and physical-readiness review;
- a confirmation is valid for at most ten seconds and a backwards clock fails
  closed;
- a pending operation survives restart and is never automatically repeated;
- another operation is blocked until a pending outcome is reconciled;
- the same operation ID with different bindings or action is a conflict;
- an applied or rejected duplicate returns the persisted result without
  calling a backend again, even if capability eligibility later changes;
- PowerShort has a five-second cooldown; PowerLong and Reset impose a global
  thirty-second cooldown;
- unknown, oversized, symlinked, or group/world-writable journals are rejected.

Pending reconciliation is a future administrative workflow and must compare
read-only LED/state evidence before recording an outcome. The transition engine
enforces this: `resolve_pending` takes a read-only power-LED observation and
rejects `Unknown` with `evidence_required`, so an outcome can never be recorded
blind. The observation is recorded evidence only; the operator still classifies
the completion and the engine never infers it. Reconciliation is not an
automatic retry mechanism, and a busy or failed backend response is persisted
as rejected without another call.

## Fixed transition client

The KVMD 3.291 transition client can represent only these internal Unix-socket
POST routes, always with `wait=1`:

- `/atx/click?button=power&wait=1`;
- `/atx/click?button=power_long&wait=1`;
- `/atx/click?button=reset&wait=1`.

There is no generic URI, button, duration, GPIO line, or body. Responses must
be bounded JSON with `ok=true`. The client is tested only against a mock Unix
socket and is not reachable from the denied control broker.

## Remaining gates

Before any ATX action can be wired or advertised:

1. add a reviewed internal eligibility constructor backed by explicit user
   confirmation and the current device/session identity — provided by the
   feature-gated `AtxEligibility::reviewed_for_confirmation`, still unwired;
2. combine policy Pending persistence, the fixed client call, and completion
   persistence in one broker state machine — attached to the control broker
   under the `atx-transition-profile` feature and host-tested with a mock KVMD
   socket, still unwired to a running daemon;
3. add startup reconciliation and visible indeterminate/cooldown feedback —
   the reconciliation entry point now requires non-Unknown read-only LED
   evidence; the GUI feedback is still missing;
4. establish GPIO ownership/polarity and a separate physical test procedure;
5. run pulse timing, busy, duplicate, restart, and recovery tests.

Physical Power and Reset actions are excluded from the current session, so no
live ATX mutation test is performed.
