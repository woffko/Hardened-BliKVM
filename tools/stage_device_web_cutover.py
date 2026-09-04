#!/usr/bin/env python3
"""Replace the persistent PiKVM web listener with the Hardened GUI.

The vendor kvmd and uStreamer services remain active as private transition
backends. Only kvmd-nginx is disabled. The password is read once from stdin;
all mutations are protected by an early-boot rollback and a timed watchdog.
"""

from __future__ import annotations

import argparse
import base64
from dataclasses import dataclass
import hashlib
import json
from pathlib import Path
import re
import secrets
import shlex
import subprocess
import sys
from typing import Mapping, TypedDict, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import capture_device_web_cutover_preflight as cutover_preflight
from tools import stage_device_runtime_provision as provision
from tools import stage_device_tls_browser as tls
from tools import stage_device_prototype as prototype
from tools.capture_pikvm_baseline import write_json_0600_atomic


PROJECT_ROOT = Path(__file__).resolve().parent.parent
NGINX_CONFIG_SOURCE = PROJECT_ROOT / "packaging/nginx/hardened-blikvm.conf"
NGINX_UNIT_SOURCE = (
    PROJECT_ROOT / "packaging/systemd/hardened-blikvm-nginx.service"
)
BROWSER_SCRIPT = PROJECT_ROOT / "tools/playwright/device_cutover_setup_smoke.mjs"
CONFIG_TARGET = "/etc/hardened-blikvm/nginx.conf"
UNIT_NAME = "hardened-blikvm-nginx.service"
UNIT_TARGET = f"/etc/systemd/system/{UNIT_NAME}"
UNIT_LINK = f"/etc/systemd/system/multi-user.target.wants/{UNIT_NAME}"
STATE_ROOT = "/var/lib/hardened-blikvm"
PRODUCT_UNITS = (
    "hardened-blikvm-platform-broker.service",
    "hardened-blikvm-video-proxy.service",
    "hardened-blikvm-server.service",
)
ALLOWED_FACT_VALUES = {
    "yes",
    "active",
    "disabled",
    "enabled",
    "passed",
    "restored",
    "committed",
}
PREPARE_KEYS = {"boot_rollback_armed", "rollback_synced", "watchdog_active"}
APPLY_KEYS = {
    "certificate_reused",
    "control_boundary_absent",
    "first_setup_required",
    "hardened_nginx_active",
    "hardened_services_enabled",
    "http_redirect",
    "kvmd_active",
    "nginx_sandbox",
    "port443_exact",
    "port80_exact",
    "tls12",
    "tls13",
    "vendor_nginx_disabled",
}
COMMIT_KEYS = {
    "certificate_reused",
    "control_boundary_absent",
    "first_setup_required",
    "hardened_nginx_persistent",
    "hardened_services_persistent",
    "kvmd_active",
    "ports_exact",
    "rollback_removed",
    "vendor_nginx_disabled",
    "watchdog_removed",
}
CLEANUP_KEYS = {"cleanup_state"}


@dataclass(frozen=True)
class Paths:
    run_id: str
    root: str
    marker: str
    rollback_script: str
    watchdog_script: str
    boot_unit: str
    boot_unit_path: str
    boot_unit_part: str
    boot_unit_link: str
    watchdog_unit: str
    config_part: str
    unit_part: str
    validation_config: str
    validation_root: str


@dataclass(frozen=True)
class Options:
    endpoint: str
    username: str
    known_hosts: str
    package: str
    expected_sha256: str
    expected_source_commit: str
    expected_source_bundle_sha256: str
    expected_builder_commit: str
    baseline: str
    output: str


class CleanupReceipt(TypedDict):
    attempted: bool
    confirmed: bool
    state: str


class CutoverError(Exception):
    def __init__(
        self,
        stage: str,
        failure_type: str,
        *,
        return_code: int | None = None,
        remote_phase: str | None = None,
    ) -> None:
        super().__init__(f"{stage}: {failure_type}")
        self.stage = stage
        self.failure_type = failure_type
        self.return_code = return_code
        self.remote_phase = remote_phase


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--username", required=True)
    parser.add_argument("--known-hosts", required=True)
    parser.add_argument("--package", required=True)
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--expected-source-commit", required=True)
    parser.add_argument("--expected-source-bundle-sha256", required=True)
    parser.add_argument("--expected-builder-commit", required=True)
    parser.add_argument("--baseline", required=True)
    parser.add_argument("--output", required=True)
    namespace = parser.parse_args()
    return Options(
        endpoint=cast(str, namespace.endpoint),
        username=cast(str, namespace.username),
        known_hosts=cast(str, namespace.known_hosts),
        package=cast(str, namespace.package),
        expected_sha256=cast(str, namespace.expected_sha256),
        expected_source_commit=cast(str, namespace.expected_source_commit),
        expected_source_bundle_sha256=cast(
            str, namespace.expected_source_bundle_sha256
        ),
        expected_builder_commit=cast(str, namespace.expected_builder_commit),
        baseline=cast(str, namespace.baseline),
        output=cast(str, namespace.output),
    )


def _provision_options(options: Options) -> provision.Options:
    return provision.Options(
        endpoint=options.endpoint,
        username=options.username,
        known_hosts=options.known_hosts,
        package=options.package,
        expected_sha256=options.expected_sha256,
        expected_source_commit=options.expected_source_commit,
        expected_source_bundle_sha256=options.expected_source_bundle_sha256,
        expected_builder_commit=options.expected_builder_commit,
        output="/unused",
    )


def validate_assets(options: Options) -> provision.Assets:
    return provision.validate_assets(_provision_options(options))


def paths(package_sha256: str, run_id: str) -> Paths:
    if not re.fullmatch(r"[0-9a-f]{64}", package_sha256):
        raise ValueError("package digest is invalid")
    if not re.fullmatch(r"[0-9a-f]{16}", run_id):
        raise ValueError("run identifier is invalid")
    stem = f"hardened-blikvm-web-cutover-{package_sha256[:12]}-{run_id}"
    root = f"/var/lib/{stem}"
    boot_unit = f"{stem}-rollback.service"
    return Paths(
        run_id=run_id,
        root=root,
        marker=f"{root}/.owner",
        rollback_script=f"{root}/rollback.sh",
        watchdog_script=f"{root}/watchdog.sh",
        boot_unit=boot_unit,
        boot_unit_path=f"/etc/systemd/system/{boot_unit}",
        boot_unit_part=f"/etc/systemd/system/.{boot_unit}.part",
        boot_unit_link=f"/etc/systemd/system/sysinit.target.wants/{boot_unit}",
        watchdog_unit=f"{stem}-watchdog.service",
        config_part=f"{root}/nginx.conf.part",
        unit_part=f"{root}/nginx.service.part",
        validation_config=f"{root}/nginx-validation.conf",
        validation_root=f"{root}/nginx-validation",
    )


