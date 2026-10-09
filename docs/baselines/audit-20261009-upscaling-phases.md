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

## Slot 80: gate 2 again, gates 3 to 5

`scripts/check.sh` green on `ecdb2bc` (its first run stopped at the
formatting check on one import's order, fixed in `ecdb2bc`). Slot 80
(0.1.0-beta.4, source `ecdb2bc`) was installed on the test board through
the update API with the operator's agreement. Against the state before,
only the application slot changed (79 to 80); the video settings (1920x1080
at 60, JPEG 100, the video engine for H.264, WebRTC 8000 kbps), the
firewall's baseline mode, QUIC's switch and the switches are as found, and
the update runtime's files are the board's.

On the board (read-only, headless, upscaling off as a software-drawn
browser has it): `quic_video_device_probe` 30 s with the monitor in
Extended, 789 frames in 33.65 s by the board's clock, a frame's age 78 ms
in the median and 92 ms at the 95th percentile, one session, no decoder
restart, no frame given up, no key frame asked for; `gui_walk_device` in
both layouts and `vp8_webrtc_probe` in H.264 and VP8 passed.

The operator's phone (Auto, Fill): over QUIC the picture no longer stood
still; VP8 and MJPEG were sharp and steady; a pinch to 400 % was smooth and
touches landed where they were made at 100, 200 and 400 %. The operator
then reported the device as very slow, the processor plainly overloaded:
open. The board itself was idle when looked at (94.5 % idle, load 0.22,
60 C, its four cores at their 1.512 GHz limit); while the operator watched
H.264 its encoder made about 23 frames a second, decoding each of the
bridge's JPEG frames in 40 to 47 ms on the processor (28 ms on slot 79's
screenshot), about one core, as the monitor's encoder-unit row says.

### The slow device: the board's Wi-Fi on the viewers' network

Measured on the board (read-only) while the operator watched, at the
operator's word: the video left through the board's Wi-Fi (4 to 5 Mbit/s
out of `wlan0`, nothing out of `eth0`), the H.264 encoder took 108 to 117 %
of a core, `ustreamer` about 29 %, the QUIC gateway about 19 %, the load
2.3 to 2.5. The board is on two networks: Ethernet (100 Mbit/s) on the lab
network, and its Wi-Fi (SDIO, -67 dBm, a 72 Mbit/s link) on the network of
the operator's devices. A page opened at the board's Ethernet address
reaches it through the router, and the board answers straight onto the
viewers' network through its Wi-Fi: an asymmetric path, whose driver took
more than a core under load (`RTWHALXT` 78 %, `RTW_XMIT` 38 % in an earlier
sample) and which cannot carry MJPEG's 100 Mbit/s at all. Upscaling off
made no difference (the operator's comparison), and slot 80 changed no
code on the board. The same MJPEG stream pulled by the host over Ethernet
came at 45.6 frames a second, 102 Mbit/s, the Wi-Fi idle: one viewer of
MJPEG at 1920x1080 fills the board's 100 Mbit/s Ethernet. The board was
rebooted at 16:22 UTC in an orderly way (`systemd-logind`: "System is
rebooting") by the operator, while everything was slow; these checks only
read. The Wi-Fi and
the network are the operator's settings and were not changed.

The operator took the board's Wi-Fi off the network (the board's own
setting, by the operator). Measured again while the operator watched over
QUIC on the phone and the desktop: the video left through Ethernet (2 to
5 Mbit/s), the Wi-Fi was idle, the H.264 encoder took 71 to 124 % of a core
and `ustreamer` 22 to 31 %; the operator found the slowness gone.

## QUIC protocol 2 and the bitrate share (host)

Over slot 80 the operator found the QUIC picture less smooth than on slot
79, the quality monitor counting decoder restarts, and asked that the
transport work at least as the reference's `experimental` branch does
(`docs/design/quic-transport-v1.md`, "Protocol 2: groups of pictures"). The
comparison found that protocol 1 broke the chain of delta frames at every
lost datagram, had no reference barrier, decided a decoder had fallen
behind by 12 frames in its queue (which a burst of frames passed at once),
and had nothing that fitted the bitrate to the path. The operator chose the
barrier and a bitrate fitted to the path, and then a stream for each group
of pictures (2026-10-09). The commits:

- `0fd9911`: the page's receiver holds complete frames up to 2 s for their
  key frame (120 frames, 16 MiB); its decoder falls behind by time (a frame
  handed to it 1 s ago not yet back).
- `f1a2285`: protocol 2. A group of pictures on a unidirectional stream of
  its own, the transport sending again what the path loses; BBR instead of
  Cubic; the page says in its pings which frame it last received whole,
  and the gateway judges by that what is on its way (the transport takes
  everything into its own buffer at once): a slow key frame is waited for
  and the next one left out, a group is given up only when the page is
  more than 1.5 s behind with more on its way than a key frame, and a page
  that gives the picture up has its streams reset.
- `e12f16e`: the share of the chosen bitrate. Each session asks for what
  its path carries (down by a quarter when a queue builds, up by a fifth
  after 10 s of keeping up, never below a quarter); the server runs the
  shared H.264 encoder at the chosen bitrate times the least share of the
  sessions that watch, without storing anything, and the quality monitor
  shows it.
- `3705af5`: the page says each frame as it arrives (`received`) instead
  of in its pings, whose timer a hidden page's browser slows down (Chromium
  to once a minute after five minutes): the gateway would have taken that
  for a queue and lowered every viewer's bitrate.

Besides, `350fe35` lets the board's video probe narrow its path
(`RATE_KBPS`), and `ec4cebf` runs the video contract's phone scenario at
10 frames a second: SwiftShader upscales the zoomed picture at 20 to 29
frames a second, so at 30 the watchdog stopped upscaling, and the pinch's
check failed in two runs of five.

On the host, through the relay, 1280x720 from the test source, plain
drawing, 25 s each: clean 30.0 frames a second (age 20 ms at the 95th
percentile); 40 ms each way and 3 % lost 30.0 (151 ms); 40 ms and 10 %
29.8 (197 ms); 100 ms and 3 % 30.3 (564 ms); no frame given up and no key
frame asked for in any. Protocol 1 with the barrier drew 7.5 frames a
second on the 40 ms, 3 % path, gave up 455 frames and asked for 54 key
frames; the groups' streams with Cubic drew 5.9. A path narrowed to half
the picture's rate (`quic_share_contract.mjs`, the source at a chosen
800 kbit/s): the session asked for 75, 56, 42 and 31 % (in one run 25 and
then 30) within 8 s, and the page kept up after about 26 s at 29.8 frames
a second, its oldest frame 250 to 450 ms old; wide again, the share
climbed. Before the rules on what is on its way, the same path stood the
picture still for 11 s and the page gave the session's picture up.

The QUIC contracts (transport, input, input through the stack, video,
share) passed on the host, the video contract twice in a row after
`ec4cebf`. The board still runs slot 80 (protocol 1): the
next slot carries protocol 2, the page and the gateway together, and is
installed only with the operator's agreement, then checked on the board
and by the operator at gates 2 to 5.

Receipts: `evidence/private/upscale-20261008/quic-protocol2/`.
