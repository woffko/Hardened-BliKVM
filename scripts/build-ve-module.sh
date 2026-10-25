#!/bin/sh
# Build kernel modules for the board's kernel.
#
# usage (from the repository root):
#   build-ve-module.sh fetch WORK_DIR
#       the board's kernel release, configuration and own headers, over SSH
#       (the root password on stdin through project-memory-exec)
#   build-ve-module.sh prepare WORK_DIR
#       the build tree
#   build-ve-module.sh build WORK_DIR SOURCE_DIR OUTPUT_DIR
#       the modules of SOURCE_DIR (a directory with a Kbuild Makefile)
#
# The board's kernel was built by the board's vendor, who published neither
# its tree nor a headers package. The tree is put together from four parts:
#
# - the source of the same upstream release, for the build scripts and
#   makefiles; its SHA-256 below is the one kernel.org publishes in its signed
#   list, and the archive's own signature was checked against Greg
#   Kroah-Hartman's key (647F 2865 4894 E3BD 4571 99BE 38DB BDC8 6092 693E);
# - the board's configuration, from /proc/config.gz;
# - the compiler the kernel was built with, which /proc/version names: Arm's
#   GNU toolchain 8.3-2019.03, pinned by the SHA-256 of the archive whose MD5
#   Arm publishes beside it;
# - the kernel's own headers, from its `kheaders` module, in place of the
#   upstream ones. The vendor's kernel carries patches that upstream lacks,
#   and some change structures every module uses (one adds a member to
#   `struct vm_area_struct`), so the build must see the vendor's headers and
#   the vendor's configuration header.
#
# `prepare` proves that the parts belong together: after `olddefconfig` with
# this compiler, every option of the rebuilt configuration header has the
# board's value (the board has more: the options of its patches, which are
# listed), and the release string is the board's. Without symbol versions
# (`CONFIG_MODVERSIONS` is off) a module then needs nothing else to load. It
# is not signed; the kernel accepts that and notes it in its taint flags.
#
# Host tools: make, a C compiler, flex, bison, bc, perl and the OpenSSL
# headers (for the kernel's own build scripts).
set -eu

KERNEL_VERSION=5.19.4
KERNEL_SHA256=a9214b97085af98dfcaaa8c2e8eff4858c1d53dccd6c58931cf7b0455ff9bf87
KERNEL_URL="https://cdn.kernel.org/pub/linux/kernel/v5.x/linux-$KERNEL_VERSION.tar.xz"
TOOLCHAIN=gcc-arm-8.3-2019.03-x86_64-aarch64-linux-gnu
TOOLCHAIN_SHA256=8ce3e7688a47d8cd2d8e8323f147104ae1c8139520eca50ccf8a7fa933002731
TOOLCHAIN_URL="https://developer.arm.com/-/media/Files/downloads/gnu-a/8.3-2019.03/binrel/$TOOLCHAIN.tar.xz"