def _assignments(values: Mapping[str, str | int]) -> str:
    return "\n".join(f"{name}={shlex.quote(str(value))}" for name, value in values.items())


def _encoded(value: str) -> str:
    return base64.b64encode(value.encode()).decode("ascii")


def _digest(value: str) -> str:
    return hashlib.sha256(value.encode()).hexdigest()


def parse_facts(raw: bytes, expected_keys: set[str]) -> dict[str, str]:
    if len(raw) > 16 * 1024:
        raise ValueError("cutover output exceeded the limit")
    facts: dict[str, str] = {}
    for line in raw.decode("ascii", errors="strict").splitlines():
        key, separator, value = line.partition("=")
        if (
            separator != "="
            or key not in expected_keys
            or key in facts
            or value not in ALLOWED_FACT_VALUES
        ):
            raise ValueError("cutover returned an invalid fact")
        facts[key] = value
    if set(facts) != expected_keys:
        raise ValueError("cutover result was incomplete")
    return dict(sorted(facts.items()))


def parse_failure(raw: bytes, prefix: str) -> tuple[str | None, bool | None]:
    if len(raw) > 16 * 1024:
        return None, None
    phase: str | None = None
    cleanup: bool | None = None
    for line in raw.decode("ascii", errors="ignore").splitlines():
        key, separator, value = line.partition("=")
        if separator != "=":
            continue
        if key == f"{prefix}_failure_phase" and re.fullmatch(r"[a-z0-9-]{1,64}", value):
            phase = value
        if key == f"{prefix}_cleanup":
            cleanup = value == "restored" if value in {"restored", "retained"} else None
    return phase, cleanup


def load_baseline(value: str) -> dict[str, str]:
    path = Path(value).resolve()
    if path.is_symlink() or not path.is_file() or path.stat().st_mode & 0o077:
        raise ValueError("cutover baseline must be a private regular file")
    decoded = cast(object, json.loads(path.read_text(encoding="utf-8")))
    if not isinstance(decoded, dict):
        raise ValueError("cutover baseline root is invalid")
    root = cast(dict[str, object], decoded)
    facts_value = root.get("facts")
    if (
        root.get("schemaVersion") != 1
        or root.get("probe") != "hardened-blikvm-web-cutover-preflight"
        or not isinstance(facts_value, dict)
    ):
        raise ValueError("cutover baseline envelope is invalid")
    facts_mapping = cast(dict[object, object], facts_value)
    facts = {str(key): str(item) for key, item in facts_mapping.items()}
    if set(facts) != cutover_preflight.SAFE_KEYS:
        raise ValueError("cutover baseline facts are incomplete")
    expected = {
        "architecture": "aarch64",
        "rootfs_mode": "rw",
        "kvmd_active": "active",
        "kvmd_enabled": "enabled",
        "kvmd_nginx_active": "active",
        "kvmd_nginx_enabled": "enabled",
        "nginx_binary": "/usr/sbin/nginx",
        "nginx_config_test": "passed",
        "certificate_directive_count": "1",
        "key_directive_count": "1",
        "certificate_mode_owner": "644:0:0",
        "certificate_symlink": "no",
        "key_mode_owner": "600:0:0",
        "key_symlink": "no",
        "live_certificate_matches": "yes",
        "port80_listener_count": "2",
        "port80_scope": "wildcard",
        "port443_listener_count": "2",
        "port443_scope": "wildcard",
        "hardened_units": "yes",
        "hardened_nginx_unit": "absent",
        "app_state": "absent",
        "control_boundary": "absent",
        "staging_absent": "yes",
    }
    for key, wanted in expected.items():
        if facts.get(key) != wanted:
            raise ValueError(f"cutover baseline rejected field {key}")
    for key in (
        "certificate_sha256",
        "key_sha256",
        "kvmd_nginx_fragment_sha256",
        "nginx_config_sha256",
    ):
        if not re.fullmatch(r"[0-9a-f]{64}", facts[key]):
            raise ValueError("cutover baseline digest is invalid")
    for key in ("certificate_path", "key_path", "kvmd_nginx_fragment", "nginx_config_path"):
        if not re.fullmatch(r"/[A-Za-z0-9_./+-]{1,240}", facts[key]) or ".." in Path(facts[key]).parts:
            raise ValueError("cutover baseline path is invalid")
    if not facts["certificate_path"].startswith("/etc/kvmd/nginx/ssl/"):
        raise ValueError("certificate is outside the accepted vendor TLS root")
    if not facts["key_path"].startswith("/etc/kvmd/nginx/ssl/"):
        raise ValueError("key is outside the accepted vendor TLS root")
    return dict(sorted(facts.items()))


def render_nginx(baseline: Mapping[str, str]) -> str:
    source = NGINX_CONFIG_SOURCE.read_text(encoding="utf-8")
    rendered = source.replace(
        "/etc/hardened-blikvm/tls.crt", baseline["certificate_path"]
    ).replace("/etc/hardened-blikvm/tls.key", baseline["key_path"])
    if "/etc/hardened-blikvm/tls." in rendered:
        raise ValueError("Nginx TLS placeholders were not fully rendered")
    return rendered


def render_validation_nginx(
    baseline: Mapping[str, str], nginx_config: str, validation_root: str
) -> str:
    if not validation_root.startswith("/var/lib/hardened-blikvm-web-cutover-"):
        raise ValueError("validation Nginx root is outside the cutover root")
    rendered = nginx_config
    rendered = rendered.replace(
        "/run/hardened-blikvm-nginx/", f"{validation_root}/"
    )
    if (
        baseline["certificate_path"] not in rendered
        or baseline["key_path"] not in rendered
    ):
        raise ValueError("validation Nginx TLS paths are missing")
    if "/run/hardened-blikvm-nginx/" in rendered:
        raise ValueError("validation Nginx runtime paths were not rendered")
    return rendered


def render_unit(baseline: Mapping[str, str]) -> str:
    source = NGINX_UNIT_SOURCE.read_text(encoding="utf-8")
    rendered = source.replace(
        "/etc/hardened-blikvm/tls.crt", baseline["certificate_path"]
    ).replace("/etc/hardened-blikvm/tls.key", baseline["key_path"])
    if "/etc/hardened-blikvm/tls." in rendered:
        raise ValueError("Nginx unit TLS placeholders were not fully rendered")
    return rendered


