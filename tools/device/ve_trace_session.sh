#!/bin/sh
# One session of the register trace on the test board
# (docs/design/h616-ve-h264-encoder.md, V2R). The engine is not touched.
#
# usage (from the repository root, the board's root password on stdin through
# project-memory-exec):
#   tools/device/ve_trace_session.sh OUTPUT_DIR TRACE_DIR [LIBRARY 'RUN'...]
#
# TRACE_DIR is what `scripts/build-ve-trace.sh aarch64` made. LIBRARY is the
# vendor's encoder library in the cache outside the repository; it is copied
# to the board's tmpfs for the length of the session and removed with the
# session's directory. Each RUN is a name and settings of ve-trace-vendor.
# Without a library only the trace's own test runs.
#
# The session keeps the rules of ve_lab_session.sh, which carries it: the
# board must be quiet, and nothing is left behind.
set -eu

output=${1:-}
trace=${2:-}
[ -n "$output" ] && [ -d "$trace" ] || {
  echo "usage: $0 OUTPUT_DIR TRACE_DIR [LIBRARY 'RUN'...]" >&2
  exit 2
}
shift 2
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

files="$trace/ve-trace-selftest $trace/libselftest-client.so $script_dir/ve_trace_run.sh"
library=
if [ "$#" -gt 0 ]; then
  library=$1
  shift
  [ -f "$library" ] || { echo "no such library: $library" >&2; exit 2; }
  files="$files $trace/ve-trace-vendor $trace/libcdc_base.so $trace/libVE.so $trace/libMemAdapter.so $library"
fi

# shellcheck disable=SC2086
if [ -n "$library" ]; then
  exec sh "$script_dir/ve_lab_session.sh" "$output" $files -- \
    sh ./ve_trace_run.sh "$(basename "$library")" "$@"
else
  exec sh "$script_dir/ve_lab_session.sh" "$output" $files -- sh ./ve_trace_run.sh
fi
