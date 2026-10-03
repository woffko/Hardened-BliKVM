#!/bin/sh
# On the board, inside a lab session (ve_lab_session.sh): the lab's
# sequences through the lab module and through the encoder driver, and
# whether both give the same stream, byte for byte
# (docs/design/h616-ve-encoder-driver-v1.md, gate V4).
#
# usage: sh ve_driver_compare.sh WIDTHxHEIGHT FRAMES KEY_EVERY 'RATE...' SEQUENCE... [-- OPTION...]
#
# The engine is given to the lab module, then to the driver, once for every
# sequence and rate; ve_lab_run.sh keeps the lab's rules around each. The
# options after -- go to the lab tool's `stream` both times (search=auto,
# burst=M:R). The streams lie in the session's directory while they are
# compared; their sizes and digests are left in out/.
set -u
[ "$#" -ge 5 ] || { echo "usage: $0 WIDTHxHEIGHT FRAMES KEY_EVERY 'RATE...' SEQUENCE... [-- OPTION...]"; exit 2; }
size=$1; frames=$2; key_every=$3; rates=$4
shift 4
sequences=
while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do sequences="$sequences $1"; shift; done
[ "$#" -eq 0 ] || shift
options=$*
tool=./bli-kvm-ve-lab

status=0
: > out/compare.txt
for sequence in $sequences; do
  for rate in $rates; do
    name="$sequence-$size-$rate"
    rm -f lab.h264 driver.h264
    sh ./ve_lab_run.sh \
      "$tool stream h6 $sequence $size $frames $key_every $rate 30 lab.h264 out/$name.lab.json guards=30 $options" ||
      { echo "$name: the lab module's run failed" | tee -a out/compare.txt; status=1; continue; }
    VE_MODULE=hardened-blikvm-ve-enc sh ./ve_lab_run.sh \
      "$tool stream driver $sequence $size $frames $key_every $rate 30 driver.h264 out/$name.driver.json $options" ||
      { echo "$name: the driver's run failed" | tee -a out/compare.txt; status=1; continue; }
    if cmp -s lab.h264 driver.h264 && [ -s lab.h264 ]; then
      echo "$name: the same $(wc -c < lab.h264) bytes, sha256 $(sha256sum < lab.h264 | cut -c1-16)" |
        tee -a out/compare.txt
    else
      echo "$name: DIFFERENT: $(wc -c < lab.h264) bytes by the lab module, $(wc -c < driver.h264) by the driver; first difference: $(cmp lab.h264 driver.h264 2>&1 | head -1)" |
        tee -a out/compare.txt
      status=1
    fi
  done
done
rm -f lab.h264 driver.h264
exit "$status"
