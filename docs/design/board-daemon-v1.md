# Board daemon v1 (replacing the vendor C `kvmd-main`)

Status: design for plan phase E2 (`docs/design/gui-parity-plan.md`). It replaces
the vendor C daemon's board peripherals with one Rust daemon. Read-only recon
already exists; every mutating step below is a device gate with an explicit
operator confirmation and a private receipt.

## What the vendor daemon owns today

`/usr/bin/blikvm/{kvmd-main.sh,kvmd-main}` drives the board peripherals and
publishes them over `/var/blikvm/{atx,fan,switch}.sock` with state under
`/dev/shm/blikvm/{atx,fan,switch}`. Our board owner already replaced the ATX
part. The remaining subsystems, with the live facts our read-only captures
recorded:

| Subsystem          | Live fact                                                                                                                       | Recon receipt / baseline              |
| ------------------ | -------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------- |
| Fan                | `gpiochip0` line **269** (chip `300b000.pinctrl`, 288 lines); the line is still exported through the deprecated sysfs interface. | `docs/baselines/device-fan-gpio.md`   |
| ST7789 240x240 LCD | `/dev/spidev1.0` (`5011000.spi`, `sun6i-spi`), mode 0, 8 bits/word, requested 40 MHz; data/command **259**, reset **262**, backlight **260** (active-high, sysfs, currently used). | `docs/baselines/device-oled-topology.md`, `docs/baselines/st7789-model.md` |
| Activity LED       | `gpiochip0` line **261**; the vendor toggles it every 100 ms.                                                                     | `docs/baselines/activity-led-policy.md` |
| Display button     | `gpiochip0` line **257**; sampled every 100 ms, high is active, five consecutive samples wake the display.                        | `docs/baselines/display-button-policy.md` |
| Serial switch      | no USB serial device exists (`ttyusb_count=0`), so the vendor switch protocol has no transport on this revision.                   | `docs/baselines/device-serial-topology.md` |
| RTC                | `rtc0` = `sun6i-rtc` (`hctosys=1`), `rtc1` = `pcf8563` on I2C `0-0051` (battery-backed).                                          | `docs/baselines/device-rtc.md`        |

## Design

One Rust daemon, `bli-kvm-boardd`, with the same ownership discipline the board
owner already uses: a single process per line, a fixed control surface, no dead
capabilities.

- **Transports**:
  - GPIO through the existing `bli-kvm-gpio-chardev` driver (line handles with a
    fixed consumer label, idempotent requests, per-offset lifecycle). The fan,
    the LED, the display button, the LCD reset and the LCD backlight are output
    or input lines on `gpiochip0`.
  - SPI through `/dev/spidev1.0` with mode 0, 8 bits/word and a bounded clock
    (40 MHz requested by the vendor; the daemon pins the value and bounds every
    transfer), driven by the pure `bli-kvm-st7789-model` command encoder.
  - RTC read-only from `rtc1`/`rtc0` sysfs for the status view; time setting
    stays with the system time path that already exists.
- **Policy reuse** (all already host-tested): `bli-kvm-fan-policy` +
  `bli-kvm-fan-supervisor` (duty in percent, failsafe duty, minimum/maximum
  temperature, shutdown preparation), `bli-kvm-activity-led-policy`
  (explicit asserted level, period, pulse, idle floor),
  `bli-kvm-display-button-policy` (debounce, activation hold, wake event) and
  `bli-kvm-serial-switch-codec` (host-side only until a transport exists).
  These policies refuse a time earlier than their last one, so the daemon
  gives them a monotonic clock that starts with the process, never the wall
  clock, which a network time step or a manual correction can move back. A
  fan policy that cannot decide (that refusal included) leaves the fan at
  its failsafe duty, full speed, until the policy answers again (code audit
  of 2026-10-20, AUD-06).
- **Control surface**: the daemon exposes its state and accepts reviewed
  commands over its own Unix socket, mirroring the owner's protocol style
  (versioned request/response, fixed rejection codes, no free-form payloads).
  The server maps it to the reference-shaped endpoints and hides anything the
  hardware cannot do (the serial switch stays `unavailable`).
