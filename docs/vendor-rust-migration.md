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
| fan loop and thermal sysfs | hysteretic fan controller | observe temperature only | sensor-failure failsafe and PWM/temperature soak |
| button/activity LED loop | debounced input/output driver | observe input only | press duration, bounce, reboot, polarity |
| RTC shell commands | direct I2C/RTC ioctl adapter | detection only | address, clock direction, power-loss recovery |
| serial KVM switch | framed serial protocol adapter | enumerate only | udev identity, framing, timeout, four-port matrix |
| OLED Python/C drivers | bounded I2C/SPI display worker | disabled | model/address, bus contention, display timeout |
| USB gadget shell scripts | idempotent configfs state machine | inventory only | bind/unbind rollback and host compatibility |
| public uStreamer command | loopback/Unix-socket media service | read-only proxy | reconnect, resolution, latency, soak |
| MSD/Ventoy shell bundle | typed configfs virtual-media worker | inventory only | upload/eject, BIOS boot, interruption recovery |
| partition/update scripts | signed atomic updater | unavailable | signature, traversal, space, power-cut rollback |

## H616 source map (not physically accepted)

- ATX header-pin mapping: power 18, reset 37, power LED 36, HDD LED 41.
- Fan header pin: 15.
- Button/activity mapping: button 12 and activity LED 32.
- Thermal source: `/sys/class/thermal/thermal_zone0/temp`.
- GPIO chip assumption: `/dev/gpiochip0`.
- RTC hypothesis: PCF8563 at I2C bus 0, address 0x51.

The comments mix header positions and global GPIO numbers. Rust code must not
encode these values as production facts until line names/chip offsets are
confirmed on the test board. Prefer libgpiod line identity over deprecated
global sysfs numbering.

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
