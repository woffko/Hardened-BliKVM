# BLi-KVM target architecture

## Decision

BLi-KVM reuses the Hardened NanoKVM product shape and security model, not its
SG2002 hardware assumptions. The target consists of a platform-neutral Rust
core, a React/TypeScript frontend, and explicit platform adapters.

The current BliKVM Node, Vue, C, shell, and PiKVM implementations are
compatibility references. They are not the production trust boundary.

## Process boundary

```text
Browser
  |
  | HTTPS and authenticated WebSocket
  v
bli-kvm-server
  - unprivileged user
  - auth, API, policy, static web, update verification
  - no direct GPIO, configfs, block-device, or shell access
  |
  | permission-restricted Unix socket with fixed request types
  v
bli-platform-broker
- minimal privileged process
- exact device allowlist
- transition: authenticated allowlisted KVMD Unix-socket client
- read-only platform facts: fixed, bounded H616 thermal and external-RTC sysfs
  observations
- native: HID, USB configfs, ATX, board I/O, virtual-media LUN actions
  - credentials supplied by systemd, never by argv or the network process

Separate service users
  - video capture and Janus during transition
  - future non-networked update worker owning only its canonical slot root
  - future fixed-source typed diagnostic collector with no raw-log API
```

The transition broker is logically privileged even when it needs no root UID:
its KVMD credential can perform hardware actions. Read-only inventory and
mutating methods use separate traits and policy checks. A runtime observation
never enables a mutating method by itself.

The H616 thermal adapter is a read-only platform fact rather than a generic
sysfs facility. It opens one compiled path with no-follow semantics, validates
source metadata and a bounded integer, and returns only millidegrees Celsius or
an unavailable observation. No request field can select a path, fan, PWM, or
GPIO target.

The external-RTC adapter similarly accepts no path or clock operation. It
requires the fixed kernel-bound PCF8563 identity before reading one bounded
sysfs epoch and derives only RTC-minus-system offset. The root-only RTC
character node, raw I2C, clock setting, alarm, and synchronization directions
remain outside the broker boundary.

Fan control is currently a host-only policy library, not a broker adapter. It
can propose a bounded duty from sanitized samples and monotonic time, but it is
not linked into any runtime process. Startup, sensor fault, invalid input,
timeout, and critical heat all resolve to maximum-cooling policy; no result can
reach GPIO/PWM until the separate physical driver boundary is accepted.

An unlinked mock-backed supervisor sits above that policy. It requires a
confirmed maximum-cooling startup command, suppresses duplicate commands,
escalates actuator errors to a maximum-cooling attempt, latches after failures,
and re-confirms maximum cooling for controlled shutdown. No concrete actuator
exists, and this does not claim safety after sudden process/kernel/power loss.

Diagnostic export is currently another unlinked pure boundary. It accepts only
fixed semantic enums and bounded relative offsets, emits deterministic bounded
ASCII, and has no message, path, identity, filesystem, process, clock, network,
or device interface. A separate unlinked policy accepts exactly five existing
process roles, enforces source lifecycle epochs and sequences, global ordering,
hard capacity, and a role-specific event matrix. Its unlinked producer codec
carries one fixed typed event and requires one exact accepted/rejected response
before another event; source,
lifecycle, sequence and time never cross that wire. A fourth pure layer owns
one live and one sealed window, reserves every active source's stop record,
requires exact handoff acknowledgement, and marks an unrecordable disconnect
as visibly incomplete instead of stranding the source or claiming completeness.
The pure peer layer validates six distinct non-root UIDs plus the shared GID and
maps connection-bound PID/UID/GID facts to only one source. Numeric credentials
are not returned or exported.

That unlinked adapter now uses fixed-name safe nix account lookup, verifies the
collector effective UID, and calls Tokio `UnixStream::peer_cred` before
returning only an opaque authenticated-source token. The same crate now
contains an unlinked in-memory runtime core that consumes that token, creates
an opaque instance-bound session, decodes one exact frame, returns one exact
response, records EOF, and delegates bounded rotation/snapshot/acknowledgement
to the window policy. Invalid frames receive no response and terminate through
the disconnect path. A transient unlinked owner now accepts a caller-bound
Unix listener, creates a random nonzero instance binding, samples `Instant`
under the core mutex, bounds partial-frame and response I/O, reaps connection
tasks, and records graceful shutdown. A separate unshipped collector now owns
the single fixed socket path and a hardened local-only service boundary. Its
reversible H616 gate passed exact peer, timeout, shutdown, and restoration
checks. No shipped package, producer integration, API, or advertised capability
exists at that collector checkpoint. Numeric assignment,
credential, authenticated-peer, and connection handles intentionally expose no
generic `Debug` output.

