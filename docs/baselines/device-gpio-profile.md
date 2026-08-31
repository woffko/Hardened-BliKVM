# H616 pinned GPIO ownership profile

On 2026-08-31, a bounded metadata-only probe inspected every H616 GPIO offset
in the pinned vendor migration profile. It used libgpiod `gpioinfo` 1.6.3 and
legacy sysfs metadata only. It made no line request, read no line value,
performed no export/unexport or configuration/write, and invoked no physical
Power, Reset, LED, button, fan, or display behavior. A schema-v2 refresh added
the three source-associated ST7789 control offsets.

Chip0 was `/dev/gpiochip0`, mode 0660 `root:gpio`, with sysfs label
`300b000.pinctrl`, base 0, and 288 lines.

| Role | Offset | Live metadata | Legacy export |
| --- | ---: | --- | --- |
| ATX Power switch | 228 | `kvmd::atx::power_switch`, output, active-high, used | absent |
| ATX Reset switch | 272 | `kvmd::atx::reset_switch`, output, active-high, used | absent |
| ATX Power LED | 234 | `kvmd::atx::leds`, input, active-high, used | absent |
| ATX HDD LED | 233 | `kvmd::atx::leds`, input, active-high, used | absent |
| Display button | 257 | unnamed, unused, input, active-high | absent |
| ST7789 data/command | 259 | `sysfs`, output, active-high, used | present: `out`, `active_low=0`, edge `none` |
| ST7789 backlight | 260 | `sysfs`, output, active-high, used | present: `out`, `active_low=0`, edge `none` |
| Activity LED | 261 | `sysfs`, output, active-high, used | present: `out`, `active_low=0`, edge `none` |
| ST7789 reset | 262 | `sysfs`, output, active-high, used | present: `out`, `active_low=0`, edge `none` |
| Fan control | 269 | `sysfs`, output, active-high, used | present: `out`, `active_low=0`, edge `none` |

This confirms the source-derived chip offsets and current logical ownership,
not electrical polarity or physical behavior. In particular:

- Power and Reset remain owned by the existing KVMD service and were never
  actuated;
- LED metadata does not prove voltage polarity or UI semantics;
- the unused display-button line was not requested and its value was not read;
- ST7789 roles are source associations because the live consumer label is only
  `sysfs`; all three lines are nevertheless used legacy outputs;
- display controls, activity LED, and fan remain owned by the legacy sysfs
  stack;
- no native Hardened-BliKVM driver may become a second owner of any used line.

A native ownership transition requires an exact per-line handoff: stop the
current owner, prove release, establish a known safe electrical state, start
the new owner, verify behavior, and retain a bounded rollback path. Physical
Power/Reset exclusion, fan thermal-safety gates, LED polarity, button debounce,
display SPI/GPIO ownership and electrical behavior, crash/power-loss behavior,
and reboot persistence remain unresolved.

An unlinked pure Rust display-button policy now provides explicit polarity,
bounded debounce/hold timing, startup-held suppression, one-shot-until-release
semantics, and clock-rollback rejection. Its evidence is recorded in
`display-button-policy.md`; it does not read offset257 or change this metadata
or physical acceptance status.
