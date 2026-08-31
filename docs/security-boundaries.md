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

Read-only H616 temperature observation is restricted to the compiled
`/sys/class/thermal/thermal_zone0/temp` path. The broker uses no-follow open
semantics, rejects unsafe metadata and malformed or oversized values, bounds
the accepted result to -40 through 150 degrees Celsius, and exposes no source
path or raw value. This observation does not authorize fan, PWM, GPIO, or any
other board mutation.

External RTC observation uses only fixed, read-only kernel class attributes and
requires exact `rtc-pcf8563 0-0051` identity. It never opens `/dev/rtc1` or raw
I2C, never registers a device, and never sets either clock. Epoch and
RTC-minus-system offset are paired, range-bounded public facts; a missing,
rebound, malformed, or implausible source fails to null observations.

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

System-image verification uses `hardened-blikvm-system-v1`, `sys-` key IDs, and
the contract in `api/system-image-v1.md`. A root-owned local policy must match
the exact layout ID, inactive-slot capacities, and installed recovery and
boot-control sequences. The archive can contain only a rootfs image and an
optional separate boot filesystem image. Unknown fields and any disk, device,
partition, offset, bootloader, state, MSD, installer, hook, URL, or arbitrary
path are structurally rejected. The verifier has no block-device or boot
authority; no system writer or accepted H616 layout exists yet.

The future system journal and boot selector are separate persistent domains as
defined in `api/system-boot-control-v1.md`. A selector attempt counter must be
decremented durably before candidate boot. Mismatched operation IDs, slots,
manifest digests, generations, or increasing attempt counts are recovery hard
stops. A software journal floor alone is not claimed as physical rollback
resistance.

Recovery layout parsing is isolated in `bli-kvm-system-layout` and
`api/system-layout-v1.md`. It accepts only fixed semantic roles and numeric
sector geometry, requires equal root A/B and redundant state policy, and
rejects overlaps, unaligned or out-of-bounds ranges, unsafe raw sunxi offsets,
and the observed live single-root geometry. A structurally valid profile is
still untrusted until its exact hash and recovery artifact are accepted.

The pure boot-control model uses separate checksum domains for journal and
selector copies. It tolerates one invalid/torn copy only as degraded state and
hard-stops same-generation divergence, gaps larger than one generation,
mismatched operation/slot/digest evidence, selector generation regression, or
an increasing attempt count. Its SHA-256 checksum is corruption detection, not
authentication or physical rollback resistance.

Application/system compatibility is checked against the previous confirmed
system for the entire pending system transaction. A candidate-only application
requirement cannot be confirmed early and thereby make automatic system
rollback unusable.

The boot-control storage coordinator writes the invalid/older copy first and
requires sync plus exact readback before touching the second copy. Persist calls
carry an expected generation and reject stale state before writing. The future
adapter must provide atomic per-copy replacement and hold an exclusive recovery
lock; the trait contains no path or concrete storage implementation.

The system-action journal is a separate fixed binary domain. It binds journal
generation to validated policy generation, rejects noncanonical fields,
split-brain copies, gaps and a caller-supplied rollback floor, and plans the
non-selected copy first with an expected-generation check. Its SHA-256 is
corruption detection, not writer authentication or physical rollback
resistance. Its unlinked filesystem adapter accepts only the compile-time
root-owned mode-0700 directory and fixed A/B names, rejects POSIX ACLs and
unsafe type/device/owner/group/mode/link/size metadata, locks the directory FD,
and uses relative no-follow/nonblocking opens. It syncs and verifies a fixed
temporary file, atomically renames, syncs the directory, and reopens for exact
verification, completing the non-selected copy before the selected copy. The
root is not provisioned or packaged; an independently justified generation
floor, native filesystem/power-loss tests, confined runtime and device
acceptance remain mandatory before system actions can be enabled.

The complete store harness passed natively as a static PIE in a unique
marker-protected H616 `/tmp` stage and was removed afterwards. The run kept the
production root absent, rootfs read-write, and existing KVMD services active.
This verifies temporary-filesystem syscall behavior and cleanup only; it does
not relax the external-floor, production-filesystem, systemd, power-loss,
runtime-authorization, actuator, or post-boot boundaries.

The unshipped store-maintenance binary accepts only empty initialization and
empty-state inspection. Its static root-only units have no install section,
network namespace, device or capability authority, and expose one exact
writable path. No repair or floor override exists. Host systemd/tmpfiles checks
and AArch64 reproducibility do not authorize production-root creation; that
requires a separate reversible H616 gate with original-absence restoration.

That reversible gate has now passed for empty generation one. A transient
ten-minute watchdog preceded persistent writes; exact source binary/units and
tmpfiles initialized and inspected two equal copies, then strict rollback and a
separate final check restored the original absence of production/runtime paths
while KVMD stayed active. This does not authorize nonempty state, a rollback
floor, repair, power loss, reboot persistence or any system action.

