# The target's disk activity on the toolbar, carried out

The record of `work-plan-20261111-hdd-indicator.md`: the target's HDD LED,
which the original BliKVM interface and the PiKVM one it ported showed next
to the power LED, read by boardd on every turn and shown as an "HDD" pill
under the toolbar's power icon (variant D3, the operator's choice of
2026-11-11). Phase A read the disk as active for 500 ms after a flash and
the page asked once a second; compared with the case's light on the test
board, the pill did not follow it, and the operator chose on 2026-11-12 to
show the LEDs as KVMD does (phase B): each change held 100 ms by boardd and
sent to the page over a socket. Each commit names the test that failed on
the code before it.

## Items

| Item | What | Commit | Test that failed before | State |
| --- | --- | --- | --- | --- |
| H1 | boardd reads the case's LEDs on every 20 ms turn and counts the disk as active while its LED was lit within the last 500 ms | `c51b51f` | `a_blinking_disk_counts_as_active_between_its_blinks` (a disk LED lit 30 ms every 400 ms: 11 of 21 status reads said idle) | fixed |
| H2 | `GET /api/atx/leds` answers `powerLed` and `hddLed` from the board owner, each `null` when the board has no such LED or the owner does not answer; the inventory gains `hddLed` | `cc1e852` | `the_case_leds_are_read_from_the_board_owner` (404) and the inventory test with the disk (`hddLed` missing) | fixed |
| H3 | the page reads the route once a second while it is shown; the power icon keeps its colour and gains the "HDD" pill, amber while active, grey while idle, absent without a disk LED; the Power menu shows both LEDs | `42c04fc` | `tools/playwright/hdd_indicator_contract.mjs` (no pill; the wait for it timed out) | fixed; its reads replaced by H7 |
| H4 | boardd holds each change of a case LED 100 ms, as KVMD's debounce does, instead of H1's 500 ms window | `35bc5d1` | `a_disk_flash_shows_for_the_hold_and_no_longer` (a disk lit one turn every 400 ms read lit at every turn) | fixed |
| H5 | the board owner reads boardd's status outside the lock HID leases take | `3d6eaf5` (the plan) | - | left: already so since the code audit of 2026-11-06 (AUD-08), found when H5 was begun |
| H6 | `GET /api/atx/leds/socket`: a protected, same-origin WebSocket sending the case LEDs when it opens and at each change; one read of the owner every 50 ms while a socket is open, none while none is; at most 16 sockets; ends with its session and at the stop | `c45e995` | `the_case_leds_socket_sends_each_change_and_ends_with_the_session`, `the_case_leds_sockets_are_bounded_and_end_at_the_stop` (the handshake was answered 404) | fixed |
| H7 | the page listens to the socket while it is shown and closes it while hidden; with the socket down it reads the light route once a second and opens a new socket after 2 s | `d4cf666` | `tools/playwright/hdd_indicator_contract.mjs` (the page opened no socket) | fixed |

boardd's status keeps every field and its name (`atxHddLedOn` is now the
disk LED's level as held), so the system's health check reads it as before
and no system part changed. Both new routes are protected like the
inventory, the socket checked for its origin like `/api/ws`; no capability
was added. The route counts of the parity plan and the phase 8 note are 128.
The screenshots `02-desktop.png` and `13-power-menu.png` were shot again
with the pill (decision 4); the look did not change in phase B.

