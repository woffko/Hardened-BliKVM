#!/bin/sh
# The encoder driver's checks and register plan on the build host.
#
# usage: run.sh OUTPUT_DIR LAB_TOOL
#
# Builds plan_test.c with the plan as the driver has it, runs it with
# random requests, and compares the plan's operations for passes of the
# lab's sizes with those of the lab tool (`bli-kvm-ve-lab plan`): the
# hypothesis `h6`, which was the vendor library's and is what the board
# encodes with, and the one bit of the other search.
set -eu

output=${1:-}
lab=${2:-}
case "$output" in /*) ;; *) echo "usage: $0 OUTPUT_DIR (absolute) LAB_TOOL" >&2; exit 2 ;; esac
[ -x "$lab" ] || { echo "usage: $0 OUTPUT_DIR LAB_TOOL" >&2; exit 2; }
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
mkdir -p "$output"

"${CC:-cc}" -std=gnu11 -Wall -Wextra -Werror -O2 -fsanitize=address,undefined \
  -I "$here/shim" -o "$output/plan_test" "$here/plan_test.c" "$here/../ve_enc_plan.c"
"$output/plan_test" random 200000

passes=0
# The size, key or p, the QP, the set written and the set read, options.
for pass in \
  "1920x1080 key 30 0 0" "1920x1080 p 30 1 0" "1920x1080 p 0 2 1 wide" "1920x1080 key 51 2 2 cabac" \
  "1920x1080 p 26 0 2 cabac wide" "1366x768 key 26 1 1" "1366x768 p 31 2 0" "1280x720 key 10 0 0" \
  "1280x720 p 40 1 2 wide" "320x240 key 30 0 0 wide" "320x240 p 30 1 0" "256x64 key 12 2 2" \
  "256x64 p 12 0 1" "256x64 p 12 0 1 wide"; do
  # shellcheck disable=SC2086
  "$lab" plan $pass > "$output/lab.txt"
  "$output/plan_test" dump < "$output/lab.txt" > "$output/driver.txt"
  sed '1,/^ops$/d' "$output/lab.txt" > "$output/lab-ops.txt"
  if ! cmp -s "$output/lab-ops.txt" "$output/driver.txt"; then
    echo "the driver's plan is not the lab tool's for: $pass" >&2
    diff "$output/lab-ops.txt" "$output/driver.txt" | head -20 >&2
    exit 1
  fi
  passes=$((passes + 1))
done
echo "ve-enc plan: the driver's operations are the lab tool's for $passes passes"

# Which set of buffers a pass reads and writes: the driver's rule against
# the client's own account of it, over passes with flags by chance.
"$lab" sets 200000 20261002 > "$output/sets-lab.txt"
"$output/plan_test" sets < "$output/sets-lab.txt" > "$output/sets-driver.txt"
if ! cmp -s "$output/sets-lab.txt" "$output/sets-driver.txt"; then
  echo "the driver's sets are not the client's:" >&2
  diff "$output/sets-lab.txt" "$output/sets-driver.txt" | head -10 >&2
  exit 1
fi
echo "ve-enc plan: the driver's sets are the client's for $(wc -l < "$output/sets-lab.txt") passes, $(grep -c -- '-$' "$output/sets-lab.txt") of them refused"