The H616 trusted-floor audit found no bound TPM, TEE/OP-TEE, or exposed eMMC
RPMB interface. Generic TPM framework configuration is not a counter, and a
floor stored with the redundant journal is rollbackable with it. System actions
must remain disabled unless different verified hardware/firmware supplies an
independent monotonic primitive plus reviewed irreversible provisioning, key
custody, recovery and physical fault gates.

Diagnostic export does not accept raw log lines for post-hoc redaction. Its
unlinked pure model contains only fixed semantic enums, monotonic relative
offsets, derived severity, and bounded deterministic ASCII output. It has no
message, path, identity, filesystem, process, clock, network, or device surface.
The separate unlinked collector policy accepts only the five current process
roles, exact lifecycle epoch/sequence transitions, and a role-specific event
matrix. Source epoch and sequence never enter output. An eventual non-networked
collector process must keep the typed mapping before the authenticated server
boundary; unknown events and all raw content fail closed.
The unlinked runtime now composes the already authenticated role, exact frame
codec, window policy, a caller-bound Unix listener, OS-random instance binding,
real monotonic time, fixed I/O limits, and graceful task cleanup. It creates no
socket path or account itself. An unshipped process now supplies one fixed
socket plus hardened service/account ownership; its rollback-gated H616 test
passed and restored the exact prior account/runtime state. No diagnostic
capability is currently advertised.

The unlinked producer protocol contains one canonical eight-byte typed event
and a mandatory four-byte accepted/rejected response. It has no source,
lifecycle, sequence, time, identity, text, or arbitrary payload field. Current
service UIDs are distinct while their runtime group is shared, so future role
binding must use kernel `SO_PEERCRED` against five exact resolved non-root UIDs;
group access alone is insufficient. A producer waits for acceptance before its
next frame, preventing hard backpressure from looking like success.

The unlinked window policy keeps one live bundle and at most one sealed bundle
awaiting exact acknowledgement. `record_count + active_sources <= 32` reserves
one stop record per peer. A second required rotation returns backpressure. If
EOF cannot be recorded because an overdue window is blocked, the role is still
released and bounded omission metadata marks the bundle incomplete. Window and
session state are ephemeral and contain no durable audit claim.

The pure peer policy validates five producer UIDs and one collector UID as
nonzero and pairwise distinct, plus one nonzero shared GID. Kernel credentials
need a positive bounded PID and exact producer UID/GID. Root, collector,
unknown, missing, duplicate, and wrong-group peers fail closed. Only the
semantic source leaves the mapper; numeric credentials are discarded. The safe
adapter uses a connection-bound `peer_cred` API and never accepts source from
the producer frame.

The unlinked safe adapter resolves only six compile-time account names and one
group through nix, validates the current effective collector UID, and maps a
Tokio Unix stream through `peer_cred`. Lookup and peer failures are coarse typed
errors with no numeric values. PID/UID/GID containers do not implement `Debug`,
and only an opaque semantic-source token is returned. The token is consumed by
an unlinked runtime core; its opaque connection handle is instance-bound and
cannot be cloned, debug-printed, or consumed by a foreign core. Malformed frames
receive no response and terminate through a recorded or visibly incomplete EOF
path. Policy, backpressure, and unavailable responses remain canonical and do
not masquerade as acceptance. The transient listener samples `Instant` only
while holding the core mutex, permits only five fixed role sessions, bounds a
started partial frame and response write to five seconds, and preserves the
mandatory response for a frame committed before shutdown. Production code does
not bind, chmod, unlink, configure, or log a socket. The separate collector
binds only its compile-time path, verifies directory/socket ownership and mode,
and removes only the same inode. It is linked only into an unshipped binary and
unit; no product package, authenticated export API, or UI exists.

Recovery images use a separate `recovery-` trust domain and the complete-media
contract in `api/recovery-image-v1.md`. The truncated field image, live state,
default credentials, unresolved binaries, `NOASSERTION` components, and
automatic first-disk targeting are hard failures.

Recovery build evidence is checked by `bli-kvm-recovery-inputs`: required
component identities, source/artifact/license-text hashes, pinned revisions,
corresponding source, reviewed `cleared` status, and exact SPDX 3.0.1 offline
resource bindings. It does not make a legal decision or parse the full SPDX
license grammar; the pinned offline validator remains mandatory.

## Media processes

Transition uStreamer and Janus processes bind to loopback or Unix sockets and
run under dedicated service identities. Only the authenticated Rust server is
network-facing.

## Filesystem and update boundary

- Read-only inspection never enables write mode.
- A write requires explicit task authorization, preflight, rollback, and a
  separate maintenance transition.
- Application and system update trust are separate.
- A valid system manifest is evidence only and never selects a device or slot.
- A future update worker owns one canonical same-filesystem slot root; the
  network server never receives arbitrary filesystem or rename authority.
- No NanoKVM raw partition path or update assumption is valid for H616.
- Production release requires signature rejection tests, power-loss recovery,
  and recoverable physical media.
