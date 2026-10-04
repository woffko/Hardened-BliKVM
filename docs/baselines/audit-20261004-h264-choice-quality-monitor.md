# H.264 by engine in the video menu, and a quality monitor: outcome of the plan of 2026-10-03

The plan is `docs/baselines/work-plan-20261003-h264-choice-quality-monitor.md`.
Its design, the refinements made on the way with their reasons, and every
measurement are in `docs/design/h264-engine-choice-and-quality-monitor-v1.md`.
This note says, item by item, what was done and where its evidence is, what
was found on the way, and what is left to the operator. Receipts named here
are under `evidence/private`, which is not in Git.

## Outcome

C0 to C5 are done, and gates C1, C4 and C5 passed. The video menu offers
H.264 HW and H.264 SW, and the board's H.264 encoder follows the choice
between two frames; QM on the toolbar opens a quality monitor with a Basic
and an Extended level, whose board's side comes from `GET /api/vm/quality`.
The test board runs slot 72 `0.1.0-alpha.16` (source 1498dce), the update
runtime `0.1.0~dev36` with the encoder driver on, and the H.264 encoder of
this plan; the engine choice is `hardware`, as the operator left it.

## Items

| Item | Commits | Outcome and evidence |
| --- | --- | --- |
| Plan | b143d5b | Committed as written. |
| C0.1 | 217f22a | The design note: the API's fields with their units and bounds, the encoder's report, the demand line, the menu's cases, the monitor's rows and where each comes from, the reference and the licence question. |
| C0.2 | 11d40db | `tools/playwright/quality_fixture.mjs`: the board's answers for the page's contracts on the host. |
| C1.1 | 047f5f4 | The encoder's `engine` line, its switch between two frames both ways, its look at the device at its start, its report. |
| C1.2 | 7ec1c57 | The server keeps the choice (`h264-engine.json`, hardware unless chosen otherwise), sends it on every demand connection and when it is made; `h264Engine`, `h264HardwareAvailable` and `h264HardwareReason` in `GET /api/vm/screen`. |
| C1.3 | 6088fb9 | The host's tests in the encoder and the server. |
| Gate C1 | 4179de8 | Passed on slot 67 (`0.1.0-alpha.12`, source 6088fb9) with the encoder of the same source: 100 switches of a transient instance (the status word within 0.149 s, the key frame within 0.177 s, at most one frame before it), and 100 choices through the API with the product's encoder while a viewer watched a moving picture over QUIC (`h264Encoder` within 1.266 s, 0.134 s and 0.209 s in the median); no encoder failure, no kernel line, nothing given up by the viewer (`c1-transient-gate`, `c1-product-gate*`). |
| C2.1 | 2348bf5 | H.264 HW and H.264 SW in the video menu, the check mark on the board's choice, a note where another engine encodes, HW not offered where the engine cannot be had, nothing chosen without the right. |
| C2.2 | 7e9e6ea | `tools/playwright/h264_engine_menu_contract.mjs` in `check.sh`. |
| C3.1 | 9e89e61 | The encoders' report: written at their start, on a change, and once a second while a viewer watches; bounded, replaced whole. |
| C3.2 | 0cb01fd | `GET /api/vm/quality`: the capture, the H.264 and VP8 encoders' reports checked and their rates, the units' share of the processors, the temperature; a cache of a second. |
| C3.3 | e39791d | The report's reader with damaged, oversized, stale and out-of-bound files; the answer and its cache. |
| C4.1 | bc75c74, f331041 | QM on the toolbar, the window (dragged, resized, scrolled, opaque, closed), its two levels, remembered by the browser; then its rows drawn by a worker of their own on a canvas, off the page's main thread (the reason in gate C4). |
| C4.2 | c51d97a | The page's own numbers: QUIC from the session, WebRTC from `getStats()`, Direct from its bytes, MJPEG from the server. |
| C4.3 | 8366b3d | The rows of Basic and Extended, in English and Russian. |
| C4.4 | 3ef25c8 | `tools/playwright/quality_monitor_contract.mjs` in `check.sh`: the window and its levels, the rows of each path against the stand-in, the API asked only while the window is open. |
| Gate C4 | 958c3c7 | Passed on the host against the real server, gateway and encoder: 17.92 against 17.97 frames a second, 0.898 against 0.880 Mbit/s, 15 against 15 ms; the open monitor cost the page's main thread 0.77 ms a second in Basic and 0.24 in Extended in the median (bound 2) (`c4-gate-final2.json`). With the rows in the document it had cost 4 to 5. |
| C5.1 | - | Slot 68 (`0.1.0-alpha.13`, source 958c3c7) and the encoder of this plan (`4e3d9a8c`) through `deploy_vp8.sh`, with the reports' paths in the server's drop-in; slot 69 (`0.1.0-alpha.14`, source 5de8e6c) with the worker's count of its draws for the probes; slot 71 (`0.1.0-alpha.15`, source e477b42) with the Speed of C5.2; slot 72 (`0.1.0-alpha.16`, source 1498dce) with the quality answer of C5.3; the update runtime as it was (`update-to-seq68-*` to `-seq72-*`). |
| C5.2 | 5de8e6c, 724cf3d, 598bf51 | The probes that hold the monitor and the menu to the board, with the page's failed API requests counted, and `tools/device/frame_socket_tap.py`; the Speed to a kilobit below a megabit a second (findings 3 and 4). |
| C5.3 | e477b42, 8ffb18c, 1498dce, 7842d22 | The code of before this plan (6522ff5, built as slot 70) with the new encoder still shows H.264 over QUIC and WebRTC; on slot 72 the GUI walks, desktop and mobile, with the monitor closed and open, VP8 and H.264 over WebRTC, MJPEG and the firewall probe passed, after the fixes they found on slot 71 (findings 5 to 7) (`c5-old70-*`, `c5-s71-*`, `c5-s72-walk-*`, `-webrtc-*`, `-mjpeg-latency-*`, `-quic-firewall*`). |
| Gate C5 | 1547671 | Passed on slot 72: the menu's choice changed the engine within 0.46 s for a watching viewer both ways, over QUIC and WebRTC, and the monitor named the engine that encoded; its Basic numbers agree with the probes as in gate C4 on every path the board offers (over QUIC the Speed with the frames the encoder handed to the gateway); with it open the web server took 0.40% of a core more and the encoder none (bound 2); no API error, no kernel line (`c5-s72-*`). |
| C5.4 | the commit of this note | This note, the video design note, `docs/licensing.md` (nothing of RustAdmin was taken); the design note's record came with gate C5; the memory note. |
| C5.5 | the commit after it | The end state below. |

