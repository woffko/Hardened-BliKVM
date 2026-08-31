# BliKVM vendor-to-Rust migration map

Baseline: `blikvm/blikvm@3e34c04a1dfa84636050585d12d5e585098258ea`
(`v2.2.3-alpha`). Source-derived pin numbers and behavior are hypotheses until
the physical gate confirms them on the designated H616 board.

## Why this is a rewrite, not a wrapper

The vendor controller is a root service combining GPIO, ATX, fan, display,
button, RTC, serial switch, configuration, shared state, and supervision. Its
implementation uses world-writable files and sockets, `system()`/`popen()`,
shell globbing, fixed device names, unbounded string copies, detached infinite
threads, and inconsistent success returns. Installer scripts also use recursive
mode 0777, public video binds, raw partition operations, and broad configfs/HID
permissions.

Those behaviors are compatibility evidence only. Hardened-BliKVM does not run
the vendor daemon beside a second GPIO owner.

## Module mapping

| Vendor function | Rust target | Initial mode | Required gate |
| --- | --- | --- | --- |
| `blikvm_server` process/thread supervisor | systemd units plus `bli-platform-broker` | no hardware | crash/restart and dependency-order tests |
| JSON config reader | strict versioned Serde config | read-only parse | unknown-field, range, symlink, owner/mode tests |
| H616 GPIO mapping | immutable board profile | source evidence | line identity, polarity, exclusive ownership |
| ATX socket and GPIO pulses | typed ATX driver | disabled | bounded timing, busy/idempotency, LED polarity |
| fan loop and thermal sysfs | pure policy plus latched supervisor, then a separate driver | mock supervisor only; no wiring | pin/polarity/frequency/minimum-duty, crash/sensor/stall failure and PWM/temperature soak |
| button/activity LED loop | pure one-shot button and LED schedule policies, then separate drivers | two unlinked no_std policies; no I/O | input/output ownership, pull/polarity, physical bounce/press/visible timing, display integration and reboot |
| RTC shell commands | fixed kernel-RTC sysfs adapter first; ioctl only if later required | detection only | identity/rebind, drift, clock direction, battery and power-loss recovery |
| serial KVM switch | bounded pure frame codec before any TTY adapter | metadata absence plus unlinked no_std exact-frame codec | reviewed non-unique identity/routing, streaming, timeout, hotplug/reconnect, four-port matrix |
| OLED Python/C drivers | pure ST7789 transaction model, then bounded SPI worker | topology plus unlinked no_std model; disabled | exclusive SPI/GPIO handoff, initialization/electrical policy, timeout, crash/reboot and physical screen matrix |
| NetworkManager/IP/Wi-Fi shell routes | sanitized sysfs inventory, then transactional network policy and privileged adapter | metadata-only topology; no Rust adapter or mutation | typed eligibility, secret handling, commit/confirm watchdog rollback, out-of-band recovery and reboot matrix |
| system-info/reboot/hostname/log routes | separate sanitized adapters, guarded typed actions, and fixed semantic diagnostics | actions lack a trusted floor; collector and denial-only broker passed reversible H616 exact-event plus stalled-response/32-denial gates with no diagnostic control dependency | authenticated bounded export/UI, collector packaging, privacy/soak/reboot and separate hostname work |
| USB gadget shell scripts | idempotent configfs state machine | inventory only | bind/unbind rollback and host compatibility |
| public uStreamer command | loopback/Unix-socket media service | read-only proxy | reconnect, resolution, latency, soak |
| MSD/Ventoy shell bundle | typed configfs virtual-media worker | inventory only | upload/eject, BIOS boot, interruption recovery |
| partition/update scripts | signed atomic updater | unavailable | signature, traversal, space, power-cut rollback |

## H616 source map (not physically accepted)

- ATX header-pin mapping: power 18, reset 37, power LED 36, HDD LED 41.
- Fan header pin: 15.
- Live metadata: chip0 offset 269 is already `output active-high [used]` by
  consumer `sysfs`; `/sys/class/gpio/gpio269` is `out`, `active_low=0`. This
  proves existing logical ownership, not physical fan polarity.
