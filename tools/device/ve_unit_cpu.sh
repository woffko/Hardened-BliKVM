#!/bin/sh
# On the board: the processor time of every unit on the picture's way (its
# control group's cpu.stat, every thread and every process it had), the
# board's processor time in all, and the SoC's temperatures, every five
# seconds for SECONDS (docs/design/h616-ve-h264-encoder.md, V7.1). Reads
# /proc and /sys only.
#
# usage: sh ve_unit_cpu.sh SECONDS
set -u
seconds=${1:?usage: $0 SECONDS}
units="hardened-blikvm-ustreamer hardened-blikvm-video-encoder-h264 hardened-blikvm-video-encoder hardened-blikvm-janus hardened-blikvm-quic hardened-blikvm-server-current hardened-blikvm-server-candidate hardened-blikvm-nginx hardened-blikvm-board-video-proxy"
sample() {
  printf 'at=%s' "$(date +%s.%N)"
  for unit in $units; do
    usage=$(awk '$1 == "usage_usec" { print $2 }' "/sys/fs/cgroup/system.slice/$unit.service/cpu.stat" 2>/dev/null)
    printf ' %s=%s' "$unit" "${usage:-0}"
  done
  for zone in /sys/class/thermal/thermal_zone*; do
    printf ' %s=%s' "$(cat "$zone/type")" "$(cat "$zone/temp")"
  done
  # user, nice, system, irq and softirq time of all processors, in ticks.
  printf ' all_ticks=%s\n' "$(awk '/^cpu / { print $2 + $3 + $4 + $7 + $8 }' /proc/stat)"
}
echo "ticks a second: $(getconf CLK_TCK)"
end=$(($(date +%s) + seconds))
while [ "$(date +%s)" -lt "$end" ]; do
  sample
  sleep 5
done
sample
