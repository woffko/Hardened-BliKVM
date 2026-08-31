# H616 ST7789 display topology baseline

The BliKVM source calls this subsystem `OLED`, but its H616 v4 path selects an
ST7789 240x240 color SPI LCD. At pinned vendor revision
`3e34c04a1dfa84636050585d12d5e585098258ea`, the build enables
`H616=1 ST7789=1`. The vendor init selects SPI channel 1, chip select 0, mode 0,
8 bits per word, and a requested 40 MHz clock. Its source mapping associates
GPIO offsets 259, 262, and 260 with data/command, reset, and backlight.

Those source values are compatibility evidence, not physical acceptance. The
vendor init is unsuitable for discovery because it opens `/dev/spidev1.0`
read-write, changes SPI settings, requests output GPIOs, toggles reset and
backlight, and immediately transmits display commands.

On 2026-08-31, two bounded metadata-only probes inspected the designated H616
test device using source commit `816ac46`. They did not open a raw SPI node,
invoke an SPI ioctl or transfer, request/configure a GPIO, read a GPIO value,
change driver binding, write the filesystem, or change the screen.

Live SPI metadata:

- `/dev/spidev1.0` is a character device, major/minor `153:0`, mode 0600,
  owned by `root:root`;
- sysfs exposes `spi1.0`, bound to driver `spidev`;
- modalias is `spi:dh2228fv` and the non-unique firmware compatible is
  `rohm,dh2228fv`;
- SPI master `spi1` resolves to controller `5011000.spi`, using `sun6i-spi`;
- the root filesystem remained in its existing read-write state.

The SPI-node mode establishes an exact root-only access boundary, but this
probe deliberately did not inspect process file descriptors and therefore does
not claim which process currently has the node open.

Live GPIO metadata:

| Source-associated role | Offset | Live metadata | Legacy export |
| --- | ---: | --- | --- |
| ST7789 data/command | 259 | `sysfs`, output, active-high, used | `out`, `active_low=0`, edge `none` |
| ST7789 backlight | 260 | `sysfs`, output, active-high, used | `out`, `active_low=0`, edge `none` |
| ST7789 reset | 262 | `sysfs`, output, active-high, used | `out`, `active_low=0`, edge `none` |

The role labels come from pinned source mapping; live metadata identifies only
the generic `sysfs` consumer. All three lines already have an owner, so a Rust
display worker cannot request them concurrently. Logical active-high metadata
does not establish electrical polarity, safe reset level, backlight semantics,
or physical wiring.

The OLED/display capability remains unavailable. A native transition requires
an explicit SPI and GPIO owner handoff with rollback, exact controller and line
identity, safe reset/backlight states, bounded transfer sizes and timeouts,
validated mode/frequency, deterministic RGB565 rendering, burn-in and
information-disclosure policy, crash/restart/reboot handling, and physical
screen tests. None of those gates is satisfied by this metadata baseline.