def build_boot_unit(paths_value: Paths) -> str:
    return "\n".join(
        (
            "[Unit]",
            "Description=Hardened BliKVM web cutover emergency rollback",
            "DefaultDependencies=no",
            "After=local-fs.target",
            "Before=kvmd-nginx.service hardened-blikvm-nginx.service multi-user.target",
            f"ConditionPathExists={paths_value.marker}",
            "",
            "[Service]",
            "Type=oneshot",
            "User=root",
            "Group=root",
            "UMask=0077",
            "NoNewPrivileges=yes",
            "PrivateDevices=yes",
            "PrivateNetwork=yes",
            "ProtectHome=yes",
            "ProtectKernelLogs=yes",
            "ProtectKernelModules=yes",
            "ProtectKernelTunables=yes",
            "RestrictAddressFamilies=AF_UNIX",
            "RestrictNamespaces=yes",
            f"ExecStart=/bin/sh {paths_value.rollback_script} boot",
            "TimeoutStartSec=5min",
            "",
            "[Install]",
            "WantedBy=sysinit.target",
            "",
        )
    )


REMOTE_HELPERS = r"""
service_enabled() {
  state=$(systemctl is-enabled "$1" 2>/dev/null || true)
  printf '%s' "$state"
}
control_boundary_absent() {
  [ "$(systemctl is-active hardened-blikvm-control-broker.service 2>/dev/null || true)" = inactive ]
  [ "$(service_enabled hardened-blikvm-control-broker.service)" = disabled ]
  [ "$(systemctl show -p LoadState --value hardened-blikvm-control-broker.socket 2>/dev/null || true)" = not-found ]
}
product_inactive_disabled() {
  for unit in hardened-blikvm-platform-broker.service hardened-blikvm-video-proxy.service hardened-blikvm-server.service; do
    [ "$(systemctl is-active "$unit" 2>/dev/null || true)" = inactive ] || return 1
    [ "$(service_enabled "$unit")" = disabled ] || return 1
  done
}
product_active_enabled() {
  for unit in hardened-blikvm-platform-broker.service hardened-blikvm-video-proxy.service hardened-blikvm-server.service; do
    [ "$(systemctl is-active "$unit" 2>/dev/null || true)" = active ] || return 1
    [ "$(service_enabled "$unit")" = enabled ] || return 1
  done
}
listener_count() {
  port=$1
  ss -H -ltn | awk -v suffix=":$port" '$4 ~ (suffix "$" ) { count++ } END { print count + 0 }'
}
listener_scope() {
  port=$1
  ss -H -ltn | awk -v suffix=":$port" '
    $4 ~ (suffix "$" ) {
      seen=1
      if ($4 ~ /^0[.]0[.]0[.]0:/ || $4 ~ /^\[::\]:/ || $4 ~ /^\*:/) wildcard=1
      else if ($4 ~ /^127[.]0[.]0[.]1:/ || $4 ~ /^\[::1\]:/) loopback=1
      else other=1
    }
    END {
      if (!seen) print "absent"
      else if (wildcard && !other) print "wildcard"
      else if (loopback && !wildcard && !other) print "loopback"
      else print "mixed"
    }'
}
ports_exact() {
  [ "$(listener_count 80)" = 2 ] && [ "$(listener_scope 80)" = wildcard ] &&
  [ "$(listener_count 443)" = 2 ] && [ "$(listener_scope 443)" = wildcard ]
}
served_certificate_hash() {
  certificate=$(
    printf '\n' | openssl s_client -connect 127.0.0.1:443 -servername localhost -showcerts 2>/dev/null |
      awk '/-----BEGIN CERTIFICATE-----/ { capture=1 } capture { print } /-----END CERTIFICATE-----/ { exit }'
  )
  [ -n "$certificate" ]
  printf '%s\n' "$certificate" | openssl x509 -outform DER 2>/dev/null | sha256sum | awk '{print $1}'
}
source_certificate_der_hash() {
  openssl x509 -in "$certificate_path" -outform DER 2>/dev/null | sha256sum | awk '{print $1}'
}
first_setup_required() {
  [ ! -e "$state_root/account.json" ]
  response=$(mktemp /tmp/hardened-blikvm-setup-check.XXXXXX)
  chmod 600 "$response"
  status=$(curl --noproxy '*' --insecure --silent --max-time 5 --output "$response" --write-out '%{http_code}' https://127.0.0.1/api/auth/setup 2>/dev/null || true)
  valid=no
  if [ "$status" = 200 ] && grep -q '"required":true' "$response"; then valid=yes; fi
  rm -f -- "$response"
  [ "$valid" = yes ]
}
direct_first_setup_required() {
  [ ! -e "$state_root/account.json" ]
  response=$(mktemp /tmp/hardened-blikvm-setup-direct.XXXXXX)
  chmod 600 "$response"
  status=$(curl --noproxy '*' --silent --max-time 5 --output "$response" --write-out '%{http_code}' http://127.0.0.1:18080/api/auth/setup 2>/dev/null || true)
  valid=no
  if [ "$status" = 200 ] && grep -q '"required":true' "$response"; then valid=yes; fi
  rm -f -- "$response"
  [ "$valid" = yes ]
}
cgroup_empty() {
  unit=$1
  group=$(systemctl show -p ControlGroup --value "$unit" 2>/dev/null || true)
  [ -z "$group" ] && return 0
  [ ! -e "/sys/fs/cgroup$group/cgroup.procs" ] && return 0
  [ -z "$(sed -n '1p' "/sys/fs/cgroup$group/cgroup.procs")" ]
}
remove_validation_runtime() {
  [ -e "$validation_root" ] || return 0
  [ -d "$validation_root" ] && [ ! -L "$validation_root" ]
  unexpected=$(find "$validation_root" -mindepth 1 -maxdepth 1 ! -name client-temp ! -name proxy-temp ! -name nginx.pid -print -quit)
  [ -z "$unexpected" ]
  if [ -e "$validation_root/nginx.pid" ]; then
    [ -f "$validation_root/nginx.pid" ] && [ ! -L "$validation_root/nginx.pid" ]
    [ "$(stat -c '%h' "$validation_root/nginx.pid")" = 1 ]
    [ "$(stat -c '%s' "$validation_root/nginx.pid")" -le 32 ]
    rm -f -- "$validation_root/nginx.pid"
  fi
  for directory in "$validation_root/client-temp" "$validation_root/proxy-temp"; do
    if [ -e "$directory" ]; then
      [ -d "$directory" ] && [ ! -L "$directory" ]
      [ -z "$(find "$directory" -mindepth 1 -print -quit)" ]
      rmdir "$directory"
    fi
  done
  rmdir "$validation_root"
}
remove_account_state() {
  [ -e "$state_root" ] || return 0
  server_uid=$(getent passwd hardened-blikvm-server | awk -F: '{print $3}')
  shared_gid=$(getent group hardened-blikvm | awk -F: '{print $3}')
  [ -d "$state_root" ] && [ ! -L "$state_root" ]
  [ "$(stat -c '%a:%u:%g' "$state_root")" = "700:$server_uid:$shared_gid" ]
  unexpected=$(find "$state_root" -mindepth 1 -maxdepth 1 ! -name account.json ! -name '.account-*' -print -quit)
  [ -z "$unexpected" ]
  for path in "$state_root/account.json" "$state_root"/.account-*; do
    [ -e "$path" ] || continue
    [ -f "$path" ] && [ ! -L "$path" ]
    [ "$(stat -c '%u:%g:%h' "$path")" = "$server_uid:$shared_gid:1" ]
    [ "$(stat -c '%s' "$path")" -le 4096 ]
    rm -f -- "$path"
  done
  rmdir "$state_root"
}
""".strip()


