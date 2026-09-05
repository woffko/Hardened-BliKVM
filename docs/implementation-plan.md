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

Checkpoint: source commit `6de71c2` adds an unlinked in-memory runtime core to
the safe peer adapter. An opaque non-cloneable authenticated token is consumed
into an opaque instance-bound connection; one exact producer frame is decoded,
submitted to the two-window policy, and answered with one canonical response.
Invalid frames receive no response and terminate through EOF accounting;
foreign-core handles fail without being consumed, while policy/backpressure/
unavailable responses keep the connection usable. The full host suite with 122
Python tests and two byte-identical 4,780,015-byte prototype archives passed,
as did direct adapter/window/protocol/peer/collector/export/nix AArch64 rlib,
metadata, and ThinLTO comparisons. No listener, real clock, account, process,
systemd unit, persistence, API, UI, package, device gate, or capability exists;
see `baselines/diagnostic-runtime-core.md`.

Checkpoint: source commit `8905093` adds a single-use transient listener owner
around that core. It accepts only a caller-bound Unix listener, authenticates
every stream before reading, owns an OS-random instance binding and `Instant`
clock, samples time under the core mutex, bounds partial frames and responses,
reaps tasks before accepts, and joins lifecycle cleanup on shutdown. Twenty-four
adapter/listener tests, the full host suite, RustSec scan, and two byte-identical
4,780,338-byte AArch64 archives passed, as did direct listener/adapter and every
lower rlib/metadata/ThinLTO comparison. No socket path, account, process,
systemd unit, producer integration, persistence, API, UI, package, device gate,
or capability exists; see `baselines/diagnostic-listener-runtime.md`.

Checkpoint: collector source `632c081`, gate source `9997c5b`, and final gate
source `28d189a` add the only allowed adapter consumer: an unshipped fixed-socket
collector, hardened non-installable systemd unit, separate sysusers candidate,
and marker/watchdog H616 harness. The host matrix passed with 133 Python tests;
two AArch64 archives and the 666,032-byte collector were byte-identical. The
native gate accepted all five sources, rejected root/unknown/duplicate peers,
enforced partial/malformed behavior, closed an acknowledged live connection on
graceful shutdown, and restored all account DB files and staged paths. DynamicUser-only
identity lookup is rejected on this H616 because `getent` cannot resolve it;
temporary exact sysusers plus hash-guarded restoration passed instead. No
collector artifact is shipped, no raw log was read, and no API/UI/capability is
enabled; see `baselines/device-diagnostic-collector.md`.

Checkpoint: source `49173a7` adds an unlinked fixed-path Rust producer client.
It serializes one persistent stream, bounds connect/write/response, preserves
all typed rejection reasons, and distinguishes definitely-not-sent failures
from post-write delivery ambiguity. Ambiguous or cancelled exchanges discard
the stream and never retry automatically; only a later explicit send may
reconnect. Eleven focused tests, the host matrix with 134 Python tests, two
byte-identical 4,780,599-byte archives, and byte-identical producer/protocol/
export AArch64 rlib, metadata and ThinLTO pairs passed. No shipped process,
unit, package, API/UI or capability links the client; see
`baselines/diagnostic-producer-client.md`.

Checkpoint: source `5ccd68b` adds an unlinked non-blocking reporter above the
producer client: eight fixed slots, `try_lock` submission, typed drop receipts,
one ordered worker, no retry, saturating semantic counters, and bounded
drain/abort shutdown. The complete 19-test crate passed twelve repeated runs,
the host suite with 134 Python tests, two byte-identical 4,781,022-byte
archives, and byte-identical updated AArch64 rlib/metadata/ThinLTO pairs. No
service or package links it; see `baselines/diagnostic-producer-reporter.md`.

Checkpoint: source `a143a75` links that reporter only into the denial-only
control broker. One fixed capability-disabled event is attempted after the
response write; reporter construction and every receipt are fail-open for
diagnostics and cannot change the denial. Five focused broker tests, including
all queue receipts and a deliberately blocked diagnostic callback, passed 20
consecutive runs. Follow-up `bc2e5f0` removes the broker's direct path-dependent
error derive. The full host matrix, RustSec scan, two byte-identical
4,795,272-byte archives, all six shipped ELF pairs, and the updated broker
ThinLTO pair passed. Only non-shipped Rust metadata remains root-dependent
through the existing Serde/core graph. Gate source `e4f12ed` then accepted the
exact control event through the real collector identity policy and 32 bounded
denials during a seven-second collector-response stall. The browser regression
and complete rollback passed at bundle/documentation source `34187aa`; see
`baselines/device-diagnostic-control.md`.

