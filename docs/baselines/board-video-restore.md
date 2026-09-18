# Board video restored under our own units (E0)

Date: 2026-09-16. Plan phase E0 of
`docs/design/gui-parity-plan.md`. Receipt:
`evidence/private/board-video-restore-20260916-a.json`.

## What was wrong

The serving cutover retired KVMD, and KVMD was the component that launched the
ustreamer capture. The vendor streamer is not ours to remove, so the board went
without a picture: `/api/stream/mjpeg` had no proxy and no source.

## What was done

The vendor `ustreamer` stays the capture engine, now started by our unit:

- `hardened-blikvm-ustreamer.service` runs
  `/usr/bin/ustreamer --device=/dev/kvmd-video --persistent --format=mjpeg
  --resolution=1280x720 --desired-fps=40 --drop-same-frames=30
  --unix=/run/hardened-blikvm-ustreamer/ustreamer.sock --unix-rm
  --unix-mode=0660 --slowdown` as `hardened-blikvm-video` with the `video`
  supplementary group.
- `hardened-blikvm-board-video-proxy.service` runs our
  `bli-kvm-video-proxy` from the streamer socket to
  `/run/hardened-blikvm-video/mjpeg.sock`, which the board server already
  consumes for the authenticated `/api/stream/mjpeg`.
- `packaging/udev/99-hardened-blikvm-video.rules` maps the USB capture
  (`345f:2131`, video index 0) to `/dev/kvmd-video` in group `video`. It
  replaces the KVMD rule, whose helper belongs to the retired stack and whose
  rule ordering would otherwise override the node group; the old rule was
  backed up to `/var/lib/hardened-blikvm/backup-20260916/serving/99-kvmd.rules`
  and removed.
- The board bundle now ships both units, the udev rule and
  `libexec/bli-kvm-video-proxy`, verified as bundle `ef83368d` with 41 members.

## Gates

Live on `https://<board-address>/` (authenticated):

- `video-frame-timing-20260916-b.json`: passed, 277 frames, 27.9 fps,
  p95 frame interval 48.7 ms;
- `video-two-client-probe-20260916-a.json`: passed, two simultaneous clients;
- `video-soak-probe-20260916-a.json`: passed, 60 s, 12 samples, 330 MB streamed.

## Defect the gate exposed

`video_soak_probe.mjs` sampled on every network chunk. Against the old
KVMD-supervised stream that produced a few hundred samples, but the direct
stream writes about 5 MB/s, so the first run produced 74 138 samples and a
4.85 MB receipt that exceeds the acceptance report's 1 MiB receipt limit. The
probe now waits out the sampling interval after each progress checkpoint
(12 samples for 60 s, 924-byte receipt).

## Rollback

Stop and disable both units; remove the rule, restore the backed-up KVMD rule
and re-run `udevadm control --reload-rules` plus
`udevadm trigger --subsystem-match=video4linux`. KVMD stays masked, so the
rollback restores the rule file only, not the retired daemon.
