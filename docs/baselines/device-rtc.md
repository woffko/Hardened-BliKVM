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

## Runtime inventory checkpoint

The bounded Rust adapter subsequently passed its host, reproducibility,
reversible systemd, and TLS Chromium gates on 2026-08-31. The accepted
development bundle is:

- source commit: `cc37a3ca5d6d223de964eba85ee774fe14551ff3`;
- archive SHA-256:
  `aa16c7c3cb19e1b4f5141e5cacb99e67e95f068b688c74d30e33a91c36c65fed`;
- archive size: 4,776,772 bytes;
- AArch64 server SHA-256:
  `489e3fb857d05d4c701824cfe74e3b8c8ae57d4370981776a413b897144c4a08`;
- AArch64 platform-broker SHA-256:
  `af8501be6278f3f4c79041d382677ce0e8faeb1b6d041d9a822ad28043dcde98`;
- web tree SHA-256:
  `d85320a7cb3b6db3b2b5b55117ebab167f7c566bce69839a84f767fbe4731c44`.

Two fresh independent AArch64 target/output directories produced
byte-identical archives. Their manifests bind the same six Rust payloads, web
tree, lock hashes, and committed source. `releaseAccepted` and
`deviceAccepted` remain false.

The exact archive passed the focused reversible systemd profile:

- every payload checksum matched and the dedicated KVMD credential was
  delivered only through `LoadCredential`;
- broker, video-proxy, and server sandbox identity, capability, device,
  network, socket, and credential boundaries passed;
- setup/login and 20 authenticated sanitized inventory requests returned HTTP
  200 and schema v3;
- external RTC epoch `1759491069` and RTC-minus-system offset `-28648078`
  seconds passed the paired bounds;
- RTC capability was `read_only` with `runtime_verified` evidence;
- the focused profile explicitly skipped video stream throughput while still
  validating the video unit sandbox;
- htpasswd rollback was byte-identical, temporary account/units/runtime/staging
  disappeared, watchdog and port were inactive, KVMD services remained active,
  and rootfs stayed read-write.

The same archive passed the TLS Chromium gate. Chromium validated schema v3,
the bounded paired RTC observation, runtime capability, and exact External RTC
and RTC offset rendering in Settings / About. TLS 1.2/1.3, secure cookies,
password rotation, logout, MJPEG decode, four reconnects, stream revocation,
zero control requests, and final cleanup also passed.

No raw I2C or RTC character device was opened by the Rust adapter. No hardware
or system clock, alarm, wakeup setting, device registration, filesystem state,
GPIO, fan, PWM, HID, ATX, media, partition, reboot, or physical action was
performed. The stale offset is intentionally observable but never corrected.
