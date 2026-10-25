# Hardware H.264 encoding on the H616: outcome of the plan of 2026-10-02

The plan is `docs/baselines/work-plan-20261002-hardware-h264.md`. What is
known of the engine, the lab and every measurement are in
`docs/design/h616-ve-h264-encoder.md`; the driver's design, its tests and
gate V4 in `docs/design/h616-ve-encoder-driver-v1.md`. This note says, item
by item, what was done and where its evidence is, what was found on the
way, and what is left to the operator. Receipts named here are under
`evidence/private`, which is not in Git; the lab's diary is
`evidence/private/ve-lab-log.md`.

## Outcome

V0 to V7 are done and every gate passed; V2 passed after V2R, which the
operator agreed to after the incident of V2.1. The test board runs slot 66
`0.1.0-alpha.11` (source `b31149a`), the update runtime `0.1.0~dev36` with
the encoder driver switched on, and the H.264 encoder with the engine:
since 2026-10-03 14:44 UTC the board's H.264 over WebRTC and over QUIC
comes from the video engine, and from x264 where the engine is not there
or fails three frames in a row.

## Items

| Item | Commits | Outcome and evidence |
| --- | --- | --- |
| Plan | d8ae2df | Committed as written. |
| V0.1 | c1ff844 | The sources with their licences, the clean-room rules, the two open register maps side by side (`h616-ve-h264-encoder.md`, "Sources" to "The stream's headers"); `docs/licensing.md`. |
| V0.2 | 4c66187 | The lab's method, its rules for experiments, the test sequences, the measures, receipts and log ("The lab"). |
| V0.3 | 4b6a3d8 | The lab tool's host part: picture generator, header writer, software stand-in, register plans of both maps, PSNR. |
| V0.4 | 2758001 | `check.sh` runs the tool's tests; a packaging test keeps the lab out of every package. |
| V1.1 | 23ac2a1 | Linux 5.19.4 from kernel.org (signature checked), the board's configuration and own headers, Arm's GCC 8.3: modules that load on the board (`ve-lab-v1-20261002-session-dry`, `-module-check`). |
| V1.2 | 387e2a9 | The lab module and the lab tool's way to the engine. |
| V1.3 | 20c462e | The engine's version registers answer; `cedrus` bound and suspended afterwards. |
| V1.4 | 71e118f | The lab's buffers, with guards. |
| V1.5, gate V1 | d3ddd8e | The registers' masks, read without a trigger; gate V1 passed. |
| V2.1 | fe08536, e68d669 | The bit writer works; its first bits went to the wrong place (the incident, `ve-lab-v2-20261002-incident`), explained by V2R; then into the buffer (`-bits-h6-a`, `-bits-h6-b`). |
| V2R.1 to V2R.5 | a39e913, 71e3ee7, 701bf6d, 279cc1e, 589bd0f | With the operator's yes: the vendor's encoder library run on the board behind a closed register window, without the engine, its traces read (`ve-trace-20261002-*`); the encoder block takes its addresses in units of 256 bytes; the lab tool's hypothesis `h6`. The library's files were removed from the board and never entered Git or an artifact. |
| V2.2, gate V2 | e06cae9 | Key frames at four sizes, decoded by ffmpeg and Chromium, 7 ms at 1920x1080, 2 to 3 dB below x264 at the same QP; gate V2 passed. |
| V3.1 | 437f748, dee5a06, e17b723, 8f887b6, 69838cd, a6deff2, e47c5e9, c1e1bec | P frames without drift; the engine's write extents in its work buffers, the end of its stream buffer honoured, the smallest picture it finishes (13 macroblocks wide); `ve-lab-v3-20261002-*`, `-extents*`, `ve-lab-v3-20261003-noise`. |
| V3.2 | 117b101, 1e951ae, d28cf30 | Constrained baseline, the level, cropping, a key frame on request; Chromium decodes every frame. |
| V3.3 | e3412b7, 0763ab7, cb246e4 | 7 ms a frame, key or P; thirty frames a second for ten minutes, idle and with three cores busy; at most two passes a frame. |
| V3.4 | 8970271, 17e0493, 47bc63d, f5c7949, d4b46c7 | Against the product's x264: at most 0.18 dB below at 8 Mbit/s, 0.13 dB at 2.5 Mbit/s on the plan's sequences; how far the engine's search reaches; text that moves fast is worse. |
| V3.5 | 8c48d2f, afa12d0, 84d66a7, c113be6, 266613f | Rate control in software: a QP a frame, a second pass, frames left out when no QP fits, the search chosen for every frame. |
| V3.6 | a3c4622, f16a1e4, a4df079, ecedbaf, acc0e42 | An hour without a wrong frame; a timeout and the reset; a thousand starts and stops. |
| Gate V3 | cde6165 | Passed, with the time and rate conditions read as written there. |
| V4.1 | 0b621d6 | The driver's design note. |
| V4.2 | d763f32 | The driver. |
| V4.3 | b0cd482, 299bbe2, 32516a3 | 200,000 random requests on the host; the hostile client on the board (third run, the first two failed on the client's own faults). |
| Gate V4 | d3b5120 | 23 of 23 streams the same bytes through the driver as through the lab module; 50 runs, the kernel's image unchanged (`ve-lab-v4-20261003-*`). |
| V5.1 | d6dd3fd | `bli-kvm-ve-encoder` and the third engine of `bli-kvm-video-encoder`; the design record "V5". |
| V5.2 | e7eb19f | Choice and fallback; `h264Encoder` in `GET /api/vm/screen`; the screen settings. |
| V5.3 | 99c28c6, 1fafaab | The unit: the one device; the device's group made with the unit (found on the board, below). |
| V5.4 | 89ad20c | The driver's sets against the client's account; `check.sh` green. |
| V6.1 | be0fa2b | Update runtime `0.1.0~dev36`: the module for 5.19.4-sunxi64 only, its loader and unit (disabled), examples, the source; `tools/device/deploy_ve.sh`; the capture unit by its rule's name. |
| V6.2 | 564611b | The SD image refuses a driver for another kernel. |
| V6.3 | b31149a | `docs/licensing.md`; packaging tests; `check.sh` green. |
| V7.1 | 44d18d6, f6fd0f3, b68f8f5, 5d5d6b5 | Installed: dev36, the encoder, slot 66. The transient instance (`ve-v7-measure-20261003-*`), the windows end to end (`ve-v7-e2e-*`), twelve hours (`ve-v7-hold-20261003`). |
| V7.2 | f21b327 | On slot 66 with the engine: GUI walks desktop and mobile, VP8 and H.264 over WebRTC, H.264 over QUIC, MJPEG (the 20 Mbit/s viewer around its 300 ms limit, as before), the firewall probe (after it was taught to wait for the server's five-second status); slot 65 showed H.264 on the new system (`gui-walk-seq66-v7-*`, `vp8-webrtc-seq66-v7`, `h264-webrtc-seq66-v7`, `quic-video-device-seq66-v7`, `mjpeg-latency-seq66-v7*`, `quic-firewall-device-seq66-v7*`, `ve-v7-e2e-slot65-*`). |
| V7.3 | d474b7a | This note, the two design notes, the video design note, the feasibility note's outcome, `docs/security-boundaries.md`; the memory note. |
| Gate V7 | f70213c | Passed: 1.56 and 1.70 cores at 30 frames a second of a moving 1920x1080 picture against 3.07 and 3.27 for x264, which made 18.8 and 12.5 of them; twelve hours on the engine without a failure or a kernel line from the driver or the engine. The engine stays on. |
| V7.4 | the commit after it | The end state below. |

## Found on the way

1. In V2.1 the engine's bit writer wrote its first bits into the kernel's
   memory: the encoder block of this generation takes its addresses in
   units of 256 bytes. Explained by tracing the vendor's library without the
   engine (V2R); the operator power-cycled the board once, after a reset
   over SSH had hung, and nothing lasting was found. Rule 9 (the kernel's
   image compared word by word after every session) came from it.
2. The engine does not finish a frame that refers to another when the
   picture is narrower than 13 macroblocks; the driver begins at 256 pixels.
3. A stall interrupt that nobody waited for answered the next wait in the
   lab module; the lab tool asks for what the module keeps after a reset,
   and the driver forgets an interrupt when it resets the engine.
4. Gate V3's "33 ms a frame": with three passes a frame some frames took
   longer than their time; at 1920x1080 a frame takes two at most since,
   and the product's encoder gives a frame the passes its time allows.
5. The first deploy of the encoder's units left H.264 without an encoder
   for about a minute (216/GROUP: the unit named a group that only
   `deploy_ve.sh` made); `deploy_vp8.sh` now makes it, and `deploy_ve.sh`
   no longer needs `/etc/sysusers.d`, which the board does not have.
6. The QUIC gateway made its certificate at boot from a clock that still
   said 2026-07-29, so every QUIC session was refused with "clock" until
   the gateway was restarted. Not this plan's (left to the operator below).
7. The encoder's gate admits a frame no sooner than one interval of its
   limit after the one before; fed at twice the limit, as the board's
   capture feeds it, it let about twenty frames a second through instead of
   thirty in the lab's runs, whichever engine encoded (the board's own
   capture was not measured with a moving target). Not this plan's (left to
   the operator below).
