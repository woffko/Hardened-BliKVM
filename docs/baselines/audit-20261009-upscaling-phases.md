# Screen upscaling, phases 1 to 7: gates and slots (2026-10-09)

The record of `work-plan-20261008-screen-upscaling.md` after gate 0
(`audit-20261008-upscaling-gate0.md`), gate by gate. Receipts, probes and
the operator's screenshots are under `evidence/private/upscale-20261008`
(not in Git).

## Gate 1: the rendering core (host)

Phase 1 (`cbe5129` to `424b3ac`): the renderer agrees with its CPU reference
within one level in 24 cases (every mode, VP8's table, 4/3, 2 and an uneven
ratio, a part of the frame, made smaller, an ImageBitmap, an `<img>` and a
VideoFrame); frames of an unchanged size make no GL object; a lost context
draws nothing and draws again once restored; the pointer lands on the same
picture pixel at 100, 150 and 200 % with pixel ratios 1 and 2, and with a
capped buffer of another shape; the enlargement is reported and kept
current. `scripts/check.sh` green. Two changes to SGSR came out of the
reference: its bilinear sample from texels, and the edge threshold of 8.37
levels instead of 8, where a 4-level difference tied and each GPU broke the
tie its own way (up to 23 levels apart).

## Gate 2: QUIC

Phase 2 (`5b270b4` to `7b5de16`, with `29e638d` and `b96e5f0` after the
operator's check): the QUIC view through the upscaler, the setting and the
Screen menu's entry (built ahead of phase 6 so that the operator could
switch it on), the quality monitor's row, and the QUIC contract upscaled
at 200 %. `scripts/check.sh` green on `7b5de16`.

Slot 79 (0.1.0-beta.4, source `7b5de16`) was installed on the test board
through the update API with the operator's agreement. Against the state
before, only the application slot changed (78 to 79); the video settings
(1920x1080 at 60, JPEG 100, the video engine for H.264, WebRTC 8000 kbps),
the firewall's baseline mode, QUIC's switch and the switches are as found.
`quic_video_device_probe` on slot 79 (headless, upscaling off): 520 frames
in 30 s, a frame's age 66 ms in the median, nothing dropped.

The operator's phone (Chrome on Android, Adreno 830), H.264 over QUIC,
Auto, the picture zoomed in: the monitor's row said the picture was drawn
by deringing and SGSR, and the browser received, decoded and drew 14.9,
15.9 and 15.9 frames a second and dropped none in that sample. The QUIC
path lost datagrams over the operator's VPN (1140 lost, 96 frames given
up, 32 key-frame requests). Two findings on the way:

- The phone first showed no change: it was on WebRTC, which phase 3
  upscales; and at the phone's own fit (0.3 of the picture's width, times
  its pixel ratio of 3.25) the picture is shown at its size or smaller, so
  Auto rightly draws it plainly until it is zoomed in. Upscaling makes an
  enlarged picture sharp; it does not enlarge it. Filling the window beyond
  100 % is phase 5's.
- The monitor's row named the sharpness by a Screen menu key the monitor's
  worker does not have (`screen.upscaleMedium`), and was cut on a phone:
  fixed in `29e638d` and `b96e5f0`, for the next slot.

The desktop's check was left for the next slot by the operator.

### Slot 79: the picture stood still with upscaling on

