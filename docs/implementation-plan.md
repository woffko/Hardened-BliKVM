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

Checkpoint: the commit-bound unprivileged server slice has passed its
loopback-only H616 run, HTTP-policy, graceful-stop, and cleanup gate. Dedicated
KVMD broker credentials, authenticated on-device inventory, TLS proxy, and the
browser authentication matrix remain before the Phase 2 exit gate.

## Phase 3: HID and power

- Add the restricted Rust hardware broker.
- Port keyboard, absolute and relative mouse, ATX actions, and LED state.
- Establish exclusive GPIO ownership and rollback.

Exit gate: HID event, mode, ATX timing, LED, reboot, and recovery tests pass.

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

## Phase 7: Release acceptance

- Use at least two devices and two recovery cards.
- Exercise clean install, update, rollback, repeated reboot, networking, TLS,
  browser/mobile, HDMI, HID, ATX, media, and board peripherals.
- Bind every result to exact source and artifact hashes.

Exit gate: the complete matrix is accepted; GPL sources and attribution are
published with the exact artifacts.
