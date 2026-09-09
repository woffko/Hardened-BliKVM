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

The post-gate browser preservation check passed for seq7: desktop/mobile
read-status, video decoding and logout revocation, with no control request or
WebSocket. Private receipt: `installed-upload-seq7-postgate-browser-20260909.json`.
These results do not establish new-slot publication, HTTP/GUI upload,
activation, or forced expiry of the packaged RuntimeMax limit.

## Next publication candidate

Sequence eight deliberately uses the same verified payload and version
`0.1.0-dev6` from source `7c813a7844b75774fea451516fc2b82d6a579d13`.
Only the signed application sequence changes; this is a controlled reinstall
candidate for exercising the actual publication and later GUI activation path,
not a claim of new application code. Two independently signed archives match:

- Archive SHA-256 `d2acf270fa2aa9cb924c362066ece50f4b0fbd4c4c8b2975648dab5ac0b4f849`.
- Archive size 8,724,480 bytes; payload 8,683,648 bytes in 27 files.
- Manifest SHA-256 `375f9800a147b8f273073349af1613e3fb0cf9f7b97af914d7a75e9446ea694b`.
- Expected slot `app-00000000000000000008-375f9800a147b8f273073349af1613e3fb0cf9f7b97af914d7a75e9446ea694b`.

The local Rust supervisor passed the signed archive verification, idempotence
and negative-state-preservation checks without executing a payload or
contacting the device. Private receipt:
`application-write-optin-seq8-host-verification.json`.
Native publication passed on 2026-09-09; activation has not occurred.

The explicit `seq8-publish` profile now prepends a `published` receipt check
to the original nine vectors. It requires seven existing slots and an absent
seq8 target before opening admission. After publication it permits only the
new candidate tree: all prior entries, authoritative status and serving PID
must match. It verifies the complete new tree, owner/group, immutable modes,
signed proof bytes and payload against the bound archive, then pins that tree
for subsequent comparisons. The default remains `seq7-retry`.

The durable plan records an attempted publication before sending the archive.
On a failed or lost reply, restoration stops admission and workers, inspects
the actual state, and retains a valid published candidate without activation.
An invalid or unexplained delta fails closed; it never rolls back the journal
or deletes a candidate. Manual `restore` cleans the owned guard only after
verification succeeds. This is a same-boot gate, not post-reboot acceptance:
a changed serving PID/runtime or lost temporary archive requires a separate
read-only recovery assessment, not skipping verification from a saved receipt.

The device passed all ten `seq8-publish` vectors, including a new `published`
receipt followed by verified idempotent retries at eight retained slots.
The complete seq8 tree matched its bound archive; all prior state and the
serving seq7 PID remained unchanged. Authorized body probes again observed
the dedicated identity, empty capabilities, sandbox and anonymous spool;
all worker exits were natural. The upload socket was stopped, the new empty
spool and owned guard/controller files removed. Private receipt:
`installed-upload-seq8-publication-20260909.json`.
Post-publication browser preservation and HTTP/GUI activation remain pending.