Checkpoint: source `6189f5b` adds a pure unlinked diagnostic export-v1 codec
with an
exact 16-byte read/ack request and a canonical 24-byte response header with at
most 8 KiB of typed bundle payload. Snapshot and acknowledgement now carry the
collector's nonzero instance binding as well as the window ID, preventing stale
pre-restart acknowledgements from clearing a new window with the same numeric
ID. Five codec tests plus the existing 35 window/runtime tests, the full host
matrix with 137 Python tests, two byte-identical 4,796,389-byte archives, and
byte-identical export-protocol/window/adapter/export rlib, metadata and ThinLTO
pairs passed. No export listener, consumer identity, persistence, client,
API/UI, package, or device gate exists yet; see
`baselines/diagnostic-export-protocol.md` and `api/diagnostic-export-v1.md`.

Checkpoint: source `19a750f` adds a separate unlinked export listener that
accepts only a
caller-bound Unix listener and the semantic platform-broker role returned by
the existing fixed-name/`SO_PEERCRED` adapter. It uses two non-waiting permits,
fixed two-/five-second request/response bounds, one exchange per connection,
single-use ownership, and bounded task joins. Nine listener tests plus the
updated 25 peer-adapter tests cover all response mappings, invalid/partial and
unauthorized peers, connection pressure, stalled snapshot/ack responses,
runtime-lock release, ambiguous acknowledged writes, and shutdown. No collector
process, fixed export path, client, persistence, API/UI, package, or device gate
links it. The full host matrix with 138 Python tests, two byte-identical
4,796,617-byte archives, and byte-identical listener/adapter/window/protocol/
export rlib, metadata, and ThinLTO pairs passed; see
`baselines/diagnostic-export-listener.md`.

Checkpoint: the unshipped collector atomically binds fixed
`collector.sock` and `export.sock` paths and runs producer plus authenticated
export loops over one `Arc<DiagnosticCollectorRuntime>`. Completion or panic of
either task broadcasts shutdown through a drop guard; both tasks are joined
before exact inode cleanup. Five collector tests cover dual-socket modes,
partial-bind rollback, replacement refusal, shutdown broadcast including
unwind, and typed task/runtime failure mapping. No bundle, client, persistence,
API/UI, or product capability exists. Gate source `a17b867` plus hardened final
source `437b65f` passed the full host matrix, two byte-identical 4,796,918-byte
archives, an identical 715,032-byte collector, all export-stack rlib pairs, and
the reversible H616 exact-consumer/pressure/repeat/mismatch/ack/shutdown matrix
with complete rollback; see `baselines/device-diagnostic-export.md`.

Checkpoint: source `3f1984d` adds a separate unlinked fixed-path Rust export
client. It performs one fresh two-/two-/five-second bounded connection/write/
response exchange per call, requires exact EOF framing, owns at most 8 KiB of
typed payload, and exposes nonzero instance/window bindings. Read failures are
retryable only by an explicit caller because reads do not mutate state;
acknowledgement failures and cancellation after polling are conservatively
delivery ambiguous and never retried. Thirteen tests cover every response,
maximum payload, malformed/partial/oversize/EOF/read/write/connect/time bounds,
wrong binding/operation, cancellation, and no hidden retry. The full host
matrix with 139 Python tests, two byte-identical 4,797,360-byte archives, and
byte-identical client/protocol/export AArch64 artifacts passed. It remains
outside the platform broker, bundle, persistence, API, UI, package, and device
runtime; see `baselines/diagnostic-export-client.md`.

Checkpoint: final source `3461d77` adds a default-off diagnostic integration
across the broker protocol/client and Platform Broker. Inventory remains schema
v1; the unshipped feature uses read-only schema v2 and rejects cross-version
request/response types. It re-parses every returned bundle into the closed
semantic model, authenticates the exact Server peer through the existing
six-distinct-UID/shared-GID policy and `SO_PEERCRED`, maps only bounded outcomes,
and contains no acknowledgement call. Default and all-feature host matrices
with 140 Python tests passed. Two independent AArch64 builds produced identical
default bundles and default/feature final ELFs; upper Serde metadata retained
known target-root differences while every ThinLTO member matched. Live H616
proved startup without collector, typed unavailable, two identical exact-Server
reads, root/wrong-role denial, direct-frame equality, no acknowledgement, and
complete rollback. The feature is absent from the prototype, server, HTTP/UI,
capabilities, persistence, and install units; see
`baselines/device-diagnostic-broker.md` and `api/broker-v1.md`.

