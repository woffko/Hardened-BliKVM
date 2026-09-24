#!/usr/bin/env bash
set -euo pipefail

# Stage only; the caller owns provisioning and service activation.
[[ $# -eq 1 && -d "$1" && ! -L "$1" ]]
project_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
stage=$1
for role in board-server server-candidate server-current; do
  destination="$stage/share/systemd/hardened-blikvm-$role.service.d"
  install -d -m 0755 "$destination"
  install -m 0644 \
    "$project_root/packaging/systemd/hardened-blikvm-board-server.service.d/94-application-update.conf" \
    "$destination/94-application-update.conf"
done
