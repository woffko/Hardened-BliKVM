#!/bin/sh
# On the board, inside a lab session (tools/device/ve_lab_session.sh): the
# product's H.264 encoder on the video engine for HOURS of encoding, with a
# viewer's demand held on the board itself, so that no connection from
# outside is needed and rule 1 can still be watched
# (docs/design/h616-ve-h264-encoder.md, V7.1). The engine is switched on with
# tools/device/deploy_ve.sh and, whenever somebody connects to the board, off
# again until the board has been quiet for five minutes: then x264 serves
# whoever came. Every minute the encoder's statistics lines, the kernel's
# new lines, the SoC's temperatures and the encoder's processor time are
# kept under out/.
#
# usage: sh ve_hold_run.sh HOURS KBPS [MAX_HOURS]
#   HOURS      hours of encoding on the engine to collect
#   KBPS       the bitrate the held viewer asks for (the operator's WebRTC
#              quality, as the web server would send it)
#   MAX_HOURS  the most the run may take in all (default: HOURS + 8)
#
# It ends with the engine off, as it found it. A lost connection does not
# end it: the session gives the watchdog up and removes its directory, so
# the run keeps what it needs in a directory of its own under /run, goes on
# watching for whoever connects, and removes that directory at its end.
set -u
trap '' HUP
hours=${1:?usage: $0 HOURS KBPS [MAX_HOURS]}
kbps=${2:?usage: $0 HOURS KBPS [MAX_HOURS]}
max_hours=${3:-$((hours + 8))}
want=$((hours * 3600)); limit=$((max_hours * 3600))
work=$(mktemp -d /run/ve-hold.XXXXXX) || exit 1
cp ./deploy_ve.sh ./ve_demand_hold.py "$work/" || { rm -rf "$work"; exit 1; }
encoder=hardened-blikvm-video-encoder-h264.service
demand=/run/hardened-blikvm-h264/demand.sock
holder=

in_use() {
  [ "$(ss -Htn state established '( sport = :443 )' | wc -l)" -gt 0 ] && return 0
  sessions=$(journalctl -b -o cat --no-pager -u hardened-blikvm-quic.service 2>/dev/null |
    awk '/session from .* opened/ { opened++ } /session from .* ended/ { ended++ } END { print opened - ended }')
  [ "${sessions:-0}" -gt 0 ]
}

release() {
  if [ -n "$holder" ]; then
    kill "$holder" 2>/dev/null
    wait "$holder" 2>/dev/null
    holder=
  fi
}

engine_off() {
  release
  sh "$work/deploy_ve.sh" remove > out/deploy-off.txt 2>&1 || echo "$(date -u +%FT%TZ) deploy_ve.sh remove failed" >> out/hold.txt
}

finish() {
  engine_off
  echo "$(date -u +%FT%TZ) end: $on seconds on the engine, $switches times given up for somebody" >> out/hold.txt
  rm -rf "$work"
}
trap 'finish; exit 143' TERM INT

hold() {
  python3 "$work/ve_demand_hold.py" "$demand" h264 "$kbps" >> out/holder.txt 2>&1 &
  holder=$!
}

temperatures() {
  for zone in /sys/class/thermal/thermal_zone*; do
    printf '%s=%s ' "$(cat "$zone/type")" "$(cat "$zone/temp")"
  done
}

: > out/hold.txt
dmesg > kernel.before
began=$(date +%s); on=0; switches=0; state=off; quiet_since=$began
journal_from=$(date '+%Y-%m-%d %H:%M:%S')
while :; do
  now=$(date +%s)
  [ "$on" -ge "$want" ] && break
  [ $((now - began)) -ge "$limit" ] && break
  if in_use; then
    quiet_since=$now
    if [ "$state" = on ]; then
      engine_off
      state=off
      switches=$((switches + 1))
      echo "$(date -u +%FT%TZ) somebody connected: the engine is off" >> out/hold.txt
    fi
  elif [ "$state" = off ] && [ $((now - quiet_since)) -ge 300 ]; then
    sh "$work/deploy_ve.sh" install > out/deploy-on.txt 2>&1 ||
      { echo "$(date -u +%FT%TZ) deploy_ve.sh install failed"; cat out/deploy-on.txt; } >> out/hold.txt
    hold
    state=on
    echo "$(date -u +%FT%TZ) the engine is on" >> out/hold.txt
  fi
  sleep 20
  # The encoder closes the demand when it restarts: the viewer comes back.
  if [ "$state" = on ] && ! kill -0 "$holder" 2>/dev/null; then
    echo "$(date -u +%FT%TZ) the demand was closed; held again" >> out/hold.txt
    hold
  fi
  if [ "$state" = on ]; then
    on=$((on + 20))
    if [ $((on % 60)) -eq 0 ]; then
      pid=$(systemctl show -p MainPID --value "$encoder")
      echo "$(date -u +%FT%TZ) on=$on pid=$pid cpu_ticks=$(awk '{ print $14 + $15 }' "/proc/$pid/stat" 2>/dev/null) $(temperatures)" >> out/samples.txt
    fi
  fi
done
finish
journalctl -o short-iso --no-pager -u "$encoder" --since "$journal_from" > out/encoder-journal.txt 2>&1
dmesg > kernel.after
diff kernel.before kernel.after | sed -n 's/^> //p' > out/kernel-new.txt
rm -f kernel.before kernel.after
grep -c "the video engine:" out/encoder-journal.txt > out/engine-errors.txt
echo "lines from the kernel: $(wc -l < out/kernel-new.txt); the engine's failures in the journal: $(cat out/engine-errors.txt)"
tail -n 3 out/hold.txt
