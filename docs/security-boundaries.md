# Security boundaries

## Secret handling

- Test-device routing and credentials belong only in encrypted Project Memory.
- Do not write credentials to Git, command arguments, environment variables,
  logs, evidence bundles, update archives, or delegated prompts.
- Prefer one-time secret stdin handles for reviewed non-interactive device
  operations.
- Never import accounts, SSH material, history, cookies, or tokens from a field
  image into a BLi-KVM release.

## Network process

`bli-kvm-server` is unprivileged and owns HTTP policy only. It must not receive
GPIO, configfs, raw block, arbitrary file, or arbitrary process permissions.
The development server binds to loopback and refuses a public bind. Passwords
are sent only over the TLS/reverse-proxy boundary; BLi-KVM
does not use a fixed browser-side key as reversible password obfuscation.

## Privileged broker

The broker accepts typed operations over a permission-restricted Unix socket.
Its transition backend connects to KVMD only over `/run/kvmd/kvmd.sock` and
receives a dedicated KVMD credential through systemd credentials. The
network-facing server never receives that credential. Native backends use fixed
device paths, strict field validation, bounded timeouts, and an operation
allowlist. No backend may invoke a shell with request-derived content.

KVMD's public Nginx routes use `/api/<route>`, but the local Unix socket uses
the rewritten internal `/<route>` form. The transition client therefore
allowlists only `/info`, `/atx`, `/hid`, `/msd`, and `/streamer`; it also sends
a fixed Host header required by HTTP/1.1. Endpoint-level diagnostic logs expose
only a fixed endpoint enum and coarse error class, never a response body or
credential.

Read-only observations and mutating methods are separate interfaces. HID, ATX,
media, GPIO, or configfs methods stay unavailable until their adapter and
physical gate pass.

Future mutations use the separate bounded control protocol described in
`api/control-v1.md`; they never extend the read-only inventory protocol. Typed
control messages still require a denial-by-default runtime policy, operation
replay protection, arming or leases, cooldown where applicable, and backend
gates before a capability can be advertised.

MJPEG is the first exposed read-only media path. A dedicated video proxy is the
only process with supplementary `kvmd` group access to the existing uStreamer
Unix socket. It has a private network namespace, closed device policy, no
capabilities, a two-stream limit, and accepts only one exact local HTTP route.
The network server is not in `kvmd`; it receives only the proxy's validated
multipart body. Streaming remains bound to the authenticated session, so
logout, password rotation, or expiry stops forwarding further frames. Video
availability does not enable WebSocket, HID, ATX, media, or video-configuration
controls.

Read-only ATX LED observation is independent from ATX mutation eligibility.
The transition policy and fixed client are described in
`api/atx-transition.md`; the public policy constructor is disabled and the
client is not linked into the control broker. A future backend must atomically
persist Pending before the call and its final outcome afterwards, never retry
an indeterminate operation, enforce persisted cooldown, and require explicit
confirmation.

Virtual-media observation is separate from mutation. The opaque catalog,
persisted policy, and fixed transition client are described in
`api/media-transition.md`. No caller-controlled path or URL reaches KVMD, the
public eligibility constructor is disabled, and mutating web controls require
`read_write` rather than merely an available/read-only status.

Application update verification uses the separate Ed25519 trust domain and
deterministic archive contract in `api/application-update-v1.md`. It has no
unsigned fallback and accepts only `bin`, `libexec`, and `share` payloads for
the H616 application slot. Full-system images require a different format and
key; application verification can never authorize a partition or boot change.
The non-networked staging library snapshots an input once, verifies and
extracts only from the resulting unlinked read-only inode, publishes a
read-only slot with no-replace `renameat2`, and revalidates signed proof plus
the entire tree. It does not select an active slot or expose an update API.
Its slot root is process-locked, and automatic crash cleanup is limited to
bounded direct children carrying the exact staging/final receipt and containing
no link, special file, foreign owner, or unexpected top-level path.
Garbage collection is local and explicit. It cannot delete any journal or
pointer reference, and uses a durable marker plus atomic tombstone rename
before bounded recursive removal; ambiguous deletion state is a hard stop.
The separate activation library uses a locked bounded journal, a relative
atomic `current` symlink, a boot-bound monotonic health window, persisted replay
records, and a highest-confirmed sequence floor. Only a future non-networked
supervisor may supply clock and health results. The local supervisor now reads
kernel evidence itself and accepts a one-use token only through a group-gated
Unix socket as exact raw stdin. It has no activation socket, TCP family,
device, capability, shell, or block authority. These units remain uninstalled
and unenabled on H616.

Candidate readiness is not inferred from process existence alone. A separate
reporter validates the exact loopback Rust health response before forwarding a
systemd-delivered one-use token to the Unix-only confirmation socket. Fixed
root apply services may restart only the literal candidate or confirmed unit;
the update identity itself has no general service-manager authority.

## Media processes

Transition uStreamer and Janus processes bind to loopback or Unix sockets and
run under dedicated service identities. Only the authenticated Rust server is
network-facing.

## Filesystem and update boundary

- Read-only inspection never enables write mode.
- A write requires explicit task authorization, preflight, rollback, and a
  separate maintenance transition.
- Application and system update trust are separate.
- A future update worker owns one canonical same-filesystem slot root; the
  network server never receives arbitrary filesystem or rename authority.
- No NanoKVM raw partition path or update assumption is valid for H616.
- Production release requires signature rejection tests, power-loss recovery,
  and recoverable physical media.
