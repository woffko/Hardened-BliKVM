#!/bin/sh
# One lab session of the hardware H.264 plan on the test board
# (docs/design/h616-ve-h264-encoder.md, "Rules for experiments").
#
# usage (from the repository root, the board's root password on stdin through
# project-memory-exec):
#   tools/device/ve_lab_session.sh OUTPUT_DIR FILE... -- COMMAND [ARGUMENT...]
#
# The files are copied to a directory on the board's tmpfs (/tmp: /run does
# not let a program run), the command runs
# there as root with its arguments as given (shell syntax needs `sh -c`), and
# what it leaves in `out/` comes back into OUTPUT_DIR with the session's log
# and the kernel's new lines. Around the command the board
# side of this script keeps the rules that need no knowledge of the
# experiment:
#
# - it runs only while no browser is connected to the board and no QUIC
#   session is open (the HID lease cannot be seen from outside its holder, and
#   nobody can hold it without one of the two); otherwise it ends with status
#   75 and nothing was done;
# - `sync` first, and the SoC's watchdog armed for the length of the command,
#   so that a kernel that hangs is reset; the watchdog is disarmed afterwards;
# - the kernel log is compared before and after, and a new warning, oops or
#   taint is reported;
# - afterwards nothing of the session is left: its directory is removed and
#   the modules named in BLI_KVM_VE_LAB_MODULES are checked to be unloaded.
#
# The device address comes from the operator environment, never from Git.
set -eu

HOST=${BLI_KVM_DEVICE_HOST:?set BLI_KVM_DEVICE_HOST to the board address}
KNOWN_HOSTS=${BLI_KVM_KNOWN_HOSTS:-evidence/private/ssh-known-hosts}
MODULES=${BLI_KVM_VE_LAB_MODULES:-"hardened_blikvm_ve_lab ve_hello ve_probe_test"}
REQUIRE_QUIET=${BLI_KVM_VE_LAB_REQUIRE_QUIET:-yes}

output=${1:-}
case "$output" in ""|-*) echo "usage: $0 OUTPUT_DIR FILE... -- COMMAND [ARGUMENT...]" >&2; exit 2 ;; esac
shift
files=
while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do
  [ -f "$1" ] || { echo "no such file: $1" >&2; exit 2; }
  files="$files $1"
  shift
done
[ "${1:-}" = "--" ] && [ "$#" -ge 2 ] || { echo "a command must follow --" >&2; exit 2; }
shift
mkdir -p "$output"

# One authentication: a master connection that the copies and commands share.
# A session says nothing while its command runs, and a connection that is
# silent for half an hour is forgotten by an address translation on the
# way: the board's side then ends and what it sends back is lost. The
# client asks the board every twenty seconds whether it is there, and gives
# the connection up after five minutes without an answer.
exec 3<&0
control=$(mktemp -d "${TMPDIR:-/tmp}/ve-lab-ssh.XXXXXX")
socket="$control/master"
finish() {
  ssh -S "$socket" -O exit "root@$HOST" 2>/dev/null || true
  rm -rf "$control"
}
trap finish EXIT HUP INT TERM
sshpass -d 3 ssh -M -S "$socket" -o ControlPersist=600 -o StrictHostKeyChecking=yes \
  -o UserKnownHostsFile="$KNOWN_HOSTS" -o ConnectTimeout=10 \
  -o ServerAliveInterval=20 -o ServerAliveCountMax=15 -fN "root@$HOST"
board() { ssh -S "$socket" "root@$HOST" "$@"; }

directory=$(board 'mktemp -d /tmp/hardened-ve-lab.XXXXXX')
case "$directory" in /tmp/hardened-ve-lab.*) ;; *) echo "no session directory" >&2; exit 1 ;; esac
# shellcheck disable=SC2086
[ -z "$files" ] || scp -q -o ControlPath="$socket" $files "root@$HOST:$directory/"

