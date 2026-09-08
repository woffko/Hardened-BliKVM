# Automatic restore timer completion

On 2026-09-08, device job `54a37cb604194eeea376d729a82109b4` passed the
two-SSH-command automatic completion probe. The host gate had passed in
`fb9cba48087e4543a9e4f69ad1f214d2` before the device run.

The first SSH command verified the retained application and serving identities,
created the fixed 120-second transient timer, verified its unrun restoration
command and monotonic deadline, and exited successfully without disarming it.
The host persisted that handoff before opening the second SSH command.

The second command held its own bounded `Manager.Subscribe` connection and an
exact-unit JSON monitor. It resolved the systemd bus owner to PID 1, rechecked
the unrun guard and identical deadline, then observed job `18576352` complete
with `done` for the exact restoration service. The observer did not request
that service's start. The completion was accepted within the allowed window
after the timer deadline using the observer's device-monotonic receive clock.
This receive timestamp is not a precise command-execution timestamp.

The final checks confirmed unchanged application status, boot/main process
identities and restart counters, an active idle status socket, and removal of
the temporary timer/service. Both SSH commands exited with status zero and
the receipt reports `guardMayRemain=false`.

Private receipt: `evidence/private/status-timer-automatic-completion.json`.
The host revalidated the handoff, completion schema, event timing and all
twelve source-module hashes against the exact tested source.

This proves automatic execution after the arming SSH command **exited
normally**. The status socket stayed available throughout; it does not prove
restoration from an actual status outage, browser stale/error/recovery
behavior, physical power-loss recovery, or loss of the entire observer host.
