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
other board controls remain unavailable. At source commit `329194e`, the full
host suite, two byte-identical AArch64 builds, a focused reversible systemd
profile with 20 authenticated inventory requests, and TLS Chromium
Settings/About rendering passed with complete cleanup. The capability is
runtime-verified. Controlled missing-sensor behavior, longer observation,
permanent packaging, and reboot persistence remain open; exact hashes and the
separate video-gate limitation are recorded in `baselines/device-thermal.md`.

Checkpoint: the next board-function slice reads only the fixed kernel-bound
PCF8563 `rtc1` identity and sysfs epoch. Inventory schema v3 carries a paired,
bounded external epoch and RTC-minus-system offset; Settings/About formats both
without any raw I2C, RTC character-device open, registration, or clock write.
At source commit `6c9006a`, two independent AArch64 builds were byte-identical
and 20 authenticated H616 broker requests returned valid schema v3 observations
with complete cleanup. The final runtime-evidence archive at source commit
`cc37a3c` was again byte-identical across two builds and passed focused systemd
and TLS Chromium Settings/About rendering with complete rollback. The
capability is runtime-verified. Rebind/fault, drift, battery, reboot, alarm, and
any clock-write direction remain separate gates; exact evidence is recorded in
`baselines/device-rtc.md`.

Checkpoint: a separate `bli-kvm-fan-policy` crate now models only proposed fan
commands. It validates thresholds, hysteresis, duty curves, sensor timeout, and
minimum on/off dwell; startup, sensor failure, invalid input, timeout, and
critical heat fail to 100 percent. The crate has no hardware/runtime dependency
and is not linked into the broker or server. GPIO/PWM ownership, polarity,
frequency, minimum spinning duty, stall/fault injection, boot/restart behavior,
and physical thermal soak remain required before any fan capability exists. At
source commit `09cd041`, the full host suite, two byte-identical prototype
archives, and byte-identical extracted AArch64 ThinLTO policy object passed;
exact hashes and the non-shipped rlib metadata boundary are recorded in
`baselines/fan-policy.md`.

Checkpoint: an additional unlinked `bli-kvm-fan-supervisor` composes the policy
with an abstract mockable actuator. It requires confirmed 100-percent startup,
sends changed commands only, attempts and latches maximum cooling after any
actuator error, blocks samples while actuation is uncertain, supports explicit
failsafe retry, and re-confirms maximum cooling on controlled shutdown. It has
no concrete driver and makes no crash-safety claim; those remain physical
kernel/hardware gates. At source commit `9979c77`, the full host suite, two
byte-identical prototype archives, and byte-identical extracted AArch64 ThinLTO
supervisor object passed; exact hashes and the non-shipped metadata boundary are
recorded in `baselines/fan-supervisor.md`.

Checkpoint: a metadata-only H616 probe confirmed that chip0 has 288 lines and
offset 269 is already a used legacy sysfs output (`out`, `active_low=0`). No
line request or value read occurred. This blocks a concrete second owner until
legacy-owner handoff and rollback are proven; logical active-high metadata does
not replace electrical polarity/PWM/minimum-duty or thermal-soak evidence. See
`baselines/device-fan-gpio.md`.

Checkpoint: a later metadata-only profile confirmed all pinned offsets without
line requests or values. KVMD owns Power/Reset and ATX LED lines; legacy sysfs
owns activity LED and fan; display-button offset 257 is unused input. This
blocks native second owners and preserves physical Power/Reset, polarity,
debounce, fan-safety and owner-handoff gates. See
`baselines/device-gpio-profile.md`.

Checkpoint: a metadata-only serial topology probe found no ttyUSB0-3 device and
no serial/by-id directory. It opened no TTY, changed no termios and read/wrote
no frames or unique identifier. The serial-switch capability remains hidden;
only a pure bounded protocol codec can proceed without deliberately attached
hardware. See `baselines/device-serial-topology.md`.