## Found on the way

1. A choice reaches the encoder only on a demand connection: while nobody
   watches H.264 the server keeps it, and the encoder takes it with the
   next viewer, until then `h264Encoder` names the engine that encoded
   last (C1). The menu shows the difference only while the browser's own
   mode is H.264.
2. The monitor's rows in the page's document cost its main thread 4 to 5
   ms a second; drawn by a worker of their own on a canvas they cost 0.24
   to 0.77 (C4.1, gate C4).
3. On the board's still picture a quarter of what the QUIC probe's relays
   count is the transport's own: the monitor's Speed (the frames' bytes)
   was held to the frames the encoder handed to the gateway instead, read
   beside it from the frame socket by `tools/device/frame_socket_tap.py`
   (C5.2).
4. With two places of Mbit/s the Speed showed VP8's small frames of about
   five kilobits as ten, 18% above `getStats()`; it has three places below
   one megabit a second since (C5.2).
5. A fresh answer of `GET /api/vm/quality` took 0.51 to 0.61 s, waiting for
   the update runtime's status for the slot's version; while the settings'
   System tab loaded, it took longer than the monitor's 0.9 s and the GUI
   walks with the monitor open failed on that request. The version is read
   in the background since; an answer takes 11 to 20 ms (C5.3).
6. The firewall probe, and with it the regression, expected one H.264
   entry in the video menu; it takes both entries since (C5.3).
7. The MJPEG latency probe turned this host's monotonic times into true
   time with SNTP readings before and after a run only; the WSL host's
   monotonic clock changes its rate within a run, and four runs of six
   failed its limit while the stream was as before. It reads SNTP every
   two seconds since, and passed three runs of three (C5.3).
8. `GET /api/application-update/status` answers in about half a second on
   the board (0.50 to 0.62 s). Not this plan's; it no longer reaches the
   monitor.
9. `check.sh` had one load flake: a frame socket test of the QUIC plan got
   a delta frame where it waited for the periodic key frame while a slot
   was built beside it, and passed when run again alone.
10. Slot 66 could not be installed again for the regression: the update
    path takes only a sequence above the confirmed one, and nothing in
    the trust path was changed; the same code was built as slot 70.

## Not shown

- A moving picture through the monitor on the board: the target stood
  still in every window of C5. The monitor met a moving picture on the
  host (gate C4), and the board's engine switch did in gate C1.
- H.264 Direct: this board does not offer the mode; the monitor's Direct
  rows were held to the stand-in on the host only.
- Browsers other than Chromium, and a real phone (the mobile layout was
  walked in Chromium's phone emulation).
- The monitor or the menu while somebody holds the HID lease: nothing of
  this plan asks for it.

## Board end state (2026-10-04)

Compared field by field with the state found before C1
(`c1-state-as-found-20261003.json`, `c5-state-after-20261004.json`): the
switch for RAW system updates on as the operator left it; the terminal,
scripts, remote download and PicoClaw switches off; firewall Baseline;
video at 60 frames a second, quality 100, 1920x1080; WebRTC quality
8 Mbit/s; the QUIC transport on; session lock 3600 s; SSH enabled; the
operator's two images in the catalog and no drive attached;
`h264Encoder: hardware`. The only differences are this plan's new fields:
`h264Engine` says `hardware`, the choice as the operator left it, and
`h264HardwareAvailable` says the engine can be had.

What the plan left on the board: slot 72 `0.1.0-alpha.16`, the update
runtime `0.1.0~dev36` and the encoder driver as they were, the encoders of
this plan (one binary, `4e3d9a8c`) with their units, and the web server's
drop-in that names the encoders' reports. No probe, tap, measuring script
or file of the plan is on the board, and no process of it runs; the
kernel's log had no line from the first of C5's measurements to the last.

## Left to the operator

- H.264 SW in the video menu is the board's choice for every H.264 viewer
  until somebody chooses H.264 HW again; x264 costs more of the processors
  for a moving picture and makes fewer of its frames.
- Slots 67 to 72 and the update runtime `0.1.0~dev36` are not published.
  Slot 70 is the code from before this plan (6522ff5) under the label
  `0.1.0-alpha.11`, built for the regression only.
- Whether to look at the update runtime's status that takes half a second
  (finding 8), and at the findings of the hardware plan that are still
  open: the QUIC gateway's certificate made before the clock is set, the
  gate that lets about twenty of thirty frames a second through, and the
  firewall status that may be five seconds old after a change.