- **Unit**: `hardened-blikvm-boardd.service`, non-root where the hardware
  allows it, `DevicePolicy=closed` with explicit `DeviceAllow` for
  `/dev/spidev1.0` and `/dev/gpiochip0`, `ProtectSystem=strict`, and the LED /
  button / fan lines released from the vendor sysfs export before start.

## Implemented slice (2026-09-16)

`crates/bli-kvm-boardd` now exists with: the operator configuration file
(bounded, strict, every board fact supplied there), the fan actuator (soft-PWM
thread over the reviewed line, level-only mode for gates without actuation), the
control surface (typed JSON lines: `status`, `fan_mode_set`, `fan_duty_set`
with fixed error codes) and the run modes `--once` (single automatic evaluation
and status print, no actuation) and `--serve`. It ships in the board bundle as
`libexec/bli-kvm-boardd` with `hardened-blikvm-boardd.service`, and the fan
supervisor gained `actuator_mut` for the manual duty path. Host tests cover the
config bounds, the duty split, level and PWM actuation, the protocol state
machine and one socket round-trip.

## Implemented slice 2 (2026-09-16): ST7789 panel

`display` and `spi` modules: the ioctl numbers for `/dev/spidev1.0` are computed
from the Linux ABI at compile time and pinned by a test, the controller
initialization sequence (commands, payloads, settle delays), the line offsets
and polarity, and the reset/backlight timing all come from the operator
configuration. The driver requests the three panel lines, pulses reset, runs the
initialization with the data/command line driven per transfer, and only then
enables the backlight; it draws a colour-bar test pattern across the full panel
in bounded chunks through the pinned `bli-kvm-st7789-model` encoder, which now
has exactly one reviewed consumer. The panel lines gained reviewed roles
(`DisplayDataCommand`, `DisplayReset`, `DisplayBacklight`) and the unit allows
`/dev/spidev1.0`. The control surface now also carries
`display_test` and `display_backlight_set` plus the panel fields in the status
view (`displayPresent`, `displayInitialized`, `displayBacklightOn`); a panel
that fails to come up never stops the fan, it is reported on stderr and shown as
absent. Still open before the panel gate: the panel lines handoff (unexport the
vendor sysfs lines 259/262/260 and provision the operator configuration).

## Implemented slice 3 (2026-09-16): activity LED

`led` module and its reviewed configuration section: the activity LED runtime
keeps the line in step with `bli-kvm-activity-led-policy` (asserted level, period
and pulse from the operator), writes only when the level changes, and exposes
`led_enabled_set` plus the `ledPresent` / `ledEnabled` / `ledLevel` status
fields. The sampling thread drives the pulse; a missing or failing LED never
stops the fan. The LED policy keeps its purity guard, now expressed as exactly
one reviewed consumer (the board daemon), the same rule the panel model follows.

## Implemented slice 4 (2026-09-16): display button

The GPIO driver seam gained a `read_line` method (line values ioctl in the
character-device driver, with the ioctl number pinned by the existing ABI
tests), and the daemon gained the button runtime: it samples the reviewed input
line, debounces and holds it through `bli-kvm-display-button-policy`, and on a
wake cycle lights the panel again; the status view reports `buttonPresent` and
`buttonWakeups`. A missing button or panel never stops the fan, and a wake
without a panel is counted without failing.

The status view also carries the external RTC observation
(`externalRtcSeconds`, `externalRtcOffsetSeconds` and a classified error when
the observation is unavailable), read through the platform RTC reader.

Still open in the daemon: the boot-config view, then the device gates for fan,
panel, LED and button.

## The display's sleep (2026-10-10)

At the operator's word, the display goes dark after a time without
activity, with the choices of Hardened NanoKVM: never (the default, as
before), 15 or 30 seconds, 1, 3, 5, 10 or 30 minutes, or an hour (Settings >
Device > Display, `GET`/`POST /api/vm/oled` with the reference's `exist` and
`sleep`).

- The board owner keeps the choice (`DisplaySleep` and `DisplaySleepState`
  on its control socket) in `/var/lib/hardened-blikvm-gadget/display-sleep.env`
  (`sleep_seconds=N`, root, 0600). The RAW system update carries that
  directory; the card image starts without the file, that is never.