Checkpoint: source `11ee117` adds the unlinked pure
`bli-kvm-diagnostic-journal`. Two fixed 16,552-byte XChaCha20-Poly1305 copies
hide count/length for at most two canonical windows and authenticate format,
algorithm, key ID, generation, nonce, and fixed ciphertext size. The active and
one previous key ID support bounded rotation; new writes use only the active
key and reject retained nonce reuse. A/B selection, non-selected-first update,
copy repair, generation gap/split-brain/rollback floor, stable format vector,
tamper/wrong-key and fixed retention tests pass. The reconciliation planner can
authorize ack only from authenticated selected state, persists ambiguous state,
requires reread before retry, and marks unprovable absent/restarted state
`orphaned`. The full host matrix with 141 Python tests, RustSec and independent
AArch64 archive/journal/RustCrypto artifact comparisons passed. No filesystem,
RNG, key, broker link, acknowledgement I/O, HTTP/UI, package, or device gate
exists; see `design/diagnostic-journal-v1.md` and
`baselines/diagnostic-journal.md`.

Checkpoint: source `2efe1f7` adds the unlinked
`bli-kvm-diagnostic-store`. Its production constructor accepts no path and
opens only the fixed mode-0700 broker-owned diagnostics root. A pinned,
exclusively locked no-follow directory descriptor, four-name allowlist,
same-device/single-link/mode-0600/ACL checks, `openat`/`renameat`, authenticated
readback, file and directory sync, and exact post-write reselection protect the
two encrypted copies. `OsRng` supplies one fresh nonce per initialization or
update; keys remain typed caller-owned values and are neither loaded nor
retained. Eighteen host security/fault tests, the full 142-Python-test matrix,
RustSec, and two byte-identical AArch64 archive/store/journal rlib, metadata and
ThinLTO builds passed. The production root, key credential, external rollback
floor, broker/collector link, acknowledgement I/O, HTTP/UI and package remain
absent; see `design/diagnostic-store-v1.md` and
`baselines/diagnostic-store.md`.

Checkpoint: source `b3266c3` adds the unlinked
`bli-kvm-diagnostic-key-loader` and rejects duplicate journal key material even
under distinct IDs. One canonical 96-byte binary envelope carries active plus
fixed optional previous slots. The argument-free production API accepts only
the standard systemd environment directory when it equals one of two exact
unit paths, opens one fixed filename through pinned no-follow descriptors,
requires exact identity/mode/type/link/ACL/length state, and compares two reads
in constant time. Raw and typed keys zeroize and expose no payload-bearing
`Debug`, error, serializer, generator, or path API. Ten loader tests, twelve
journal tests, the full 143-Python-test matrix, RustSec, and two byte-identical
AArch64 archive/loader/journal rlib, metadata and ThinLTO builds passed. No
credential source, production key, root, maintenance binary/unit, store call,
broker/ack linkage, HTTP/UI or package exists; see
`design/diagnostic-key-credential-v1.md` and
`baselines/diagnostic-key-credential.md`.

Checkpoint: source `dfb47a2` adds the unshipped
`bli-kvm-diagnostic-maintenance`, a nonmutating store read path, exact
mode-0700 tmpfiles root, and separate non-enabled init/inspect units. Official
systemd 249 source review corrected the loader to accept either its exact
root-owned service-UID ACL layout or no-ACL ownership fallback and narrowed key
authority to the two action units. The CLI exposes only `initialize-empty` and
`inspect-empty`; init alone receives one writable path, while inspect rejects
and preserves crash temporaries and degraded state. The full 144-Python-test
source matrix, RustSec, two byte-identical AArch64 maintenance executables plus
archive/maintenance/loader/store/journal artifacts passed. The rollback harness
then reached 154 Python tests and a reversible H616 gate at job
`286a6e60c60d4720964eaf8556a6021b`: real systemd credential delivery, two
equal encrypted empty copies, nonmutating inspection, temporary rejection, and
complete account/path/service rollback passed without Power/Reset. Production
key/source ceremony, external floor, live acknowledgement, HTTP/UI, packaging,
soak/reboot and release remain absent; see
`design/diagnostic-maintenance-v1.md` and
`baselines/diagnostic-maintenance.md`.

