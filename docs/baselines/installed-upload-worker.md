# Installed upload worker rehearsal

This bounded gate passed on the device on 2026-09-09. Its first scope is the existing confirmed
sequence-seven archive only. It does not publish a new slot, activate an
application, enable HTTP writes, restart the serving application, change the
updater package, or operate target power.

The controller validates exact dev9 package files, the seq7/floor7/generation37
state, persistent status drop-ins, absent write environment settings and inert
write transports before starting only the upload socket. A 600-second timer
independently stops that socket and its workers without acquiring the main
controller's lock. The normal controller has a shorter 450-second budget.
Protected snapshots are compared after each vector and after restoration;
only owned probe files and a newly created, verified empty spool directory are
removed. Existing unexplained worker instances cause a stop, not silent cleanup.

Vectors: exact idempotent retry, a byte-flipped signed payload, short and
surplus bodies, malformed metadata, unauthorized root peer, incomplete-body
idle timeout, lost reply, and exact retry. The last two do not establish new
publication: the slot already exists. HTTP/GUI and a new-slot publication gate
remain required separately.

For authorized body probes, a bounded prefix/heartbeat handshake permits live
inspection of the dedicated UID/GID, empty effective capabilities,
NoNewPrivileges, executable hash, loaded sandbox and anonymous owner-only
zero-link spool descriptor. A private `RefUnit` connection retains the unit's
record while its process terminates naturally. Terminal exit kind/code and
invocation are checked before resetting that owned instance. Early rejection
failures are retained by the audited `CollectMode=inactive`; missing evidence
fails the gate. No sleep is used as a delivery or worker-liveness guarantee.

The API and garbage-collection basis are the upstream systemd v249
[unit contract](https://github.com/systemd/systemd/blob/v249/man/systemd.unit.xml)
and [unit reference implementation](https://github.com/systemd/systemd/blob/v249/src/core/dbus-unit.c).
The test intentionally observes the upload idle timeout. It does not force
expiry of the separate packaged 180-second RuntimeMax limit.

The installed native command passed all nine vectors. Authorized body probes
observed the dedicated identity, empty effective capabilities, unchanged
packaged executable/sandbox, and the anonymous spool. Natural exits were 0 for
retry and the delivered tamper rejection, 50 for truncated/surplus/malformed
and idle-timeout streams, 96 for the unauthorized root peer, and 50 for the
deliberately lost reply; the following exact retry succeeded. No forced worker
stop was used as terminal proof. Protected state and the serving PID were
unchanged; the socket, new empty spool, guard and controller staging were
removed/restored as designed. Private receipt:
`installed-upload-seq7-rehearsal-20260909.json`.

The post-gate browser preservation check remains pending. These results do not
establish new-slot publication, HTTP/GUI upload, activation, or forced expiry
of the packaged RuntimeMax limit.
