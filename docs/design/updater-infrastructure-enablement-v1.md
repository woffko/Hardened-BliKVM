# Idle updater infrastructure enablement

Prerequisite: the exact retained dev4 package and generation/floor-one signed
slot serving checkpoint. The only new authority enabled here is the local
watchdog timer and one-use Unix confirmation socket. No application activation,
API endpoint, target control or signing operation belongs to this gate.

Before changes, verify the installed package, immutable trust/configuration,
initial journal and current slot, serving process identities, TLS/accounts,
disabled update units, absence of pending health/apply requests, and absent
active tmpfiles configuration. Save persistent state and runtime fingerprints.

Arm a synchronized persistent rollback guard before installing the package's
exact tmpfiles example at `/etc/tmpfiles.d/hardened-blikvm-update.conf`. The
example provisions only the existing slot root and
`/run/hardened-blikvm-update-confirm` with updater ownership/shared group 0750.
Apply that exact file with `systemd-tmpfiles --create` before enabling the
socket; `DirectoryMode` alone does not express the required directory group.

Enable/start the confirmation socket and watchdog timer. Verify the directory
and socket UID/GID/modes, repeated successful real watchdog invocations,
unchanged serving PIDs and state/floor, absent pending token/apply request,
healthy HTTPS and no active candidate/activation/confirmation instances.
Status queries must not use the earlier bootstrap helper's ephemeral runtime
directory cleanup while the watchdog owns that directory.

First rehearse disabling the two units, stopping the idle watchdog, removing
only the exact local tmpfiles rule and empty owned runtime directories, then
verify original state and remove the guard. Repeat and retain only after that
rehearsal succeeds. A committed marker prevents late recovery from disabling
retained infrastructure. Never delete nonempty runtime directories or restore
an older journal as a shortcut. Early-boot recovery may leave volatile runtime
cleanup for a later independent inspection instead of waiting on boot ordering.

This gate does not prove application-update browser continuity or replay and
reboot recovery of a real production-key update. Those remain the next steps.