Checkpoint: the separate `bli-kvm-serial-switch-codec` now models typed ports,
exact ten-byte select commands, and exact five-byte status frames. It rejects
all other lengths, padding, noise, fixed-byte mutations and non-port bytes; it
is no_std, dependency-free, unlinked, and has no I/O capability. At source
commit `2410e42`, the full host suite, two byte-identical prototype archives,
two byte-identical AArch64 rlibs and byte-identical extracted ThinLTO codegen
passed. This is source/codegen evidence rather than a wire capture. Identity,
exclusive 19200/8N1 ownership, bounded streaming/timeouts, reconnect and
four-port physical behavior remain mandatory. See
`baselines/serial-switch-codec.md`.

Checkpoint: metadata-only probes confirmed that the H616 vendor-named `OLED`
path is an ST7789 240x240 LCD on root-only `/dev/spidev1.0`, with the expected
SPI child and `sun6i-spi` master present. Source-associated GPIO259/260/262 are
all used legacy sysfs outputs. No SPI open/ioctl/transfer, GPIO request/value,
driver binding, or screen action occurred. This blocks a concurrent Rust
display worker until an explicit SPI/GPIO owner handoff, safe electrical state,
bounded rendering, crash/reboot and physical-screen gates pass. See
`baselines/device-oled-topology.md`.

Checkpoint: the unlinked `bli-kvm-st7789-model` now provides strict 240x240
windows, typed column/row/memory-write phases, bijective rotations,
deterministic RGB565 bytes, bounded 240-pixel chunks, and exact completion. It
is no_std, dependency-free, and has no initialization or I/O authority. At
source commit `8bf6784`, the full host suite, two byte-identical prototype
archives, two byte-identical AArch64 rlibs and byte-identical extracted ThinLTO
codegen passed. Existing SPI/GPIO ownership and every physical display gate
remain unchanged. See `baselines/st7789-model.md`.

Checkpoint: the unlinked `bli-kvm-display-button-policy` replaces the vendor's
100-ms sample counter and repeated held-button triggers with explicit bounded
polarity/debounce/hold configuration, startup-held suppression, a monotonic
one-shot wake intent, debounced-release rearming, and rollback-safe time
handling. At source commit `2c4e882`, the full host suite, two byte-identical
prototype archives, two byte-identical AArch64 rlibs and byte-identical
extracted ThinLTO codegen passed. No GPIO value or button was tested and no
capability was enabled. See `baselines/display-button-policy.md`.

Checkpoint: the unlinked `bli-kvm-activity-led-policy` replaces the vendor's
fixed 20-sample output loop with explicit asserted polarity, bounded
period/pulse/idle timing, disabled/fault inactive states, drift-free monotonic
phase, and deterministic restart. At source commit `012ebe3`, the full host
suite, two byte-identical prototype archives, two byte-identical AArch64 rlibs
and byte-identical extracted ThinLTO codegen passed. Offset261 was not written,
its legacy owner remains, and no capability was enabled. See
`baselines/activity-led-policy.md`.

Checkpoint: a sanitized network source/live audit separates fixed interface
type/link/driver and manager-state metadata from every address, MAC, SSID,
BSSID, route, resolver, profile, endpoint and credential. The live H616
baseline confirms NetworkManager, Ethernet and Wi-Fi ownership without invoking
`nmcli` or changing network state. Vendor root shell routes, profile mode0777,
manager restart and connection cycling are rejected. No Rust adapter or network
capability exists; typed commit/confirm, watchdog rollback, out-of-band recovery
and reboot gates remain. See `baselines/device-network-topology.md`.

Checkpoint: the pinned system-control audit rejects the vendor's combined
identity-bearing system-info route, unconfirmed shell reboot, rootfs/hostname
rewrite, raw-log shell path, and client-only shutdown assumption. Existing
independent adapters already provide the useful sanitized facts, so no broader
live capture was performed. Reboot, shutdown, hostname, service, filesystem,
log and clock actions remain unavailable pending typed eligibility,
confirmation/idempotency, update/storage guards, rollback/privacy, post-boot
and persistence gates. See `baselines/vendor-system-control-boundary.md`.

