#!/bin/sh
# Runs a command again and again beside busy loops, to show whether it holds
# where the host is loaded (docs/baselines/work-plan-20261004-review-fixes.md,
# A1):
#
#   stress-under-load.sh RUNS [LOAD] -- COMMAND [ARGUMENT...]
#
# RUNS is how often COMMAND runs, one run after the other. LOAD is how many
# busy loops run beside it: at most four times the cores, one and a half times
# the cores where it is left out. Every run has a time limit, 300 seconds
# unless STRESS_TIMEOUT says otherwise, and a run over it fails. The output of
# a run that failed is shown (its last forty lines); with STRESS_LOGS set to a
# directory the whole output of every run is kept there as run-N.log.
# STRESS_WARMUP (seconds, 2 by default) is how long the loops run before the
# first run.
#
# The last line is `stress: RUNS runs, N passed, M failed (LOAD busy loops)`;
# the status is 0 only where every run passed, 2 for a mistake in the
# arguments. The loops are stopped on every exit and on SIGINT and SIGTERM,
# and each notices on its own when this script is gone, SIGKILL included. Each
# carries `stress-under-load-loop-PID` in its command line, PID being this
# script's, so that they can be told from other processes and counted.
#
# The loops take as much of the processors as they get: run it on a host
# nobody else is waiting for.
set -u

usage() {
  echo "usage: $0 RUNS [LOAD] -- COMMAND [ARGUMENT...]" >&2
  echo "  RUNS 1 to 10000; LOAD 0 to four times the cores (default: one and a half times)" >&2
  exit 2
}

number() { # a non-negative whole number
  case ${1:-} in
    '' | *[!0-9]*) return 1 ;;
  esac
}

cores=$(nproc 2>/dev/null || getconf _NPROCESSORS_ONLN 2>/dev/null || echo 1)
number "$cores" || cores=1

[ "$#" -ge 3 ] || usage
runs=$1
shift
if [ "$1" = "--" ]; then
  load=$((cores * 3 / 2))
  [ "$load" -ge 1 ] || load=1
else
  load=$1
  shift
  [ "${1:-}" = "--" ] || usage
fi
shift
[ "$#" -ge 1 ] || usage
number "$runs" && number "$load" || usage
[ "$runs" -ge 1 ] && [ "$runs" -le 10000 ] || usage
if [ "$load" -gt $((cores * 4)) ]; then
  echo "stress: $load busy loops are more than four times the $cores cores" >&2
  exit 2
fi
limit=${STRESS_TIMEOUT:-300}
warmup=${STRESS_WARMUP:-2}
number "$limit" && [ "$limit" -ge 1 ] || usage
number "$warmup" || usage

keep=
if [ -n "${STRESS_LOGS:-}" ]; then
  mkdir -p "$STRESS_LOGS" || exit 2
  work=$STRESS_LOGS
  keep=1
else
  work=$(mktemp -d "${TMPDIR:-/tmp}/stress-under-load.XXXXXX") || exit 2
fi

marker="stress-under-load-loop-$$"
loops=""
child=""
cleanup() {
  [ -z "$child" ] || kill "$child" 2>/dev/null
  # shellcheck disable=SC2086
  [ -z "$loops" ] || kill $loops 2>/dev/null
  wait 2>/dev/null
  [ -n "$keep" ] || rm -rf "$work"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# A loop asks whether this script is still there, which costs a system call
# and not a process, and ends when it is not.
i=0
while [ "$i" -lt "$load" ]; do
  sh -c 'while kill -0 "$1" 2>/dev/null; do :; done' "$marker" "$$" &
  loops="$loops $!"
  i=$((i + 1))
done
echo "stress: $runs runs of: $*"
echo "stress: $load busy loops on $cores cores (marked $marker), a run may take $limit s"
[ "$warmup" -eq 0 ] || sleep "$warmup"

passed=0
failed=0
n=1
while [ "$n" -le "$runs" ]; do
  log="$work/run-$n.log"
  if command -v timeout >/dev/null 2>&1; then
    timeout "$limit" "$@" >"$log" 2>&1 &
  else
    "$@" >"$log" 2>&1 &
  fi
  child=$!
  wait "$child"
  status=$?
  child=""
  if [ "$status" -eq 0 ]; then
    passed=$((passed + 1))
    echo "run $n: passed"
  else
    failed=$((failed + 1))
    echo "run $n: failed (status $status)"
    tail -n 40 "$log"
  fi
  n=$((n + 1))
done

echo "stress: $runs runs, $passed passed, $failed failed ($load busy loops)"
if [ "$failed" -eq 0 ]; then
  exit 0
fi
exit 1
