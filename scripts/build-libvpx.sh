#!/bin/sh
# Build a static, VP8-encoder-only libvpx from pinned sources.
#
# usage: build-libvpx.sh aarch64|host OUTPUT_DIR
#
# aarch64 cross-compiles with clang against musl 1.2.5 headers and the Rust
# toolchain's self-contained musl objects; host builds for the build machine
# so the encoder wrapper can be tested. Sources are verified by SHA-256
# before any file is extracted.
set -eu

LIBVPX_VERSION=1.15.2
LIBVPX_SHA256=26fcd3db88045dee380e581862a6ef106f49b74b6396ee95c2993a260b4636aa
LIBVPX_URL="https://github.com/webmproject/libvpx/archive/refs/tags/v$LIBVPX_VERSION.tar.gz"
MUSL_VERSION=1.2.5
MUSL_SHA256=a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4
MUSL_URL="https://musl.libc.org/releases/musl-$MUSL_VERSION.tar.gz"

target=${1:-}
output=${2:-}
case "$target" in aarch64|host) ;; *) echo "usage: $0 aarch64|host OUTPUT_DIR" >&2; exit 2 ;; esac
case "$output" in /*) ;; *) echo "output directory must be absolute" >&2; exit 2 ;; esac

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
fetch "libvpx-$LIBVPX_VERSION.tar.gz" "$LIBVPX_URL" "$LIBVPX_SHA256"

work=$(mktemp -d /tmp/hardened-blikvm-libvpx.XXXXXX)
cleanup() {
  case "$work" in /tmp/hardened-blikvm-libvpx.*) rm -rf -- "$work" ;; esac
}
trap cleanup EXIT HUP INT TERM
tar -xzf "$sources/libvpx-$LIBVPX_VERSION.tar.gz" -C "$work"
source_dir="$work/libvpx-$LIBVPX_VERSION"

common_options="--disable-vp9 --disable-vp8-decoder --enable-vp8-encoder \
  --enable-realtime-only --disable-examples --disable-tools --disable-docs \
  --disable-unit-tests --disable-webm-io --disable-libyuv --enable-pic \
  --disable-shared --enable-static --enable-multithread"

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
    CC="clang $base" CXX="clang++ $base" AS="clang $base" LD="clang $base" \
      AR=llvm-ar-18 NM=nm STRIP=true \
      LDFLAGS="-fuse-ld=lld -static -L$sysroot/lib -nostdlib++ -Wl,--unresolved-symbols=ignore-all" \
      ./configure --target=arm64-linux-gcc --disable-runtime-cpu-detect \
        --disable-neon-dotprod --disable-neon-i8mm --disable-sve --disable-sve2 \
        $common_options >/dev/null
    make -j"$(nproc)" libvpx.a >/dev/null
  )
else
  (
    cd "$source_dir"
    CFLAGS="-ffile-prefix-map=$work=/src" ./configure $common_options >/dev/null
    make -j"$(nproc)" libvpx.a >/dev/null
  )
fi

grep -q '^#define CONFIG_MULTITHREAD 1' "$source_dir/vpx_config.h"
if [ "$target" = aarch64 ]; then
  # The H616 Cortex-A53 implements ARMv8.0 NEON only; without runtime CPU
  # detection any newer extension would fault with SIGILL on the device.
  grep -q '^#define HAVE_NEON 1' "$source_dir/vpx_config.h"
  for extension in NEON_DOTPROD NEON_I8MM SVE SVE2; do
    grep -q "^#define HAVE_$extension 0" "$source_dir/vpx_config.h"
  done
fi
grep -q '^#define CONFIG_VP8_ENCODER 1' "$source_dir/vpx_config.h"
rm -rf -- "$output"
mkdir -p "$output/lib" "$output/include/vpx"
cp "$source_dir/libvpx.a" "$output/lib/"
cp "$source_dir"/vpx/vpx_encoder.h "$source_dir"/vpx/vpx_codec.h \
  "$source_dir"/vpx/vpx_image.h "$source_dir"/vpx/vpx_integer.h \
  "$source_dir"/vpx/vp8.h "$source_dir"/vpx/vp8cx.h "$source_dir"/vpx/vpx_ext_ratectrl.h \
  "$source_dir"/vpx/vpx_tpl.h "$output/include/vpx/" 2>/dev/null || true
test -f "$output/include/vpx/vpx_encoder.h"
cp "$source_dir/LICENSE" "$output/LICENSE"
cp "$source_dir/PATENTS" "$output/PATENTS"
printf '%s\n' "libvpx $LIBVPX_VERSION $LIBVPX_SHA256" "target $target" > "$output/SOURCE"
sha256sum "$output/lib/libvpx.a"
