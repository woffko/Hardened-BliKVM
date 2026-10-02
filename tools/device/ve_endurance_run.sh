#!/bin/sh
# On the board, as a command of ve_lab_run.sh: the lab's endurance runs
# (docs/design/h616-ve-h264-encoder.md, V3.6).
#
# usage: sh ve_endurance_run.sh soak MAP WIDTHxHEIGHT SECONDS RATE SEQUENCE FLOOR [repeat]
#            a stream for SECONDS at RATE (qpN or kilobits a second); every
#            frame is decoded by the board's ffmpeg while the run goes on
#            and compared with its source (30 pictures, played forth and
#            back); a frame below FLOOR dB of luma PSNR is a wrong frame.
#            The engine goes as fast as the decoding takes its frames.
#            With repeat (and a fixed QP) the stream repeats itself every
#            580 frames, and a frame that is not, byte for byte, the frame
#            580 before it is a wrong frame too.
#        sh ve_endurance_run.sh sustained MAP WIDTHxHEIGHT SECONDS RATE SEQUENCE [BUSY]
#            a stream of 30 frames a second for SECONDS, each frame begun
#            at its time, with BUSY cores kept busy; nothing is decoded
#        sh ve_endurance_run.sh timeout MAP WIDTHxHEIGHT SEQUENCE
#            300 frames, of which the hundredth is not waited for: the
#            engine is reset and the stream goes on with a key frame; the
#            stream must decode
#        sh ve_endurance_run.sh starts MAP COUNT
#            COUNT streams of two frames, each begun and ended: the device
#            opened, buffers taken, two frames encoded, everything given back
#
# Nothing but numbers is left in out/.
set -u
tool=./bli-kvm-ve-lab
action=${1:-}

cma_free() { sed -n 's/^CmaFree: *\([0-9]*\) kB/\1/p' /proc/meminfo; }

busy_pids=
stop_busy() {
  for pid in $busy_pids; do kill "$pid" 2>/dev/null; done
  busy_pids=
}
trap stop_busy EXIT
trap 'exit 129' HUP INT TERM

case "$action" in
  soak)
    [ "$#" -ge 7 ] || { echo "usage: $0 soak MAP WIDTHxHEIGHT SECONDS RATE SEQUENCE FLOOR [repeat]"; exit 2; }
    map=$2; size=$3; seconds=$4; rate=$5; sequence=$6; floor=$7
    name="soak-$sequence-$size-$rate"
    # 30 pictures forth and back are 58 frames; a key frame every 290, of
    # which every second has the same number in its header.
    key_every=300; period=
    if [ "${8:-}" = repeat ]; then key_every=290; period=period=580; fi
    command -v ffmpeg > /dev/null || { echo "no ffmpeg on this board"; exit 1; }
    rm -f stream.fifo
    mkfifo stream.fifo
    (
      # The luma plane alone, as it is: less to convert and to carry.
      ffmpeg -v error -nostdin -i stream.fifo -vf extractplanes=y -f rawvideo -pix_fmt gray - \
        2> "out/$name.ffmpeg.txt" |
        $tool psnr "$sequence" "$size" 1000000 /dev/stdin ring=30 luma "floor=$floor" > compared.txt 2>&1
    ) &
    checker=$!
    status=0
    # shellcheck disable=SC2086
    $tool stream "$map" "$sequence" "$size" 1000000 "$key_every" "$rate" 30 stream.fifo "out/$name.json" \
      ring=30 guards=300 "seconds=$seconds" $period || status=1
    wait "$checker"
    rm -f stream.fifo
    {
      echo "engine: $(tail -1 compared.txt)"
      echo "ffmpeg complaints about the engine's stream: $(wc -c < "out/$name.ffmpeg.txt") bytes"
    } > "out/$name.psnr.txt"
    rm -f compared.txt
    cut -c1-300 "out/$name.psnr.txt"
    exit "$status"
    ;;
  sustained)
    [ "$#" -ge 6 ] || { echo "usage: $0 sustained MAP WIDTHxHEIGHT SECONDS RATE SEQUENCE [BUSY]"; exit 2; }
    map=$2; size=$3; seconds=$4; rate=$5; sequence=$6; busy=${7:-0}
    name="sustained-$sequence-$size-$rate-busy$busy"
    count=$busy
    while [ "$count" -gt 0 ]; do
      sh -c 'while :; do :; done' &
      busy_pids="$busy_pids $!"
      count=$((count - 1))
    done
    status=0
    $tool stream "$map" "$sequence" "$size" "$((seconds * 30))" 300 "$rate" 30 /dev/null "out/$name.json" \
      paced ring=30 guards=300 || status=1
    stop_busy
    exit "$status"
    ;;
  timeout)
    [ "$#" -eq 4 ] || { echo "usage: $0 timeout MAP WIDTHxHEIGHT SEQUENCE"; exit 2; }
    map=$2; size=$3; sequence=$4
    $tool stream "$map" "$sequence" "$size" 300 300 qp30 30 timeout.h264 out/timeout.json \
      ring=30 timeout=100 || exit 1
    decoded=$(ffmpeg -v error -nostdin -i timeout.h264 -f rawvideo -pix_fmt nv12 - 2> out/timeout.ffmpeg.txt | wc -c)
    rm -f timeout.h264
    echo "after the timeout and the reset: $decoded bytes decoded, $(wc -c < out/timeout.ffmpeg.txt) bytes of complaints"
    ;;
  starts)
    [ "$#" -eq 3 ] || { echo "usage: $0 starts MAP COUNT"; exit 2; }
    map=$2; count=$3
    before=$(cma_free)
    done_ok=0
    index=0
    while [ "$index" -lt "$count" ]; do
      if $tool stream "$map" drag 320x240 2 2 qp30 30 start.h264 out/start.json > /dev/null 2>&1; then
        done_ok=$((done_ok + 1))
      fi
      index=$((index + 1))
    done
    decoded=$(ffmpeg -v error -nostdin -i start.h264 -f rawvideo -pix_fmt nv12 - 2> out/start.ffmpeg.txt | wc -c)
    rm -f start.h264
    echo "starts and stops: $done_ok of $count; the last stream: $decoded bytes decoded; contiguous memory free: $before kB before, $(cma_free) kB after"
    [ "$done_ok" -eq "$count" ]
    ;;
  *)
    echo "usage: $0 soak|timeout|starts ..."
    exit 2
    ;;
esac
