#!/bin/sh
# Switch an installed BliKVM from the legacy update profile to the native
# execution-helper profile. Runs on the device as root, from a staging
# directory that contains:
#   update.deb                  hardened-blikvm-update-runtime 0.1.0~dev19
#   native/*.service            rendered native unit replacements
#   exec-helper.board.json      reviewed board requirements for readiness
# Usage: deploy_native_profile.sh prepare|cutover|rollback STAGING_DIR
#
# prepare  - back up state, install the inert package, helper config and
#            protected roots; nothing that serves traffic changes.
# cutover  - install native units, disable legacy update authority, migrate
#            the journal to the native lifecycle and start the composition
#            through the helper. Disruptive; the web UI restarts.
# rollback - restore the backed-up units, configuration and journal and
#            restart the legacy composition.
set -eu

action=${1:-}
staging=${2:-}
case "$action" in prepare|cutover|boot|rollback) ;; *) echo "usage: $0 prepare|cutover|boot|rollback DIR" >&2; exit 2 ;; esac
case "$staging" in /*) ;; *) echo "staging directory must be absolute" >&2; exit 2 ;; esac
[ "$(id -u)" = 0 ] || { echo "must run as root" >&2; exit 2; }

backup=/var/backups/hardened-blikvm-native
config=/etc/hardened-blikvm/update-supervisor.json
helper_config=/etc/hardened-blikvm/exec-helper.json
units=/etc/systemd/system
managed="hardened-blikvm-board-owner hardened-blikvm-board-video-proxy hardened-blikvm-boardd hardened-blikvm-server-current"
# Legacy update authority. Masking also removes their Before= edges, which
# systemd otherwise reports as After= on the managed units.
legacy="hardened-blikvm-update-recover.service hardened-blikvm-update-confirm.socket hardened-blikvm-update-confirm@.service hardened-blikvm-update-apply-candidate.service hardened-blikvm-update-apply-confirmed.service hardened-blikvm-update-watchdog.timer hardened-blikvm-update-watchdog.service hardened-blikvm-update-activate@.service"
state_root=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["stateRoot"])' "$config")
slot_root=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["slotRoot"])' "$config")

supervisor() {
  # The CLI trusts the root-owned configuration only as a systemd credential.
  systemd-run --quiet --wait --pipe --collect --unit=hardened-blikvm-native-cli \
    -p User=hardened-blikvm-update -p Group=hardened-blikvm \
    -p LoadCredential=update-supervisor-config:"$config" \
    -p ProtectSystem=strict -p ReadWritePaths="$slot_root $state_root /run/hardened-blikvm-update" \
    /usr/libexec/hardened-blikvm/bli-kvm-update-supervisor \
    --config /run/credentials/hardened-blikvm-native-cli.service/update-supervisor-config "$@"
}

write_helper_config() {
  python3 - "$config" "$staging/exec-helper.board.json" "$helper_config" <<'PY'
import json, os, pwd, sys
supervisor = json.load(open(sys.argv[1]))
board = json.load(open(sys.argv[2]))
floor = int(os.environ.get("NATIVE_MINIMUM_SEQUENCE", "14"))
document = {
    "schemaVersion": 1,
    "keyId": supervisor["keyId"],
    "publicKeyHex": supervisor["publicKeyHex"],
    "coordinatorUid": pwd.getpwnam("hardened-blikvm-update").pw_uid,
    "updaterSlotRoot": supervisor["slotRoot"],
    "executionRoot": "/var/lib/hardened-blikvm-exec",
    "ledgerRoot": "/var/lib/hardened-blikvm-exec-ledger",
    "systemSequence": supervisor["systemSequence"],
    "minimumApplicationSequence": floor,
    "maxSlots": 3,
    "maxPayloadBytes": 134217728,
    "ownerSocket": "/run/hardened-blikvm-gadget/control.sock",
    "videoSocket": "/run/hardened-blikvm-video/mjpeg.sock",
    "boardSocket": "/run/hardened-blikvm-boardd/control.sock",
    "webPort": 18080,
    "board": board,
}
path = sys.argv[3]
temporary = path + ".next"
with open(temporary, "w") as handle:
    json.dump(document, handle, indent=1)
os.chmod(temporary, 0o600)
os.rename(temporary, path)
PY
}

prepare() {
  if [ -e "$backup/complete" ]; then
    echo "backup already exists at $backup; refusing to overwrite it" >&2
    exit 1
  fi
  rm -rf "$backup.partial"
  install -d -m 0700 "$backup.partial"
  tar -C / -cpf "$backup.partial/etc-systemd.tar" \
    $(cd / && ls -d etc/systemd/system/hardened-blikvm* etc/systemd/system/*.wants/hardened-blikvm* 2>/dev/null)
  tar -C / -cpf "$backup.partial/state.tar" "${state_root#/}"
  tar -C / -cpf "$backup.partial/etc-hardened.tar" etc/hardened-blikvm
  dpkg-query -W 'hardened-blikvm*' > "$backup.partial/packages.txt"
  cp -p /usr/libexec/hardened-blikvm/bli-kvm-update-supervisor "$backup.partial/"
  for unit in $managed hardened-blikvm-update-confirm.socket hardened-blikvm-update-recover; do
    printf '%s %s\n' "$unit" "$(systemctl is-enabled "$unit" 2>/dev/null || true)"
  done > "$backup.partial/enablement.txt"
  mv "$backup.partial" "$backup"
  touch "$backup/complete"

  dpkg -i "$staging/update.deb"
  install -d -m 0755 -o root -g root /var/lib/hardened-blikvm-exec
  install -d -m 0700 -o root -g root /var/lib/hardened-blikvm-exec-ledger
  write_helper_config
  systemctl daemon-reload
  systemctl enable --now hardened-blikvm-update-exec.socket
  echo "prepared; backup at $backup"
}

cutover() {
  [ -e "$backup/complete" ] || { echo "run prepare first" >&2; exit 1; }
  # A rollback removes the helper configuration; recreate it from the
  # reviewed sources rather than failing inside the helper.
  [ -s "$helper_config" ] || write_helper_config
  for file in "$staging"/native/*.service; do
    install -m 0644 "$file" "$units/$(basename "$file")"
  done
  # Existing device drop-ins re-point ExecStart at the updater slot; the last
  # drop-in resets every managed unit to the protected execution copy.
  exec_root=/var/lib/hardened-blikvm-exec/current
  for pair in \
    "hardened-blikvm-board-owner:libexec/bli-kvm-control-broker" \
    "hardened-blikvm-board-video-proxy:libexec/bli-kvm-video-proxy" \
    "hardened-blikvm-boardd:libexec/bli-kvm-boardd --config /etc/hardened-blikvm-gadget/boardd.json" \
    "hardened-blikvm-server-current:bin/bli-kvm-server" \
    "hardened-blikvm-server-candidate:bin/bli-kvm-server"; do
    unit=${pair%%:*}
    command=${pair#*:}
    install -d -m 0755 "$units/$unit.service.d"
    {
      echo "# Native profile: execute only the helper's protected copy."
      echo "[Service]"
      echo "ExecStart="
      echo "ExecStart=$exec_root/$command"
      case "$unit" in
        *server*)
          echo "Environment=BLI_KVM_WEB_ROOT=$exec_root/share/web"
          echo "Environment=BLI_KVM_UPDATE_NATIVE_ADMISSION=1"
          ;;
      esac
    } > "$units/$unit.service.d/99-native.conf"
  done
  # Packages built before the rotated-root alignment lack the -v2 paths.
  install -d -m 0755 "$units/hardened-blikvm-update-native-boot.service.d"
  printf '%s\n' '[Service]' \
    "ReadWritePaths=-$slot_root -$state_root" \
    > "$units/hardened-blikvm-update-native-boot.service.d/10-rotated.conf"
  systemctl disable hardened-blikvm-update-confirm.socket $managed 2>/dev/null || true
  systemctl stop hardened-blikvm-update-confirm.socket || true
  systemctl mask $legacy
  systemctl daemon-reload
  supervisor migrate-native-idle
  systemctl enable hardened-blikvm-update-native-boot.service
  systemctl enable hardened-blikvm-hid-gadget.service
  boot
}

boot() {
  systemctl reset-failed hardened-blikvm-update-native-boot.service 2>/dev/null || true
  systemctl start hardened-blikvm-update-native-boot.service || true
  systemctl --no-pager status hardened-blikvm-update-native-boot.service | head -20
  for unit in $managed; do
    printf '%s %s\n' "$unit" "$(systemctl is-active "$unit")"
  done
}

rollback() {
  [ -e "$backup/complete" ] || { echo "no backup at $backup" >&2; exit 1; }
  systemctl disable --now hardened-blikvm-update-native-boot.service hardened-blikvm-update-exec.socket hardened-blikvm-hid-gadget.service 2>/dev/null || true
  systemctl unmask $legacy 2>/dev/null || true
  systemctl stop $managed 2>/dev/null || true
  for file in "$staging"/native/*.service; do
    rm -f "$units/$(basename "$file")"
  done
  for unit in hardened-blikvm-board-owner hardened-blikvm-board-video-proxy hardened-blikvm-boardd \
    hardened-blikvm-server-current hardened-blikvm-server-candidate; do
    rm -f "$units/$unit.service.d/99-native.conf"
  done
  rm -rf "$units/hardened-blikvm-update-native-boot.service.d"
  tar -C / -xpf "$backup/etc-systemd.tar"
  tar -C / -xpf "$backup/etc-hardened.tar"
  rm -f "$helper_config"
  tar -C / -xpf "$backup/state.tar"
  install -m 0755 "$backup/bli-kvm-update-supervisor" /usr/libexec/hardened-blikvm/bli-kvm-update-supervisor
  systemctl daemon-reload
  while read -r unit state; do
    case "$state" in enabled) systemctl enable "$unit" 2>/dev/null || true ;; esac
  done < "$backup/enablement.txt"
  systemctl start hardened-blikvm-update-confirm.socket || true
  systemctl restart hardened-blikvm-update-recover.service || true
  systemctl start $managed
  for unit in $managed; do
    printf '%s %s\n' "$unit" "$(systemctl is-active "$unit")"
  done
}

"$action"
