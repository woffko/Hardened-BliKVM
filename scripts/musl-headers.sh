#!/bin/sh
# Installs the musl C library's headers for aarch64 into OUTPUT_DIR.
# usage: musl-headers.sh OUTPUT_DIR
#
# Rust crates that compile C for the board (ring, in the QUIC gateway's TLS)
# need libc headers and nothing else of a C toolchain: clang compiles against
# these headers and rustc links with rust-lld against the toolchain's own
# musl. The source is the release the encoder library scripts pin
# (build-x264.sh, build-libvpx.sh, build-libjpeg-turbo.sh).
set -eu

MUSL_VERSION=1.2.5
MUSL_SHA256=a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4
MUSL_URL="https://musl.libc.org/releases/musl-$MUSL_VERSION.tar.gz"

output=${1:-}
case "$output" in /*) ;; *) echo "usage: $0 OUTPUT_DIR (absolute)" >&2; exit 2 ;; esac
. "$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)/safe-output.sh"
refuse_unsafe_output "$output" || exit 2

sources=${BLI_KVM_THIRD_PARTY_SOURCES:-/mnt/d/debug/blikvm/hardened-blikvm/third-party-src}
mkdir -p "$sources"
archive="$sources/musl-$MUSL_VERSION.tar.gz"
if [ ! -f "$archive" ]; then
  curl -fsSL -m 300 -o "$archive.partial" "$MUSL_URL"
  mv "$archive.partial" "$archive"
fi
echo "$MUSL_SHA256  $archive" | sha256sum -c --quiet -

work=$(mktemp -d /tmp/hardened-blikvm-musl-headers.XXXXXX)
cleanup() {
  case "$work" in /tmp/hardened-blikvm-musl-headers.*) rm -rf -- "$work" ;; esac
}
trap cleanup EXIT HUP INT TERM
tar -xzf "$archive" -C "$work"
(
  cd "$work/musl-$MUSL_VERSION"
  ./configure --target=aarch64-linux-musl --prefix="$work/sysroot" \
    CC="clang --target=aarch64-linux-musl" >/dev/null
  make install-headers >/dev/null
)
test -f "$work/sysroot/include/stdint.h"

refuse_unsafe_output "$output" || exit 2
rm -rf -- "$output"
mkdir -p "$output"
cp -R "$work/sysroot/include" "$output/include"
printf 'musl %s headers, aarch64-linux-musl\nsha256 %s\n' "$MUSL_VERSION" "$MUSL_SHA256" > "$output/SOURCE"
