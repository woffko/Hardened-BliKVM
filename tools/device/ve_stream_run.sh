#!/bin/sh
# On the board, as a command of ve_lab_run.sh: streams of the lab's
# sequences by the engine and, at the same bitrates, by the product's x264
# (the encoder crate's own wrapper, `h264_reference`); both decoded by the
# board's ffmpeg, every frame compared with its source
# (docs/design/h616-ve-h264-encoder.md, "Measures").
#
# usage: sh ve_stream_run.sh MAP WIDTHxHEIGHT FRAMES KEY_EVERY FPS 'RATE...' SEQUENCE... [-- OPTION...]
#
# RATE is qpN or kilobits a second; several are given in one argument. The
# options after -- go to the lab tool's `stream` (paced, ring=N, guards=N).
# In the environment: BUSY=N keeps N cores busy during the engine's runs;
# KEEP=yes leaves the engine's streams in out/; REFERENCE=no skips x264;
# DECODE=no skips the decoding and comparing.
#
# Raw pictures never touch the disk or tmpfs: they go through pipes. A
# stream lies in the session's directory while it is decoded.
set -u
[ "$#" -ge 7 ] || { echo "usage: $0 MAP WIDTHxHEIGHT FRAMES KEY_EVERY FPS 'RATE...' SEQUENCE... [-- OPTION...]"; exit 2; }
map=$1; size=$2; frames=$3; key_every=$4; fps=$5; rates=$6
shift 6
sequences=
while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do sequences="$sequences $1"; shift; done
[ "$#" -eq 0 ] || shift
tool=./bli-kvm-ve-lab
width=${size%x*}; height=${size#*x}
decode=${DECODE:-yes}
if [ "$decode" = yes ]; then
  command -v ffmpeg > /dev/null || { echo "no ffmpeg on this board"; exit 1; }
fi

busy_pids=
stop_busy() {
  for pid in $busy_pids; do kill "$pid" 2>/dev/null; done
  busy_pids=
}
trap stop_busy EXIT
trap 'exit 129' HUP INT TERM

# Decodes a stream and prints the tool's comparison with the source.
compare() {
  ffmpeg -v error -nostdin -i "$1" -f rawvideo -pix_fmt nv12 - 2> "$2" |
    $tool psnr "$sequence" "$size" "$frames" /dev/stdin 2>&1 | tail -1
}

status=0
for sequence in $sequences; do
  for rate in $rates; do
    name="$sequence-$size-$rate"
    count=${BUSY:-0}
    while [ "$count" -gt 0 ]; do
      sh -c 'while :; do :; done' &
      busy_pids="$busy_pids $!"
      count=$((count - 1))
    done
    $tool stream "$map" "$sequence" "$size" "$frames" "$key_every" "$rate" "$fps" \
      engine.h264 "out/$name.json" "$@" || status=1
    stop_busy
    {
      if [ "$decode" = yes ]; then
        echo "engine: $(compare engine.h264 "out/$name.ffmpeg.txt")"
        echo "ffmpeg complaints about the engine's stream: $(wc -c < "out/$name.ffmpeg.txt") bytes"
      fi
      case "$rate" in
        qp*) ;;
        *)
          if [ "${REFERENCE:-yes}" = yes ] && [ -x ./h264_reference ]; then
            rm -f source.fifo
            mkfifo source.fifo
            $tool generate "$sequence" "$size" "$frames" source.fifo > /dev/null &
            ./h264_reference "$width" "$height" "$fps" "$rate" "$key_every" ultrafast 3 \
              < source.fifo > x264.h264 2> x264.txt
            wait $!
            echo "x264 run: $(tail -1 x264.txt)"
            echo "x264: $(compare x264.h264 /dev/null)"
            rm -f source.fifo x264.h264 x264.txt
          fi
          ;;
      esac
    } > "out/$name.psnr.txt"
    cat "out/$name.psnr.txt" | cut -c1-240
    if [ "${KEEP:-no}" = yes ]; then mv engine.h264 "out/$name.h264"; else rm -f engine.h264; fi
  done
done
exit "$status"
