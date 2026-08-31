# H616 fan GPIO ownership baseline

On 2026-08-31, a bounded metadata-only probe inspected the designated
test-only H616 device. It invoked libgpiod `gpioinfo` 1.6.3 for chip/line-info
only. It performed no line request, line-value read, direction/configuration
change, filesystem write, PWM operation, or physical action.

Observed chip metadata:

- `/dev/gpiochip0` is a character device, mode 0660, owned by `root:gpio`;
- sysfs label is `300b000.pinctrl`, base is 0, and the chip exposes 288 lines;
- vendor-derived fan offset 269 is within that chip.

The exact line-info result for offset 269 was:

`line 269: unnamed "sysfs" output active-high [used]`

The corresponding legacy sysfs export `/sys/class/gpio/gpio269` was present
with direction `out`, `active_low=0`, and edge `none`. The probe deliberately
did not read its `value` attribute.

This is sufficient to reject concurrent driver ownership. The running legacy
stack already owns line 269 through the deprecated sysfs GPIO interface; a
second libgpiod/PWM owner must not request it. Any future migration requires a
bounded owner handoff, verified rollback, and proof that the legacy controller
has stopped touching the line before a new driver starts.

`active-high` and `active_low=0` describe the kernel's logical line
configuration only. They do not prove electrical fan polarity, whether 100
percent means maximum cooling, PWM frequency/duty behavior, minimum spinning
duty, or state after owner/process/kernel failure. No physical fan acceptance
can be inferred from this baseline.

The fan capability remains unavailable. Required gates still include physical
pin tracing, electrical polarity, PWM/frequency and minimum-duty measurement,
exclusive handoff/rollback, startup/controlled-stop/crash behavior,
sensor/stall fault injection, and sustained thermal soak.

The later full pinned-line ownership capture is recorded in
`device-gpio-profile.md`.
