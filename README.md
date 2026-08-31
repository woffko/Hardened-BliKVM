# BLi-KVM

BLi-KVM is a security-focused KVM-over-IP stack for BliKVM v4 H616 hardware.
It is being built as a GPL-3.0-only Rust control plane with a React/TypeScript
web interface derived from the proven Hardened NanoKVM product shape.

The target is not a themed copy of the existing BliKVM application. BLi-KVM
uses a platform-neutral core and explicit hardware adapters so that security,
API, update, and recovery invariants remain testable across platforms.

The planned canonical GitHub repository is `woffko/Hardened-BliKVM`. It has
not been created or published from this worktree yet.

## Current status

The reversible H616 prototype and the application-update core are implemented:

- versioned capability, runtime-inventory, and broker contracts;
- a fail-closed, fully rebranded GUI with first-account setup;
- Argon2id passwords, opaque HttpOnly sessions, CSRF/origin checks, revocation,
  and scoped login lockout;
- a loopback-only Axum server with protected `/api/inventory`;
- a credential-isolated read-only platform broker and bounded KVMD Unix-socket
  client; broker schema v1 has no mutation variants;
- bounded read-only H616 thermal observation through runtime-inventory schema
  v3, plus external-RTC epoch/offset observation, with no clock, fan, PWM, or
  GPIO control path;
- an isolated read-only MJPEG proxy with session-bound browser delivery;
- fail-closed Rust HID, ATX, and virtual-media transition cores that are not yet
  wired to a hardware-mutating backend;
- pure host-only fan policy and mock-backed supervisor crates that enforce
  failsafe/latch semantics but are not linked to any runtime or hardware
  backend;
- pure typed diagnostic export/source/protocol/window/peer layers plus an
  unlinked safe fixed-account and Unix-peer adapter, with no raw message, path,
  or identity output;
- a signed Ed25519 application verifier, immutable stager, anti-rollback
  journal, one-use health credential, watchdog rollback, and transactional
  slot garbage collection;
- a host-only, separately keyed system-image verifier bound to an explicit
  recovery/boot-control/layout policy and incapable of selecting a disk;
- a recovery-only geometry validator that requires equal A/B roots, redundant
  state policy, bounded sunxi raw ranges, and rejects the current live layout;
- a pure dual-copy system journal/selector model that hard-stops split brain,
  generation gaps, mismatched evidence, and increasing attempt counters;
- a canonical recovery-input gate requiring pinned provenance, corresponding
  source, explicit redistribution clearance, and SPDX 3.0.1 validation;
- H616 gates for systemd credential delivery, a real candidate server, health
  confirmation, rejected candidate timeout, rollback, and restored service;
- a source-evidence-only H616 GPIO/RTC profile and vendor-to-Rust migration map;
- reproducible stripped AArch64 static-PIE builds for all shipped Rust
  processes and the prototype bundle;
- architecture, field-image, upstream, licensing, and security baselines;
- project-scoped semantic LSP smoke fixtures.

No hardware-mutating backend, production updater installation, production
signing key, full-system image, H616 A/B recovery path, or production release
is accepted yet. Host builds and isolated device gates do not establish the
remaining physical, interruption, reboot, or power-cut matrices. The
user-supplied field image is truncated relative to its partition table and is
explicitly rejected as a recovery/release image.

## Architecture

```text
React/TypeScript web UI
        |
        | HTTPS / WebSocket
        v
bli-kvm-server (unprivileged Rust process)
        |
        | restricted Unix socket
        v
bli-platform-broker (credential-isolated Rust process)
        |
        +-- now: five allowlisted read-only KVMD GETs
        +-- later: typed HID / USB configfs / virtual media
        +-- later: ATX / fan / OLED / RTC / buttons / serial switch

Separate media services
        +-- loopback/Unix-socket uStreamer transition
```

See [the architecture](docs/architecture.md),
[implementation plan](docs/implementation-plan.md), and
[security boundaries](docs/security-boundaries.md).

## Development

```sh
./scripts/check.sh
cargo run -p bli-kvm-server
scripts/build-aarch64.sh
```

The check runner uses `/mnt/d/debug/blikvm/hardened-blikvm/pnpm-store` for
large temporary frontend dependencies. Override it with the task-specific
`BLI_KVM_PNPM_STORE` variable when that path is unavailable.

The development server binds to `127.0.0.1:18080` and refuses non-loopback
binds. Device exposure must use a separately hardened TLS reverse proxy. Do not
place device images, credentials, private evidence, or extracted root
filesystems in this repository.