8. The hostile client failed twice on its own faults before it ran clean:
   Python cannot pass an address as an `ioctl` number, the kernel notes the
   client's own page-cache drops, and a mapping that is given back was
   reused by the client's own stream buffer.

## Not shown

- A moving picture end to end on the board with the engine: the target
  stood still in the windows of the probes. The moving picture is shown in
  the transient instance with the lab's pictures, and for about an hour of
  the twelve by what the target showed.
- A boot with the driver loaded by its unit: the unit is enabled and the
  next boot is the first to load it so. Without `cedrus` at boot the
  capture becomes the first video device, which the capture unit takes by
  the udev rule's name; that too waits for the next boot.
- An SD image or a RAW system payload with the driver: none was built in
  this plan; the image check is tested on the host.
- Three passes a frame below 1920x1080 (the product's rule allows them at
  1366x768 and smaller): not measured on the board.
- The engine while somebody holds the HID lease: by rule 1, never before
  gate V7.
- Browsers other than Chromium, and phones.

## Board end state (2026-10-03)

As found before V7 and compared field by field afterwards
(`v7-state-as-found-20261003.json`, `v7-state-after-20261003.json`): the
switch for RAW system updates on as the operator left it; the terminal,
scripts, remote download and PicoClaw switches off; firewall Baseline;
video at 60 frames a second, quality 100, 1920x1080; WebRTC quality
8 Mbit/s; the QUIC transport on; session lock 3600 s; SSH enabled; the
operator's two images in the catalog and no drive attached. The only
differences are the capture's measured rates and the new field
`h264Encoder`, which says `hardware`.

