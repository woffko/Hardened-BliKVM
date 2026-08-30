#!/bin/sh
set -eu

rust_sysroot=$(rustc --print sysroot)
rust_host=$(rustc -vV | sed -n 's/^host: //p')
rust_lld="$rust_sysroot/lib/rustlib/$rust_host/bin/rust-lld"
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

if [ ! -x "$rust_lld" ]; then
  echo "rust-lld was not found in the active Rust toolchain" >&2
  exit 1
fi

exec /usr/bin/python3 "$script_dir/rust_lld_static_pie.py" "$rust_lld" "$@"
