# Vendor settings audit (H616 V4)

Compares our reviewed `/etc/hardened-blikvm-gadget/boardd.json` with the
retained vendor sources under `/opt/bin/blikvm/src` on the board (2026-09-19).

## Lines and polarity: match

| Setting | Vendor | Ours | Match |
| --- | --- | --- | --- |
| Fan PWM line | `FAN_PIN 15` = GPIO 269 (`blikvm_fan.c`, H616) | `fan.line = 269`, active-high | yes |
| Activity LED | `SW2_LED 32` = GPIO 261 (`blikvm_gpio.c`, `act`) | `led.line.offset = 261`, asserted-high | yes |
| Display button | `SW1 12` = GPIO 257 (`blikvm_gpio.c`) | `button.line.offset = 257`, active-high | yes |
| ATX power LED | `PIN_LED_PWR 36` = GPIO 234 (`blikvm_atx.c`) | `atxLeds.power.offset = 234` | yes |
| ATX HDD LED | `PIN_LED_HDD 41` = GPIO 233 (`blikvm_atx.c`) | `atxLeds.hdd.offset = 233` | yes |
| Panel D/C, reset, backlight | pins 40/22/38 -> GPIO 259/262/260 (`oled_run.c`, `spi_lcd.c`) | `dataCommand 259`, `reset 262`, `backlight 260` | yes |
| Panel init | `LCD_1IN3_InitReg` + MADCTL `0x70` | same bytes | yes |
| Panel background/refresh | `oled_info.bmp`, ~5 s | same image, 5 s | yes |

## Differences that need a decision

1. **Fan PWM period.** The vendor uses `softPwmCreate(FAN_PIN, 0, 100)`, which
   its own comment documents as ~100 Hz (10 ms period). We run
   `pwmPeriodMs = 40` (25 Hz). A 25 Hz software PWM can produce audible
   commutation noise; matching the vendor means `pwmPeriodMs = 10`.
2. **Fan curve.** The vendor `GetDuty()` returns 100/90/80/70/60/50 % for
   >=60/55/53/50/45/<45 C and only runs the fan above 60 C (or above 55 C when
   `fan_enable` is set), otherwise 0 %. Ours is much gentler: off below 45 C,
   30 % at 50 C and 80 % at 70 C, with a 20 % minimum and no duty below 50 C
   over 30 %. Matching the vendor would keep the fan at >=50 % whenever it
   runs and start it far earlier.
3. **Panel/OLED capability.** The platform still advertises `oled` as
   `not_exposed` while the daemon now drives the panel. The reference exposes an
   OLED enable/disable toggle; if we want that surface, the feature must be
   advertised and the owner given a reviewed toggle.

The line and polarity facts are already identical, so no change is required
there. Items 1 and 2 change thermal and acoustic behaviour and are left for the
operator's explicit choice; item 3 is a UI decision.