Checkpoint: the unlinked `no_std` `bli-kvm-diagnostic-coordinator` now composes
fresh typed collector observations with an abstract journal repository and a
caller-supplied nonzero external rollback floor. Every mutation is followed by
a clean authenticated re-selection; the external floor must catch up before a
fresh observation can establish readiness, and a second immediate selection
is required before one linear acknowledgement permit is issued. Not-sent and
mismatch outcomes never mutate or retry. Delivery ambiguity, collector
unavailability during acknowledgement, dropped permits, partial writes, and
post-write errors fail closed through explicit recovery, while degraded copies
require separate repair. The crate contains no concrete store/client link,
key, runtime, timer, background task, HTTP/UI, unit, package, or enabled
feature; see `design/diagnostic-coordinator-v1.md`.

Source `945eb19` passed 26 focused coordinator tests, the complete host matrix
with 157 Python tests, a 1,233-advisory RustSec scan, and two independent
byte-identical AArch64 archive/rlib/rmeta/ThinLTO builds. Because coordinator
methods are generic, two additional fully instantiated static-PIE AArch64 test
executables were built and also matched byte-for-byte. They were not run on the
device, and the source remains unlinked; see
`baselines/diagnostic-coordinator.md`.

Checkpoint: the unlinked `bli-kvm-diagnostic-persistence` library now composes
only the accepted loaded-key, fixed-store, fixed-export-client, and coordinator
types. One bounded poll performs one read and at most one permitted
acknowledgement with exact not-sent/mismatch/unavailable/ambiguous mapping.
Read cancellation is nonmutating; acknowledgement cancellation forces durable
recovery before a later read. The adapter uses nonmutating store selection,
adds no path/configuration/retry/background task, and remains absent from every
broker, executable, unit, package, HTTP route, UI, and prototype payload; see
`design/diagnostic-persistence-v1.md`.

Source `57959b0` passed 19 focused adapter tests, the complete host matrix with
158 Python tests, a 1,233-advisory RustSec scan, and two independent
byte-identical AArch64 archive/rlib/rmeta/ThinLTO builds. Two fully instantiated
static-PIE AArch64 test executables also matched byte-for-byte. They have not
yet run on H616, and no live key/store/socket was used; see
`baselines/diagnostic-persistence.md`.

Checkpoint candidate: the unshipped zero-argument
`bli-kvm-diagnostic-persistence-gate` now scripts two typed sealed windows
through the real fixed loader/store/client/adapter stack. It plans generations
one through seven, deliberately cancels one stalled acknowledgement, requires
ambiguity recovery without a read, and emits only nine fixed semantic facts.
The accompanying H616 orchestrator uses a synthetic credential/floor,
marker/hash/account-CAS rollback, a ten-minute watchdog, and no Power/Reset or
KVMD restart. Source `7bef2bc` passed four Rust and eleven orchestration tests,
the complete 170-Python-test host matrix, RustSec, two byte-identical AArch64
gate/bundle builds, and rollback-gated live H616 execution through generation
seven with complete cleanup; see `design/diagnostic-persistence-gate-v1.md`
and `baselines/device-diagnostic-persistence.md`.

Checkpoint candidate: `bli-kvm-core` now defines a closed diagnostics-view v1
derived only from validated runtime inventory, and the server exposes it at
authenticated GET-only `/api/diagnostics`. Temperature, RTC and fixed component
observations are current-only; missing values are explicit and persistent
history is fixed `persistence_not_provisioned`. The server links no diagnostic
key/store/floor/export-client/acknowledgement authority. Source `f0973ce`
passed 17 core tests, 27 server tests, the complete 171-Python-test host matrix,
RustSec, two byte-identical AArch64 server/archive builds, and a reversible H616
auth/privacy/current-health gate after two fully rolled-back assertion
refinements. See `api/diagnostics-v1.md` and
`baselines/device-diagnostics-api.md`.

