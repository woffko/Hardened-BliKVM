#!/bin/sh
# On the board, inside a lab session (ve_lab_session.sh): gives the video
# engine to the lab module for the length of the given commands and back to
# the decoder driver afterwards, whatever the commands do.
#
# usage: sh ve_lab_run.sh 'COMMAND' ['COMMAND'...]
# Each argument is one command line; they run in order until one fails.
#
# With ve_memory_scan.py among the session's files, rule 9 of the lab is kept
# here: the kernel's image is copied before the engine is handed over and
# compared after it is back, and memory is read for the parameter sets the
# bit writer was given. A word of the kernel's image that changed, and that
# the kernel did not switch itself, ends the run with status 79.
set -u
node=1c0e000.video-codec
decoder=/sys/bus/platform/drivers/cedrus

restore() {
  if grep -q '^hardened_blikvm_ve_lab ' /proc/modules; then
    rmmod hardened_blikvm_ve_lab || echo "the lab module did not unload"
  fi
  if [ ! -e "/sys/bus/platform/devices/$node/driver" ] && [ -d "$decoder" ]; then
    echo "$node" > "$decoder/bind" || echo "the decoder driver did not bind"
  fi
}
trap restore EXIT
trap 'exit 129' HUP INT TERM

scan=
if [ -f ./ve_memory_scan.py ] && command -v python3 > /dev/null; then
  mkdir -p out
  python3 ./ve_memory_scan.py snapshot kernel-image.before && scan=yes
fi

found=$(basename "$(readlink "/sys/bus/platform/devices/$node/driver" 2>/dev/null)" 2>/dev/null)
echo "the engine's driver as found: ${found:-none}"
if [ "$found" = cedrus ]; then
  echo "$node" > "$decoder/unbind" || exit 1
fi
insmod ./hardened-blikvm-ve-lab.ko || exit 1
[ -c /dev/hardened-blikvm-ve-lab ] || { echo "no lab device"; exit 1; }
chmod 0755 ./bli-kvm-ve-lab

status=0
for command in "$@"; do
  echo "+ $command"
  sh -c "$command" || { status=$?; echo "status $status"; break; }
done

restore
trap - EXIT
if [ -n "$scan" ]; then
  python3 ./ve_memory_scan.py compare kernel-image.before > out/kernel-image.txt 2>&1 || status=79
  cat out/kernel-image.txt
  rm -f kernel-image.before
  python3 ./ve_memory_scan.py > out/memory-scan.txt 2>&1
  cat out/memory-scan.txt
fi
exit "$status"