Checkpoint: the unlinked `bli-kvm-diagnostic-export` rejects arbitrary log
redaction as a trust boundary and accepts only fixed semantic enums with
monotonic relative offsets. A fresh export is bounded to 32 records, 24 hours
and 8,192 canonical ASCII bytes, with hard failure instead of truncation. It
has no message, path, identity or I/O surface. At source commit `071013a`, the
full host suite, two byte-identical prototype archives, and byte-identical
AArch64 rlib, metadata and ThinLTO codegen passed. No live log was read and no
runtime collector, API, UI or capability is linked; see
`baselines/diagnostic-export.md`.

Checkpoint: the unlinked `bli-kvm-diagnostic-collector-policy` accepts exactly
the five existing server/broker/proxy/supervisor roles, requires nonzero source
epochs, exact sequences, explicit stop/restart, collector-wide monotonic time,
and a closed role/event/resource matrix. Capacity and transition failures do
not advance state; epoch and sequence are not exported. At source commit
`45ed884`, the full host suite, two byte-identical prototype archives, and
byte-identical collector/export AArch64 rlib, metadata and ThinLTO pairs passed.
There is no peer binding, socket, clock, process, API, UI or capability; see
`baselines/diagnostic-collector-policy.md`.

Checkpoint: final source commit `64b256d` adds the unlinked
`bli-kvm-diagnostic-protocol`: one canonical eight-byte producer event without
source/lifecycle/sequence/time and one mandatory four-byte accepted or typed
rejection response. All request payload and response domains are exhaustively
tested. The full host suite, two byte-identical prototype archives, and
byte-identical protocol/collector/export AArch64 rlib, metadata and ThinLTO
pairs passed. The earlier event-only `009a9ce` is rejected as incomplete because
it could not distinguish backpressure from acceptance. No socket, peer binding,
runtime or capability exists; see `api/diagnostic-producer-v1.md` and
`baselines/diagnostic-protocol.md`.

Checkpoint: the unlinked `bli-kvm-diagnostic-window-policy` owns one live and
one sealed window, exact nonwrapping IDs/acknowledgement, collector-owned
sessions/time, and the invariant `records + active_sources <= 32`. A second
required rotation returns typed backpressure without mutation. EOF always
releases its role; an unrecordable stop increments visible incomplete metadata
and forces rotation. At source commit `a00e1a8`, the full host suite, two
byte-identical prototype archives, and byte-identical window/protocol/collector/
export AArch64 rlib, metadata and ThinLTO pairs passed. No identity adapter,
socket, process, real clock, persistence, API, UI or capability exists; see
`baselines/diagnostic-window-policy.md`.

Checkpoint: the unlinked `bli-kvm-diagnostic-peer-policy` validates five
producer UIDs plus a distinct collector UID, exact nonzero shared GID, and a
present positive bounded PID. Every zero and duplicate configuration, root,
collector, unknown and wrong-group peer fails closed; only `DiagnosticSource`
leaves the mapper. At source commit `fcfde44`, the full host suite, two
byte-identical prototype archives, and byte-identical peer/collector/export
AArch64 rlib, metadata and ThinLTO pairs passed. No account lookup,
`SO_PEERCRED` call, socket, process or capability exists; see
`baselines/diagnostic-peer-policy.md`.

Checkpoint: the unlinked `bli-kvm-diagnostic-peer-adapter` resolves only six
fixed accounts and one group through safe nix APIs, requires the process eUID
to match the collector account, obtains connection credentials through Tokio
`UnixStream::peer_cred`, and returns only the mapped source. Numeric identity
containers have no `Debug`. At source commit `b62d827`, the full host suite,
two byte-identical prototype archives, and byte-identical adapter/peer/nix
AArch64 rlib, metadata and ThinLTO pairs passed. No listener, collector account,
process, systemd unit or capability exists; see
`baselines/diagnostic-peer-adapter.md`.