Checkpoint: the branded web interface now has a separately navigable,
responsive `Diagnostics` tab backed only by authenticated GET
`/api/diagnostics`. A closed runtime validator gates temperature, RTC, video,
HID, power observation, virtual media, power LED, and explicitly unprovisioned
history. Generation-based request ownership aborts the prior GET before manual
or scheduled refresh; hidden/unmounted views stop, and errors do not start an
automatic retry. Twenty-three strings have exact tuple parity across all 26
canonical locales. Source `8339018` passed the complete 172-Python-test host
matrix, two byte-identical AArch64 archives, and a reversible H616 TLS Chromium
gate covering desktop, 390x844 `zh_tw`, loading/unavailable/stale/error,
session revocation, video, privacy, and full rollback with no control request
or Power/Reset action. See `baselines/device-diagnostics-ui.md`. Permanent
packaging, production history/key/floor custody, soak, reboot, other browser
engines, and release acceptance remain.

Checkpoint: the first permanent-format package slice is a deterministic inert
`arm64` Debian package. A read-only H616 preflight confirmed Jammy/dpkg/systemd,
clean production paths/accounts/units, free loopback ports, and sufficient
space. The AArch64 build now remaps project/Cargo/Rust source roots and rejects
host paths in every shipped ELF. Two independent remapped archives and two
independent `.deb` files matched byte-for-byte. A five-minute-watchdog H616
gate installed the package, verified all 38 file hashes/modes/root ownership
with units inactive and accounts/config/state absent, then purged it and
confirmed complete functional cleanup while retaining normal dpkg audit
history. A separate two-phase gate then synchronized early-boot and transient
rollback before reinstalling the exact package, repeated the closed-tree,
file-capability, mode/owner, package-database, account/Nginx/KVMD, unit and port
checks, disarmed rollback only after revalidation, and deliberately retained
the inactive package. A new SSH session independently confirmed that state
through a read-only inspector. See
`baselines/device-debian-runtime-package.md`. Account/credential provisioning,
service activation, Nginx integration, soak, reboot, update trust, and release
acceptance remain.

Checkpoint: the retained package now has its complete fixed sysusers identity
set and a separate device-generated KVMD credential. Two-pass synthetic ID
prediction, all account and auxiliary-file hashes/metadata, exact htpasswd-line
rollback, CSPRNG readiness, same-filesystem root-only credential publication,
early-boot rollback, timed watchdog, marker-bound recovery, and independent
read-only inspection passed on H616. The first live attempt exposed a
23-versus-24-byte username validation defect; the bounded recovery path restored
the package-only state before the fixed gate was rerun. All product services
remain disabled/inactive and ports remain free. See
`baselines/device-runtime-provisioning.md`. Loopback activation, Nginx/TLS,
soak, reboot, update trust, and release acceptance remain.

Checkpoint: the installed platform broker, MJPEG proxy, and loopback server now
pass a complete reversible native activation gate using the retained package,
permanent identities, and dedicated KVMD credential. The gate verified exact
systemd users/groups/credentials/sandboxes, zero capabilities, network
namespaces, loopback-only TCP, no UDP/abstract sockets, absent control
service/socket, authenticated inventory/Diagnostics, MJPEG reconnect, session
revocation, empty cgroups, and complete state/drop-in/private-artifact cleanup.
Two fail-closed runs refined revocation phase reporting and the mandatory
logout Origin header; both restored the provisioned-inactive state. See
`baselines/device-runtime-activation.md`. Persistent start/enablement,
production Nginx/TLS, soak, reboot, update trust, and release acceptance remain.

Checkpoint: the same installed units now pass the full established
desktop/mobile Chromium flow behind an unprivileged transient Nginx at only
`127.0.0.1:18443`, reached exclusively through strict SSH forwarding. TLS
1.2/1.3, secure cookies, setup/login/password rotation/logout, installed
inventory/Diagnostics, responsive localized UI states, MJPEG reconnect/session
revocation, zero control/WebSocket requests, unchanged PiKVM Nginx, empty
cgroups and complete certificate/state/rollback cleanup passed. See
`baselines/device-installed-tls-browser.md`. Production certificate/listener
enrollment, persistent app account/service activation, soak, reboot, update
trust, and release acceptance remain.

Checkpoint: the persistent web listener cutover is now accepted on the test
H616. A synchronized early-boot rollback and ten-minute watchdog protected the
transaction that disabled only `kvmd-nginx`, enabled the installed Rust
broker/video/server and split-privilege Hardened Nginx, reused the unchanged
device TLS pair, and moved exact wildcard 80/443 ownership to the Hardened
frontend. TLS 1.2/1.3, redirect, loopback isolation, capability bounds,
account-free First Setup, zero control/WebSocket requests, strict-forwarded and
direct-LAN Chromium, rollback removal, and an independent final SSH check
passed. See `baselines/device-persistent-web-cutover.md`. Operator first-account
creation, soak, guarded reboot persistence, update/recovery and release
acceptance remain.

