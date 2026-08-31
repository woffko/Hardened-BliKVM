# Packaging boundary

These files describe the intended service boundary; they are not installed by
the host development build.

`hardened-blikvm-platform-broker.service` has no TCP address family, no device
namespace access, no capabilities, and no writable path outside its runtime
directory. During the KVMD transition it joins the `kvmd` supplementary group
only to connect to `/run/kvmd/kvmd.sock`. Its additional H616 thermal and RTC
inputs are compiled, read-only sysfs paths; no request can select a path,
synchronize a clock, or control a fan, PWM channel, or GPIO line.

`hardened-blikvm-video-proxy.service` is a second, distinct non-root identity.
It joins the `kvmd` supplementary group only to read the existing uStreamer
Unix socket, has no network or device namespace, and accepts only `GET /mjpeg`
on `/run/hardened-blikvm-video/mjpeg.sock`. It rewrites that request to the
fixed uStreamer `GET /stream` route and permits at most two concurrent streams.
The public Rust server is not a member of `kvmd`; it reaches video only through
this narrow proxy socket.

`hardened-blikvm-control-broker.service` is a third isolated identity and is
denied by default. It has no `kvmd` group, credential, network namespace,
device access, capability, or backend path. It accepts only the bounded typed
control protocol and returns `capability_disabled` for HID, ATX, and media.
Installing this binary does not expose a server route or change a capability.
Backend access may be added only after the protocol-specific arming,
idempotency, cooldown, and physical gates documented in `control-v1.md`.

`hardened-blikvm-server.service` runs under a different non-root identity. It
can use only Unix sockets and IPv4 loopback, may bind only TCP port 18080, and
has no device or capability access. Its only persistent writable path is the
systemd-managed mode-0700 state directory. Secure cookies assume an HTTPS
reverse proxy; the Rust process itself remains loopback-only. Video responses
remain session-bound: logout, password change, or session expiry closes the
Rust response body before further MJPEG frames are forwarded.

All five current service identities use the shared primary group
`hardened-blikvm`, so future diagnostic socket group access cannot identify a
producer role. The unlinked peer policy instead requires six distinct non-root
UIDs, exact shared GID and a positive bounded PID, then returns only one semantic
source. The safe adapter implements fixed-name nix lookup and Tokio `peer_cred`
without numeric output. The separate fixed-socket collector, systemd service,
and diagnostics sysusers fragment are host-verified and passed a reversible
H616 gate, but remain explicitly excluded from `build-prototype-bundle.sh` and
have no `[Install]` target. No diagnostic capability is packaged or advertised.
The producer client and bounded reporter are linked only inside the existing
denial-only control-broker binary; they add no separate artifact, unit, socket
activation, ordering dependency, or install target. The client exposes no
configurable path or timeout and never retries ambiguous delivery. The reporter
uses one fixed eight-event ring, non-blocking typed receipts, one ordered
no-retry worker, and abort-on-drop ownership. The broker reports only its fixed
capability-disabled denial after writing the control response and ignores the
receipt. Because the collector remains excluded, this does not package or
advertise a diagnostic export capability. The linked path passed a reversible
H616 exact-event, stalled-response, queue-pressure, shutdown, browser, and
cleanup gate without adding a unit dependency.
The separate sealed-window export-v1 codec remains unlinked and absent from the
bundle. It adds no export socket, service identity, persistence, client, API,
UI, or install target. Its caller-bound exact-platform-broker listener is also
excluded from the prototype binary set. The unshipped collector now links it
and owns a second fixed `export.sock`, but the collector binary/unit/sysusers
fragment remain excluded from `build-prototype-bundle.sh`; no linked client,
install target, ordering dependency, persistence, API, UI, or advertised export
capability exists. The dual-socket collector passed a reversible native H616
gate but remains deliberately outside the bundle.
The separate Rust export client is also absent from every shipped manifest and
binary. It accepts no configurable path or timeout, creates no files, opens one
fresh fixed-socket connection per operation, and never retries a read or
acknowledgement automatically. Its accepted Platform Broker integration now
exists only behind default-off feature
`diagnostic-export`. `build-prototype-bundle.sh` enables no such feature: its
broker binary contains no export-socket literal and schema v1 remains
inventory-only. The separately built schema-v2 feature binary has no install
unit and passed a transient H616 gate, but is not copied into the bundle. It can
read only for the exact Server peer and has no acknowledgement, persistence,
server/HTTP/UI call site, retry, or advertised capability.
The pure encrypted diagnostic journal is also a workspace-only library. It is
not a dependency of the default or feature broker, no key or store path exists,
and neither it nor its RustCrypto artifacts are copied into the prototype. A
future fixed-root store must pass its own ownership/ACL/no-follow/atomic-sync
and credential gates before this changes.

