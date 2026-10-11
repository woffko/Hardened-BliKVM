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
`ec4cebf`. The full check then failed once in the MJPEG upscaling
contract (17.9 of the live stream's 20 frames a second drawn in
SwiftShader on a busy host, against 90 %); `e508d76` draws that stream at
10, and the full check passed on it.

The gateway is no part of the application slot: the update-runtime
package installs it, and the plan keeps the board's system as it is. The
built package differed from the board's `0.1.0~dev36` only in the gateway
and its manifest. Asked, the operator chose to update the gateway on the
board (2026-10-10): `1900dae` makes it `0.1.0~dev37`, installed with the
next slot through `dpkg`, `0.1.0~dev36` as built for slot 80 (the board's
gateway, `7e3683e8`) kept to go back to. The server and the gateway say the
same protocol number on the link, so the two go on together. The operator
also allowed a probe on a narrowed path, which lowers the H.264 bitrate of
every viewer for a while.

Receipts: `evidence/private/upscale-20261008/quic-protocol2/`.

### Slot 81 and update-runtime dev37 on the board (2026-10-10)

The full check passed on `ee84786`, from which slot 81 was built. At the
operator's word it went onto the board through the update API, and
update-runtime `0.1.0~dev37` through `dpkg` right after it (between the
two, 18 s, the board's old gateway refused the server's link protocol 2);
the gateway was restarted, as its unit had been active. The board's state
after it differed from before only in the confirmed slot (80 to 81), the
previous one (79 to 80) and the runtime (dev36 to dev37): switches,
firewall mode, video settings and the H.264 engine as found.

