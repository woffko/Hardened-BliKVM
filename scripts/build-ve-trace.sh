#!/bin/sh
# Build the register trace of the hardware H.264 plan (tools/ve-trace): a
# lab instrument that goes into no package.
#
# usage (from the repository root):
#   build-ve-trace.sh host OUTPUT_DIR
#       the trace's own test for this machine; it is run, and must pass
#   build-ve-trace.sh aarch64 WORK_DIR OUTPUT_DIR [INTERFACE_DIR]
#       the same test for aarch64, with the compiler that
#       build-ve-module.sh keeps in WORK_DIR; with INTERFACE_DIR, the
#       directory of the vendor encoder library's interface headers in the
#       cache outside the repository, also the program that runs that
#       library behind the closed window, and the three empty companions
#       it is loaded with
#
# Nothing of the vendor's is built in or copied: its headers are read at
# compile time for the types of its interface, and its library is named
# only when the program is run.
set -eu

mode=${1:-}
project_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
source_dir="$project_root/tools/ve-trace"
flags="-std=gnu11 -O2 -Wall -Wextra -Werror"
core="harness.c window.c model.c log.c"

usage() {
  echo "usage: $0 host OUTPUT_DIR | aarch64 WORK_DIR OUTPUT_DIR [INTERFACE_DIR]" >&2
  exit 2
}
absolute() { case "$1" in /*) ;; *) usage ;; esac; }

build_selftest() {
  # $1: compiler, $2: output directory
  mkdir -p "$2"
  ( cd "$source_dir" &&
    # shellcheck disable=SC2086
    "$1" $flags -fPIC -shared -o "$2/libselftest-client.so" selftest_client.c &&
    "$1" $flags -o "$2/ve-trace-selftest" selftest.c $core -ldl )
}

case "$mode" in
  host)
    [ "$#" -eq 2 ] || usage
    absolute "$2"
    build_selftest "${CC:-cc}" "$2"
    "$2/ve-trace-selftest" "$2/libselftest-client.so"
    ;;
  aarch64)
    [ "$#" -eq 3 ] || [ "$#" -eq 4 ] || usage
    absolute "$2"; absolute "$3"
    compiler="$2/gcc-arm-8.3-2019.03-x86_64-aarch64-linux-gnu/bin/aarch64-linux-gnu-gcc"
    [ -x "$compiler" ] || { echo "no compiler in $2: run build-ve-module.sh prepare first" >&2; exit 1; }
    # No path of the build machine: no debug information, and no symbol
    # table (it names the toolchain's start files by their paths).
    flags="$flags -s"
    build_selftest "$compiler" "$3"
    if [ "$#" -eq 4 ]; then
      absolute "$4"
      [ -f "$4/vencoder.h" ] || { echo "$4 has no vencoder.h" >&2; exit 1; }
      ( cd "$source_dir" &&
        # shellcheck disable=SC2086
        "$compiler" $flags -I"$4" -rdynamic -o "$3/ve-trace-vendor" vendor_run.c vendor_ops.c $core \
          -Wno-error -Wno-strict-prototypes vendor_ion.c -ldl &&
        for name in libcdc_base.so libVE.so libMemAdapter.so; do
          "$compiler" -s -shared -fPIC -Wl,-soname,"$name" -o "$3/$name" companion_stub.c
        done )
    fi
    for built in "$3"/ve-trace-* "$3"/*.so; do
      if strings "$built" | grep -q "$project_root\|$HOME"; then
        echo "$(basename "$built") carries a path of the build machine" >&2
        exit 1
      fi
      echo "$(sha256sum "$built" | cut -c1-64)  $(basename "$built")"
    done
    ;;
  *) usage ;;
esac
