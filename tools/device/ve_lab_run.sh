#!/bin/sh
# On the board, inside a lab session (ve_lab_session.sh): gives the video
# engine to the lab module for the length of the given commands and back to
# the decoder driver afterwards, whatever the commands do.
#
# usage: sh ve_lab_run.sh 'COMMAND' ['COMMAND'...]
# Each argument is one command line; they run in order until one fails.
#
# Rule 1 is kept for the whole run, not only at its beginning: every twenty
# seconds the board is asked whether somebody has connected to it (an HTTPS
# connection or a QUIC session, without which nobody can hold the HID
# lease). If so the running command is ended, the engine is given back, and
# the run ends with status 75.
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

# Whether somebody is connected to the board.
in_use() {
  [ "$(ss -Htn state established '( sport = :443 )' | wc -l)" -gt 0 ] && return 0
  sessions=$(journalctl -b -o cat --no-pager -u hardened-blikvm-quic.service 2>/dev/null |
    awk '/session from .* opened/ { opened++ } /session from .* ended/ { ended++ } END { print opened - ended }')
  [ "${sessions:-0}" -gt 0 ]
}

status=0
for command in "$@"; do
  echo "+ $command"
  # In a process group of its own, so that all of it can be ended.
  setsid sh -c "$command" &
  running=$!
  (
    while sleep 20; do
      if in_use; then
        echo "the board is in use: the command is ended"
        : > stopped-for-use
        kill -TERM -- "-$running" 2>/dev/null
        break
      fi
    done
  ) &
  watcher=$!
  wait "$running" || status=$?
  kill "$watcher" 2>/dev/null
  wait "$watcher" 2>/dev/null
  if [ -e stopped-for-use ]; then
    status=75
    break
  fi
  [ "$status" -eq 0 ] || { echo "status $status"; break; }
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
