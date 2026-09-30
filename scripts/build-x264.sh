#!/bin/sh
# Build a static, 8-bit 4:2:0 x264 library from a pinned source snapshot.
#
# usage: build-x264.sh aarch64|host OUTPUT_DIR
#
# aarch64 cross-compiles with clang against musl 1.2.5 headers and the Rust
# toolchain's self-contained musl objects, like build-libvpx.sh, with x264's
# NEON assembly; host builds without assembly so the encoder wrapper can be
# tested on the build machine. Sources are verified by SHA-256 before any
# file is extracted. x264 is GPL-2.0-or-later, compatible with the project's
# GPL-3.0-only.
set -eu

# The head of x264's stable branch (X264_BUILD 165).
X264_COMMIT=b35605ace3ddf7c1a5d67a2eb553f034aef41d55
X264_SHA256=6eeb82934e69fd51e043bd8c5b0d152839638d1ce7aa4eea65a3fedcf83ff224
X264_URL="https://code.videolan.org/videolan/x264/-/archive/$X264_COMMIT/x264-$X264_COMMIT.tar.bz2"
MUSL_VERSION=1.2.5
MUSL_SHA256=a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4
MUSL_URL="https://musl.libc.org/releases/musl-$MUSL_VERSION.tar.gz"

target=${1:-}
output=${2:-}
case "$target" in aarch64|host) ;; *) echo "usage: $0 aarch64|host OUTPUT_DIR" >&2; exit 2 ;; esac
case "$output" in /*) ;; *) echo "output directory must be absolute" >&2; exit 2 ;; esac
. "$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)/safe-output.sh"
refuse_unsafe_output "$output" || exit 2

sources=${BLI_KVM_THIRD_PARTY_SOURCES:-/mnt/d/debug/blikvm/hardened-blikvm/third-party-src}
mkdir -p "$sources"
fetch() {
  file="$sources/$1"
  if [ ! -f "$file" ]; then
    curl -fsSL -m 300 -o "$file.partial" "$2"
    mv "$file.partial" "$file"
  fi
  echo "$3  $file" | sha256sum -c --quiet -
}
fetch "x264-$X264_COMMIT.tar.bz2" "$X264_URL" "$X264_SHA256"

work=$(mktemp -d /tmp/hardened-blikvm-x264.XXXXXX)
cleanup() {
  case "$work" in /tmp/hardened-blikvm-x264.*) rm -rf -- "$work" ;; esac
}
trap cleanup EXIT HUP INT TERM
tar -xjf "$sources/x264-$X264_COMMIT.tar.bz2" -C "$work"
source_dir="$work/x264-$X264_COMMIT"

common_options="--enable-static --enable-pic --disable-cli --disable-opencl \
  --disable-avs --disable-swscale --disable-lavf --disable-ffms --disable-gpac \
  --disable-lsmash --bit-depth=8 --chroma-format=420 --disable-interlaced"

if [ "$target" = aarch64 ]; then
  fetch "musl-$MUSL_VERSION.tar.gz" "$MUSL_URL" "$MUSL_SHA256"
  sysroot="$work/sysroot"
  tar -xzf "$sources/musl-$MUSL_VERSION.tar.gz" -C "$work"
  (
    cd "$work/musl-$MUSL_VERSION"
    ./configure --target=aarch64-linux-musl --prefix="$sysroot" \
      CC="clang --target=aarch64-linux-musl" >/dev/null
    make install-headers >/dev/null
  )
  rust_sysroot=$(rustc --print sysroot)
  rust_host=$(rustc -vV | sed -n 's/^host: //p')
  self_contained="$rust_sysroot/lib/rustlib/aarch64-unknown-linux-musl/lib/self-contained"
  mkdir -p "$sysroot/lib" "$work/toolbin"
  cp "$self_contained"/crt1.o "$self_contained"/crti.o "$self_contained"/crtn.o \
    "$self_contained"/Scrt1.o "$self_contained"/crtbegin.o "$self_contained"/crtend.o \
    "$self_contained"/crtbeginS.o "$self_contained"/crtendS.o "$self_contained"/libc.a \
    "$sysroot/lib/"
  cp "$self_contained/crtbegin.o" "$sysroot/lib/crtbeginT.o"
  # configure only link-tests trivial programs; musl keeps these in libc.a.
  for library in gcc gcc_s gcc_eh pthread m rt dl; do
    llvm-ar-18 rc "$sysroot/lib/lib$library.a"
  done
  ln -s "$rust_sysroot/lib/rustlib/$rust_host/bin/rust-lld" "$work/toolbin/ld.lld"
  resource_dir=$(clang -print-resource-dir)
  base="--target=aarch64-linux-musl --sysroot=$sysroot -nostdinc -isystem $sysroot/include \
    -isystem $resource_dir/include -B$work/toolbin -ffile-prefix-map=$work=/src"
  (
    cd "$source_dir"
    CC="clang $base" AS="clang $base" AR="llvm-ar-18" RANLIB="llvm-ranlib-18" STRIP=true \
      LDFLAGS="-fuse-ld=lld -static -L$sysroot/lib -Wl,--unresolved-symbols=ignore-all" \
      ./configure --host=aarch64-linux-musl $common_options >/dev/null
    make -j"$(nproc)" libx264.a >/dev/null
  )
  # The H616 Cortex-A53 implements ARMv8.0 NEON; the NEON paths are in.
  grep -q '^#define HAVE_NEON 1' "$source_dir/config.h"
else
  (
    cd "$source_dir"
    CFLAGS="-ffile-prefix-map=$work=/src" ./configure --disable-asm $common_options >/dev/null
    make -j"$(nproc)" libx264.a >/dev/null
  )
fi

grep -q '^#define HAVE_THREAD 1' "$source_dir/config.h"
grep -q '^#define X264_BIT_DEPTH *8' "$source_dir/x264_config.h"
refuse_unsafe_output "$output" || exit 2
rm -rf -- "$output"
mkdir -p "$output/lib" "$output/include"
cp "$source_dir/libx264.a" "$output/lib/"
cp "$source_dir/x264.h" "$source_dir/x264_config.h" "$output/include/"
cp "$source_dir/COPYING" "$output/COPYING"
printf '%s\n' "x264 $X264_COMMIT $X264_SHA256" "target $target" > "$output/SOURCE"
sha256sum "$output/lib/libx264.a"
