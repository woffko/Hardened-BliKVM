# BLi-KVM implementation plan

The plan is intentionally gate-driven. Later phases do not erase an earlier
physical or security failure.

## Phase 0: Baselines and governance

- Reserve `woffko/Hardened-BliKVM` as the canonical GitHub repository identity;
  create or publish it only as an explicit publication action.
- Pin upstream source revisions and retain their licenses and attribution.
- Maintain a sanitized hardware manifest; keep endpoints and credentials only
  in encrypted Project Memory.
- Define the v1 feature boundary and current evidence quality.
- Record license, trademark, binary-redistribution, and recovery blockers.

Exit gate: the repository identity is fixed, all source inputs are pinned, no
private image or credential is in Git, and unresolved redistribution rights
remain explicit blockers.

## Phase 1: Contract and shared core

- Version the capability, REST, and WebSocket contracts.
- Extract reusable Hardened auth/security/update primitives behind platform
  interfaces.
- Keep imported GUI regression checks passing while replacing NanoKVM-specific
  assumptions with explicit Bli adapters.
- Serve a mock and read-only Bli capability endpoint.

Exit gate: contract tests pass; the GUI can render from the manifest; no
hardware mutation exists in the network process.

## Phase 2: Reversible H616 prototype

- Cross-compile for aarch64.
- Run on recovery media beside the vendor stack, loopback or alternate port
  first.
- Add a local allowlist broker for the authenticated KVMD Unix socket; keep its
  systemd credential out of the network process.
- Establish health, first-account setup, sessions, and read-only inventory.

Exit gate: browser and authentication matrix passes on a designated test-only
device without changing its working KVM stack.

Checkpoint: the commit-bound unprivileged server and distinct-UID read-only
broker have passed their loopback H616 execution, authenticated inventory,
HTTP-policy, graceful-stop, and cleanup gates. The temporary broker test reused
the current test-device credential without exposing it to the server.
Dedicated least-privilege KVMD account provisioning, systemd
`LoadCredential`, and runtime unit sandboxing have now passed a reversible
device gate with byte-identical htpasswd restoration. A loopback Nginx proxy
then passed TLS 1.2/1.3 and a real Chromium setup/login/inventory/password/logout
flow through an SSH-only tunnel. Permanent provisioning, certificate
enrollment, additional browser engines, and reboot persistence remain beyond
this Phase 2 development checkpoint.

Checkpoint: the first Phase 4 slice was intentionally pulled forward after
the read-only broker boundary stabilized. A distinct-UID, Unix-only MJPEG
proxy and session-bound Rust response passed the reversible H616 systemd gate,
including frame capture, sequential reconnects, two concurrent streams,
logout revocation, and full cleanup. The capability was then promoted only to
`read_only`; the GUI kept WebSocket, HID, ATX, media, and writable video
controls disabled. Real Chromium subsequently decoded and reconnected MJPEG
over the TLS/SSH-forward boundary with zero control or WebSocket requests.
Long-duration video, WebRTC, hotplug, configuration, reboot, and physical
acceptance remain open.

Checkpoint: virtual-media state is now advertised read-only while mutating UI
requires `read_write`. A private opaque-ID catalog removes caller-controlled
paths; a persisted policy provides confirmation, Pending crash ambiguity,
replay/conflict protection, busy/disconnected checks, and cooldown. A separate
mock-tested KVMD client exposes only catalog-resolved `set_params` and typed
`set_connected`. Upload/remove/reset and broker/server wiring remain absent.

## Phase 3: HID and power

- Add the restricted Rust hardware broker.
- Port keyboard, absolute and relative mouse, ATX actions, and LED state.
- Establish exclusive GPIO ownership and rollback.

Exit gate: HID event, mode, ATX timing, LED, reboot, and recovery tests pass.

Checkpoint: the bounded v1 control protocol, client, and isolated broker now
exist, but the broker has no backend and always returns
`capability_disabled`. Its static-PIE AArch64 bundle passed a non-actuating H616
gate for HID, ATX, and virtual-media request families while the browser issued
zero control/WebSocket requests and all staging was removed. This proves the
fail-closed boundary only; HID leases/translation and ATX arming/timing remain
required before either capability can change.

Checkpoint: a pure Rust HID translator, exclusive 30-second
session-and-target-bound lease manager, five-route KVMD transition client, and
atomic release ledger now pass local failure/recovery tests. Backend errors
revoke the lease and attempt release-all; a simulated process restart replays a
persisted key release before granting a new lease. These components are not
wired to the denial broker or server. Safe external target/focus, broker-side
attestation, on-device startup replay, event soak, reconnect, and reboot gates
remain required.

Checkpoint: read-only H616 power-LED evidence is now advertised independently
while the ATX action capability remains unavailable. A private atomic journal
implements confirmation age, Pending crash ambiguity, persisted replay,
operation-ID conflict detection, and five/30-second cooldowns. Its public
eligibility is compile-time disabled. A separate mock-tested KVMD client has
only the three fixed waiting ATX routes and is not connected to the broker.
No Power or Reset action was invoked.

## Phase 4: Video

- Proxy loopback-only MJPEG first.
- Integrate Janus through a capability-selected React adapter.
- Develop native Rust WebRTC only after stable parity metrics exist.

Exit gate: no-signal, hotplug, resolution, two-client, repeated mode-switch,
latency, CPU/memory, and 24-hour soak gates pass without new kernel alerts.

## Phase 5: Virtual media and board functions

- Reimplement configfs media insertion/ejection and safe upload.
- Port fan, OLED, RTC, buttons, serial switch, network, and system controls in
  shadow-first slices.

Exit gate: BIOS boot from virtual media and physical board tests pass, with
safe recovery from interruption.

## Phase 6: Hardening and updates

- Remove broad privilege and default credentials.
- Enforce secure cookies, CSRF, origin checks, lockout, revocation, least
  privilege, systemd sandboxing, and firewall rollback.
- Ship signed atomic application updates before full-system updates.
- Keep full-system updates recovery-media-only until H616 A/B boot and
  power-loss rollback are proven.

Exit gate: negative signature/archive/path tests, power-cut recovery, SBOM,
dependency audit, secret scan, and privilege audit pass.

Checkpoint: the first application-only artifact verifier now uses strict
in-process Ed25519, a distinct `app-` trust domain, deterministic raw-tar
metadata, exact payload-tree matching, per-file SHA-256, and bounded resource
limits. Negative tests cover key/signature/payload/path/type/tree failures.
Two clean AArch64 builds at source commit `5603c2b` produced byte-identical
linked payloads and prototype archives; the exact hashes and the isolated
non-shipped Rust metadata variance are recorded in
`baselines/application-update-verifier.md`.
The separate stager now performs one bounded source copy, verifies and extracts
from the same unlinked read-only inode, fsyncs a read-only slot, publishes it
with no-replace `renameat2`, and fully revalidates idempotent existing slots.
It remains an unwired library. Active-slot selection, anti-rollback state,
crash cleanup, health confirmation, rollback, and the separately signed
system-image format remain open.

## Phase 7: Release acceptance

- Use at least two devices and two recovery cards.
- Exercise clean install, update, rollback, repeated reboot, networking, TLS,
  browser/mobile, HDMI, HID, ATX, media, and board peripherals.
- Bind every result to exact source and artifact hashes.

Exit gate: the complete matrix is accepted; GPL sources and attribution are
published with the exact artifacts.
