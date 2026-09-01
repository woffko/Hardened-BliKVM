#!/bin/sh
set -eu

if [ "$#" -eq 0 ]; then
  echo "usage: $0 BINARY..." >&2
  exit 2
fi

for binary in "$@"; do
  if [ ! -f "$binary" ]; then
    echo "missing binary: $binary" >&2
    exit 1
  fi

  file "$binary" | rg -q 'ELF 64-bit LSB pie executable, ARM aarch64.*static-pie linked.*stripped'
  readelf -W -h "$binary" | rg -q 'Type:[[:space:]]+DYN \(Position-Independent Executable file\)'
  readelf -W -h "$binary" | rg -q 'Machine:[[:space:]]+AArch64'
  readelf -W -l "$binary" | rg -q 'GNU_RELRO'
  readelf -W -l "$binary" | rg -q 'GNU_STACK.*RW[[:space:]]+0x?0?$'
  readelf -W -d "$binary" | rg -q '\(FLAGS\).*BIND_NOW'
  readelf -W -d "$binary" | rg -q '\(FLAGS_1\).*NOW PIE'

  if readelf -W -l "$binary" | rg -q 'INTERP'; then
    echo "unexpected ELF interpreter: $binary" >&2
    exit 1
  fi
  if readelf -W -d "$binary" | rg -q '\((NEEDED|RPATH|RUNPATH)\)'; then
    echo "unexpected dynamic dependency or search path: $binary" >&2
    exit 1
  fi
  if strings "$binary" | rg -q '(/home/[^/]+/|/root/|/mnt/[A-Za-z]/|[A-Za-z]:\\Users\\)'; then
    echo "host-specific build path entered ELF: $binary" >&2
    exit 1
  fi
done
