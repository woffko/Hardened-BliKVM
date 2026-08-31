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

Checkpoint: the first board-function slice observes only the fixed H616 SoC
thermal source. Its Rust adapter uses no-follow open semantics, rejects unsafe
metadata, malformed/oversized input, and values outside -40 through 150 degrees
Celsius, and adds a nullable millidegree observation to inventory schema v2.
The React interface displays the sanitized value while fan, PWM, GPIO, and all
other board controls remain unavailable. Host gates pass; H616 systemd and TLS
browser runtime evidence remain required before promotion from source evidence.

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
Two clean AArch64 builds at source commit `0bc0087` produced byte-identical
linked payloads, stager object code, and prototype archives; exact hashes are
recorded in `baselines/application-update-stager.md`.
The stager remains an unwired library. Activation policy is handled by the
separate checkpoint below; staging cleanup, process orchestration, live
watchdog acceptance, and the separately signed system-image format remain
open.

Checkpoint: the separate activation core now persists a bounded exclusive
journal and replay history, switches one relative application pointer
atomically, raises the sequence floor only after boot-bound health
confirmation, and rolls back across prepared, switched, timeout, reboot,
explicit-failure, corrupt-target, and pointer-mismatch states. It remains
unwired; systemd process orchestration, kernel clock acquisition, one-use
health credentials, watchdog scheduling, live interruption, and staging/slot
cleanup remain open.
Two clean AArch64 builds at source commit `fdaf418` produced byte-identical
linked payloads, manager object code, and prototype archives; exact hashes are
recorded in `baselines/application-update-manager.md`.

Checkpoint: a local supervisor now samples kernel boot ID and `CLOCK_BOOTTIME`,
issues a mode-0400 one-use health credential, accepts exactly 32 raw stdin
bytes, zeroizes secret buffers, reconciles crash residue, and drives manager
confirmation or rollback. A sandboxed two-second watchdog timer and
group-gated health-only Unix socket are packaged but not installed or enabled.
There is no activation socket, production trust key, HTTP/GUI route, process
restart wiring, or H616 acceptance yet.
Its local `stage` command now verifies one absolute archive path and publishes
an immutable slot without changing `current`, so device acceptance can use the
same production CLI rather than manually constructing slot trees.
The exact linked supervisor then passed an isolated H616 gate for three signed
slots, confirmation, floor enforcement, downgrade and wrong-token rejection,
lost-token rollback, transient systemd sandboxing, and complete `/tmp` cleanup.
Details and remaining production-process gates are recorded in
`baselines/device-update-supervisor.md`.

Checkpoint: staging now holds an exclusive slot-root lock and writes a durable
structured marker before extraction. Local `stage` and watchdog paths clean
only marker-owned bounded residue after link/owner/device/type validation;
read-only partial trees and symlink-escape refusal are tested. Reference-aware
transactional slot garbage collection now protects every journal/current
reference, retains a bounded newest-candidate set, and uses a durable marker
plus atomic tombstone rename with pre/post-rename crash recovery.
The exact GC-linked AArch64 supervisor passed the corresponding isolated H616
gate: sequence 1 was deleted while active sequence 2 and protected previous
sequence 3 remained, followed by clean marker/tombstone and `/tmp` cleanup.

Checkpoint: a minimal Rust health reporter now validates the real server's
loopback health contract, reads the token only from a systemd credential, and
submits exactly 32 bytes to the Unix confirmation socket. Static current,
candidate, activate, apply-candidate, and apply-confirmed units use literal
paths/unit names and no shell. The exact Rust server, supervisor, reporter,
systemd credential delivery, socket confirmation, failed-candidate timeout,
rollback, and restored server then passed a temporary isolated H616 process
gate. The confirmation sandbox writes only its state and runtime directories;
all staging and transient units were removed, KVMD remained active, and no
physical action occurred. Two independent AArch64 builds at source commit
`2be3f7f` produced byte-identical binaries and prototype archives. Exact hashes,
the sandbox finding, and the remaining installation/interruption/recovery gates
are recorded in `baselines/device-update-process-orchestration.md`.

Checkpoint: the first host-only system-image verifier now uses a separate
`sys-` Ed25519 domain and canonical `hardened-blikvm-system-v1` manifest. A
root-owned policy binds the accepted layout ID, exact inactive rootfs/boot
capacities, and minimum recovery and boot-control sequences. The archive has no
field for a disk, partition, offset, bootloader, state, MSD, installer, hook,
URL, or arbitrary path, and the crate has no writer. Negative tests cover
cross-domain keys, policy mismatch, signatures, non-canonical/unknown JSON,
payload tamper, filesystem signatures, entry order/type/mode, extra bootloader
content, unsafe source inodes, and archive bounds. The conflicting upstream p3
roles and truncated four-partition field image are recorded in
`baselines/system-layout-evidence.md`; they remain hard blockers for an H616
layout or device write. A separate fixture builder generates public-only
rootfs-only and split-boot archives while keeping its ephemeral private key in
process memory.
The exact verifier checkpoint, fixture hashes, two-build AArch64 codegen result,
and the non-shipped `lib.rmeta` variance are recorded in
`baselines/system-image-verifier.md`.

Checkpoint: the host-only boot-control contract now separates verifier,
recovery writer, selector adapter, and health agent authority. It defines
WritePrepared, CandidateReady, BootPrepared, BootArmed, AwaitingHealth,
Confirmed, and rollback phases; a one-to-three attempt counter decremented
before boot; exact journal/selector crash reconciliation; and a floor raised
only after bound health. It also records the need for rollback-resistant
storage and two independently verified recovery cards. No selector adapter,
writer, reboot path, or H616 layout is implemented or authorized.

