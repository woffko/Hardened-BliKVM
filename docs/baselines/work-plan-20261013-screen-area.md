# Work plan: the absolute mouse on a target with more than one monitor (2026-10-13)

The operator's target extends its desktop over two monitors, and the board
captures one of them. The page sends the absolute mouse as a share of the
picture (0 to 32767 on each axis), and the target spreads the absolute
range over its whole desktop: the pointer lands in the wrong place, the
farther the more, and the other monitor takes half of the picture's width.
The board cannot see the target's layout. The operator chose a setting kept
on the board: the size of the target's whole desktop and where the captured
monitor lies in it; the page then maps the picture into that part of the
absolute range.

Rules as in the earlier plans: English repository text, secrets only through
`project-memory-exec`, one commit per item with its own tests,
`scripts/check.sh` green before the slot build, the slot installed through
the update API on the test board at the operator's word and verified there,
the operator's settings ending as found. On the board the agent sends no
input to the target: the operator moves the mouse.

## Items

1. The mapping (`web/src/lib/screen-area.ts`): a layout is the desktop's
   width and height and the captured screen's left, top, width and height in
   it, in the target's pixels, the screen inside the desktop; none means the
   screen is the whole desktop, as now. A point of the picture (a share of
   its width and height) becomes the share of the desktop the absolute
   report carries. The absolute report (`MouseReportAbsolute`) maps every
   position it is given; the page keeps its own positions in the picture's
   terms, so the cursor the phone draws and the view that follows it are
   unchanged. The relative mouse is not touched.
2. The board keeps the layout (`GET` and `POST /api/hid/screen-area`): the
   server checks it (whole pixels, at most 32767, the screen inside the
   desktop) and writes it in its state directory, which a RAW update
   carries; a new card starts without it.
3. The page reads it when it starts and after it is changed; the Mouse menu
   has a Desktop entry: the whole desktop, or the desktop's size and the
   screen's place and size typed in (the fields keep their keys from the
   target, as the Screen menu's do), with a help line; English and Russian.
4. Tests: the mapping and its edges (a unit test); the route (checks, the
   file, a layout that is not one refused); the page with a layout set: the
   mouse and a finger send the mapped position, the menu writes what was
   typed, in both languages (contracts).
5. Records: `docs/design/screen-area-v1.md`, the README's features, the
   route counts, the audit note.

## On the board

A slot from the commit `scripts/check.sh` passes on, installed at the
operator's word. The operator sets the layout of the target (the captured
monitor on the left, the other on the right) and moves the mouse to the
captured monitor's four corners and its middle, plainly and with the
picture enlarged, and on the phone. The agent's probes check that the page
reads and writes the layout and sends nothing else.