Checkpoint: the operator-created test account passed direct-LAN Chromium
login, authenticated inventory/Diagnostics, secure-cookie and logout checks
without changing the account or retaining a session. The persistent runtime
then passed a 30-sample, ten-second-interval read-only soak with stable service
PIDs/restart counters, bounded cgroup RSS, 42.101–49.634 degrees Celsius,
continuous exact 80/443 ownership, 30 health/setup checks, 90 expected
unauthenticated denials, and TLS 1.2/1.3 before and after. No persistent write,
device reboot, or target Power/Reset action occurred. See
`baselines/device-persistent-account-soak.md`.

Checkpoint: a real guarded software reboot of the test BliKVM now passes. The
first attempt failed closed because two enabled read-only units used one-shot
`ConditionPathExists` checks for KVMD runtime sockets; systemd skipped them just
before KVMD published the paths, and the guard restored vendor web. Source
`531f5f1` removed only those transient conditions and produced two
byte-identical AArch64 bundles plus two byte-identical `0.1.0~dev2` packages.
A package-aware early-boot/timed rollback gate upgraded the persistent device
while preserving its account, TLS, configuration, and exact revision 1
rollback package. Source `79ff492` then bound reboot acceptance to revision 2
and the corrected unit semantics. The changed boot ID, service persistence,
TLS 1.2/1.3, exact listeners, account hash, zero cutover orphans and guard
cleanup passed; source `9350739` subsequently passed post-reboot Chromium
login, inventory/Diagnostics, four MJPEG responses, decode/reconnect, logout
stream revocation, zero control/WebSocket requests, and an independent final
zero-restart audit. See `baselines/device-runtime-upgrade-reboot.md`.
Repeated/interrupted reboot, power-cut, production update/recovery and release
acceptance remain; target power controls stay excluded.

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

Checkpoint: the Rust application-update process boundary is now also packaged
separately from the working runtime. `hardened-blikvm-update-runtime` contains
only the supervisor/reporter, nine disabled systemd templates, public
specifications, and documentation-only sysusers/tmpfiles examples; it depends
exactly on runtime `0.1.0~dev2` and carries no trust/configuration/state/slot or
route. Source `17d8562` produced two byte-identical canonical-mode `.deb`
files. A package-aware early-boot/timed rollback gate then retained the exact
package on H616 while keeping every update unit inactive/not-enabled, all
update authority absent, the persistent service PIDs/restart counters and
account/Nginx/TLS hashes unchanged, and the browser login/Diagnostics/decoded
MJPEG flow healthy. An independent receipt-bound SSH verification repeated the
state without mutation. See `baselines/device-update-runtime-package.md`.
Production trust enrollment, state/slot initialization, activation,
interruption/reboot and update GUI remain separate gates.

Checkpoint: source `0ee863d` adds a pure Rust initial application-trust
enrollment contract. Its closed public-only request binds product, target,
generation 1, null replacement, verifier-compatible Ed25519 key ID/point, and
the SHA-256 fingerprint of the exact public bytes. It rejects private-key,
path, timeout, sequence, URL, command, rotation, and unknown fields, then
renders only the existing supervisor schema with compile-time paths, initial
system sequence, and health timeout. The complete host matrix and two
independent AArch64 workspace builds passed; the 118,044-byte ThinLTO member
was byte-identical. See `baselines/application-trust-enrollment.md`. No real
production key or device configuration was created. Reviewed public-key
provisioning with rollback remains next.

The companion source `14a8dc8` CLI reads only the bounded request from stdin,
requires its exact SHA-256 plus explicit reviewed-public-key confirmation, and
writes only canonical config to stdout. Two independent 503,800-byte AArch64
static-PIE binaries matched byte-for-byte. The standard target builder verifies
it, but it is intentionally absent from shipped runtime packages; a later
provisioning gate may use it only as a transient validator.

