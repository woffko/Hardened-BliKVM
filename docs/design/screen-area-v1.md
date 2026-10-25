# The absolute mouse on a desktop of several monitors (2026-10-13)

Plan: `docs/baselines/work-plan-20261013-screen-area.md`.

## The problem

The page sends the absolute mouse as a share of the picture: 0 to 32767 on
each axis, the picture's left or top edge at 0 and its right or bottom edge
at 32767. A target spreads that range over its whole desktop. With one
monitor the two agree. A target that extends its desktop over two monitors
reads the range over both, and the board captures one: on the operator's
target, the captured monitor on the left and another of the same size on
its right, the pointer moved twice as far as the picture said, and the
picture's right half put it on the other monitor.

The board cannot see the target's layout, and nothing in the USB report can
say it: an absolute pointer has one range, which the target maps as it
will. Windows and X11 map it to the whole desktop. A touch screen
(a HID digitizer) could be bound to one monitor in the target's own
settings; that would change the board's USB description and need a setting
on every target, and was not chosen.

## The layout

A layout is the size of the target's whole desktop and the captured
screen's place and size in it, from the desktop's top left corner, in the
target's pixels (as its display settings show them; the capture may be
smaller, which does not matter: the picture is mapped as a share). No
layout means the screen is the whole desktop, as before.

- `web/src/lib/screen-area.ts`: `isScreenArea` (whole pixels up to 32767,
  the screen inside the desktop) and `mapToDesktop`: a position of the
  picture becomes the share of the desktop. The target reads the range as
  32768 steps over the desktop, so each axis is mapped to the screen's
  pixel, kept between the middles of its first and last pixel, and rounded
  down: an edge mapped exactly onto the screen's edge, or rounded up, would
  be the next monitor's first pixel.
- `web/src/lib/mouse.ts`: `MouseReportAbsolute` asks for the layout with
  every report and maps the positions it is given. The page keeps its own
  positions in the picture's terms, so what follows them (the phone's
  cursor, the view that scrolls under it, the trackpad's way) is unchanged.
  The relative mouse is not touched.
- The board keeps the layout for every browser: `GET` and
  `POST /api/hid/screen-area` (`crates/bli-kvm-server/src/screen_area.rs`),
  `{"area": null}` or `{"area": {"desktop": {"width", "height"}, "screen":
  {"x", "y", "width", "height"}}}`. The server checks it and writes
  `screen-area.json` in its state directory (`/var/lib/hardened-blikvm`),
  which a RAW update carries; a missing, oversized or malformed file, or
  one that is not a layout, is none. A refusal leaves the kept layout as it
  was. The card image leaves the source board's layout out. The platform
  names the feature `hid_screen_area`.
- The page reads it when it starts (`pages/desktop/index.tsx`), and the
  Mouse menu's Desktop entry (`menu/mouse/desktop-area.tsx`), on a desktop
  and on a phone, sets it: the whole desktop, or part of one with the six
  numbers typed in; "Left of two" and "Right of two" fill them for two
  monitors of this screen's size side by side. The fields keep their keys
  from the target.
- While the target agent's monitors make a layout, the mouse uses theirs
  and the page keeps it on the board; the Desktop entry then says it is the
  agent's and offers nothing of its own (`docs/design/target-agent-v1.md`).

## Tests

- `tools/test_screen_area.mjs`: layouts that are and are not; without one
  the position is the picture's; for three layouts (this screen on the
  left, on the right, smaller and lower than its neighbour) every seventh
  pixel of the picture lands on its own pixel of the screen as the target
  reads the range, and the corners on the screen's own first and last
  pixel; the report's bytes.
- `crates/bli-kvm-server/src/screen_area.rs` and the server's route test:
  the checks, the file, a refusal without CSRF or for a screen outside its
  desktop leaving the layout, the whole desktop again.
- `tools/playwright/screen_area_contract.mjs`: "Left of two" written
  through the menu; the mouse's reports then on the left monitor (the
  picture's right edge on columns up to 2559 of 5120, its middle on 1280);
  read back after a reload; the whole desktop again; a finger on a phone
  stopping at the screen's last column; Russian.

The side panel's divider (the PicoClaw sidebar's splitter, disabled while
the panel is closed) lies over the picture's last two CSS pixels on a
desktop; the contract points three pixels in from the edge.