- The daemon reads the file on every sample, as Hardened NanoKVM reads its
  own: a missing, oversized or malformed file, or a time that is not one of
  the choices, is never. Its unit grants no write outside its runtime
  directory and needs none here; as root it reads the owner's file, which
  is root's.
- Once the display has had no activity for the time, the daemon turns its
  backlight off and draws nothing until it is lit again. Activity is the
  daemon's start, the button's wake cycle, a new choice, and a request on
  the socket to light the backlight; each lights the display, draws the
  status at once and starts the time over. A backlight that could not be
  turned off is tried again on the next sample. A request to turn the
  backlight off keeps the display dark until the button or a new choice.
  Nothing else wakes it: the status screen changes every few seconds
  (temperature, load), which would light it as often. The daemon samples
  the button once a sample (a second on the board), and the policy wants a
  debounced press held for its hold time: the button is held for two to
  three seconds before the display lights (the operator's check on slot 84,
  2026-10-11).
- The status the daemon answers keeps its fields: the update runtime's
  health check reads it strictly (`board_readiness.rs`), so the display's
  state is not in it; a dark display reports `displayBacklightOn: false`,
  which the check allows.

## Ownership handoff and rollback

The vendor C daemon and our daemon must never own a line at the same time:

1. Stop and mask `kvmd-main` (already masked) and verify no process holds the
   sysfs exports; unexport the vendor lines only inside the gate.
2. Start the daemon with the lines unrequested; it requests them only when a
   subsystem is enabled, and the gate checks `gpioinfo` ownership before and
   after.
3. Rollback: stop the daemon, unexport/return the lines, re-enable the vendor
   unit from its backup if needed.

## Fan gate runbook (needs explicit operator confirmation)

Read-only preflight receipt: `evidence/private/boardd-fan-preflight-20260916-a.json`.
It records that lines 259, 260, 261, 262 and 269 are still claimed by the
deprecated sysfs export (consumer `sysfs`, direction `out`, all `active-high`),
line 257 is unused, no vendor process is running, the only holder of
`/dev/gpiochip0` is our board owner, and the thermal zone reads about 51.5 C.

Confirmed order for the gate (every step reversible):

1. Xexport the vendor sysfs lines (`echo <n> > /sys/class/gpio/unexport` for
   269, 261, 259, 260, 262) after checking no process depends on them; the
   previous values are recorded first.
2. Write `/etc/hardened-blikvm-gadget/boardd.json` with the reviewed facts
   (fan line 269, active-high, PWM period, thresholds, curve, sample interval,
   socket path) — no value comes from the vendor source.
3. Install `libexec/bli-kvm-boardd` and `hardened-blikvm-boardd.service` from
   the verified bundle, then run `bli-kvm-boardd --config ... --once`: it
   requests the line, applies the failsafe duty and prints the status without
   enabling PWM.
4. Start the unit and run a bounded manual sweep (`fan_mode_set` manual, then
   `fan_duty_set` at the reviewed points) with the operator watching the fan;
   the receipt records the duty, the temperature and what the operator saw.
5. Rollback: stop the unit, unexport nothing, re-export the vendor lines with
   their recorded values, and re-enable the vendor daemon from its backup if
   the fan must return to it.

## Gates (one per subsystem, in order)

1. **Fan**: read-only temperature + line ownership capture, then a bounded duty
   sweep with the physical fan observed by the operator (the fan is audible);
   receipt records duty, temperature and the observed response. No timing or
   minimum-duty value is baked into code: they come from the gate.
2. **LCD**: SPI loopback-free bring-up with a fixed test pattern (colour bars
   plus a status line), verifying mode/clock/bounded transfers and a clean
   reset/backlight sequence; the operator confirms what the panel shows.
3. **LED and button**: LED pulse patterns from the policy, button wake event,
   both verified by the operator with the same receipts.
4. **RTC and boot config**: RTC read view and the boot selector behaviour
   already modelled by `bli-kvm-system-boot-control`.
5. **Retirement**: after 1-4 pass, stop and mask the vendor `kvmd-main`,
   `kvmd-fix` and their sockets, and record the baseline.

The serial switch stays hidden with a capability note: no transport exists on
this revision, and inventing one would be guessing.