def _path_values(paths_value: Paths) -> dict[str, str]:
    return {
        "rollback_root": paths_value.root,
        "marker": paths_value.marker,
        "rollback_script": paths_value.rollback_script,
        "watchdog_script": paths_value.watchdog_script,
        "boot_unit": paths_value.boot_unit,
        "boot_unit_path": paths_value.boot_unit_path,
        "boot_unit_part": paths_value.boot_unit_part,
        "boot_unit_link": paths_value.boot_unit_link,
        "watchdog_unit": paths_value.watchdog_unit,
        "config_part": paths_value.config_part,
        "unit_part": paths_value.unit_part,
        "validation_config": paths_value.validation_config,
        "validation_root": paths_value.validation_root,
        "config_target": CONFIG_TARGET,
        "unit_target": UNIT_TARGET,
        "unit_link": UNIT_LINK,
        "nginx_unit": UNIT_NAME,
        "state_root": STATE_ROOT,
    }


def build_rollback_script(
    paths_value: Paths,
    assets: provision.Assets,
    baseline: Mapping[str, str],
    nginx_config: str,
    nginx_unit: str,
) -> str:
    boot_unit = build_boot_unit(paths_value)
    values = {
        **_path_values(paths_value),
        "marker_value": f"{paths_value.run_id}:{assets.package.sha256}",
        "config_sha256": _digest(nginx_config),
        "unit_sha256": _digest(nginx_unit),
        "boot_unit_sha256": _digest(boot_unit),
        "certificate_path": baseline["certificate_path"],
        "certificate_sha256": baseline["certificate_sha256"],
        "key_path": baseline["key_path"],
        "key_sha256": baseline["key_sha256"],
    }
    body = r"""
set -eu
export LC_ALL=C
[ "$#" = 1 ]
case "$1" in boot|watchdog|explicit) ;; *) exit 1 ;; esac
[ -f "$marker" ]
[ "$(cat "$marker")" = "$marker_value" ]
systemctl stop "$nginx_unit" >/dev/null 2>&1 || true
for unit in hardened-blikvm-server.service hardened-blikvm-video-proxy.service hardened-blikvm-platform-broker.service; do
  systemctl stop "$unit" >/dev/null 2>&1 || true
done
systemctl disable "$nginx_unit" hardened-blikvm-server.service hardened-blikvm-video-proxy.service hardened-blikvm-platform-broker.service >/dev/null 2>&1 || true
count=0
while [ "$count" -lt 100 ]; do
  if product_inactive_disabled && cgroup_empty "$nginx_unit" &&
     cgroup_empty hardened-blikvm-server.service &&
     cgroup_empty hardened-blikvm-video-proxy.service &&
     cgroup_empty hardened-blikvm-platform-broker.service; then break; fi
  sleep 0.1
  count=$((count + 1))
done
[ "$count" -lt 100 ]
remove_account_state
if [ -e "$config_target" ]; then
  [ -f "$config_target" ] && [ ! -L "$config_target" ]
  [ "$(sha256sum "$config_target" | awk '{print $1}')" = "$config_sha256" ]
  rm -f -- "$config_target"
fi
if [ -e "$unit_target" ]; then
  [ -f "$unit_target" ] && [ ! -L "$unit_target" ]
  [ "$(sha256sum "$unit_target" | awk '{print $1}')" = "$unit_sha256" ]
  rm -f -- "$unit_target"
fi
systemctl daemon-reload >/dev/null 2>&1
systemctl enable kvmd-nginx.service >/dev/null 2>&1
if [ "$1" = boot ]; then
  systemctl start --no-block kvmd-nginx.service >/dev/null 2>&1 || true
else
  systemctl start kvmd-nginx.service >/dev/null 2>&1
fi
if [ "$1" != boot ]; then
  count=0
  while [ "$count" -lt 100 ]; do
    if [ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ] && ports_exact; then break; fi
    sleep 0.1
    count=$((count + 1))
  done
  [ "$count" -lt 100 ]
  [ "$(served_certificate_hash)" = "$(source_certificate_der_hash)" ]
fi
[ "$(service_enabled kvmd-nginx.service)" = enabled ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(sha256sum "$certificate_path" | awk '{print $1}')" = "$certificate_sha256" ]
[ "$(sha256sum "$key_path" | awk '{print $1}')" = "$key_sha256" ]
control_boundary_absent
if [ -e "$boot_unit_link" ]; then
  [ -L "$boot_unit_link" ] && [ "$(readlink "$boot_unit_link")" = "../$boot_unit" ]
  rm -f -- "$boot_unit_link"
fi
if [ -e "$boot_unit_path" ]; then
  [ -f "$boot_unit_path" ] && [ ! -L "$boot_unit_path" ]
  [ "$(sha256sum "$boot_unit_path" | awk '{print $1}')" = "$boot_unit_sha256" ]
  rm -f -- "$boot_unit_path"
fi
systemctl daemon-reload >/dev/null 2>&1
remove_validation_runtime
rm -f -- "$config_part" "$unit_part" "$validation_config" "$watchdog_script" "$rollback_script" "$marker"
rmdir "$rollback_root"
""".strip()
    return f"#!/bin/sh\n{_assignments(values)}\n{REMOTE_HELPERS}\n{body}\n"