action=${1:-}
work=${2:-}
usage() {
  echo "usage: $0 fetch WORK_DIR | prepare WORK_DIR | build WORK_DIR SOURCE_DIR OUTPUT_DIR" >&2
  exit 2
}
case "$action" in fetch|prepare|build) ;; *) usage ;; esac
case "$work" in /*) ;; *) echo "work directory must be absolute" >&2; exit 2 ;; esac
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
. "$script_dir/safe-output.sh"

sources=${BLI_KVM_THIRD_PARTY_SOURCES:-/mnt/d/debug/blikvm/hardened-blikvm/third-party-src}
tree="$work/linux-$KERNEL_VERSION"
cross="$work/$TOOLCHAIN/bin/aarch64-linux-gnu-"

fetch_source() {
  mkdir -p "$sources"
  file="$sources/$1"
  if [ ! -f "$file" ]; then
    curl -fsSL -m 1800 -o "$file.partial" "$2"
    mv "$file.partial" "$file"
  fi
  echo "$3  $file" | sha256sum -c --quiet -
}

kernel_make() {
  make -s -C "$tree" ARCH=arm64 CROSS_COMPILE="$cross" LOCALVERSION="$local_version" "$@"
}

board_facts() {
  [ -s "$work/board/config" ] && [ -s "$work/board/kheaders.tar.xz" ] && [ -s "$work/board/release" ] ||
    { echo "run fetch first" >&2; exit 1; }
  release=$(cat "$work/board/release")
  case "$release" in
    "$KERNEL_VERSION"*) ;;
    *) echo "the board runs $release, not a $KERNEL_VERSION kernel" >&2; exit 1 ;;
  esac
  # What the vendor's build appended to the upstream version.
  local_version=${release#"$KERNEL_VERSION"}
}

fetch() {
  host=${BLI_KVM_DEVICE_HOST:?set BLI_KVM_DEVICE_HOST to the board address}
  known_hosts=${BLI_KVM_KNOWN_HOSTS:-evidence/private/ssh-known-hosts}
  mkdir -p "$work/board"
  # The SSH password arrives on fd 0 from project-memory-exec; keep it on fd 3.
  exec 3<&0
  # One session (the password can be read only once). `kheaders` is loaded
  # only if it is not, and unloaded again; nothing stays on the board.
  sshpass -d 3 ssh -o StrictHostKeyChecking=yes -o UserKnownHostsFile="$known_hosts" "root@$host" '
    set -e
    d=$(mktemp -d /run/hardened-ve-fetch.XXXXXX)
    loaded=no
    cleanup() { rm -rf "$d"; [ "$loaded" = yes ] && rmmod kheaders; true; }
    trap cleanup EXIT
    uname -r > "$d/release"
    cat /proc/version > "$d/version"
    zcat /proc/config.gz > "$d/config"
    if [ ! -e /sys/kernel/kheaders.tar.xz ]; then modprobe kheaders; loaded=yes; fi
    cat /sys/kernel/kheaders.tar.xz > "$d/kheaders.tar.xz"
    modinfo -F vermagic sunxi_cedrus > "$d/vermagic"
    tar -C "$d" -cf - release version config kheaders.tar.xz vermagic
  ' | tar -xf - -C "$work/board"
  sha256sum "$work/board/config" "$work/board/kheaders.tar.xz" > "$work/board/SHA256SUMS"
  echo "fetched: kernel $(cat "$work/board/release")"
}

prepare() {
  board_facts
  grep -q "8.3-2019.03" "$work/board/version" ||
    { echo "the board's kernel was not built with the pinned compiler" >&2; exit 1; }
  fetch_source "linux-$KERNEL_VERSION.tar.xz" "$KERNEL_URL" "$KERNEL_SHA256"
  fetch_source "$TOOLCHAIN.tar.xz" "$TOOLCHAIN_URL" "$TOOLCHAIN_SHA256"
  if [ ! -x "${cross}gcc" ]; then
    refuse_unsafe_output "$work/$TOOLCHAIN" || exit 2
    rm -rf -- "$work/$TOOLCHAIN"
    tar -xf "$sources/$TOOLCHAIN.tar.xz" -C "$work"
    echo "$TOOLCHAIN $TOOLCHAIN_SHA256" > "$work/$TOOLCHAIN/SOURCE"
  fi
  refuse_unsafe_output "$tree" || exit 2
  rm -rf -- "$tree"
  tar -xf "$sources/linux-$KERNEL_VERSION.tar.xz" -C "$work"
  echo "linux $KERNEL_VERSION $KERNEL_SHA256" > "$tree/SOURCE"

  cp "$work/board/config" "$tree/.config"
  kernel_make olddefconfig
  kernel_make -j"$(nproc)" modules_prepare

  # The rebuilt configuration against the board's: every option of ours must
  # have the board's value. PAHOLE_VERSION is the build machine's, not the
  # kernel's.
  definitions() { grep -E '^#define CONFIG_' "$1" | grep -v ' CONFIG_PAHOLE_VERSION ' | sort; }
  mkdir -p "$work/kheaders"
  tar -xf "$work/board/kheaders.tar.xz" -C "$work/kheaders" ./include/generated/autoconf.h ./include/generated/utsrelease.h
  definitions "$tree/include/generated/autoconf.h" > "$work/autoconf.rebuilt"
  definitions "$work/kheaders/include/generated/autoconf.h" > "$work/autoconf.board"
  if [ -n "$(comm -23 "$work/autoconf.rebuilt" "$work/autoconf.board")" ]; then
    echo "the rebuilt configuration differs from the board's:" >&2
    comm -23 "$work/autoconf.rebuilt" "$work/autoconf.board" >&2
    exit 1
  fi
  comm -13 "$work/autoconf.rebuilt" "$work/autoconf.board" | awk '{ print $2 }' > "$work/board-only-options.txt"
  cmp -s "$tree/include/generated/utsrelease.h" "$work/kheaders/include/generated/utsrelease.h" ||
    { echo "the release string differs from the board's" >&2; exit 1; }
  [ "$(cat "$tree/include/config/kernel.release")" = "$release" ]

  # The board's headers over the upstream ones, its configuration header
  # among them.
  tar -xf "$work/board/kheaders.tar.xz" -C "$tree"
  echo "prepared: $tree for $release ($(wc -l < "$work/board-only-options.txt") options only the board has)"
}

build() {
  source_dir=${3:-}
  output=${4:-}
  case "$source_dir" in /*) ;; *) source_dir=$PWD/$source_dir ;; esac
  case "$output" in /*) ;; *) echo "output directory must be absolute" >&2; exit 2 ;; esac
  [ -f "$source_dir/Makefile" ] || { echo "$source_dir has no Makefile" >&2; exit 2; }
  board_facts
  [ -f "$tree/SOURCE" ] && [ -f "$tree/include/config/kernel.release" ] || { echo "run prepare first" >&2; exit 1; }
  # Kbuild writes beside the sources: build a copy, under a name that does
  # not depend on where the repository is.
  copy="$work/build/$(basename "$source_dir")"
  refuse_unsafe_output "$copy" || exit 2
  rm -rf -- "$copy"
  mkdir -p "$copy"
  cp -R "$source_dir/." "$copy/"
  echo "copy of $(basename "$source_dir")" > "$copy/SOURCE"
  # No symbol table of the kernel exists here, so modpost cannot tell which
  # symbols the kernel exports; the board tells when the module is loaded.
  log="$work/build/$(basename "$source_dir").log"
  if ! kernel_make KBUILD_MODPOST_WARN=1 KCFLAGS="-fmacro-prefix-map=$copy/= -fdebug-prefix-map=$copy=." \
    M="$copy" modules > "$log" 2>&1; then
    cat "$log" >&2
    exit 1
  fi
  grep -v -e 'WARNING: modpost: .* undefined!' -e 'unresolved symbol warnings' \
    -e 'Module.symvers" is missing' -e 'Modules may not have dependencies' "$log" >&2 || true
  mkdir -p "$output"
  found=no
  for module in "$copy"/*.ko; do
    [ -f "$module" ] || continue
    found=yes
    magic=$("${cross}readelf" -p .modinfo "$module" | sed -n 's/.*vermagic=//p')
    [ "$magic" = "$(cat "$work/board/vermagic")" ] ||
      { echo "$(basename "$module"): vermagic $magic is not the board's" >&2; exit 1; }
    if strings "$module" | grep -q "$work\|$HOME"; then
      echo "$(basename "$module") carries a path of the build machine" >&2
      exit 1
    fi
    cp "$module" "$output/"
    echo "$(sha256sum "$module" | cut -c1-64)  $(basename "$module")"
  done
  [ "$found" = yes ] || { echo "no module was built" >&2; exit 1; }
}

case "$action" in
  fetch) fetch ;;
  prepare) prepare ;;
  build) [ "$#" -eq 4 ] || usage; build "$@" ;;
esac