Checkpoint: source `c26f0a8` added the guarded public-trust provisioning gate,
and source `3721e3a` added an interactive non-overwriting encrypted Ed25519 key
ceremony helper. The operator separately backed up and explicitly reviewed the
public request for `app-production-2026-09-01`. A read-only H616 preflight then
passed before a rollback-gated run atomically published only the canonical
mode-0600 supervisor configuration. Independent final verification found all
guards/staging removed, state/slots/runtime absent, update units inert, and the
persistent runtime, account, Nginx, TLS, PIDs, and restart counters unchanged.
No private key, restart, reboot, or target power-control action entered the
gate. See `baselines/device-application-trust-enrollment.md`. State/slot
initialization remains the next separate application-update authority gate.

The first encrypted key passphrase was subsequently unavailable before any
state or slot existed. Source `091866a` added an explicit owner-only
passwordless-key policy, and source `14562c2` added a narrowly bounded
unused-trust removal gate. The old exact config was removed only after a
read-only H616 preflight proved it unused; independent final verification then
proved the no-trust state. A separately backed up and reviewed passwordless
`app-production-2026-09-05-01` request passed another read-only preflight and
ordinary rollback-gated enrollment. The new mode-0600 config is persistent,
all guards/staging are absent, state/slots/runtime remain absent, update units
remain inert, and the running application is unchanged. The retired encrypted
key files were not read, modified, or deleted. State/slot initialization is
still the next gate, but work is paused before production bundle signing. A
late terminal process produced one bundle under the retired key; it was never
uploaded and was moved to an explicit `retired-old-key` staging directory so it
cannot be confused with the future current-key bundle.

Resumed checkpoint: the first sequence-one bundle under the current passwordless
key has now been built twice from the independent accepted dev2 packages.
Both archives match, and the real host Rust supervisor passed staging,
idempotency, immutable-payload verification, generation-one initialization,
pointer/status checks, and tamper/wrong-key rejection. See
`baselines/production-application-bundle.md`. Before live initialization,
resolve the root-only config versus unprivileged updater access mismatch and
verify configuration delivery under the actual service identity. Device
state/slots have not been initialized by this host-only step.

Configuration delivery is now implemented in source through systemd
credentials with read-only mount and ownership checks in the Rust loader.
The new local `validate-config` command validates the public config without
creating state. Runtime verification under the service identity, AArch64
rebuild and replacement of the inert updater package must pass before live
state/slot initialization.

The transient AArch64 credential check now passed on H616 under the actual
updater UID: direct root-only config access failed, systemd credential delivery
succeeded, temporary services and files were cleaned, and existing runtime and
trust remained unchanged. The dev2 update-runtime package is being prepared;
the installed dev1 package has not yet been replaced.

The dev2 updater package is now retained on H616 after successful read-only
preflight, real dev2-to-dev1 rollback rehearsal, and a second guarded upgrade.
Independent final checks prove package files, preserved trust, unchanged serving
runtime, inert update services and complete guard cleanup. The next task is
installed-supervisor rehearsal with the actual signed bundle, followed by
persistent state/slot initialization.

Installed-supervisor rehearsal with the actual current-key bundle now passed
on H616, including staging, initialization, pointer/status, all 27 installed
payload files, repeat-stage idempotency and payload tamper rejection. Fixture
group ownership was corrected to match systemd's explicit shared group;
temporary roots were removed and the serving runtime/trust rechecked. The
remaining initial-state gate must publish only identity-bound prepared roots
and quarantine those same roots on rollback before any activation work.

The initial-state implementation now includes the no-overwrite root transition
helper, native root controller with early-boot/timed recovery, and host
preflight/rehearsal/retention orchestration. Filesystem interruption tests pass.
Persistent-path device rehearsal and retaining initialization remain pending;
the existence of this source is not device acceptance.

Persistent-path acceptance now passed on H616: a read-only preflight, real
quarantine rollback rehearsal, then retaining initialization with independent
post-cleanup status verification. The signed initial slot and generation-one
journal are persistent; update/current/candidate units are still disabled.
The GUI continues to run from the package-backed service. See
`baselines/device-application-bootstrap.md`. Next is guarded serving-service
cutover to the signed slot, including decoupling Nginx from the old server's
Requires dependency before any server transition.

Signed-slot serving is now retained after guarded rehearsal, browser acceptance,
explicit restoration of the old server, and a second verified switch/commit.
The original server is disabled; Nginx is independent of backend lifetime and
kept its PID. Login, Diagnostics, decoded MJPEG, reconnect and logout revocation
passed. See `baselines/device-signed-slot-serving.md`. Before enabling the
watchdog timer, replace unconditional apply-service restarts with idempotent
reconciliation; clean two-second watchdog ticks must not restart the GUI.