What the plan left on the board, as gate V7 decided: slot 66
`0.1.0-alpha.11`, the update runtime `0.1.0~dev36`, the encoder of
`b31149a` with its units, and the encoder driver switched on: its loader
unit enabled and active, the module loaded and holding the engine,
`/dev/hardened-blikvm-ve-enc` the group `hardened-blikvm-ve`'s with mode
0660, the udev rule and the modprobe line in `/etc`, and `sunxi_cedrus`
loaded but bound to nothing (it stays out from the next boot). The QUIC
gateway was restarted once (finding 6).

No lab tool, lab module, measuring script, temporary directory or process
of the plan is on the board: every lab session gave the engine back to
`cedrus` and left the kernel's image as it was before it. The kernel's
taint flags are 13312 since the first out-of-tree module of this boot and
stay so until the next.

## Left to the operator

- Reboot the board once when it suits: the driver should load at boot,
  `cedrus` stay out, the capture come up by its link, and the screen
  settings say "H.264 encoder: video engine (hardware)". If the QUIC
  transport then refuses with "clock", restart
  `hardened-blikvm-quic.service` (finding 6).
- Whether to take up the three findings that are not this plan's: the
  QUIC gateway's certificate made before the clock is set (6), the gate
  that lets about twenty of thirty frames a second through (7), and the
  firewall page's status that may be five seconds old after a change
  (V7.2 in `h616-ve-h264-encoder.md`).
- `tools/device/deploy_ve.sh remove` switches the engine off, and x264
  encodes again; `install` switches it on.
- Slot 66 and the update runtime `0.1.0~dev36` are not published.