- Button/activity mapping: button 12 and activity LED 32.
- Thermal source: `/sys/class/thermal/thermal_zone0/temp`.
- GPIO chip assumption: `/dev/gpiochip0`.
- RTC source hypothesis confirmed read-only: kernel-bound PCF8563 identity
  `0-0051`, exposed as `rtc1`; its clock was stale and `hctosys` was disabled.

The comments mix header positions and global GPIO numbers. Rust code must not
encode these values as production facts until line names/chip offsets are
confirmed on the test board. Prefer libgpiod line identity over deprecated
global sysfs numbering.

Read-only live line-info now confirms the original seven offsets plus the three
ST7789 source-associated controls. KVMD owns Power/Reset and both ATX LED
inputs; legacy sysfs owns display DC/backlight/reset, activity LED, and fan
outputs; display-button offset 257 is unused input. No line value was read.
These facts block concurrent ownership but do not establish electrical
polarity or physical behavior.

The H616 vendor `OLED` path is actually an ST7789 240x240 SPI LCD. Live metadata
confirms root-only `/dev/spidev1.0`, the `spidev` child and `sun6i-spi` master,
while GPIO259/260/262 are already used sysfs outputs. The vendor init cannot be
used for discovery because it opens/configures SPI, requests outputs, toggles
reset/backlight and transmits commands. See
`baselines/device-oled-topology.md`.

The unlinked Rust model now rejects invalid/empty 240x240 windows, emits typed
`0x2a`/`0x2b`/`0x2c` command and address-data phases, maps rotations, converts
RGB565 deterministically, and enforces exact bounded pixel counts. It contains
no initialization or I/O and does not relax the ownership blocker; see
`baselines/st7789-model.md`.

The vendor H616 button loop polls source-associated offset257 every 100 ms,
counts five high samples and can trigger repeatedly while held. The unlinked
Rust policy replaces sample counts with explicit monotonic debounce/hold
timing, requires a stable release before arming, emits once per press, and keeps
polarity configurable. No GPIO observation exists yet; see
`baselines/display-button-policy.md`.

The vendor activity output writes LOW for one 100-ms poll and then HIGH for 19
polls, regardless of external activity. The unlinked Rust LED policy makes
asserted level, period, and pulse explicit, starts and fails inactive, restarts
the phase on enable, and computes delayed phases without drift. It does not
write the used sysfs output; see `baselines/activity-led-policy.md`.

The pinned Node network routes mix interface state with addresses, gateways,
DNS, connection names, SSIDs and BSSIDs, then perform root shell mutation,
profile mode0777 changes, NetworkManager restart and connection cycling without
a watchdog rollback. Only fixed-candidate sysfs type/link/driver facts and
manager active state were retained in the sanitized live baseline; see
`baselines/device-network-topology.md`.

