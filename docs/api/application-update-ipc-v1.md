# Application update IPC v1

The HTTP process does not read updater-owned journals, publish slots, read
health credentials or invoke privileged service management. Separate Unix
socket endpoints expose separate protocols for status, signed-upload staging
and generation-fenced activation. This document describes status; unknown
request variants must fail closed. See `application-update-v1.md` for upload
and `application-activation-v1.md` for opt-in activation source. Installed
write-path acceptance and writable GUI capability remain required parts of
this same integration, not omitted product requirements.

Messages are closed UTF-8 JSON framed by a four-byte big-endian payload length.
Frames are nonempty and at most 16 KiB. Every envelope carries schema version
one and a nonzero request ID echoed in the response. Each connection handles
one bounded request/response and closes. There is no command, path, URL, key,
health token or arbitrary systemd unit field in the status request.

The status worker runs as the updater identity and validates the connecting
network-server UID through kernel peer credentials before returning data.
Its trusted public configuration is delivered through systemd credentials.
It holds the manager's state lock while reading state and verifying referenced
slot metadata. It must have no apply-service success hook and no filesystem
write allowlist. Status reads do not acknowledge events or create/remove the
watchdog runtime directory.

The response distinguishes journal-selected and confirmed slots; selected does
not claim that a particular HTTP process has already switched during a pending
transaction. Versions/sequences come from verified manifests. Pending state
and the last rollback reason are coarse typed fields; local paths and secrets
are never returned. The API route is authenticated. A separate configuration
gate controls advertisement of read-only status support, and writable update
capability requires later upload/activation acceptance.

System-image updates and raw partition controls are outside this application
IPC. The Bli GUI must not call inherited NanoKVM system-update/firewall routes
merely because an application-status endpoint becomes available.

The initial socket template uses a public-traversable directory containing
only its group-write-gated socket; it contains no secret file. Kernel UID
verification narrows access further than the shared socket group. The worker
uses socket stdin/stdout, a five-second start deadline, four maximum socket
connections, bounded read/write timeouts and no success/failure dispatch hook.
Its status path does not instantiate the health store. These templates remain
outside the retained updater package until native read-only acceptance and
network integration are complete.

The status worker also has no dependency that starts recovery: even an
unauthorized local connection must not initiate journal repair. Mandatory
server startup verification remains a separate dependency of the application
server, not a side effect of status polling.

The protected HTTP route is `GET /api/application-update/status`. It is enabled
through the explicit `BLI_KVM_UPDATE_STATUS_SOCKET` server setting; without that
setting the capability remains unavailable and authenticated requests return
503. HTTP status data carries schema version one and decimal strings for all
u64 counters, preserving values above JavaScript's safe-integer range. The Bli
settings pane validates exact fields and slot/counter relationships, shows
stale/unavailable data explicitly, and polls only while visible. Writable
controls remain capability-gated; inherited system-update/firewall routes
are never invoked. Installed upload/activation acceptance remains required.