def build_prepare_script(
    paths_value: Paths,
    assets: provision.Assets,
    baseline: Mapping[str, str],
    nginx_config: str,
    nginx_unit: str,
) -> str:
    boot_unit = build_boot_unit(paths_value)
    rollback = build_rollback_script(
        paths_value, assets, baseline, nginx_config, nginx_unit
    )
    watchdog = (
        "#!/bin/sh\nset -eu\nsleep 600\nexec /bin/sh "
        f"{shlex.quote(paths_value.rollback_script)} watchdog\n"
    )
    values = {
        **_path_values(paths_value),
        "marker_value": f"{paths_value.run_id}:{assets.package.sha256}",
        "boot_unit_b64": _encoded(boot_unit),
        "boot_unit_sha256": _digest(boot_unit),
        "rollback_b64": _encoded(rollback),
        "rollback_sha256": _digest(rollback),
        "watchdog_b64": _encoded(watchdog),
        "watchdog_sha256": _digest(watchdog),
    }
    body = r"""
set -eu
export LC_ALL=C
umask 077
emit() { printf '%s=%s\n' "$1" "$2"; }
phase=preflight
on_exit() {
  code=$?
  trap - EXIT
  if [ "$code" -ne 0 ]; then emit prepare_failure_phase "$phase"; fi
  exit "$code"
}
trap on_exit EXIT
[ "$(id -u)" = 0 ]
[ ! -e "$rollback_root" ]
[ ! -e "$boot_unit_path" ] && [ ! -e "$boot_unit_part" ] && [ ! -e "$boot_unit_link" ]
[ ! -e "$config_target" ] && [ ! -e "$unit_target" ] && [ ! -e "$unit_link" ]
[ "$(systemctl show -p LoadState --value "$watchdog_unit" 2>/dev/null || true)" = not-found ]
product_inactive_disabled
control_boundary_absent
[ ! -e "$state_root" ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
[ "$(service_enabled kvmd-nginx.service)" = enabled ]
phase=rollback-root
mkdir -m 700 "$rollback_root"
printf '%s\n' "$marker_value" > "$marker"
chmod 600 "$marker"
printf '%s' "$rollback_b64" | base64 -d > "$rollback_script"
printf '%s' "$watchdog_b64" | base64 -d > "$watchdog_script"
[ "$(sha256sum "$rollback_script" | awk '{print $1}')" = "$rollback_sha256" ]
[ "$(sha256sum "$watchdog_script" | awk '{print $1}')" = "$watchdog_sha256" ]
chmod 700 "$rollback_script" "$watchdog_script"
phase=boot-rollback
printf '%s' "$boot_unit_b64" | base64 -d > "$boot_unit_part"
[ "$(sha256sum "$boot_unit_part" | awk '{print $1}')" = "$boot_unit_sha256" ]
chmod 644 "$boot_unit_part"
mv "$boot_unit_part" "$boot_unit_path"
ln -s "../$boot_unit" "$boot_unit_link"
sync
systemctl daemon-reload
[ "$(systemctl is-enabled "$boot_unit" 2>/dev/null || true)" = enabled ]
phase=watchdog
systemd-run --quiet --collect --unit="$watchdog_unit" \
  --property=NoNewPrivileges=yes --property=PrivateDevices=yes \
  --property=PrivateNetwork=yes --property=ProtectHome=yes \
  --property=ProtectKernelLogs=yes --property=ProtectKernelModules=yes \
  --property=ProtectKernelTunables=yes --property=RestrictAddressFamilies=AF_UNIX \
  /bin/sh "$watchdog_script"
count=0
while [ "$count" -lt 50 ] && [ "$(systemctl is-active "$watchdog_unit" 2>/dev/null || true)" != active ]; do
  sleep 0.1
  count=$((count + 1))
done
[ "$count" -lt 50 ]
sync
phase=complete
emit rollback_synced yes
emit boot_rollback_armed yes
emit watchdog_active yes
""".strip()
    return f"{_assignments(values)}\n{REMOTE_HELPERS}\n{body}"