Using the phone further, the operator found the picture standing still now
and then and coming back soft, and recovering; with upscaling Off at the
same zoom and on the same path it did not (the operator's comparison). The
monitor at such a moment: path QUIC, 0 frames a second received, no round
trip and no gateway counters (the session was not open), 1428 frames
received, 161 given up, 51 key frames asked for. The gateway's log for the
operator's checks (read-only): sessions lived 5 to 110 s, and most ended
a second or so before the next one opened, as when the page gives a session
up (six seconds without an answer to its pings) and opens another; once
two were still counted and a third was refused for the login's limit of
two (`Sessions`), which the page did not try again: H.264 stayed on WebRTC until QUIC was switched off and on,
as the operator also reported ("QUIC is not active until I toggle it").
Gate 2 was missed on slot 79, and phase 2's time box of 4 hours was spent
(about 7.5 hours with the checks and the waits); the operator decided to
fix the causes found and check gate 2 again on the next slot, with gates 3
and 4.

What was found, and its fix:

- An upscaled draw slower than the frames come makes the QUIC worker's
  decoder fall behind; at twelve frames waiting it is started again from a
  key frame, and the picture stands still until one comes (the hardware
  engine's key frame comes at a higher QP to fit its one-second budget,
  hence soft). On the host, SwiftShader drawing a 1917x1078 or 3839x2160
  buffer from a 1280x720 frame at 30 a second: 11 and 5 frames drawn a
  second, 27 and 30 decoder restarts and key-frame requests in 20 s, whole
  seconds without a frame; upscaling off, 30 drawn and none. Handing the
  decoded frames over to a task of their own changed nothing (the decoder
  itself slowed), and was left out. The plan's watchdog (phase 6) is built
  now for QUIC (`8cb812c`): fewer than 90 % of the frames handed to the
  decoder drawn over 5 s stops upscaling for the page load with a notice;
  frames the network loses are not handed to the decoder and do not count.
- Mounting the new canvas when upscaling starts or stops unsubscribed and
  subscribed again at once, and the gateway's answer to the
  unsubscription ("stopped") came after the new subscription and sent
  H.264 to WebRTC for the page load: switching upscaling in the Screen
  menu always did so, as the watchdog's first run on the host showed
  (`8ad95e2`).
- The monitor did not show the decoder's restarts nor sessions lost and
  opened anew: both are rows now (`470ae4c`).
- A session lost and not regained within its three attempts, or refused
  for the login's limit, left the page on WebRTC for its load: the page
  tries again by itself when it is looked at again and every 35 s, and the
  picture comes back over the new session (`28bee3f`).

Not proven: why the phone's upscaled draw would be slow at all (gate 0
measured 1.09 ms a 2560x1440 frame on its GPU), and whether the session's
losses come from the path alone. Slot 79's row was cut (`screen.ups...`)
and showed no time; the next slot shows the draw's time, the decoder's
restarts and the sessions opened, and the watchdog switches upscaling off
where it loses frames. The operator also noted that a pinch zooms to
400 % while the menu stops at 200 %: on the phone the buffer reaches
3840x2160 at about 320 % upright and 180 to 210 % on its side, and the
browser stretches it beyond that (phase 5, item 4).

## Phases 3 and 4: WebRTC and MJPEG (host)

Phase 3 (`601b047` to `e897044`) and phase 4 (`c4a848c`, `6470bb9`) were
built while gate 2 waited for the operator, and the watchdog came to both
views with the fixes of gate 2 (`5111093`). On the host, in Chromium
drawing with SwiftShader and named a GPU:

- WebRTC (`vp8_range_contract.mjs`), upscaled at a pixel ratio of 2: VP8's
  greys within 0 levels of the JPEG with the same samples (gate: 1.5) and
  its colours within 2 (gate: 4), H.264 within 1 (gate: 1.5), the buffer of
  the shown device pixels, no SVG filter, the mouse within 2 of 32767; a
  draw slowed by 240 ms a frame stopped upscaling after 4.4 s, and the
  video played as the screen.
- MJPEG (`mjpeg_upscale_contract.mjs`), at a pixel ratio of 2: the patches
  within 0.0 levels of the plain image, the squares' edges rising in
  0.88 px against 1.10 px of the browser's own scaling of the image (at
  most 0.85 of it), the mouse where it is; with a stream that goes on at 20
  frames a second a slowed draw stopped upscaling after 5 to 9 s, and the
  image was the screen again.

Gates 3 and 4 are to be checked on the operator's devices with the next
slot.

## Phase 5: larger than 100 %, and phones (host)

Items 1 to 3 (`3d9fefc`, `8a2f845`, `eb6f0ac`): an upscaled picture fills
the view or is laid out at the target's resolution, set in the Screen
menu; a pinch keeps the canvas's buffer while the fingers move (on the
host 1280x720 throughout, 2304x1296 once they stopped); a hidden page
draws nothing (12 s hidden on the host: no frame drawn, H.264 kept on the
session, the picture on again once seen, no notice from the watchdog).

Item 4 (drawing the visible region only) is not built: the plan builds it
if the capped buffer costs a phone too much. The operator's note that a
pinch zooms to 400 % shows the cap's other cost, the browser stretching
the capped buffer beyond about 320 % upright and 180 to 210 % on its side;
it is judged at gate 5, whose touches are checked at 400 % as well as at
100 and 200 %.
