# H616 serial-switch topology baseline

On 2026-08-31, a bounded metadata-only probe inspected the designated
test-only H616 device without opening a TTY. It did not change termios, read or
write a frame, enumerate `/dev/serial/by-id` names, read USB serial numbers,
write the filesystem, or invoke a physical action.

Observed state:

- no `ttyUSB` class device was present;
- `/dev/ttyUSB0` through `/dev/ttyUSB3` were absent;
- no `/dev/serial/by-id` directory was present;
- rootfs remained in its existing read-write state.

The serial KVM switch is therefore absent or not enumerated in the current
test topology. This is not evidence that every BliKVM accessory is unsupported,
and it must be refreshed after a deliberate switch-device hotplug.

No runtime adapter may scan and open arbitrary ttyUSB devices as the vendor
daemon does. A future implementation needs a pure bounded frame parser/encoder,
an explicit non-unique VID/PID plus reviewed stable routing policy, fixed
19200/8N1 settings only after authorization, exclusive TTY ownership, bounded
read/write/timeouts, reconnect/hotplug tests, four-port physical behavior, and
no retention or publication of unique USB identifiers.

The serial-switch capability remains unavailable.
