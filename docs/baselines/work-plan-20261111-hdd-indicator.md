# Work plan: the target's disk activity on the toolbar

The original BliKVM interface, and the PiKVM one it ported, showed the target's
HDD LED next to its power LED. This board reads both: boardd samples the ATX
inputs (HDD on line 233, power on 234) and the board owner answers them in
`BoardStatus` (`hdd_led_present`, `hdd_led_on`), but the server passes only the
power LED to the page, and the page shows it only as the power icon's colour.
On the test board both inputs are wired: the HDD LED read lit and unlit over
ten seconds of reads.

The look was chosen by the operator on 2026-11-11 from mockups: variant D3, a
small "HDD" pill under the toolbar's power icon, amber while the disk works and
grey while it is idle, and the Power menu showing both LEDs under its title.

Its items are H1, H2, ..., so as not to be confused with earlier plans.

## Rules

The rules of the earlier plans hold:

- English repository text, and secrets only through `project-memory-exec`
  (never a program on its standard input). On the board, SSH uses the GUI
  record.
- One commit per item, starting with a test that fails on the code as it is.
- One new HTTP route, H2's, and no new capability.
- The slot of this plan runs on beta.11's system (RAW sequence 11, update
  runtime `0.1.0~dev46`), and changes no system part: boardd's status keeps
  every field and its name, since the system's health check reads it
  strictly.
- `RUST_TEST_THREADS=16 sh scripts/check.sh` before each phase's last commit
  and before every build, a run stopped by a flaky test run again in full and
  the test named.
- The device rules and the target's rules: consent for every install, no
  probe while the operator uses the board, nothing on the board that changes
  its settings; nothing is sent to the target. Disk activity on the target is
  the operator's to make (copying a file, for instance).
- Before a slot install, the board's USB controller is read: it must be
  `configured`, or the install's health check refuses the slot.

## Where the work starts

- Branch `wip/corrective-audit-checkpoint` at `d9c7f44`; `main` holds the
  same content. 0.1.0-beta.12 is published (tag on `f153a9b`).
- The test board runs slot 108 on the system of sequence 11.

## Decisions

1. **Release.** An application release 0.1.0-beta.13 (slot 109) at the
   operator's word, and the branch merged into `main` when the operator asks
   (recommended). The alternative is the code alone, with the next release.
2. **The read.** A light route, `GET /api/atx/leds` (recommended; the
   toolbar's live indicator needs it, and the inventory, which the Power menu
   reads today, gathers media, video and temperature too). The alternative,
   pushing the LEDs over the page's socket, is a larger change.
3. **Timing.** The disk counts as active when its LED was lit within the last
   500 ms; the page reads once a second while it is shown (recommended). The
   indicator is therefore lit while the disk works, not a copy of the LED's
   flicker.
4. **Screenshots.** `02-desktop.png` and `13-power-menu.png` shot again with
   the indicator (recommended), or left.

## Phase A: board, server and page (slot 109)

- **H1. boardd reads the ATX LEDs on every turn and keeps the disk's
  activity.** `crates/bli-kvm-boardd/src/lib.rs`, `atx_leds.rs`: the LEDs are
  read on each 20 ms turn instead of with the 1 s sample; `atx_hdd_led_on`
  answers whether the HDD LED was lit at a turn within the last 500 ms, and
  `atx_power_led_on` the power LED's level as read. The status keeps its
  fields. Test (the sampling thread's turns on a played clock): an HDD line
  lit 30 ms every 400 ms is reported active by every status read between its
  blinks (today, read with the 1 s sample, most reads say off).
- **H2. `GET /api/atx/leds`.** `crates/bli-kvm-server`: a protected,
  read-only route answering `{ "powerLed": bool|null, "hddLed": bool|null }`
  from the owner's `BoardStatus`, `null` for an LED the board does not have
  or when the owner does not answer; the inventory gains `hddLed` beside
  `powerLed`. The route lists follow (`tools/test_web_api_routes.py`, the
  route counts in `docs/design/gui-parity-plan.md` and
  `docs/design/phase8-acceptance.md`). Tests: the route's answer from a
  played owner with each LED present, absent and silent; no session, no
  answer (today the route does not exist).
- **H3. The page shows the disk.** `web/src/pages/desktop/menu/power`: the
  toolbar's power button keeps its colour from the power LED and gains the
  "HDD" pill under a 15 px icon, amber while the disk is active, grey while
  idle, absent when the board has no HDD LED or the route fails; the Power
  menu shows "Power on/off" and "Disk" under its title. The page reads the
  route once a second while it is visible and not while it is hidden;
  English and Russian. Tests: a contract through the built page and a played
  board (the pill and the menu follow the route's answers, no pill without
  an HDD LED, no read while the page is hidden; today there is no pill), the
  GUI source tests, and with decision 4 the two screenshots.

Then slot 109 is built, installed at the operator's word, and the watching
probes run. The operator makes the target's disk work and compares the pill
with the case's HDD light.

## Phase W: the record

The review note `docs/baselines/audit-20261111-hdd-indicator.md`: each item's
commit and test, what the board showed, the operator's comparison; the
design notes of the board daemon and of the page; a full `check.sh` on the
final commit. With decision 1, the release follows the recipe at the
operator's word.

## Size

Three items, one slot, and with decision 1 an application release.

## Operator steps

- The decisions above.
- Consent for slot 109's install; making the target's disk work and saying
  whether the pill follows the case's light; the release and the merge.
