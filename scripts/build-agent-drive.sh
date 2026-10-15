#!/bin/sh
# The target agent's drive (docs/design/target-agent-v1.md): a small disk with one FAT12 partition labelled
# BLIKVMAGENT (a FAT label has at most 11 characters) holding the agent, its README and its SHA-256, which the board
# presents read-only while its "Target agent" switch is on. The board presents it as a fixed disk, so that it is no
# media of the operator's, and Windows finds no volume on a fixed disk without a partition table: the disk has a DOS
# partition table and its one partition starts at 1 MiB. The agent is built by scripts/build-target-agent.sh; the
# table is written by sfdisk with a fixed id, the file system with mkfs.fat's invariant values and mtools, the files'
# times fixed and the clock's zone UTC, so the same source gives the same bytes. OUTPUT_DIR receives
# hardened-blikvm-agent.exe, SHA256SUMS and agent-drive.img.
#
# usage: scripts/build-agent-drive.sh OUTPUT_DIR
set -eu
out=${1:?usage: build-agent-drive.sh OUTPUT_DIR}
root=$(cd "$(dirname "$0")/.." && pwd)
case "$out" in /*) ;; *) out="$PWD/$out" ;; esac
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT HUP INT TERM
sh "$root/scripts/build-target-agent.sh" "$work/agent" >/dev/null
mkdir -p "$out"
cp "$work/agent/hardened-blikvm-agent.exe" "$out/hardened-blikvm-agent.exe"
(cd "$out" && sha256sum hardened-blikvm-agent.exe > SHA256SUMS)
mkdir "$work/drive"
cp "$out/hardened-blikvm-agent.exe" "$work/drive/hardened-blikvm-agent.exe"
cp "$root/packaging/target-agent/README.TXT" "$work/drive/README.TXT"
# Windows reads the sums with its own line ends.
sed 's/$/\r/' "$out/SHA256SUMS" > "$work/drive/SHA256SUMS.TXT"
for file in "$work/drive/"*; do
  touch -d 2026-01-01T00:00:00Z "$file"
done
image="$work/agent-drive.img"
# 3 MiB: the table, then the 2 MiB partition from sector 2048 (1 MiB).
dd if=/dev/zero of="$image" bs=1024 count=3072 status=none
printf 'label: dos\nlabel-id: 0x424c4b41\nunit: sectors\nstart=2048, size=4096, type=1\n' |
  sfdisk --no-reread --no-tell-kernel -q "$image" >/dev/null
mkfs.fat --invariant -i 424C4B41 -n BLIKVMAGENT -F 12 -h 2048 --offset 2048 "$image" 2048 >/dev/null
for name in README.TXT SHA256SUMS.TXT hardened-blikvm-agent.exe; do
  TZ=UTC MTOOLS_SKIP_CHECK=1 mcopy -m -i "$image@@1M" "$work/drive/$name" "::$name"
done
dd if="$image" of="$work/partition.img" bs=1M skip=1 status=none
fsck.fat -n "$work/partition.img" >/dev/null
cp "$image" "$out/agent-drive.img"
(cd "$out" && sha256sum hardened-blikvm-agent.exe agent-drive.img)