Checkpoint: a bounded read-only H616 inventory now confirms one non-removable
MMC disk with the same four MBR/ext4 sizes as the field image: root/logs, an
unmounted 3 MiB partition, data, and MSD. The complete disk has a
2,586,836,992-byte unpartitioned tail, smaller than the 5,448,400,896-byte root,
so it cannot provide an unused equal A/B slot. UUID-rooted Armbian boot scripts
exist, but extlinux, slot hints, and fw_env tools do not. The ten-byte raw read,
unchanged services/mounts, and remaining boot-chain unknowns are recorded in
`baselines/device-storage-layout.md`; no layout or write target was accepted.

Checkpoint: two further read-only probes found `eGON.BT0` at the standard 8 KiB
sunxi location but not at 128 KiB, no exposed eMMC boot partitions, and no
bootcount/bootlimit/altbootcmd/saveenv/upgrade primitives in the root-hosted
Armbian boot scripts. The unmounted 3 MiB p2 is clean ext4 without a journal;
only `lost+found` exists among eight allowlisted boot paths. Exact hashes,
bounded-read policy, official offset reference, and remaining uncertainty are
recorded in `baselines/device-boot-chain.md`. P2 and all raw boot ranges remain
unaccepted write targets.

Checkpoint: `bli-kvm-system-layout` now validates canonical recovery-only MBR
or GPT geometry with 512-byte sectors, fixed semantic roles, 1 MiB alignment,
non-overlap/bounds, primary sunxi raw range, equal read-only root A/B, optional
paired boot A/B, separate writable state, redundant journal/selector copies,
bounded attempts, and mandatory external recovery media. Seven host tests cover
valid MBR/GPT examples, the exact live geometry rejection, overlaps, ordering,
alignment, capacity, role/pair policy, table limits, unsafe boot-control, raw
offsets, and non-canonical/unknown documents. The crate returns only profile
hash and capacities and contains no device writer or accepted built-in profile.

Checkpoint: `bli-kvm-system-boot-control` now models canonical independently
checksummed journal/selector copies and pure crash reconciliation. It selects a
single valid copy in degraded mode, accepts only adjacent generations, and
hard-stops no-valid-copy, same-generation split brain, larger generation gaps,
invalid phase material, mismatched layout/operation/slot/digest, selector
generation regression, and attempt-count increase. Nine tests cover clean,
write-ready, interrupted arm, armed, candidate-booted, awaiting-health,
confirmation, and rollback states. Checksums are explicitly corruption-only;
there is no persistence, selector adapter, writer, reboot, or physical
anti-rollback claim.
The exact host gates, independent AArch64 codegen hashes, byte-identical
prototype bundle, and non-shipped metadata variance are recorded in
`baselines/system-recovery-core.md`.

The boot-control core now also validates application/system floor coupling.
Across all seven pending system phases it permits application confirmation only
when `minimumSystemSequence` remains compatible with the previous confirmed
system; the candidate sequence becomes usable only after final system
confirmation. Two additional tests cover rollback-anchor enforcement, stable
post-confirmation acceptance, and invalid application/system evidence.

The same crate now defines a path-free redundant storage trait and ordered
persistence coordinator. Nine in-memory fault-injection tests cover clean
initialization, pre-existing-state refusal, first/second copy replace errors,
torn writes, sync loss, crash discard, degraded old/new recovery, convergence,
selector parity, read failure, generation exhaustion, and stale expected
generation before write. A concrete adapter and transition authorizer remain
absent.
The compatibility/storage AArch64 checkpoint and fault-matrix hashes are
recorded in `baselines/system-recovery-core.md`.

Checkpoint: `api/recovery-image-v1.md` now defines a third `recovery-` trust
domain and complete removable-media contract. It binds a raw image, accepted
layout, boot chain, build recipe, source/license manifests, and canonical SPDX
3.0.1 SBOM; requires byte-identical builds and two recovery cards; and rejects
truncation, live state, secrets/default accounts, automatic target selection,
and every unresolved binary/license/source decision. No artifact can be built
until redistribution-cleared boot/rootfs inputs, key ceremony, and an accepted
layout exist.

Checkpoint: `bli-kvm-recovery-inputs` now validates canonical recovery build
evidence with ten required component kinds, separately named firmware,
source/artifact/license-text hashes, pinned revisions, corresponding source,
reviewed `cleared` status, cross-reference hashes, and exact SPDX 3.0.1
context/schema/model plus structural/semantic results. Nine tests reject
missing/renamed/unsorted inputs, `blocked`/`no_assertion`, missing source,
unreviewed binary provenance, branch-like revisions, invalid license/hash,
SPDX drift, cross-reference mismatch, unknown fields, and non-canonical or
oversized documents. The crate performs no legal inference, network access,
scanning, signing, or image build, and no real passing manifest exists yet.
The two-build codegen checkpoint and exact missing real inputs are recorded in
`baselines/recovery-inputs.md`; synthetic test evidence is not a recovery
release candidate.

## Phase 7: Release acceptance

- Use at least two devices and two recovery cards.
- Exercise clean install, update, rollback, repeated reboot, networking, TLS,
  browser/mobile, HDMI, HID, ATX, media, and board peripherals.
- Bind every result to exact source and artifact hashes.

Exit gate: the complete matrix is accepted; GPL sources and attribution are
published with the exact artifacts.
