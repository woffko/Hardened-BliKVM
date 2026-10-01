#!/bin/sh
# Build a static TurboJPEG library (libjpeg-turbo) from a pinned release.
#
# usage: build-libjpeg-turbo.sh aarch64|host OUTPUT_DIR
#
# aarch64 cross-compiles with clang against musl 1.2.5 headers, like
# build-libvpx.sh and build-x264.sh, with libjpeg-turbo's NEON code required
# (REQUIRE_SIMD); host builds for the build machine so the decoder wrapper can
# be tested. The release archive is verified by SHA-256 before any file is
# extracted; that digest was taken from the archive whose detached signature
# verified against the libjpeg-turbo project's signing key (fingerprint
# 0338 C8D8 D9FD A62C F9C4 21BD 7EC2 DBB6 F4DB F434). libjpeg-turbo is under
# the IJG License, the Modified (3-clause) BSD License and the zlib License,
# all compatible with the project's GPL-3.0-only; LICENSE.md and README.ijg go
# with the library.
#
# Only the library is built (no command-line tools, so no upstream regression
# tests); the encoder's own tests compare its output with the zune-jpeg path.
set -eu

LIBJPEG_TURBO_VERSION=3.2.0
LIBJPEG_TURBO_SHA256=6f30092cef9fb839779646608f4ee14ae3cbac989c47fa05e841b0841f09878e
LIBJPEG_TURBO_URL="https://github.com/libjpeg-turbo/libjpeg-turbo/releases/download/$LIBJPEG_TURBO_VERSION/libjpeg-turbo-$LIBJPEG_TURBO_VERSION.tar.gz"
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
fetch "libjpeg-turbo-$LIBJPEG_TURBO_VERSION.tar.gz" "$LIBJPEG_TURBO_URL" "$LIBJPEG_TURBO_SHA256"

work=$(mktemp -d /tmp/hardened-blikvm-libjpeg-turbo.XXXXXX)
cleanup() {
  case "$work" in /tmp/hardened-blikvm-libjpeg-turbo.*) rm -rf -- "$work" ;; esac
}
trap cleanup EXIT HUP INT TERM
tar -xzf "$sources/libjpeg-turbo-$LIBJPEG_TURBO_VERSION.tar.gz" -C "$work"
source_dir="$work/libjpeg-turbo-$LIBJPEG_TURBO_VERSION"
build_dir="$work/build"

common_options="-G Ninja -DCMAKE_BUILD_TYPE=Release -DENABLE_SHARED=OFF \
  -DENABLE_STATIC=ON -DWITH_TURBOJPEG=ON -DWITH_TOOLS=OFF -DWITH_TESTS=OFF \
  -DWITH_SIMD=ON -DREQUIRE_SIMD=ON \
  -DCMAKE_POSITION_INDEPENDENT_CODE=ON"

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
  resource_dir=$(clang -print-resource-dir)
  # The library is static and nothing is linked here, so configure checks
  # compile to static libraries instead of linking test programs.
  cat > "$work/toolchain.cmake" <<EOF
set(CMAKE_SYSTEM_NAME Linux)
set(CMAKE_SYSTEM_PROCESSOR aarch64)
set(CMAKE_C_COMPILER clang)
set(CMAKE_C_COMPILER_TARGET aarch64-linux-musl)
set(CMAKE_ASM_COMPILER clang)
set(CMAKE_ASM_COMPILER_TARGET aarch64-linux-musl)
set(CMAKE_AR llvm-ar-18)
set(CMAKE_RANLIB llvm-ranlib-18)
set(CMAKE_SYSROOT $sysroot)
set(CMAKE_C_FLAGS_INIT "-nostdinc -isystem $sysroot/include -isystem $resource_dir/include -ffile-prefix-map=$work=/src")
set(CMAKE_ASM_FLAGS_INIT "-nostdinc -isystem $sysroot/include -isystem $resource_dir/include")
set(CMAKE_TRY_COMPILE_TARGET_TYPE STATIC_LIBRARY)
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
EOF
  cmake -S "$source_dir" -B "$build_dir" $common_options \
    -DCMAKE_TOOLCHAIN_FILE="$work/toolchain.cmake" > "$work/configure.log" 2>&1 ||
    { tail -n 30 "$work/configure.log" >&2; exit 1; }
else
  CFLAGS="-ffile-prefix-map=$work=/src" \
    cmake -S "$source_dir" -B "$build_dir" $common_options > "$work/configure.log" 2>&1 ||
    { tail -n 30 "$work/configure.log" >&2; exit 1; }
fi
cmake --build "$build_dir" --target turbojpeg-static -- -j"$(nproc)" > "$work/build.log" 2>&1 ||
  { tail -n 30 "$work/build.log" >&2; exit 1; }

# The SIMD extensions must be in: on the H616 they are what makes the
# library worth linking.
simd=$(sed -n 's/^.*SIMD extensions: //p' "$work/configure.log")
case "$target:$simd" in
  aarch64:*ARM64*"(WITH_SIMD = 1)"* | host:*"(WITH_SIMD = 1)"*) ;;
  *) echo "libjpeg-turbo was configured without the expected SIMD extensions: $simd" >&2; exit 1 ;;
esac
refuse_unsafe_output "$output" || exit 2
rm -rf -- "$output"
mkdir -p "$output/lib" "$output/include"
cp "$build_dir/libturbojpeg.a" "$output/lib/"
cp "$source_dir/src/turbojpeg.h" "$output/include/"
cp "$source_dir/LICENSE.md" "$source_dir/README.ijg" "$output/"
printf '%s\n' "libjpeg-turbo $LIBJPEG_TURBO_VERSION $LIBJPEG_TURBO_SHA256" "target $target" > "$output/SOURCE"
sha256sum "$output/lib/libturbojpeg.a"