`nginx/hardened-blikvm.conf` is the production TLS boundary. It permits TLS
1.2 and 1.3, proxies only to the fixed loopback server port, preserves the
external Host value required by origin/CSRF checks, and carries WebSocket
upgrade headers. Certificate and key files are deployment inputs and never
belong in the source bundle.

`tools/stage_device_tls_browser.py` renders this boundary with alternate
loopback ports and an ephemeral certificate, exposes it only through an SSH
local forward, runs the browser flow, and removes all generated TLS material.
It is a development gate, not certificate provisioning.

`bli-kvm-update-supervisor` is a distinct non-networked application-update
identity. Its watchdog service can write only the systemd-managed slots, state,
and private runtime directories. It reads the fixed root-owned
`/etc/hardened-blikvm/update-supervisor.json`, which contains an Ed25519 public
key and non-secret paths but no signing key or credential. The source bundle
does not ship this file: production key selection and provisioning require a
separate reviewed key ceremony.

The confirmation socket is Unix-only, mode 0620, and group-gated. It accepts
exactly one 32-byte raw health token on stdin for a per-connection service. The
token itself remains mode 0400 in the update runtime directory so PID 1 can
later deliver it as a systemd credential to the candidate process. No token is
accepted through argv, environment, JSON, HTTP, or the browser. The watchdog
timer runs every two seconds, but its condition prevents operation before a
valid configuration and initialized journal exist. These units are templates
only and are not enabled by the prototype bundle.

`ProtectSystem=strict` is paired with explicit path allowlists. Confirmation
can write only the update state and runtime directories. Activation and the
watchdog additionally need the separately provisioned slots root for the
atomic `current` pointer and marker-bound cleanup. Unix ownership and mode
checks remain mandatory; `ReadWritePaths` does not grant permissions by
itself. No update service receives a broad `/var/lib`, `/run`, or root-filesystem
write exception.

The local `stage` command is intentionally separate from `activate`: it accepts
an absolute archive path, applies the configured public trust root, and can
only publish an immutable slot. It cannot change the active pointer. No upload
or activation API is exposed by these service templates.

The local `gc N` command is also explicit and unavailable over the health
socket. It retains the newest `N` unreferenced signed candidates, never deletes
active/confirmed/previous/pending/current references, and completes any
durable deletion tombstone before considering new collection.

Confirmed and candidate server templates both execute the signed `current`
slot. The candidate requires `application-health-token` through systemd
credentials and runs the dedicated Rust health reporter only after startup.
The reporter validates the exact loopback health response before submitting
the token to the Unix socket. Fixed root apply services restart only the
literal candidate or confirmed unit after activate/watchdog success; the
unprivileged update service does not receive general `systemctl` authority.
The slots root is provisioned by tmpfiles as mode 0750, owned by the update
identity and read-only to the shared runtime group; group write is forbidden.
Tmpfiles also creates the confirmation socket parent as mode 0750 owned by the
update identity and shared runtime group, so the server identity can traverse
to the mode-0620 socket without making the directory world-accessible.

The two files below must be provisioned locally as root-owned mode 0400 files
and must never be included in a package, image source tree, command line,
environment variable, log, or support bundle:

- `/etc/hardened-blikvm/kvmd-user`
- `/etc/hardened-blikvm/kvmd-password`

They should identify a dedicated KVMD account, not an administrator or SSH
account. The service loads them with systemd `LoadCredential`; the broker
accepts no credential arguments and has no compiled default.

Creating that KVMD account and installing these files are separate on-device
mutations. They require explicit authorization, `rw`, backup/rollback, and a
reboot/recovery gate. `tools/stage_device_systemd_gate.py` exercises only a
temporary test account and runtime units with a watchdog, then requires a
byte-identical htpasswd restore and removes all staging. It is not a production
installer or permanent account-provisioning tool.