Updater dev3 source now uses a bounded generation/slot apply request on health
confirmation or recovery, followed by a matching acknowledgement after the
fixed service action. Clean ticks create no request and candidate application
is dispatched only by explicit activation. Unit and CLI tests cover stale ACKs
and repeated clean ticks. Native isolated systemd verification now passed,
including confirmation, failed-apply request retention, retry and fifteen
clean ticks without restarts. Exact-unit rerun preserved the real serving
runtime and removed all fixture/controller staging. See
`baselines/device-update-apply-events.md`. Reproducible dev3 packaging and a
state-preserving guarded upgrade are now accepted, including an explicit
dev3-to-dev2 rollback rehearsal and a second retaining installation. See
`baselines/device-updater-dev3.md`. The watchdog and confirmation socket remain
disabled. Candidate post-command credential access and the complete isolated
packaged-template activation/confirmation/timeout rollback chain now passed;
see `baselines/device-candidate-health.md` and `baselines/device-activation-flow.md`.
Updater dev4 is now reproducibly built, rollback-rehearsed and retained;
see `baselines/device-updater-dev4.md`. Idle installed updater enablement is
also retained after explicit disabling/restoration and a second verified
enablement: `baselines/device-updater-infrastructure.md`. Next are the
production-key application update flow and browser/recovery acceptance.

The installed production-key sequence-two update now passed, including a real
health-timeout return to slot one, phase-bound browser acceptance, normal
confirmation to floor two/generation seven and independent guard cleanup.
See `baselines/device-installed-application-update.md`. Next add mandatory
boot-time journal/slot verification before starting current/candidate servers,
then prove confirmed and interrupted-update reboot behavior. The signed-update
API/GUI and remaining hardware-adapter work are not completed by this gate.

Mandatory startup verification passed isolated missing-trust and tampered-tree
rejection/restore tests. Updater dev5 is now retained after a full dev5-to-dev4
rollback rehearsal, preserving operational sequence two and enabled updater
services (`baselines/device-updater-dev5.md`). Next prove one real clean reboot:
new boot identity, unchanged persistent state, verification completion before
server startup and browser behavior. Interrupted-update reboot remains separate.

The clean operational reboot and subsequent browser check passed
(`baselines/device-operational-reboot.md`). Next exercise a real pending
sequence-three candidate across software reboot, requiring boot-ID-change
rollback to confirmed sequence two; see
`design/interrupted-application-reboot-v1.md`. No abrupt-power-loss claim is
made by either reboot gate.

Reboot during a real sequence-three health window now passed, restoring
sequence two with `boot_changed`, floor two and generation ten. Browser and
cleanup acceptance also passed (`baselines/device-interrupted-update-reboot.md`).
Next connect the verified engine to authenticated API/GUI through separate
bounded status, upload and activation IPC boundaries. Status polling must never
dispatch apply services; writable update capability stays unavailable until
the complete mutation path is implemented and accepted.

The first network-integration slice now has a closed bounded status protocol,
four-request-limited Unix client, kernel-UID-checked read-only worker, protected
HTTP status route and a separate Bli status pane. Counters remain exact decimal
strings in HTTP/GUI, unsupported legacy routes are not called, and status has
no write paths or recovery/apply dependency. Full host checks and the temporary
native socket-worker gate passed after fixing a duplicate slot-lock acquisition
(`baselines/device-update-status-ipc.md`). Installed API/browser acceptance and
deployment remain required. Signed upload
and activation from the GUI are the following slices, not completed features.

Updater dev6 now includes the read-only status worker and socket and is retained
after an exact-package rollback rehearsal, preserving signed slot two and all
serving state (`baselines/updater-dev6-package.md`). Status transport is still
disabled. The updated signed server/UI and configured HTTP/browser acceptance
remain the next steps; an updater-only package upgrade does not supply them.

The signed application is now confirmed at sequence five after the GUI
notification lifecycle fix. The complete temporary installed status API/GUI
cycle passed on desktop/mobile, including authenticated exact counters,
Refresh, restoration, logout and independent cleanup verification
(`baselines/status-api-seq5-acceptance.md`). Persistent status wiring is now
retained, with corrected early-boot restoration, updater dev7 installation,
automatic post-reboot startup and desktop/mobile status/browser acceptance
(`baselines/persistent-status-acceptance.md`). Longer resource observation,
status-unavailability behavior and signed upload/activation GUI remain next.

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
