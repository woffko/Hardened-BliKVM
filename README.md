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
- an unlinked fixed-size XChaCha20-Poly1305 diagnostic journal and fixed-root
  atomic A/B store with OS entropy, a fixed 96-byte systemd key loader, and a
  cancellation-aware `no_std` write-before-ack coordinator; all enforce strict
  floor/reselection/inode/ACL/zeroization checks and have no provisioned key,
  live transport/store link, API, or UI;
- an unlinked default-off diagnostic persistence adapter that composes only
  the accepted typed keys, fixed store, fixed export client, and coordinator;
  it has no floor source, executable, service, package, HTTP, or UI;
- an unshipped zero-argument synthetic persistence gate and rollback harness,
  accepted reversibly on H616 and absent from the prototype payload;
- an authenticated GET-only `/api/diagnostics` current-health view for
  temperature, RTC, and fixed component states; persistent history is
  explicitly unavailable and the server has no key/store/floor/ACK authority;
- a dedicated responsive Diagnostics GUI with exact 26-locale coverage,
  cancellation-safe refresh, explicit loading/unavailable/stale/error states,
  and reversible desktop/mobile Chromium acceptance on H616;
- an unshipped empty-only diagnostic maintenance binary whose static units,
  synthetic systemd credential, encrypted store initialization, nonmutating
  inspection, and full rollback passed on the H616 test device;
- a signed Ed25519 application verifier, immutable stager, anti-rollback
  journal, one-use health credential, watchdog rollback, and transactional
  slot garbage collection;
- a pure Rust public-only application trust enrollment contract that validates
  the Ed25519 point/fingerprint and renders only fixed supervisor configuration;
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
  processes and the prototype bundle, with host build paths remapped and
  rejected by the ELF privacy gate;
- a deterministic inert `arm64` Debian runtime package whose real H616
  install/hash/mode verification/purge transaction and crash-guarded retained
  installation passed;
- a separate reproducible inert application-update package, retained on H616
  with its Rust supervisor/reporter and systemd templates disabled/inactive,
  no trust/configuration/state/slots, and no restart of the working GUI;
- rollback-gated fixed service identities and a separate device-generated KVMD
  credential, followed by a rollback-gated persistent web cutover: PiKVM Nginx
  is disabled, the Hardened broker/video/server/Nginx units are enabled, and
  the account-free First Setup page passed tunneled and direct-LAN Chromium;
- reversible installed-unit activation for platform broker, MJPEG proxy, and
  loopback server, including systemd isolation, Diagnostics, video reconnect,
  session revocation, and complete state/drop-in/cgroup cleanup;
- installed-runtime TLS 1.2/1.3 and full desktop/mobile Chromium acceptance
  behind an unprivileged loopback-only shadow Nginx, with the working PiKVM
  proxy unchanged and complete rollback;
- persistent TLS 1.2/1.3 on public 80/443 using the unchanged device
  certificate, split-privilege Nginx, disabled vendor web service, and zero
  browser control requests;
- direct-LAN operator-account login/logout plus a 30-sample persistent
  resource/TLS/auth-denial soak with stable service PIDs and restart counters;
- a rollback-gated upgrade to the reproducible `0.1.0~dev2` runtime package,
  followed by a real guarded BliKVM software reboot, preserved account/TLS,
  persistent services, decoded/reconnected MJPEG, Diagnostics, and final
  zero-restart/zero-guard inspection;
- architecture, field-image, upstream, licensing, and security baselines;
- project-scoped semantic LSP smoke fixtures.

No hardware-mutating backend, production updater installation, production
signing key, full-system image, H616 A/B recovery path, or production release
is accepted yet. One guarded software reboot is accepted, but host builds and
isolated device gates do not establish the remaining repeated/interrupted
reboot, physical, or power-cut matrices. The
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
