# Linux GPIO character-device driver

Status: host-only ABI validation; the ioctls run only on the designated board.
Phase 2 groundwork for the Rust board daemon.

`crates/bli-kvm-gpio-chardev` implements the platform `GpioDriver` seam over
the v1 line-handle ioctls of `/dev/gpiochipN`:

- the `gpiohandle_request`/`gpiohandle_data` layouts are pinned by compile-time
  size assertions, and the `_IOWR` ioctl numbers are asserted against the
  kernel ABI in tests;
- request flags map direction and polarity, and the consumer label is the fixed
  board label, never caller input;
- one line is requested at a time, driven by offset, and released by closing
  its handle; a repeated request is idempotent and an unrequested write reports
  `NotRequested` instead of touching the chip.

`bli-kvm-atx-gpio-executor` gains the `chardev-driver` feature and
`chardev_atx_executor`, which opens the chip and builds the reviewed ATX
executor over the driver and the real sleeper.

## Gates

- the reviewed physical handoff that transfers ATX line ownership from KVMD;
- line identity, polarity and pulse timing confirmed on the board;
- an owner that selects the chardev runner instead of an injected mock.
