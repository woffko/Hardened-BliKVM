#!/bin/sh
# On the board, inside a lab session (tools/device/ve_lab_session.sh) whose
# ve_lab_run.sh has loaded the encoder driver (VE_MODULE=hardened-blikvm-ve-enc):
# a second, transient instance of the product's H.264 encoder beside the
# product's units, fed with pictures as a capture stream
# (ve_capture_play.py), with the video engine or with x264
# (docs/design/h616-ve-h264-encoder.md, V7.1). For every run: the
# encoder's statistics lines, the processor time it used, the SoC's
# temperatures every five seconds, and the frames the player sent.
#
# usage: sh ve_encoder_measure.sh PICTURES FPS SECONDS KBPS NAME ENGINE...
#   PICTURES  JPEG pictures one after the other, played forth and back
#   FPS       the player's rate and the encoder's limit
#   SECONDS   measured after ten seconds of warming up
#   ENGINE    hardware (the driver's device) or x264
#
# The instance is the product's binary with the product unit's settings
# but its own capture socket, an RTP port nobody reads, no demand socket
# (it encodes all the time) and no frame socket. The driver's device is
# root's while the lab session holds it: the product's own encoder cannot
# open it, and stays on x264.
set -u
[ "$#" -ge 6 ] || { echo "usage: $0 PICTURES FPS SECONDS KBPS NAME ENGINE..."; exit 2; }
pictures=$1; fps=$2; seconds=$3; kbps=$4; name=$5
shift 5
encoder=/usr/libexec/hardened-blikvm/bli-kvm-video-encoder
[ -x "$encoder" ] || { echo "no encoder at $encoder"; exit 1; }
ticks=$(getconf CLK_TCK)
children=

stop() {
  for pid in $children; do kill "$pid" 2>/dev/null; done
  for pid in $children; do wait "$pid" 2>/dev/null; done
  children=
}
trap 'stop; exit 143' TERM INT

# User and system time of a process, in ticks.
cpu_ticks() {
  awk '{ print $14 + $15 }' "/proc/$1/stat" 2>/dev/null || echo 0
}

temperatures() {
  for zone in /sys/class/thermal/thermal_zone*; do
    printf '%s=%s ' "$(cat "$zone/type")" "$(cat "$zone/temp")"
  done
  echo
}

status=0
for engine in "$@"; do
  case "$engine" in
    hardware) device=/dev/hardened-blikvm-ve-enc ;;
    x264) device= ;;
    *) echo "unknown engine $engine"; exit 2 ;;
  esac
  run="$name-$engine-${fps}fps-$kbps"
  socket=$PWD/capture-$engine.sock
  python3 ./ve_capture_play.py "$socket" "$pictures" "$fps" $((seconds + 20)) > "out/$run.player.txt" 2>&1 &
  player=$!
  children="$player"
  waited=0
  while [ ! -S "$socket" ] && [ "$waited" -lt 50 ]; do sleep 0.1; waited=$((waited + 1)); done
  env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin \
    BLI_KVM_USTREAMER_SOCKET="$socket" \
    BLI_KVM_ENCODER_CODEC=h264-ultrafast \
    BLI_KVM_ENCODER_RTP_PORT=5996 \
    BLI_KVM_ENCODER_CONVERTERS=3 \
    BLI_KVM_VP8_MAX_FPS="$fps" \
    BLI_KVM_VP8_KBPS="$kbps" \
    BLI_KVM_VP8_THREADS=3 \
    ${device:+BLI_KVM_ENCODER_HARDWARE=$device} \
    "$encoder" 2> "out/$run.encoder.txt" &
  pid=$!
  children="$children $pid"
  sleep 10
  start=$(cpu_ticks "$pid"); began=$(date +%s.%N)
  temperatures > "out/$run.temperatures.txt"
  elapsed=0
  while [ "$elapsed" -lt "$seconds" ]; do
    sleep 5
    elapsed=$((elapsed + 5))
    temperatures >> "out/$run.temperatures.txt"
    kill -0 "$pid" 2>/dev/null || break
  done
  end=$(cpu_ticks "$pid"); ended=$(date +%s.%N)
  alive=yes
  kill -0 "$pid" 2>/dev/null || alive=no
  stop
  cpu=$(awk -v a="$start" -v b="$end" -v t="$ticks" -v s="$began" -v e="$ended" \
    'BEGIN { printf "%.1f", 100 * (b - a) / t / (e - s) }')
  echo "$run: cpu ${cpu}% of one core over $(awk -v s="$began" -v e="$ended" 'BEGIN { printf "%.0f", e - s }') s, alive to the end: $alive" |
    tee -a out/measure.txt
  grep -h "fps=" "out/$run.encoder.txt" | tail -n 3 | sed "s/^/  /" | tee -a out/measure.txt
  grep -h -i "engine\|x264" "out/$run.encoder.txt" | grep -v "fps=" | head -n 3 | sed "s/^/  /" | tee -a out/measure.txt
  [ "$alive" = yes ] || status=1
  rm -f "$socket"
done
exit "$status"