The denial-only control broker is now the sole runtime consumer of the fixed
producer client and its reporter. It starts diagnostics best-effort, submits
only `RequestRejected(CapabilityDisabled)` after the control response has been
written, ignores every queue receipt, and retains the reporter owner only for
the broker lifetime. The client still bounds connect/write/response, discards
uncertain streams, and never retries automatically. The reporter still uses
one fixed eight-event ring, non-blocking submission, one ordered worker,
bounded semantic counters, and bounded abort-on-drop shutdown. No collector is
packaged, no service dependency was added, and no diagnostic API or product
capability is advertised. The reversible H616 exact-event and stalled-collector
integration gates passed with complete cleanup; see
`baselines/device-diagnostic-control.md`.

The next unlinked pure boundary is a fixed sealed-window export codec. It binds
acknowledgement to both the OS-random collector instance and window ID, accepts
only a typed `DiagnosticBundle` for encoding, and bounds one 16-byte request
plus a 24-byte header and at most 8 KiB of payload. It adds no socket, consumer
identity, persistence, API, UI, or retry behavior yet.

An unlinked listener now composes that codec with the existing collector
runtime and fixed account resolver. It authenticates only the semantic
platform-broker role from `SO_PEERCRED`, accepts a caller-bound Unix listener,
uses two non-waiting connection permits, and bounds one request/read and
response/write. Snapshot and acknowledgement complete under the shared runtime
before socket output, so stalled export I/O holds no collector state lock. No
shipped package links this listener. The unshipped collector now binds fixed
`collector.sock` and `export.sock` paths atomically and runs both loops over one
shared runtime. A drop guard broadcasts shutdown if either task returns or
panics; both tasks are joined before inode-bound socket cleanup. No export
product route exists. A separate unlinked Rust client now opens only the fixed
`export.sock`, uses one fresh connection per operation, returns an owned
fixed-capacity snapshot, and distinguishes retryable non-mutating reads from
delivery-ambiguous acknowledgements. It never retries, persists, logs, or
accepts a caller-selected path. The reversible H616 gate accepted exact
consumer identity, connection pressure with producer progress, repeatable sealed reads,
mismatch-preserving and exact acknowledgement, dual-loop shutdown, and complete
rollback; see `baselines/device-diagnostic-export.md` and
`baselines/diagnostic-export-client.md`.

## Core invariants

1. Merely rendering a page cannot acquire or mutate hardware.
2. No user-controlled value reaches a shell command.
3. A network-facing process does not run as root.
4. Hardware actions use typed requests and explicit authorization.
5. Capability availability and evidence quality are separate facts.
6. A host build is never treated as physical hardware acceptance.
7. Application updates and full-system updates have separate trust and
   recovery gates.

The application updater is a local-only verifier, immutable stager,
anti-rollback manager, one-use health runtime, and fixed supervisor CLI. It has
no network activation interface, shell, block-device access, or authority to
select a system boot slot. The network server receives no direct slot, journal,
process, watchdog, or signing-key authority.

The first local supervisor implementation now provides that evidence and a
one-use stdin-only confirmation path. Its systemd socket is health-only; local
activation remains an explicit fixed CLI operation for a future update worker.
No HTTP route or browser capability is connected to it.

Candidate process orchestration is split again: PID 1 copies the one-use token
into a read-only credential visible to the candidate unit, while a small Rust
reporter validates the loopback server health contract and can only write that
token to the health socket. Fixed root apply units perform the two literal
candidate/confirmed restarts after activation or watchdog completion.

The system-image verifier is a separate crate and Ed25519 trust domain. It can
authenticate only fixed rootfs and optional boot filesystem payloads against a
root-owned exact layout policy. It accepts no device node, partition table,
offset, bootloader, installer, or hook and has no writer. Recovery provisioning,
inactive-slot writes, boot selection, reboot, and rollback remain future
separate processes and gates. Their journal/selector authority and crash points
are constrained by `api/system-boot-control-v1.md` without selecting an H616
storage implementation. `bli-kvm-system-layout` validates canonical recovery
geometry and returns only exact capacities/hash evidence; it has no device or
partition API and no profile is compiled in as accepted.
`bli-kvm-system-boot-control` similarly models checksummed dual-copy records
and reconciliation without a filesystem or selector adapter. Its path-free
storage trait requires per-copy atomic replace, sync, exact readback, expected
generation, and an external exclusive recovery lock.

Recovery media is a third signed domain described in
`api/recovery-image-v1.md`. It is a complete independently bootable removable
image with full provenance/SBOM/license gates, not a network update payload.
`bli-kvm-recovery-inputs` validates only the canonical public evidence document
and contains no downloader, package manager, scanner, signer, or image builder.

## Transition video design

The first Bli adapter may keep uStreamer and Janus as loopback-only processes.
The web application selects a Janus or native WebRTC adapter from the
capability manifest. Janus is removed only after a Rust V4L2/native WebRTC path
passes the same latency, format, reconnect, multi-client, and endurance gates.