Checkpoint: the unlinked `bli-kvm-system-action-policy` now models only semantic
reboot/shutdown requests with local-admin/reauthentication/confirmation, boot,
update, storage, recovery and post-boot guards. Separate prepared/dispatching
snapshots, crash ambiguity, terminal replay, bounded history, startup/cooldown
and post-boot acceptance fail closed. At source commit `6c75377`, the full host
suite, two byte-identical prototype archives, two byte-identical AArch64 rlibs,
metadata and ThinLTO codegen passed. There is no durable store, actuator,
runtime link or enabled capability; see `baselines/system-action-policy.md`.

Checkpoint: the unlinked `bli-kvm-system-action-journal` now encodes policy
state into one strict fixed-size format with domain-separated SHA-256, binds
journal and policy generations, selects redundant copies with split-brain,
gap and external-floor hard stops, and plans non-selected-first writes with
expected-generation/readback checks. At source commit `b9106ab`, the full host
suite, two byte-identical prototype archives, policy/journal AArch64 rlibs,
metadata and ThinLTO codegen passed. SHA-256 is corruption detection only; no
filesystem adapter, lock, sync, actuator or enabled capability exists. See
`baselines/system-action-journal.md`.

Checkpoint: the unlinked `bli-kvm-system-action-store` now owns one compile-time
root and fixed A/B names, rejects unsafe identity/mode/ACL/type/link/size state,
locks the directory FD, uses relative no-follow/nonblocking opens, and completes
file sync, exact pre/post-rename readback, atomic rename and directory sync for
the non-selected copy before the selected copy. Fifteen host fault/security
tests pass. An initial AArch64 rmeta mismatch was traced to and removed with the
direct `thiserror` dependency; at final source commit `5a764fd`, clean archive,
policy, journal and store rlib/rmeta/ThinLTO pairs are byte-identical. No root
was provisioned and no runtime was linked; see
`baselines/system-action-store.md`.

Checkpoint: the exact static-PIE store test harness subsequently passed all 15
tests natively on H616 inside one marker-protected `/tmp` stage. The production
root remained absent, rootfs stayed read-write, KVMD services stayed active,
and remote/local staging was removed. This accepts temporary-filesystem syscall
behavior only, not production provisioning, an external rollback floor,
systemd confinement, power-cut durability or any system action. See the native
section in `baselines/system-action-store.md`.

Checkpoint: the unshipped maintenance binary now exposes only
`initialize-empty` and `inspect-empty`; no repair, path, floor override or
action exists. Exact mode-0700 tmpfiles and two static root-only one-shot units
are host-verified with network/device/capability denial and one exact writable
path. At source commit `b455f06`, two byte-identical AArch64 maintenance
binaries/rlibs plus lower-layer rlibs and prototype archives passed. No H616
production root was created; see `baselines/system-action-maintenance.md`.

Checkpoint: the exact maintenance binary, runtime units and tmpfiles source
then passed a ten-minute-watchdog H616 gate. Empty generation one initialized
and inspected under systemd; two canonical copies matched. Marker/hash/metadata
rollback restored the original absence of production root, binary parent,
binary, runtime units, stage and watchdog while rootfs stayed read-write and
KVMD stayed active. External floor, nonempty state, power cut and every system
action remain unaccepted; see `baselines/system-action-maintenance.md`.

Checkpoint: pinned kernel/DTB plus live metadata show no usable TPM, TEE/OP-TEE
or eMMC RPMB interface. Generic TPM kernel support has no bound device, TEE is
disabled, and the single MMC block exposes no RPMB node. No device or identifier
was read and no key/object was provisioned. A same-filesystem value is not an
independent floor, so reboot/shutdown remain disabled; see
`baselines/trusted-floor-topology.md`.

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
