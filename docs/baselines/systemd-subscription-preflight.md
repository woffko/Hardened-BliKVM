# Bounded systemd subscription preflight

On 2026-09-08, read-only device job `a4d1ac1afc6b40cdbb00bd70fc064e51`
passed the live subscription/monitor readiness probe. No timer or service was
created, started or stopped by this probe.

The separate native `Manager.Subscribe` connection confirmed an empty success
reply and remained held in its bounded worker. The exact-match JSON monitor
confirmed registration. Both used the same explicit local system-bus address.
Two fixed D-Bus daemon queries resolved systemd's connection owner and confirmed
that its Unix process ID was 1. No event arrived during the short idle check for
the nonce-specific restore-service filter.

Both child processes exited after the probe. The subscription worker exited
with status zero, after its control pipe was closed. Worker setup and lifetime
have independent process-level bounds; the native method timeout alone is not
treated as a complete setup bound.

The private receipt is `evidence/private/systemd-subscription-preflight.json`.
Its closed schema and all six module source hashes were revalidated against
the tested local source after completion.
The complete host gate passed in job `f2a1ae1d85cd44a1a648ebfc24d8a34a`.

This proves live library compatibility, successful subscription, filtered
monitor registration, owner identity and normal cleanup. It does not prove
delivery of a positive `JobRemoved` event, rejection of runtime noise under
load, automatic timer firing, controller-loss recovery or browser outage
acceptance. Those remain separate gates.
