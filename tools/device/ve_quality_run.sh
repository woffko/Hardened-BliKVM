#!/bin/sh
# On the board, as a command of ve_lab_run.sh: frames of the lab's sequences
# encoded by the engine and, for comparison, by x264 at the same QP; both
# streams decoded by the board's ffmpeg; the luma PSNR of both against the
# source (docs/design/h616-ve-h264-encoder.md, "Measures").
#
# usage: sh ve_quality_run.sh MAP WIDTHxHEIGHT FRAMES KEY_EVERY QP SEQUENCE...
#
# The reference is libx264 through the board's ffmpeg with the product's
# preset, tune and profile, at a constant QP for every frame type: the
# product's own wrapper of x264 has no constant QP. Raw pictures stay in the
# session's directory and go with it; the streams and the numbers are left
# in out/.
set -u
[ "$#" -ge 6 ] || { echo "usage: $0 MAP WIDTHxHEIGHT FRAMES KEY_EVERY QP SEQUENCE..."; exit 2; }
map=$1; size=$2; frames=$3; key_every=$4; qp=$5
shift 5
command -v ffmpeg > /dev/null || { echo "no ffmpeg on this board"; exit 1; }
tool=./bli-kvm-ve-lab

for sequence in "$@"; do
  name="$sequence-$size-qp$qp"
  $tool generate "$sequence" "$size" "$frames" source.nv12 > /dev/null || exit 1
  $tool encode "$map" "$sequence" "$size" "$frames" "$key_every" "$qp" "out/$name.h264" "out/$name.json" || exit 1
  # What a decoder that is not ours makes of the engine's stream.
  ffmpeg -v error -nostdin -i "out/$name.h264" -f rawvideo -pix_fmt nv12 -y engine.nv12 \
    2> "out/$name.ffmpeg.txt" || echo "ffmpeg failed on the engine's stream"
  ffmpeg -v error -nostdin -f rawvideo -pix_fmt nv12 -s "$size" -r 30 -i source.nv12 \
    -frames:v "$frames" -c:v libx264 -preset ultrafast -tune zerolatency -profile:v baseline \
    -g "$key_every" -qp "$qp" -x264-params ipratio=1.0:pbratio=1.0 \
    -f h264 -y "out/$name.x264.h264" 2> "out/$name.x264.txt" || echo "the reference failed"
  ffmpeg -v error -nostdin -i "out/$name.x264.h264" -f rawvideo -pix_fmt nv12 -y x264.nv12 2>> "out/$name.x264.txt"
  engine=$($tool psnr "$sequence" "$size" "$frames" engine.nv12 2>&1 | tail -1)
  reference=$($tool psnr "$sequence" "$size" "$frames" x264.nv12 2>&1 | tail -1)
  echo "$name: engine $(wc -c < "out/$name.h264") bytes, x264 $(wc -c < "out/$name.x264.h264") bytes"
  echo "$name engine: $engine"
  echo "$name x264: $reference"
  echo "$name ffmpeg on the engine's stream: $(wc -c < "out/$name.ffmpeg.txt") bytes of complaints, $(wc -c < engine.nv12) bytes decoded"
  {
    echo "engine: $engine"
    echo "x264: $reference"
  } > "out/$name.psnr.txt"
  rm -f source.nv12 engine.nv12 x264.nv12
done
