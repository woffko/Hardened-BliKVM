#!/bin/sh
# On the board, inside a lab session (ve_lab_session.sh): the register trace
# of the hardware H.264 plan (tools/ve-trace). No engine is involved and no
# module is loaded: the traced code runs as an unprivileged user against a
# stand-in, in the session's directory on tmpfs, and is gone with it.
#
# usage: sh ve_trace_run.sh [LIBRARY 'NAME [SETTING=VALUE...]'...]
#
# Without arguments only the trace's own test runs. With a library (a file
# of the session's directory), each further argument is one run of it: a
# name for its results under out/, and settings of ve-trace-vendor.
set -u

command -v setpriv > /dev/null || { echo "no setpriv on this board"; exit 1; }
group=$(id -g nobody) || exit 1
unprivileged() {
  setpriv --reuid=nobody --regid="$group" --clear-groups --no-new-privs "$@"
}

# The session's directory belongs to root; the unprivileged user may pass
# through it and read what was copied, and writes only where it is given room.
chmod 0711 . || exit 1
chmod 0755 ./ve-trace-selftest || exit 1
chmod 0644 ./*.so || exit 1

echo "+ the trace's own test"
unprivileged ./ve-trace-selftest ./libselftest-client.so || { echo "status $?"; exit 1; }

library=${1:-}
[ -n "$library" ] || exit 0
shift
[ -f "./$library" ] && [ -x ./ve-trace-vendor ] || { echo "no library or no trace program"; exit 1; }
chmod 0755 ./ve-trace-vendor
sha256sum "./$library"

for run in "$@"; do
  name=${run%% *}
  settings=${run#"$name"}
  case "$name" in *[!a-z0-9-]*|"") echo "not a run name: $name"; exit 2 ;; esac
  mkdir -p "out/$name" && chown nobody "out/$name" || exit 1
  echo "+ $name:$settings"
  status=0
  # shellcheck disable=SC2086
  LD_LIBRARY_PATH=. unprivileged ./ve-trace-vendor "./$library" "out/$name" $settings \
    > "out/$name/library.log" 2>&1 || status=$?
  echo "status $status, $(wc -c < "out/$name/trace.bin" 2>/dev/null || echo 0) bytes of trace"
done
exit 0