# The board's side.
board "cat > $directory/session.sh" <<'BOARD'
#!/bin/sh
set -u
directory=$1; require_quiet=$2; modules=$3; shift 3
cd "$directory" || exit 1
mkdir -p out
log=out/session.log
say() { echo "$*" >> "$log"; }

quiet() {
  connections=$(ss -Htn state established '( sport = :443 )' | wc -l)
  sessions=$(journalctl -b -o cat --no-pager -u hardened-blikvm-quic.service 2>/dev/null |
    awk '/session from .* opened/ { opened++ } /session from .* ended/ { ended++ } END { print opened - ended }')
  say "quiet check: $connections HTTPS connections, $sessions QUIC sessions"
  [ "$connections" -eq 0 ] && [ "${sessions:-0}" -le 0 ]
}

keeper=
disarm() {
  if [ -n "$keeper" ]; then
    kill -TERM "$keeper" 2>/dev/null
    wait "$keeper" 2>/dev/null
    keeper=
  fi
}
cleanup() {
  disarm
  tar -C "$directory" -cf - out 2>/dev/null
  cd /
  rm -rf "$directory"
}
# A lost connection must not leave the watchdog armed and unfed.
trap 'disarm; cd /; rm -rf "$directory"; exit 129' HUP INT TERM

say "session $(date -u +%Y-%m-%dT%H:%M:%SZ) kernel $(uname -r) taint $(cat /proc/sys/kernel/tainted)"
if [ "$require_quiet" = yes ]; then
  for attempt in 1 2 3; do
    if ! quiet; then
      say "the board is in use; nothing was done"
      cleanup
      exit 75
    fi
    [ "$attempt" = 3 ] || sleep 4
  done
fi

dmesg > out/kernel.before
sync
# The dead man: the SoC resets the board unless this keeps writing. A
# written V before closing disarms it.
(
  trap '' HUP INT
  exec 9> /dev/watchdog || exit 1
  trap 'printf V >&9; exec 9>&-; exit 0' TERM
  while :; do
    printf . >&9
    sleep 2 9>&- &
    wait $!
  done
) &
keeper=$!
sleep 1
if ! kill -0 "$keeper" 2>/dev/null; then
  say "the watchdog could not be armed; nothing was done"
  keeper=
  cleanup
  exit 76
fi
say "watchdog armed"

status=0
"$@" >> "$log" 2>&1 || status=$?
say "command status $status"

disarm
say "watchdog disarmed"
dmesg > out/kernel.after
# What the kernel said during the session.
awk 'NR == FNR { seen[$0] = 1; next } !($0 in seen)' out/kernel.before out/kernel.after > out/kernel.new
rm -f out/kernel.before out/kernel.after
left=
for module in $modules; do
  if grep -q "^$module " /proc/modules; then left="$left $module"; fi
done
say "modules left:${left:- none}"
say "taint $(cat /proc/sys/kernel/tainted)"
say "video engine driver: $(basename "$(readlink /sys/bus/platform/devices/1c0e000.video-codec/driver 2>/dev/null)" 2>/dev/null) $(cat /sys/bus/platform/devices/1c0e000.video-codec/power/runtime_status 2>/dev/null)"
cleanup
[ -z "$left" ] || exit 77
exit "$status"
BOARD

# Every word of the command quoted for the board's shell.
quoted=
for word in "$@"; do
  quoted="$quoted '$(printf '%s' "$word" | sed "s/'/'\\\\''/g")'"
done
status=0
board "sh $directory/session.sh $directory $REQUIRE_QUIET '$MODULES' $quoted" > "$control/out.tar" || status=$?
tar -xf "$control/out.tar" -C "$output" --strip-components=1 2>/dev/null || true
remaining=$(board 'ls -d /tmp/hardened-ve-lab.* 2>/dev/null | wc -l')
[ "$remaining" = 0 ] || { echo "a session directory is left on the board" >&2; status=78; }
echo "session status $status; results in $output"
exit "$status"
