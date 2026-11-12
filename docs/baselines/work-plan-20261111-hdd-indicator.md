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
   **Changed by the operator on 2026-11-12**: a socket that sends each
   change, beside the light route (decision 3, phase B).
3. **Timing.** The disk counts as active when its LED was lit within the last
   500 ms; the page reads once a second while it is shown (recommended). The
   indicator is therefore lit while the disk works, not a copy of the LED's
   flicker.
   **Changed by the operator on 2026-11-12** (with decision 2), after slot
   109's comparison: as KVMD does it. KVMD (on the board,
   `kvmd/plugins/atx/gpio.py` and `kvmd/aiogp.py`) waits for the line's
   edges, changes the LED's value at most once per 100 ms
   (`hdd_led_debounce: 0.1`), and sends each change to the page over its
   socket at once. Read once a second with the 500 ms window, the pill stayed
   amber while the case's light went on and off; read four times a second,
   it would stand amber or blink out of step, the case's light changing in
   steps of 250 ms on this target. See phase B.
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

## Phase B: as KVMD shows it (slot 110)

The operator's change of decisions 2 and 3 on 2026-11-12. Slot 109 stays a
test build: the release, with decision 1, is slot 110.

- **H4. boardd holds each change of a case LED at least 100 ms.**
  `atx_leds.rs`: on each 20 ms turn the reported level of each LED follows
  the line's, but changes at most once per 100 ms, to the level read at the
  turn, as KVMD's debounce does; the 500 ms window of H1 goes. The status
  keeps its fields. Test: a disk LED lit for one turn every 400 ms reads lit
  for 100 ms after each flash and idle until the next (today it reads lit
  throughout), and a steady change shows at the next turn.
- **H5. The board owner reads boardd's status outside its request lock.**
  `crates/bli-kvm-control-broker`: `BoardStatus` read boardd's socket with a
  blocking call while it held the lock every V2 request takes, the HID
  lease's among them. The read now runs on a blocking worker with the lock
  released. Test: while boardd takes 600 ms to answer a status read, another
  V2 request is answered at once (today it waits for the read).
- **H6. The case LEDs as a socket.** `crates/bli-kvm-server`: a protected,
  read-only WebSocket `GET /api/atx/leds/socket`, checked for its origin like
  `/api/ws`, sends `{ "powerLed", "hddLed" }` when it opens and at each
  change. One task reads the owner every 50 ms while a socket is open and
  none while none is; at most 16 sockets are open. A socket ends at its
  session's end and at the server's stop, and frames from the page are not
  read. The route counts follow (128). Tests: the first answer and each
  change from a played owner; nothing read with no socket open; the socket
  ends at logout and at the stop; no session, no socket.
- **H7. The page listens to the socket.** `web/src/hooks/useCaseLeds.ts`:
  while shown, the page opens the socket and shows each change; hidden, it
  closes it; when the socket closes, it reads `/api/atx/leds` once a second
  until a new socket opens (tried again after 2 s). The contract follows:
  the pill follows the socket's answers within a frame, no socket while the
  page is hidden, and the read once a second while the socket is down.

Then slot 110 is built, installed at the operator's word, the watching
probes run, and the operator compares the pill with the case's light again.

## Phase W: the record

The review note `docs/baselines/audit-20261111-hdd-indicator.md`: each item's
commit and test, what the board showed, the operator's comparison; the
design notes of the board daemon and of the page; a full `check.sh` on the
final commit. With decision 1, the release follows the recipe at the
operator's word.

## Size

Three items and one slot; with the change of 2026-11-12, four items more
and a second slot; with decision 1 an application release.

## Operator steps

- The decisions above.
- Consent for slot 109's install; making the target's disk work and saying
  whether the pill follows the case's light; the release and the merge.