| Path, board | Frames a second | Age, median / 95th percentile | Given up / key frames asked / decoder restarts |
| --- | --- | --- | --- |
| direct, 30 s (the target's screen nearly still) | 8.3 | 54 / 124 ms | 0 / 0 / 0 |
| 3 % lost and 40 ms each way, 60 s | 13.5 (the encoder's 11.7) | 147 / 273 ms | 0 / 0 / 0 |
| narrowed to 3 Mbit/s, 1 s queue, 120 s | 8.6 | 241 / 778 ms | 0 / 0 / 0 |

On the narrowed path the encoder's journal shows the share at work on the
operator's 8000 kbit/s: 6000, 7200, 8000, 6000, 7200, 8000, 6000, 4480,
5360, 6400 and 4800 kbit/s, and 8000 again as soon as the next viewer of
H.264 came. `gui_walk_device` in both layouts and `vp8_webrtc_probe` in
H.264 and VP8 passed.

The operator, the pages reloaded: QUIC smooth without freezes on the phone
and the desktop; VP8, MJPEG and the pinch as on slot 80. The phone's
monitor: no frame given up, no key frame asked for, no decoder restart,
737 packets lost on its Wi-Fi and sent again, the share at 100 %, two
sessions opened. The board's journal explained the second session: twice
the phone's page was frozen with another app in front, gave its session
up when it ran again, and opened another, while the gateway kept the old
one open for the transport's 30 s; that session said nothing, looked
further behind every second, and within 9 s took the encoder to a quarter
of the chosen bitrate for every viewer. `1256462` leaves a silent page
unjudged, `28e3c01` no longer counts a frozen page's time against its
session, `8b0758e` calls the lost packets packets, and `b76e555` makes the
gateway update-runtime `0.1.0~dev38`.

Receipts: `evidence/private/update-to-seq81-010-beta4-202610100707.json`,
`quic-video-device-seq81*.json`, `gui-walk-seq81-*.json`,
`webrtc-*-seq81.json`, `upscale-20261008/board/` (state, facts, the
encoder's journal on the narrowed path, the phone's sessions).

### Slot 82 and update-runtime dev38 on the board (2026-10-10)

The full check passed on `4b863ee`; at the operator's word slot 82 went
onto the board through the update API and update-runtime `0.1.0~dev38`
(only the gateway differs from dev37) through `dpkg`, the gateway
restarted. The board's state differed afterwards only in the slots and the
runtime. The direct probe passed (no frame given up, no key frame asked
for, no decoder restart, the share whole, "lost packets 0"). With the
page's thread held for 12 s after the warm-up (`FREEZE_MS`, `73ffd78`), the
page kept its one session, and the encoder's journal shows
no change of the bitrate in that time: the silent page held nobody back.
`gui_walk_device` in both layouts and `vp8_webrtc_probe` in H.264 and VP8
passed.

Receipts: `evidence/private/update-to-seq82-010-beta4-202610101004.json`,
`quic-video-device-seq82*.json`, `gui-walk-seq82-*.json`,
`webrtc-*-seq82.json`, `upscale-20261008/board/` (state, facts, the
journal of the frozen page's probe).


### The operator's items for phase 5 (2026-10-10)

Checked on slot 82, the operator found QUIC well; then asked that the
quality monitor fit a phone and count something for gate 5's processor
comparison (the page cannot read the browser's processor use, and the
operator chose a count in the monitor over a remote DevTools profile), and
reported the phone's cursor drawn off the target's own. Added to the plan
(`f7560ce`):

- `6144c48`: the phone's arrow was placed by its box's corner, and lucide's
  arrow draws its tip 3.1 px inside the box; the tip of the arrow's outline
  is now the point (the touch contract measured it 3.7 px off before).
- `63d5d4e`: on a phone Extended spans the screen's width and is as tall
  as its rows; labels and values go on another line rather than be cut
  short (over QUIC on a phone's size in Russian: 26 rows in 374x457 px,
  none cut, one on two lines; on its side the window scrolls). The Browser
  section counts the share of the last second the page's own picture code
  took on the main thread and in the QUIC picture's worker. The idle time
  the browser gives the main thread was tried first: on the phone's
  landscape layout in SwiftShader it said 100 % busy while a profile of the
  thread showed it 99 % idle.

## Gate 5 on the operator's phone (slot 83, 2026-10-10)

The operator's Android phone over QUIC, H.264 from the video engine at
1920x1080, the same zoom, Extended open:

| | Upscaling | Frames a second | Upscaled, drawn / came | Picture code, page / worker | Given up, key frames asked, decoder restarts |
| --- | --- | --- | --- | --- | --- |
| Off | none | 10.0 | - | 1 % / 2 % | 0, 0, 0 |
| Auto | SGSR, high, 0.80 ms a frame | 5.0 | 11.4 / 11.4 | 1 % / 1 % | 0, 0, 0 |

The page's own picture code costs no more with upscaling than without, at
the scale of one percent of a core; the browser's own decoding, drawing
and its GPU process are not in that count, which the page cannot read
(the target's screen changed at different rates in the two minutes, 5 and
10 frames a second). With the pinch smooth and the touches where they were
made at 100, 200 and 400 % (slot 80), the monitor fitting the phone and the
cursor's tip on the target's cursor (slot 83, the operator's word), gate 5
passed. The 244 and 199 packets lost on the phone's Wi-Fi were sent again.

## Phase 6 and gate 6

The phase's items were built before its gate, in the phases that needed
them: the Screen menu's Upscaling entry with Off or Auto and the sharpness,
kept by the browser, Auto only with a GPU and an enlargement above 1.1
(phase 2); the watchdog in every view with its notice and the monitor's
rows (`8cb812c`, `5111093`); the plain path where the browser draws in
software, without WebGL2 or with a lost context (`58abf48` and phase 1);
English and Russian texts; the contracts of the menu and its memory, of
Auto's conditions, of the watchdog with a slowed draw and of a browser that
draws in software (`quic_video_contract`, `mjpeg_upscale_contract`,
`vp8_range_contract`, `screen_enlargement_contract`). At gate 6 the operator
chose Auto as the default (2026-10-10, "6 авто"), and said to release
everything: the application, the image and the RAW system bundle, whose
update-runtime carries the QUIC gateway of protocol 2.

## Phase 7: release (2026-10-10 and 11)

### The picture against MJPEG on the board

The release check reads the picture of each mode against the MJPEG frame
with upscaling on at 133 and 200 %
(`tools/playwright/upscale_colour_device_probe.mjs`). Its first versions
compared a plain and an upscaled browser read at the same moment: QUIC came
out within a tenth of a level, MJPEG and both WebRTC views about two levels
darker in the median patch with single patches 55 to 143 levels apart, also
with the target's screen kept still by the operator. The boxes the probe
read told why: at 133 % the plain image and video lay at the view's left
edge (x = 1), the upscaled canvas and the QUIC canvas at its middle
(x = -560), so the probe compared different parts of the screen. An image
or a video has no size of its own before its first frame, and the screen,
a flex item, was shrunk to the view's width; the view's centring, which
runs when the zoom, the fit or the resolution changes, found nothing to
scroll sideways, and the picture, once its frame came, was shown from its
left edge. A trace of the view's scroll writes on the board showed the
plain VP8 view set to 0 three times while the video was 1438 px wide, and
never after it grew to 2560. The watchdog's switch from an upscaled canvas
to the plain image did the same. The plain path did so in beta.3 too.
`29c6eb0` lays every view's screen out at the size it is drawn at from the
start; the MJPEG contract checks an enlarged image whose first frame comes
late and the watchdog's switch on an enlarged picture, and both fail
without the change ("the enlarged image at 1,-180, the view's middle at
640,360"; "the image at 1,-90 1600x900, the canvas was at -160,-90").

The probe (`0484e81`, `69dd743`, `5175211`) shows the plain view and the
upscaled one in browsers of their own beside the plain MJPEG picture, all
three given a GPU's renderer name so that the page draws as for a GPU, and
fails when they are not laid out alike (on slot 83: "the plain picture
laid out at 1,-220 2560x1440, the upscaled one at -560,-220"). The board's
video proxy serves two MJPEG streams at once (`max_connections`): a third
waited until the probe gave up, so the MJPEG browser is also the plain
MJPEG view. A reading counts where at least 90 % of the patches show the
same in the plain and the upscaled picture (each channel within 6 levels);
an upscaled canvas not yet drawn by SwiftShader is read again.

On slot 84, with the target's screen still for five minutes at the
operator's word, every view was read at the first attempt with all 96
patches the same. The upscaled picture against the plain one, red, green
and blue, as a whole:

| View | 133 % | 200 % |
| --- | --- | --- |
| MJPEG | -0.01, -0.09, -0.01 | +0.12, +0.09, +0.10 |
| VP8 over WebRTC | +0.01, -0.22, +0.15 | +0.11, -0.08, +0.12 |
| H.264 over WebRTC | -0.02, -0.09, -0.04 | +0.18, +0.04, +0.04 |
| H.264 over QUIC | +0.01, -0.10, +0.02 | +0.14, +0.08, +0.06 |

The first criterion, the upscaled picture within one level of the plain
one in each tone, failed by 0.12 in one place: VP8's middle tones at 133 %
(12 patches), green +1.12. Read against MJPEG the reason shows: there
VP8's plain path, its SVG range filter, draws green two levels below MJPEG
(127 for 129), the upscaler's shader table one (128); the upscaled picture
is the nearer (middle tones from MJPEG: plain +0.74, -1.87, +0.50,
upscaled +0.44, -0.96, +0.19). The probe now judges as the goal states it:
the upscaled picture no further from MJPEG than the plain one, and one
level, on each channel, as a whole and in each tone with four patches or
more. It passed in every view at both sizes
(`evidence/private/upscale-colour-seq84d.json`).

### The board's display (operator's item)

At the operator's word (2026-10-10, "добавь в план отключение LCD дисплея
как это в харденед наноквм сделано"; chosen: in beta.4, before slot 84,
with Hardened NanoKVM's choices), the board's ST7789 goes dark after a time
without activity (`d5f5dd2`; `docs/design/board-daemon-v1.md`, "The
display's sleep"). On slot 84 the operator set 15 seconds: the display
went dark; the button lit it once held for about three seconds (the
daemon samples the button once a second; `buttonWakeups` 1 after it), and
the operator kept three minutes.

### Slot 84 on the board

`scripts/check.sh` green on `d5f5dd2`. Slot 84 (`0.1.0-beta.4`, app tar
`073e622bf0e656f05bc67f42430544d4650652796afcc3a36cedfedc8f798ea8`),
installed through the update API at the operator's word and confirmed;
the board's state changed only in its slots and in the new `device_oled`
feature. Probes, all passed: the GUI at a desktop's and a phone's size
with the quality monitor open (12 and 11 API calls, none failed); H.264
and VP8 over WebRTC; the phone's reach; the engine chosen in the Screen
menu each way over QUIC (the encoder 472 ms, the monitor 814 ms) and over
WebRTC (404 and 822 ms), ending on the hardware engine; the QUIC picture
for 30 s with the monitor (744 frames, 1049 sent by the gateway, none
dropped, no packet lost, the share 100 %).

That first QUIC reading, right after the install, found every frame 4.05 s
old when drawn (95th percentile 4.06 s), the page's own delay 4 ms, where
slots 81 to 83 read 54 to 68 ms; after the engine probe the same probe read
54 ms. The picture was not late: the age was misread on the development
host. The page answers pings on its thread and draws in its worker, and
handed the arrival of each answer to the worker as `timeOrigin + now`,
which the worker turned into its own clock by its own `timeOrigin`. Each
origin is the wall clock when its context began, and this host's wall
clock is kept by steps of about a second against its slow monotonic clock:
workers begun one by one in a page read 3, 929 and 1858 ms apart from it
within 80 s. A worker begun four steps after its page put every frame four
seconds older than it was, for the page's life; the quality monitor, which
shows the worker's numbers, said the same. The capture times and the
board's clock were right (both the board's CLOCK_MONOTONIC), the encoder
holds only its newest frame at each stage, and the gateway saw nothing
behind (no group given up, no key frame asked for); freezing the gateway
for 3 s on the host left the age at 35 ms once it went on.
`tools/playwright/quic_video_contract.mjs` now gives the worker an origin
4 s off the page's: without the change the picture reads 4006 ms old, with
it 6 ms. The change (the page hands its times over by its monotonic clock,
`PageClock`) came after the release; a browser whose wall clock is stepped
between a page's start and its picture's (a virtual machine, a clock
corrected at once) shows a wrong age in beta.4's monitor, and nothing
else.

### The release 0.1.0-beta.4 (2026-10-11)

At the operator's word ("выпускать всё", and "Публикуем" before the
upload), published as a GitHub pre-release:
https://github.com/woffko/Hardened-BliKVM/releases/tag/v0.1.0-beta.4, the
annotated tag `v0.1.0-beta.4` on `2fc3d5c` (slot 84's source `d5f5dd2`, the
colour probe's last changes, `5175211`, from which the image and the RAW
bundle were built, and documents). Assets, SHA-256 as GitHub reports them
and as computed here:

| File | SHA-256 |
| --- | --- |
| `hardened-blikvm-h616-sd.img.zst` | `3be98a87fefba0799c0af49f9653ced4145f527180792c95f004597ec3d4315c` |
| `hardened-blikvm-system-0.1.0-beta.4-seq6.tar` | `4408ec5253b85c97f2358701e8c2d8c10b07d44adc542ec20bb72f4f4f808739` |
| `hardened-blikvm-application-0.1.0-beta.4-seq84.tar` | `073e622bf0e656f05bc67f42430544d4650652796afcc3a36cedfedc8f798ea8` |
| `IMAGE-INFO.txt` | `5f114387d442e338c76b6b8c1623c8fc1b49e002cc3c953be0648cc45fe0b48b` |
| `SHA256SUMS` | `810619fc59ad2b6c03b47d5b26d14a41fd8167ea22db2c26df93061aa45a741c` |

The unpacked image (`5f1c7bae…`) is the `.zst`'s content, and its first
partition the root file system the RAW bundle was made from (`fd11d51a…`):
update runtime `0.1.0~dev38` (gateway `3d2e6c8c`), beta.3's encoder
(`a09d9750`), slots 81 to 84, and none of the board owner's state files.
The bundle's manifest (sequence 6) verified with the system key. The image
was read from the board with no page or QUIC session open on it, at the
operator's word.

The RAW update from that bundle was installed on the test board onto
beta.3's system with slot 84 (`evidence/private/raw-install-beta4-seq6.json`):
upload and verification 6.7 minutes, write and start 36.3 minutes; the
certificate, host keys, account, slot, host name, session lock, SSH, the
switches and the display's sleep time came back, root's home as saved
before and put back after, and the new system was confirmed (the board's
floor is now 7). After it the QUIC picture was 56 ms old when drawn (95th
percentile 81 ms) and the GUI walk passed. beta.3's notes start with a
pointer to this release.

The board ends as found: the slot, the RAW system and its floor are the
release's; QUIC on, the firewall in Baseline, SSH and mDNS on, the RAW
switch on, the H.264 engine hardware, WebRTC at 8000 kbit/s, quality 100,
1920x1080 at 60; the display's sleep at the three minutes the operator
chose.

### Slot 85 after the release (2026-10-11)

At the operator's word ("Ставь"), slot 85 (`0.1.0-beta.5`, from `04745b0`,
the QUIC worker's clock; app tar
`07682967c938bfc867ad04ed47b894070e22c9169c1b151e04fcbc08414bde34`), with
`scripts/check.sh` green on that commit, was installed through the update
API and confirmed; the board's state changed only in its slots. Probes,
all passed: the QUIC picture for 30 s with the monitor (frames 57 ms old
when drawn, 95th percentile 107 ms, the monitor's delay 53 ms, none
dropped, no packet lost, the share 100 %, from the same development host
whose clocks misread slot 84's), the GUI at a desktop's and a phone's size
with the monitor open, H.264 and VP8 over WebRTC, and the phone's reach.
`scripts/check.sh` was green on `ccdada5`, which records it. The
operator looked at slot 85 on the Android phone (Chrome) and the desktop:
QUIC, VP8 and MJPEG upscaled with Auto smooth and without freezes, the
pinch and the touches where they were made, the quality monitor fitting
the phone with QUIC's delay in tens of milliseconds, the cursor's tip on
the target's ("Всё так же хорошо", 2026-10-11). Asked whether to publish
slot 85 as an application release 0.1.0-beta.5, the operator chose not
to: the change corrects a number in the monitor on machines whose wall
clock is stepped, and goes with the next release. The board ends as
found, on slot 85 over the beta.4 system.