def build_apply_script(
    paths_value: Paths,
    assets: provision.Assets,
    baseline: Mapping[str, str],
    nginx_config: str,
    nginx_unit: str,
) -> str:
    validation_config = render_validation_nginx(
        baseline, nginx_config, paths_value.validation_root
    )
    values = {
        **_path_values(paths_value),
        "marker_value": f"{paths_value.run_id}:{assets.package.sha256}",
        "nginx_config_b64": _encoded(nginx_config),
        "nginx_config_sha256": _digest(nginx_config),
        "nginx_validation_b64": _encoded(validation_config),
        "nginx_validation_sha256": _digest(validation_config),
        "nginx_unit_b64": _encoded(nginx_unit),
        "nginx_unit_sha256": _digest(nginx_unit),
        "certificate_path": baseline["certificate_path"],
        "certificate_sha256": baseline["certificate_sha256"],
        "key_path": baseline["key_path"],
        "key_sha256": baseline["key_sha256"],
    }
    body = r"""
set -eu
export LC_ALL=C
umask 077
emit() { printf '%s=%s\n' "$1" "$2"; }
phase=validate
run_succeeded=no
cleanup_state=retained
on_exit() {
  code=$?
  trap - EXIT HUP INT TERM
  if [ "$run_succeeded" != yes ]; then
    systemctl stop "$watchdog_unit" >/dev/null 2>&1 || true
    if /bin/sh "$rollback_script" explicit >/dev/null 2>&1; then cleanup_state=restored; fi
    emit apply_failure_phase "$phase"
    emit apply_cleanup "$cleanup_state"
  fi
  exit "$code"
}
trap on_exit EXIT
trap 'exit 1' HUP INT TERM
[ -f "$marker" ] && [ "$(cat "$marker")" = "$marker_value" ]
[ -f "$rollback_script" ]
[ "$(systemctl is-active "$watchdog_unit" 2>/dev/null || true)" = active ]
product_inactive_disabled
control_boundary_absent
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
[ "$(service_enabled kvmd-nginx.service)" = enabled ]
[ "$(sha256sum "$certificate_path" | awk '{print $1}')" = "$certificate_sha256" ]
[ "$(sha256sum "$key_path" | awk '{print $1}')" = "$key_sha256" ]
phase=install-config
printf '%s' "$nginx_config_b64" | base64 -d > "$config_part"
printf '%s' "$nginx_unit_b64" | base64 -d > "$unit_part"
printf '%s' "$nginx_validation_b64" | base64 -d > "$validation_config"
[ "$(sha256sum "$config_part" | awk '{print $1}')" = "$nginx_config_sha256" ]
[ "$(sha256sum "$unit_part" | awk '{print $1}')" = "$nginx_unit_sha256" ]
[ "$(sha256sum "$validation_config" | awk '{print $1}')" = "$nginx_validation_sha256" ]
chmod 600 "$config_part"
chmod 644 "$unit_part"
chmod 600 "$validation_config"
mv "$config_part" "$config_target"
mv "$unit_part" "$unit_target"
systemctl daemon-reload
phase=validate-nginx-config
mkdir -m 700 "$validation_root"
mkdir -m 700 "$validation_root/client-temp" "$validation_root/proxy-temp"
/usr/sbin/nginx -t -c "$validation_config" >/dev/null 2>&1
remove_validation_runtime
rm -f -- "$validation_config"
phase=start-runtime
systemctl enable hardened-blikvm-platform-broker.service hardened-blikvm-video-proxy.service hardened-blikvm-server.service >/dev/null
systemctl start hardened-blikvm-platform-broker.service
systemctl start hardened-blikvm-video-proxy.service
systemctl start hardened-blikvm-server.service
count=0
while [ "$count" -lt 100 ] && ! curl --noproxy '*' --silent --fail --max-time 1 --output /dev/null http://127.0.0.1:18080/api/health 2>/dev/null; do
  sleep 0.1
  count=$((count + 1))
done
[ "$count" -lt 100 ]
product_active_enabled
direct_first_setup_required
phase=listener-cutover
systemctl disable --now kvmd-nginx.service >/dev/null
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = inactive ]
[ "$(service_enabled kvmd-nginx.service)" = disabled ]
systemctl enable "$nginx_unit" >/dev/null
systemctl start "$nginx_unit"
count=0
while [ "$count" -lt 100 ] && ! ports_exact; do
  [ "$(systemctl is-active "$nginx_unit" 2>/dev/null || true)" = active ]
  sleep 0.1
  count=$((count + 1))
done
[ "$count" -lt 100 ]
[ "$(systemctl is-active "$nginx_unit" 2>/dev/null || true)" = active ]
[ "$(service_enabled "$nginx_unit")" = enabled ]
phase=nginx-sandbox
nginx_pid=$(systemctl show -p MainPID --value "$nginx_unit")
case "$nginx_pid" in ''|0|*[!0-9]*) exit 1 ;; esac
www_uid=$(getent passwd www-data | awk -F: '{print $3}')
[ "$(awk '/^Uid:/ {print $2}' "/proc/$nginx_pid/status")" = 0 ]
[ "$(awk '/^CapBnd:/ {print $2}' "/proc/$nginx_pid/status")" = 00000000000004c0 ]
nginx_effective_caps=$(awk '/^CapEff:/ {print $2}' "/proc/$nginx_pid/status")
case "$nginx_effective_caps" in 0000000000000000|00000000000004c0) ;; *) exit 1 ;; esac
[ "$(awk '/^NoNewPrivs:/ {print $2}' "/proc/$nginx_pid/status")" = 1 ]
workers=0
for worker in $(pgrep -P "$nginx_pid" 2>/dev/null || true); do
  [ "$(awk '/^Uid:/ {print $2}' "/proc/$worker/status")" = "$www_uid" ]
  [ "$(awk '/^CapEff:/ {print $2}' "/proc/$worker/status")" = 0000000000000000 ]
  workers=$((workers + 1))
done
[ "$workers" -ge 1 ]
phase=tls-certificate
[ "$(served_certificate_hash)" = "$(source_certificate_der_hash)" ]
phase=tls12
curl --noproxy '*' --insecure --silent --fail --max-time 5 --tlsv1.2 --tls-max 1.2 --output /dev/null https://127.0.0.1/api/health
phase=tls13
curl --noproxy '*' --insecure --silent --fail --max-time 5 --tlsv1.3 --tls-max 1.3 --output /dev/null https://127.0.0.1/api/health
phase=http-redirect
[ "$(curl --noproxy '*' --silent --max-time 5 --output /dev/null --write-out '%{http_code}' http://127.0.0.1/api/health)" = 308 ]
phase=first-setup-tls
first_setup_required
phase=backend-final
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
control_boundary_absent
sync
run_succeeded=yes
trap - EXIT HUP INT TERM
emit vendor_nginx_disabled yes
emit hardened_services_enabled yes
emit hardened_nginx_active active
emit port80_exact yes
emit port443_exact yes
emit tls12 passed
emit tls13 passed
emit http_redirect passed
emit certificate_reused yes
emit first_setup_required yes
emit nginx_sandbox yes
emit control_boundary_absent yes
emit kvmd_active active
""".strip()
    return f"{_assignments(values)}\n{REMOTE_HELPERS}\n{body}"


