#!/bin/sh
# On the board, inside a lab session (tools/device/ve_lab_session.sh) while
# the product's encoder leaves the video engine's device alone (the board's
# choice is x264): a second, transient instance of the product's H.264
# encoder, fed with pictures as a capture stream (ve_capture_play.py), is
# switched between the video engine and x264 through its demand socket
# (ve_engine_switch.py), as the web server does with the board's choice
# (docs/design/h264-engine-choice-and-quality-monitor-v1.md, gate C1).
#
# usage: sh ve_engine_switch_run.sh PICTURES PLAY_FPS SWITCHES PERIOD KBPS NAME
#   PICTURES  JPEG pictures one after the other, played forth and back
#   PLAY_FPS  the player's rate: the capture's, 60 a second on the board
#   SWITCHES  switches in all, x264 first; an even number ends on the engine
#   PERIOD    seconds between two switches
#
# The instance is the product's binary with the product unit's settings
# but its own capture, demand and frame sockets, status word and report in
# this directory, and an RTP port nobody reads. Its journal is kept: a line
# that the engine failed, could not be had or was given up fails the run.
set -u
[ "$#" -eq 6 ] || { echo "usage: $0 PICTURES PLAY_FPS SWITCHES PERIOD KBPS NAME"; exit 2; }
pictures=$1; play_fps=$2; switches=$3; period=$4; kbps=$5; name=$6
encoder=/usr/libexec/hardened-blikvm/bli-kvm-video-encoder
device=/dev/hardened-blikvm-ve-enc
[ -x "$encoder" ] || { echo "no encoder at $encoder"; exit 1; }
[ -c "$device" ] || { echo "no device at $device"; exit 1; }
mkdir -p out
here=$PWD
children=

stop() {
  for pid in $children; do kill "$pid" 2>/dev/null; done
  for pid in $children; do wait "$pid" 2>/dev/null; done
  children=
}
trap 'stop; exit 143' TERM INT

seconds=$((switches * period + 40))
python3 ./ve_capture_play.py "$here/capture.sock" "$pictures" "$play_fps" "$seconds" > "out/$name.player.txt" 2>&1 &
children=$!
waited=0
while [ ! -S "$here/capture.sock" ] && [ "$waited" -lt 50 ]; do sleep 0.1; waited=$((waited + 1)); done
env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin \
  BLI_KVM_USTREAMER_SOCKET="$here/capture.sock" \
  BLI_KVM_ENCODER_CODEC=h264-ultrafast \
  BLI_KVM_ENCODER_RTP_PORT=5996 \
  BLI_KVM_ENCODER_CONVERTERS=3 \
  BLI_KVM_ENCODER_HARDWARE="$device" \
  BLI_KVM_ENCODER_STATUS="$here/encoder" \
  BLI_KVM_ENCODER_REPORT="$here/report.json" \
  BLI_KVM_ENCODER_FRAME_SOCKET="$here/frames.sock" \
  BLI_KVM_VP8_DEMAND_SOCKET="$here/demand.sock" \
  BLI_KVM_VP8_DEMAND_GROUP=root \
  BLI_KVM_VP8_MAX_FPS=30 \
  BLI_KVM_VP8_KBPS="$kbps" \
  BLI_KVM_VP8_THREADS=3 \
  "$encoder" 2> "out/$name.encoder.txt" &
pid=$!
children="$children $pid"
waited=0
while { [ ! -S "$here/demand.sock" ] || [ ! -S "$here/frames.sock" ]; } && [ "$waited" -lt 50 ]; do
  sleep 0.1; waited=$((waited + 1))
done
cp "$here/report.json" "out/$name.report-at-start.json" 2>/dev/null
status=0
python3 ./ve_engine_switch.py "$here/demand.sock" "$here/frames.sock" "$here/encoder" "$pid" \
  "$switches" "$period" "$kbps" "out/$name.switches.json" > "out/$name.summary.txt" 2>&1 || status=$?
cp "$here/report.json" "out/$name.report-at-end.json" 2>/dev/null
alive=yes
kill -0 "$pid" 2>/dev/null || alive=no
stop
rm -f "$here/capture.sock" "$here/demand.sock" "$here/frames.sock" "$here/encoder" "$here/report.json"
# What the encoder said: every switch, and nothing that failed.
log="out/$name.encoder.txt"
failed=$(grep -c -e "the video engine: " -e "frames in a row" -e "does not encode" "$log")
to_x264=$(grep -c "x264 is chosen" "$log")
to_engine=$(grep -c "H.264 by the video engine" "$log")
{
  echo "$name: alive to the end: $alive; switcher status $status"
  echo "  lines: $to_engine to the engine (the first stream's among them), $to_x264 to x264, $failed of a failure"
  cat "out/$name.summary.txt"
} | tee -a out/switch.txt
[ "$alive" = yes ] && [ "$failed" -eq 0 ] && [ "$status" -eq 0 ] || exit 1
exit 0