The system routes similarly combine bounded resource facts with hostname,
serial, MAC/IP and mutable configuration; reboot is an unconfirmed shell call,
hostname rewrites root files, and logs return raw content through an unquoted
shell path. Client shutdown has no matching audited server route. The unlinked
Rust policy now models typed reboot/shutdown eligibility, two-phase intent,
idempotent terminal replay, crash ambiguity, cooldown and post-boot acceptance,
while its separate pure journal adds canonical checksummed redundant copies,
generation/floor checks and torn-write ordering. Neither layer exposes an
actuator or public enabling path. The unlinked filesystem adapter now adds fixed
root/names, strict metadata and ACL checks, locking, atomic replacement, sync
and exact readback, but is unprovisioned and has no accepted external floor or
power-loss proof. Its complete store harness passed natively in an isolated
H616 `/tmp` stage without creating the production root or changing services.
An unshipped root-only CLI and static units now permit only empty initialization
and empty-state inspection. Their H616 watchdog gate restored every persistent
and runtime path to its original absence after both operations passed. All
actions remain unavailable. The separate unlinked diagnostic-export core
accepts only fixed semantic events and bounded relative offsets; it exposes no
raw message, path, identity or I/O and does not make the vendor log route safe.
One consumer is an unlinked pure policy that accepts five current process
roles, exact lifecycle epochs/sequences and a closed event matrix. A sibling
pure codec adds one typed event and mandatory accepted/rejected response with
no source, lifecycle, sequence, time or identity field. A fourth pure policy
adds lifecycle reservation, one sealed handoff, exact acknowledgement,
backpressure and visible incomplete EOF metadata. The peer policy validates six
distinct UIDs, exact shared GID and bounded PID, returning only the semantic
source. The safe adapter resolves fixed names, verifies the collector eUID and
uses Tokio peer credentials without returning numeric identity. Its unlinked
runtime core now consumes an opaque authenticated token, composes exact frames
with the two-window manager, returns canonical responses, and terminates invalid
frames through bounded EOF accounting. A transient library owner now adds the
caller-bound Unix accept loop, random instance ID, real monotonic clock, bounded
partial-frame/response I/O, concurrent role tasks, and graceful cleanup. A
separate fixed-socket collector, hardened non-installable unit, and sysusers
candidate now pass an isolated reversible H616 gate. They remain unshipped and
there is no authenticated export API. A fixed-path Rust producer client adds
bounded connect/write/response and explicit no-retry delivery ambiguity. Its
sibling reporter adds one fixed eight-event ring, non-blocking typed receipts,
one ordered no-retry
worker, saturating semantic counters, and bounded drain/abort shutdown. The
denial-only control broker is their sole consumer and attempts only one
capability-disabled event after writing its response; it ignores every
diagnostic receipt and has no collector service dependency. See
`baselines/vendor-system-control-boundary.md`,
`baselines/system-action-policy.md`, `baselines/system-action-journal.md`,
`baselines/system-action-store.md`, `baselines/system-action-maintenance.md`,
`baselines/trusted-floor-topology.md`, `baselines/diagnostic-export.md`, and
`baselines/diagnostic-collector-policy.md`, plus
`api/diagnostic-producer-v1.md`, `baselines/diagnostic-protocol.md`,
`baselines/diagnostic-window-policy.md`, `baselines/diagnostic-peer-policy.md`,
`baselines/diagnostic-peer-adapter.md`, `baselines/diagnostic-runtime-core.md`,
`baselines/diagnostic-listener-runtime.md`, and
`baselines/device-diagnostic-collector.md`, plus
`baselines/diagnostic-producer-client.md` and
`baselines/diagnostic-producer-reporter.md`, followed by
`baselines/device-diagnostic-control.md` and `api/diagnostic-export-v1.md`.

The live metadata-only serial baseline currently has no ttyUSB0-3 nodes or
serial/by-id directory. Vendor wildcard scan, O_RDWR open, termios changes and
frame reads/writes are not acceptable discovery mechanisms.

The pure Rust codec now emits only the ten initialized bytes in the vendor
selection token and rejects the vendor's two-byte write padding. Incoming
status is deliberately narrowed to the exact five-byte token rather than the
vendor's partial position checks. It remains unlinked and cannot discover or
open a device; see `baselines/serial-switch-codec.md`.

## Broker invariants

1. The network server holds neither root access nor a KVMD credential.
2. Read-only observations and mutations use separate traits and RPC variants.
3. Every path, GPIO line, serial device, I2C address, timeout, and payload is
   selected from a board profile, never supplied freely by an HTTP request.
4. Unix sockets are group-restricted; no state or device node is mode 0777.
5. A pulse uses a bounded monotonic timer and restores the inactive level on
   cancellation or process failure where hardware permits.
6. Startup never mutates hardware merely to discover it.
7. Exactly one process owns each GPIO/configfs resource.
8. Shell execution is absent from the hardware trust boundary.

## Implementation order

1. KVMD read-only Unix-socket inventory client.
2. Broker IPC protocol and systemd-credential loader.
3. Read-only broker daemon and server inventory endpoint.
4. Recovery-media deployment beside the vendor stack on an alternate port.
5. Video proxy, still leaving vendor GPIO ownership untouched.
6. One native driver at a time: button/LED observation, fan, RTC, serial
   switch, then ATX/HID/media mutations after explicit physical approval.
