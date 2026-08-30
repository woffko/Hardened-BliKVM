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
  - native: HID, USB configfs, ATX, board I/O, virtual-media LUN actions
  - credentials supplied by systemd, never by argv or the network process

Separate service users
  - video capture and Janus during transition
  - future non-networked update worker owning only its canonical slot root
  - log collector
```

The transition broker is logically privileged even when it needs no root UID:
its KVMD credential can perform hardware actions. Read-only inventory and
mutating methods use separate traits and policy checks. A runtime observation
never enables a mutating method by itself.

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

## Transition video design

The first Bli adapter may keep uStreamer and Janus as loopback-only processes.
The web application selects a Janus or native WebRTC adapter from the
capability manifest. Janus is removed only after a Rust V4L2/native WebRTC path
passes the same latency, format, reconnect, multi-client, and endurance gates.