def build_persistent_check_script(
    paths_value: Paths,
    baseline: Mapping[str, str],
    nginx_config: str,
    nginx_unit: str,
    *,
    commit: bool,
) -> str:
    boot_unit = build_boot_unit(paths_value)
    values = {
        **_path_values(paths_value),
        "nginx_config_sha256": _digest(nginx_config),
        "nginx_unit_sha256": _digest(nginx_unit),
        "boot_unit_sha256": _digest(boot_unit),
        "certificate_path": baseline["certificate_path"],
        "certificate_sha256": baseline["certificate_sha256"],
        "key_path": baseline["key_path"],
        "key_sha256": baseline["key_sha256"],
        "commit_mode": "yes" if commit else "no",
    }
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
product_active_enabled
[ "$(systemctl is-active "$nginx_unit" 2>/dev/null || true)" = active ]
[ "$(service_enabled "$nginx_unit")" = enabled ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = inactive ]
[ "$(service_enabled kvmd-nginx.service)" = disabled ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
control_boundary_absent
ports_exact
[ -f "$config_target" ] && [ ! -L "$config_target" ]
[ "$(stat -c '%a:%u:%g:%h' "$config_target")" = 600:0:0:1 ]
[ "$(sha256sum "$config_target" | awk '{print $1}')" = "$nginx_config_sha256" ]
[ -f "$unit_target" ] && [ ! -L "$unit_target" ]
[ "$(stat -c '%a:%u:%g:%h' "$unit_target")" = 644:0:0:1 ]
[ "$(sha256sum "$unit_target" | awk '{print $1}')" = "$nginx_unit_sha256" ]
[ "$(sha256sum "$certificate_path" | awk '{print $1}')" = "$certificate_sha256" ]
[ "$(sha256sum "$key_path" | awk '{print $1}')" = "$key_sha256" ]
[ "$(served_certificate_hash)" = "$(source_certificate_der_hash)" ]
first_setup_required
if [ "$commit_mode" = yes ]; then
  [ -f "$marker" ] && [ -f "$rollback_script" ]
  [ "$(systemctl is-active "$watchdog_unit" 2>/dev/null || true)" = active ]
  systemctl stop "$watchdog_unit" >/dev/null 2>&1
  systemctl reset-failed "$watchdog_unit" >/dev/null 2>&1 || true
  count=0
  while [ "$count" -lt 50 ] && [ "$(systemctl show -p LoadState --value "$watchdog_unit" 2>/dev/null || true)" != not-found ]; do
    sleep 0.1
    count=$((count + 1))
  done
  [ "$count" -lt 50 ]
  rm -f -- "$marker"
  sync
  [ -L "$boot_unit_link" ] && [ "$(readlink "$boot_unit_link")" = "../$boot_unit" ]
  rm -f -- "$boot_unit_link"
  [ -f "$boot_unit_path" ] && [ ! -L "$boot_unit_path" ]
  [ "$(sha256sum "$boot_unit_path" | awk '{print $1}')" = "$boot_unit_sha256" ]
  rm -f -- "$boot_unit_path"
  systemctl daemon-reload
  remove_validation_runtime
  rm -f -- "$config_part" "$unit_part" "$validation_config" "$watchdog_script" "$rollback_script"
  rmdir "$rollback_root"
  sync
fi
[ ! -e "$rollback_root" ] && [ ! -e "$boot_unit_path" ] && [ ! -e "$boot_unit_link" ]
[ "$(systemctl show -p LoadState --value "$watchdog_unit" 2>/dev/null || true)" = not-found ]
emit vendor_nginx_disabled yes
emit hardened_services_persistent yes
emit hardened_nginx_persistent yes
emit certificate_reused yes
emit first_setup_required yes
emit ports_exact yes
emit control_boundary_absent yes
emit kvmd_active active
emit watchdog_removed yes
emit rollback_removed yes
""".strip()
    return f"{_assignments(values)}\n{REMOTE_HELPERS}\n{body}"


def build_cleanup_script(
    paths_value: Paths,
    baseline: Mapping[str, str],
    nginx_config: str,
    nginx_unit: str,
) -> str:
    values = {
        **_path_values(paths_value),
        "config_sha256": _digest(nginx_config),
        "unit_sha256": _digest(nginx_unit),
        "certificate_sha256": baseline["certificate_sha256"],
        "certificate_path": baseline["certificate_path"],
        "key_path": baseline["key_path"],
        "key_sha256": baseline["key_sha256"],
    }
    body = r"""
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
systemctl stop "$watchdog_unit" >/dev/null 2>&1 || true
systemctl reset-failed "$watchdog_unit" >/dev/null 2>&1 || true
count=0
while [ "$count" -lt 50 ] && [ "$(systemctl show -p LoadState --value "$watchdog_unit" 2>/dev/null || true)" != not-found ]; do
  sleep 0.1
  count=$((count + 1))
done
if [ -f "$marker" ] && [ -f "$rollback_script" ]; then
  /bin/sh "$rollback_script" explicit >/dev/null 2>&1
  emit cleanup_state restored
  exit 0
fi
if product_inactive_disabled &&
   [ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ] &&
   [ "$(service_enabled kvmd-nginx.service)" = enabled ] &&
   [ "$(systemctl show -p LoadState --value "$nginx_unit" 2>/dev/null || true)" = not-found ] &&
   [ ! -e "$config_target" ] && [ ! -e "$unit_target" ] && [ ! -e "$state_root" ] &&
   [ ! -e "$rollback_root" ] && [ ! -e "$boot_unit_path" ] && [ ! -e "$boot_unit_link" ] &&
   [ "$(systemctl show -p LoadState --value "$watchdog_unit" 2>/dev/null || true)" = not-found ] &&
   ports_exact && [ "$(served_certificate_hash)" = "$(source_certificate_der_hash)" ] &&
   [ "$(sha256sum "$certificate_path" | awk '{print $1}')" = "$certificate_sha256" ] &&
   [ "$(sha256sum "$key_path" | awk '{print $1}')" = "$key_sha256" ] &&
   control_boundary_absent; then
  emit cleanup_state restored
  exit 0
fi
if product_active_enabled &&
   [ "$(systemctl is-active "$nginx_unit" 2>/dev/null || true)" = active ] &&
   [ "$(service_enabled "$nginx_unit")" = enabled ] &&
   [ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = inactive ] &&
   [ "$(service_enabled kvmd-nginx.service)" = disabled ] &&
   ports_exact && [ "$(served_certificate_hash)" = "$(source_certificate_der_hash)" ] &&
   [ ! -e "$rollback_root" ] && [ ! -e "$boot_unit_path" ] && [ ! -e "$boot_unit_link" ]; then
  emit cleanup_state committed
  exit 0
fi
exit 1
""".strip()
    return f"{_assignments(values)}\n{REMOTE_HELPERS}\n{body}"


def tunnel_command(
    target: prototype.RemoteTarget, password_fd: int, local_port: int
) -> list[str]:
    transport = prototype.ssh_transport_command(
        target, password_fd, clear_forwardings=False
    )
    return [
        *transport[:-1],
        "-o",
        "ExitOnForwardFailure=yes",
        "-N",
        "-L",
        f"127.0.0.1:{local_port}:127.0.0.1:443",
        transport[-1],
    ]


def validate_browser(raw: bytes) -> dict[str, object]:
    if len(raw) > 16 * 1024:
        raise ValueError("cutover browser output exceeded the limit")
    decoded = cast(object, json.loads(raw))
    if not isinstance(decoded, dict):
        raise ValueError("cutover browser output root is invalid")
    result = cast(dict[str, object], decoded)
    expected = {
        "schemaVersion": 1,
        "result": "passed",
        "title": "First Setup - Hardened BliKVM",
        "setupRequired": True,
        "inputCount": 3,
        "cookieCount": 0,
        "apiMethods": ["GET"],
        "controlRequests": 0,
        "webSockets": 0,
        "legacyBrandingAbsent": True,
    }
    for key, value in expected.items():
        if result.get(key) != value:
            raise ValueError("cutover browser invariant failed")
    api_requests = result.get("apiRequests")
    if not isinstance(api_requests, int) or not 1 <= api_requests <= 16:
        raise ValueError("cutover browser request count is invalid")
    return result


def _zero(value: bytearray) -> None:
    for index in range(len(value)):
        value[index] = 0


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    assets = validate_assets(options)
    baseline = load_baseline(options.baseline)
    nginx_config = render_nginx(baseline)
    nginx_unit = render_unit(baseline)
    if not BROWSER_SCRIPT.is_file():
        raise ValueError("cutover browser script is unavailable")
    host, ssh_port = prototype.parse_endpoint(options.endpoint)
    username = prototype.validate_username(options.username)
    known_hosts = prototype.validate_private_file(options.known_hosts)
    runtime = tls.browser_runtime()
    target = prototype.RemoteTarget(host, ssh_port, username, known_hosts)
    paths_value = paths(assets.package.sha256, secrets.token_hex(8))
    provision_paths = provision.remote_paths(
        assets.sysusers.sha256, secrets.token_hex(8)
    )
    secret = prototype.read_secret_stdin()
    cleanup_required = False
    tunnel: subprocess.Popen[bytes] | None = None
    stage = "initialization"
    browser_summary: dict[str, object] | None = None
    try:
        stage = "provisioned-state"
        result = prototype.run_ssh(
            target,
            secret,
            provision.build_final_script(provision_paths, assets),
            timeout=60,
        )
        if result.returncode != 0:
            raise CutoverError(stage, "remote-command", return_code=result.returncode)
        provisioned_before = provision.parse_facts(result.stdout, provision.FINAL_KEYS)
        stage = "cutover-preflight"
        result = prototype.run_ssh(
            target, secret, cutover_preflight.REMOTE_PROBE, timeout=60
        )
        if result.returncode != 0:
            remote_phase, _ = parse_failure(result.stdout, "preflight")
            raise CutoverError(
                stage,
                "remote-command",
                return_code=result.returncode,
                remote_phase=remote_phase,
            )
        live_preflight = cutover_preflight.parse_safe_facts(result.stdout)
        if live_preflight != baseline:
            raise CutoverError(stage, "baseline-changed")
        stage = "prepare-rollback"
        cleanup_required = True
        result = prototype.run_ssh(
            target,
            secret,
            build_prepare_script(
                paths_value, assets, baseline, nginx_config, nginx_unit
            ),
            timeout=90,
        )
        if result.returncode != 0:
            phase, _ = parse_failure(result.stdout, "prepare")
            raise CutoverError(
                stage,
                "remote-command",
                return_code=result.returncode,
                remote_phase=phase,
            )
        prepared = parse_facts(result.stdout, PREPARE_KEYS)
        stage = "apply-cutover"
        result = prototype.run_ssh(
            target,
            secret,
            build_apply_script(
                paths_value, assets, baseline, nginx_config, nginx_unit
            ),
            timeout=180,
        )
        if result.returncode != 0:
            phase, restored = parse_failure(result.stdout, "apply")
            if restored:
                cleanup_required = False
            raise CutoverError(
                stage,
                "remote-command",
                return_code=result.returncode,
                remote_phase=phase,
            )
        applied = parse_facts(result.stdout, APPLY_KEYS)
        stage = "ssh-tunnel"
        local_port = tls.free_local_port()
        tunnel = tls.start_with_secret_fd(
            secret,
            lambda fd: tunnel_command(target, fd, local_port),
            stdout=subprocess.DEVNULL,
        )
        tls.wait_for_tunnel(tunnel, local_port, 15)
        stage = "first-setup-browser"
        browser_result = subprocess.run(
            [
                str(runtime.node),
                str(BROWSER_SCRIPT),
                str(runtime.module),
                str(runtime.executable),
                f"https://127.0.0.1:{local_port}",
            ],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            check=False,
            timeout=90,
        )
        if browser_result.returncode != 0:
            raise CutoverError(
                stage, "browser", return_code=browser_result.returncode
            )
        browser_summary = validate_browser(browser_result.stdout)
        tls.stop_process(tunnel)
        tunnel = None
        stage = "commit"
        result = prototype.run_ssh(
            target,
            secret,
            build_persistent_check_script(
                paths_value, baseline, nginx_config, nginx_unit, commit=True
            ),
            timeout=120,
        )
        if result.returncode != 0:
            raise CutoverError(stage, "remote-command", return_code=result.returncode)
        committed = parse_facts(result.stdout, COMMIT_KEYS)
        cleanup_required = False
        stage = "independent-final"
        result = prototype.run_ssh(
            target,
            secret,
            build_persistent_check_script(
                paths_value, baseline, nginx_config, nginx_unit, commit=False
            ),
            timeout=60,
        )
        if result.returncode != 0:
            raise CutoverError(stage, "remote-command", return_code=result.returncode)
        final_state = parse_facts(result.stdout, COMMIT_KEYS)
        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-persistent-web-cutover",
            "result": "passed",
            "packageSha256": assets.package.sha256,
            "sourceCommit": assets.package.source_commit,
            "policy": {
                "vendorWebService": "disabled-inactive-retained",
                "vendorBackend": "kvmd-active-transition-only",
                "tlsCertificate": "existing-device-pair-reused-by-reference",
                "listener": "persistent-80-443",
                "firstAccount": "user-created-after-cutover",
                "controlServiceStarted": False,
                "physicalActionsInvoked": False,
                "secretEmitted": False,
            },
            "provisionedBefore": provisioned_before,
            "prepared": prepared,
            "applied": applied,
            "browser": browser_summary,
            "committed": committed,
            "finalState": final_state,
            "cleanup": {"attempted": False, "confirmed": True, "state": "committed"},
        }
        write_json_0600_atomic(output, receipt)
        return 0
    except (
        CutoverError,
        OSError,
        subprocess.SubprocessError,
        UnicodeError,
        ValueError,
    ) as error:
        cleanup: CleanupReceipt = {
            "attempted": False,
            "confirmed": not cleanup_required,
            "state": "restored" if not cleanup_required else "unknown",
        }
        if cleanup_required:
            try:
                result = prototype.run_ssh(
                    target,
                    secret,
                    build_cleanup_script(
                        paths_value, baseline, nginx_config, nginx_unit
                    ),
                    timeout=180,
                )
                facts = (
                    parse_facts(result.stdout, CLEANUP_KEYS)
                    if result.returncode == 0
                    else {}
                )
                state = facts.get("cleanup_state", "unknown")
                cleanup = {
                    "attempted": True,
                    "confirmed": state in {"restored", "committed"},
                    "state": state,
                }
            except (OSError, subprocess.SubprocessError, UnicodeError, ValueError):
                cleanup = {
                    "attempted": True,
                    "confirmed": False,
                    "state": "unknown",
                }
        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-persistent-web-cutover",
            "result": "failed",
            "packageSha256": assets.package.sha256,
            "failure": {
                "stage": getattr(error, "stage", stage),
                "failureType": getattr(error, "failure_type", type(error).__name__),
                "returnCode": getattr(error, "return_code", None),
                "remotePhase": getattr(error, "remote_phase", None),
            },
            "cleanup": cleanup,
        }
        write_json_0600_atomic(output, receipt)
        return 1
    finally:
        if tunnel is not None:
            tls.stop_process(tunnel)
        _zero(secret)


def main() -> int:
    try:
        return execute(parse_args())
    except (
        OSError,
        subprocess.SubprocessError,
        UnicodeError,
        ValueError,
    ) as error:
        print(f"web cutover rejected: {type(error).__name__}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