Found on the way: the first check before the slot stopped in
`a_change_back_to_the_first_addressing_ends_the_window`, N5's test of
2026-11-06, whose 2 s confirmation window a host at load 14 outran once (the
first window's revert ran under the change back). Alone it passed 10 of 10;
its window is now 6 s, checked 8 s on (`69a1a23`), and the check passed in
one run after it.

Decisions 1 and 4 were taken as written; 2 and 3 were changed by the
operator after gate A (below).

## Gate A: slot 109

On the test board, at the operator's word, read-only probes only and the
board's settings left as found.

- Slot 109 (0.1.0-beta.13 as a test build) was built from a clean tree at
  `69a1a23` after a green `scripts/check.sh` (the slot's tar
  `770b3ef0…546e`). Its update runtime is the staged dev46's but for the
  package manifest: no system part changed. The USB controller read
  `configured` before the install, which was confirmed.
- The state as found but for the slot (108 to 109). The watching probes
  passed: the GUI walks on a desktop and a phone, WebRTC with H.264 and with
  VP8, QUIC video, touch reach.
- For 21 minutes `/api/atx/leds` was read every half second: the power LED
  lit throughout, the disk active in 2250 of 2288 reads.
- The operator's comparison (Russian answers translated): "Doesn't
  follow"; "the HDD LED is lit all the time" (the pill); "are you sure you
  picked the right HDD address?".
- The line is the vendor's: on the board, the BliKVM source for the H616
  (`blikvm_atx.c`) reads `PIN_LED_HDD 41 //GPIO 233` and `PIN_LED_PWR 36
  //GPIO 234`, lit when high, and KVMD's override names `hdd_led_pin: 233`.
  Read raw through debugfs about 2000 times a second, line 233 went on and
  off in steps of about 250 ms (lit 47 % of 20 s, then 70 % of a minute with
  bursts of millisecond flicker), line 234 stayed high. Asked what the
  case's light did, the operator answered: "lit nearly all the time, but it
  fades out, that is, it works often". A 500 ms window keeps a light that changes
  every 250 ms lit throughout, and so did the pill.
- KVMD on the board (`kvmd/plugins/atx/gpio.py`, `kvmd/aiogp.py`) waits for
  the line's edges, changes the value at most once per 100 ms
  (`hdd_led_debounce: 0.1`) and sends each change to its page over its
  socket. Played back through a model, the minute of line 233 changed KVMD's
  indicator about 8 times a second; read four times a second with any window
  of 100 to 250 ms, the pill stood amber or blinked out of step. The
  operator first chose faster reads, asked to look at how often PiKVM reads
  ("theirs displayed fine"), and, shown this, chose to do as KVMD does:
  phase B, slot 110.

## Gate B: slot 110

- `scripts/check.sh` stopped three times on `d4cf666`: in
  `quic_transport_contract.mjs`, whose desktop page did not show its
  settings icon within 15 s (the slow page load of 2026-11-07; alone it
  passed 3 of 3, and the harness answers the LED socket's handshake 404 in
  9 ms); in the encoder's `the_report_counts_once_a_second_while_a_viewer_watches`
  ("2 reports in about three seconds" at load 11, with another session's
  virtual machines on the host; the encoder is untouched); and at the
  format check, which H7's contract did not pass (`e6626e0` formats it).
  It passed in one run on `e6626e0`.
- Slot 110 (0.1.0-beta.13) was built from a clean tree at `e6626e0` (the
  slot's tar `78f852a0…2878`). Its update runtime is the staged dev46's but
  for the package manifest, and its encoder is slot 109's: no system part
  changed. The USB controller read `configured` before the install, which
  was confirmed.
- The state as found but for the slot (109 to 110) and the capture queue's
  live rate (50 frames a second while a page watched before, 0 after the
  restart). The watching probes passed: the GUI walks on a desktop and a
  phone, WebRTC with H.264 and with VP8, QUIC video, touch reach.
- For ten minutes `/api/atx/leds/socket` was read: 2,308 messages, about
  3.8 a second; the power LED lit throughout, the disk lit 58 % of the
  time, its changes a median 233 ms apart (a tenth under 116 ms, a tenth
  over 291 ms, the longest pause 9.2 s): the steps of the case's light,
  each held at least 100 ms.
- The operator's comparison, with the new page: "Follows the light".

## The release 0.1.0-beta.13

Published on 2026-11-12 at the operator's word as a GitHub pre-release,
[v0.1.0-beta.13](https://github.com/woffko/Hardened-BliKVM/releases/tag/v0.1.0-beta.13),
tag on `b7f1fc6` (the commits after `e6626e0`, where slot 110 was built and
`scripts/check.sh` passed, add only documents). The application only: the
card image and the RAW system update stay 0.1.0-beta.11's; beta.12's notes
now open with a line that beta.13 supersedes them. `cargo audit` with the
database of 2026-11-12 reported RUSTSEC-2026-0486 in `rustix` 1.1.4, not
reachable on a board (`docs/third-party-rust.md`); `rustix` 1.1.6 was taken
after the tag (`65c7f03`). The files, whose digests GitHub reports as the
local ones:

| File | SHA-256 |
| --- | --- |
| `hardened-blikvm-application-0.1.0-beta.13-seq110.tar` | `78f852a0c64cda8fe6bd1b99fba3c96cba3d1895771a3fe7dc3917fff5452878` |
| `hardened-blikvm-agent.exe` | `da5fc03090d5e6ba7e5f206c6a7c1f52d921beb04db545e05f59da7e98668fed` |
| `SHA256SUMS` | `12ef005df66803fe5a9e1ae0a63620b2bfebd24446ff8e55b74eea142df6303c` |

The test board ends on slot 110, update runtime dev46 and the RAW system of
sequence 11, its settings and switches as found.
