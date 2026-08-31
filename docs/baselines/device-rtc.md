# H616 read-only RTC topology baseline

On 2026-08-31, a bounded SSH probe inspected only the kernel RTC class and
device-node metadata on the designated test-only H616 device. It performed no
filesystem write, raw I2C access, RTC character-device open, clock set, or
physical action. The root filesystem remained in its pre-existing read-write
state.

Two kernel RTC class devices were present:

| Class | Kernel name | Backing device | Driver | `hctosys` | Node |
| --- | --- | --- | --- | --- | --- |
| `rtc0` | `sun6i-rtc 7000000.rtc` | `7000000.rtc` | `sun6i-rtc` | `1` | `/dev/rtc0`, character device, 0600 `root:root` |
| `rtc1` | `rtc-pcf8563 0-0051` | `0-0051` | `rtc-pcf8563` | `0` | `/dev/rtc1`, character device, 0600 `root:root` |

This confirms the source-derived external-RTC hypothesis without probing raw
I2C: the PCF8563 is already bound by the kernel at bus/address identity
`0-0051`. The vendor daemon's `i2cdetect`, `new_device`, and `hwclock -w`
sequence must not be reproduced by an observation path.

The external RTC reported 2025-10-03 while the system RTC class reported
2026-08-31 during the same capture. This is evidence that the external clock is
present but not synchronized; it is not permission to correct it. The current
slice must expose the raw bounded observation and a derived offset only. Setting
the hardware or system clock requires a separate policy, authorization,
direction-of-sync decision, and power-loss gate.

Both character nodes are root-only, and the platform broker's production
sandbox uses `PrivateDevices=yes` plus `DevicePolicy=closed`. The first Rust
adapter should therefore retain that boundary and read only the fixed
`/sys/class/rtc/rtc1/name` and `/sys/class/rtc/rtc1/since_epoch` attributes.
It must validate the exact external-RTC identity, bounded ASCII epoch, source
metadata, and a bounded system-time offset. Missing, rebound, malformed, or
implausible RTC input must become an unavailable observation without relaxing
the sandbox or falling back to shell, raw I2C, `/dev/rtc`, or clock writes.

This baseline establishes topology and one read-only observation only. It does
not accept calibration, accuracy, drift, battery retention, suspend/resume,
reboot persistence, century handling, alarm/wakeup behavior, or either clock
write direction.
