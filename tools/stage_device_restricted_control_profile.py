#!/usr/bin/env python3
"""Deploy the restricted-control (HID+ATX) test profile on a test H616.

The tool uploads the profile bundle, stages the feature-built server and
control broker under a runtime directory in `/run`, starts them as transient
units together with a temporary HTTPS reverse proxy, and verifies that the
server advertises HID and power `read_write` while default builds keep them
unavailable. Everything it creates lives in `/run` (transient units, runtime
directory and a self-signed certificate) and `--teardown` removes it, so the
retained application slots, the permanent write policy and the live GUI are
never touched.

The test broker runs as the existing `hardened-blikvm-control` account with the
`kvmd` supplementary group instead of creating the dedicated production
`hardened-blikvm-hid` account; the KVMD credential is still delivered by
systemd through `LoadCredential` and is never written by this tool.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import shlex
import stat
import subprocess
import sys
import tarfile
from typing import TypedDict, cast

if not __package__:
    sys.path.insert(0, str(__import__("pathlib").Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype


EXPECTED_ROOT = "hardened-blikvm-restricted-control-profile-aarch64"
EXPECTED_PROFILE = "restricted-control-test"
EXPECTED_FEATURES = [
    "restricted-control-surface",
    "hid-transition-profile",
    "atx-transition-profile",
    "media-transition-profile",
]
REQUIRED_MEMBERS = {
    f"{EXPECTED_ROOT}/MANIFEST.json",
    f"{EXPECTED_ROOT}/SHA256SUMS",
    f"{EXPECTED_ROOT}/libexec/bli-kvm-server",
    f"{EXPECTED_ROOT}/libexec/bli-kvm-control-broker",
    f"{EXPECTED_ROOT}/share/systemd/hardened-blikvm-control-transition.service",
    f"{EXPECTED_ROOT}/share/tmpfiles.d/hardened-blikvm-control-transition.conf",
    f"{EXPECTED_ROOT}/share/server-dropin/90-restricted-control.conf",
    f"{EXPECTED_ROOT}/share/web/index.html",
}
MAX_PROFILE_BUNDLE_BYTES = 256 * 1024 * 1024
MAX_PROFILE_BUNDLE_FILES = 256
HEALTH_PORT = 18082
HTTPS_PORT = 8443
APPLY_KEYS = {
    "extracted",
    "broker_started",
    "control_socket_present",
    "server_started",
    "health",
    "hid_availability",
    "power_availability",
    "https_ready",
    "web_ready",
}
TEARDOWN_KEYS = {
    "units_removed",
    "broker_stopped",
    "server_stopped",
    "https_stopped",
    "runtime_absent",
}


class GateError(Exception):
    def __init__(self, stage: str, failure_type: str) -> None:
        super().__init__(stage)
        self.stage = stage
        self.failure_type = failure_type


class Receipt(TypedDict):
    bundleSha256: str
    stage: str
    facts: dict[str, str]
    endpoint: str
    cleanupConfirmed: bool
    failureStage: str
    failureType: str
    physicalActionsInvoked: bool
    persistentChangeScope: str


@dataclass(frozen=True)
class Options:
    endpoint: str
    username: str
    known_hosts: str
    bundle: str
    expected_sha256: str
    output: str
    teardown: bool


@dataclass(frozen=True)
class ProfileBundle:
    path: Path
    sha256: str
    server_sha256: str
    broker_sha256: str
    source_commit: str

    @property
    def runtime_root(self) -> str:
        return f"/tmp/hardened-blikvm-rc-{self.sha256[:16]}"


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--endpoint", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--known-hosts", required=True)
    _ = parser.add_argument("--bundle", required=True)
    _ = parser.add_argument("--expected-sha256", required=True)
    _ = parser.add_argument("--output", required=True)
    _ = parser.add_argument("--teardown", action="store_true")
    namespace = parser.parse_args()
    return Options(
        endpoint=cast(str, namespace.endpoint),
        username=cast(str, namespace.username),
        known_hosts=cast(str, namespace.known_hosts),
        bundle=cast(str, namespace.bundle),
        expected_sha256=cast(str, namespace.expected_sha256),
        output=cast(str, namespace.output),
        teardown=cast(bool, namespace.teardown),
    )


def validate_profile_bundle(path_value: str, expected_sha256: str) -> ProfileBundle:
    source = Path(path_value)
    if source.is_symlink():
        raise ValueError("bundle must be a regular non-symlink file")
    path = source.resolve(strict=True)
    metadata = path.stat()
    if not stat.S_ISREG(metadata.st_mode) or not 0 < metadata.st_size <= MAX_PROFILE_BUNDLE_BYTES:
        raise ValueError("bundle size or type is invalid")
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    if digest != expected_sha256:
        raise ValueError("bundle SHA-256 mismatch")

    with tarfile.open(path, mode="r:gz") as archive:
        members = archive.getmembers()
        if not members or len(members) > MAX_PROFILE_BUNDLE_FILES:
            raise ValueError("bundle member count is invalid")
        names: set[str] = set()
        total_size = 0
        for member in members:
            member_path = PurePosixPath(member.name)
            if (
                member_path.is_absolute()
                or ".." in member_path.parts
                or not member_path.parts
                or member_path.parts[0] != EXPECTED_ROOT
                or member.name in names
                or not (member.isfile() or member.isdir())
            ):
                raise ValueError("bundle contains an unsafe member")
            names.add(member.name)
            total_size += member.size
            if total_size > MAX_PROFILE_BUNDLE_BYTES:
                raise ValueError("bundle uncompressed size exceeded the limit")
        if not REQUIRED_MEMBERS.issubset(names):
            raise ValueError("bundle is missing a required profile member")
        manifest_file = archive.extractfile(f"{EXPECTED_ROOT}/MANIFEST.json")
        if manifest_file is None:
            raise ValueError("bundle manifest is missing")
        manifest = json.loads(manifest_file.read().decode("utf-8"))

    if (
        manifest.get("schemaVersion") != 1
        or manifest.get("product") != "Hardened BliKVM"
        or manifest.get("profile") != EXPECTED_PROFILE
        or manifest.get("cargoFeatures") != EXPECTED_FEATURES
        or manifest.get("releaseAccepted") is not False
        or manifest.get("deviceAccepted") is not False
    ):
        raise ValueError("bundle manifest does not describe the expected profile")
    source_commit = manifest.get("sourceCommit")
    if not isinstance(source_commit, str) or not re.fullmatch(r"[0-9a-f]{40}", source_commit):
        raise ValueError("bundle source commit is invalid")
    artifacts = manifest.get("artifacts")
    if not isinstance(artifacts, dict):
        raise ValueError("bundle artifacts are invalid")
    server_sha = artifacts.get("server", {}).get("sha256")
    broker_sha = artifacts.get("controlBroker", {}).get("sha256")
    if (
        not isinstance(server_sha, str)
        or not re.fullmatch(r"[0-9a-f]{64}", server_sha)
        or not isinstance(broker_sha, str)
        or not re.fullmatch(r"[0-9a-f]{64}", broker_sha)
    ):
        raise ValueError("bundle artifact hashes are invalid")
    return ProfileBundle(
        path=path,
        sha256=digest,
        server_sha256=server_sha,
        broker_sha256=broker_sha,
        source_commit=source_commit,
    )


BROKER_UNIT = """[Unit]
Description=Hardened BliKVM restricted-control test broker
Documentation=https://github.com/woffko/Hardened-BliKVM
Requires=kvmd.service
After=kvmd.service

[Service]
Type=simple
User=hardened-blikvm-control
Group=hardened-blikvm
SupplementaryGroups=kvmd
UMask=0077
Environment=BLI_KVM_CONTROL_SOCKET=__ROOT__/state/control.sock
Environment=BLI_KVM_HID_PROFILE=1
Environment=BLI_KVM_HID_LEDGER=__ROOT__/state/hid-ledger.json
Environment=BLI_KVM_HID_KVMD_SOCKET=/run/kvmd/kvmd.sock
Environment=BLI_KVM_ATX_PROFILE=1
Environment=BLI_KVM_ATX_JOURNAL=__ROOT__/state/atx-journal.json
Environment=BLI_KVM_ATX_KVMD_SOCKET=/run/kvmd/kvmd.sock
Environment=BLI_KVM_MEDIA_PROFILE=1
Environment=BLI_KVM_MEDIA_JOURNAL=__ROOT__/state/media-journal.json
Environment=BLI_KVM_MEDIA_CATALOG=__ROOT__/media/catalog.json
Environment=BLI_KVM_MEDIA_KVMD_SOCKET=/run/kvmd/kvmd.sock
LoadCredential=kvmd-user:/etc/hardened-blikvm/kvmd-user
LoadCredential=kvmd-password:/etc/hardened-blikvm/kvmd-password
ExecStart=__ROOT__/libexec/bli-kvm-control-broker
Restart=no
TimeoutStartSec=15s
TimeoutStopSec=10s

NoNewPrivileges=yes
PrivateDevices=yes
PrivateNetwork=yes
ProtectClock=yes
ProtectControlGroups=yes
ProtectHome=yes
ProtectHostname=yes
ProtectKernelLogs=yes
ProtectKernelModules=yes
ProtectKernelTunables=yes
ProtectProc=invisible
ProtectSystem=strict
ProcSubset=pid
RestrictAddressFamilies=AF_UNIX
RestrictNamespaces=yes
RestrictRealtime=yes
RestrictSUIDSGID=yes
LockPersonality=yes
MemoryDenyWriteExecute=yes
RemoveIPC=yes
CapabilityBoundingSet=
AmbientCapabilities=
DevicePolicy=closed
IPAddressDeny=any
ReadWritePaths=__ROOT__
SystemCallArchitectures=native
SystemCallFilter=@system-service
SystemCallErrorNumber=EPERM
"""

SERVER_UNIT = """[Unit]
Description=Hardened BliKVM restricted-control test server
Documentation=https://github.com/woffko/Hardened-BliKVM
After=hardened-blikvm-platform-broker.service hardened-blikvm-video-proxy.service hardened-blikvm-rc-broker.service
Wants=hardened-blikvm-video-proxy.service

[Service]
Type=simple
User=hardened-blikvm-server
Group=hardened-blikvm
UMask=0077
Environment=BLI_KVM_BIND=127.0.0.1:__PORT__
Environment=BLI_KVM_WEB_ROOT=__ROOT__/share/web
Environment=BLI_KVM_STATE_DIR=__ROOT__/server-state
Environment=BLI_KVM_BROKER_SOCKET=/run/hardened-blikvm/platform-broker.sock
Environment=BLI_KVM_VIDEO_SOCKET=/run/hardened-blikvm-video/mjpeg.sock
Environment=BLI_KVM_CONTROL_SOCKET=__ROOT__/state/control.sock
Environment=BLI_KVM_RESTRICTED_CONTROLS=1
Environment=BLI_KVM_MEDIA_CATALOG=__ROOT__/media/catalog.json
Environment=BLI_KVM_MEDIA_CONTROLS=1
Environment=BLI_KVM_SECURE_COOKIES=1
Environment=BLI_KVM_ALLOW_MISSING_ORIGIN=0
ExecStart=__ROOT__/libexec/bli-kvm-server
Restart=no
TimeoutStartSec=15s
TimeoutStopSec=10s

NoNewPrivileges=yes
PrivateDevices=yes
ProtectClock=yes
ProtectControlGroups=yes
ProtectHome=yes
ProtectHostname=yes
ProtectKernelLogs=yes
ProtectKernelModules=yes
ProtectKernelTunables=yes
ProtectProc=invisible
ProtectSystem=strict
ProcSubset=pid
RestrictAddressFamilies=AF_UNIX AF_INET
RestrictNamespaces=yes
RestrictRealtime=yes
RestrictSUIDSGID=yes
LockPersonality=yes
MemoryDenyWriteExecute=yes
RemoveIPC=yes
CapabilityBoundingSet=
AmbientCapabilities=
DevicePolicy=closed
IPAddressDeny=any
IPAddressAllow=localhost
SocketBindDeny=any
SocketBindAllow=tcp:__PORT__
ReadWritePaths=__ROOT__/server-state
SystemCallArchitectures=native
SystemCallFilter=@system-service
SystemCallErrorNumber=EPERM
"""

NGINX_UNIT = """[Unit]
Description=Hardened BliKVM restricted-control test HTTPS proxy
Documentation=https://github.com/woffko/Hardened-BliKVM
After=hardened-blikvm-rc-server.service
Requires=hardened-blikvm-rc-server.service

[Service]
Type=simple
User=www-data
Group=www-data
UMask=0077
ExecStartPre=/usr/sbin/nginx -t -c __ROOT__/nginx/nginx.conf
ExecStart=/usr/sbin/nginx -c __ROOT__/nginx/nginx.conf -g "daemon off;"
Restart=no
TimeoutStartSec=15s
TimeoutStopSec=10s

NoNewPrivileges=yes
PrivateDevices=yes
ProtectClock=yes
ProtectControlGroups=yes
ProtectHome=yes
ProtectHostname=yes
ProtectKernelLogs=yes
ProtectKernelModules=yes
ProtectKernelTunables=yes
ProtectProc=invisible
ProtectSystem=strict
ProcSubset=pid
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
RestrictNamespaces=yes
RestrictRealtime=yes
RestrictSUIDSGID=yes
LockPersonality=yes
MemoryDenyWriteExecute=yes
RemoveIPC=yes
CapabilityBoundingSet=
AmbientCapabilities=
DevicePolicy=closed
IPAddressDeny=multicast
SocketBindDeny=any
SocketBindAllow=tcp:__PORT__
ReadWritePaths=__ROOT__/nginx
SystemCallArchitectures=native
SystemCallFilter=@system-service
SystemCallErrorNumber=EPERM
"""

NGINX_CONF = """worker_processes 1;
pid __ROOT__/nginx/nginx.pid;
error_log stderr notice;

events {
    worker_connections 128;
}

http {
    access_log off;
    server_tokens off;
    client_body_temp_path __ROOT__/nginx/client;
    proxy_temp_path __ROOT__/nginx/proxy;

    map $http_upgrade $hardened_blikvm_connection_upgrade {
        default upgrade;
        ''      close;
    }

    server {
        listen __HTTPS_PORT__ ssl;
        server_name _;

        ssl_certificate __ROOT__/nginx/server.crt;
        ssl_certificate_key __ROOT__/nginx/server.key;
        ssl_protocols TLSv1.2 TLSv1.3;
        ssl_session_tickets off;

        client_max_body_size 16m;
        add_header Cache-Control "no-store" always;

        location / {
            proxy_pass http://127.0.0.1:__PORT__;
            proxy_http_version 1.1;
            proxy_set_header Host $http_host;
            proxy_set_header X-Forwarded-Proto https;
            proxy_set_header X-Forwarded-For $remote_addr;
            proxy_set_header Upgrade $http_upgrade;
            proxy_set_header Connection $hardened_blikvm_connection_upgrade;
            proxy_buffering off;
            proxy_read_timeout 3600s;
        }
    }
}
"""


def build_apply_script(bundle: ProfileBundle) -> str:
    values = {
        "root": bundle.runtime_root,
        "archive": f"{bundle.runtime_root}/bundle.tar.gz",
        "expected_server_sha": bundle.server_sha256,
        "expected_broker_sha": bundle.broker_sha256,
        "port": str(HEALTH_PORT),
        "https_port": str(HTTPS_PORT),
    }
    header = "\n".join(f"{name}={shlex.quote(value)}" for name, value in values.items())
    broker_unit = BROKER_UNIT.replace("__ROOT__", bundle.runtime_root)
    server_unit = SERVER_UNIT.replace("__ROOT__", bundle.runtime_root).replace(
        "__PORT__", str(HEALTH_PORT)
    )
    nginx_unit = NGINX_UNIT.replace("__ROOT__", bundle.runtime_root).replace(
        "__PORT__", str(HTTPS_PORT)
    )
    nginx_conf = (
        NGINX_CONF.replace("__ROOT__", bundle.runtime_root)
        .replace("__PORT__", str(HEALTH_PORT))
        .replace("__HTTPS_PORT__", str(HTTPS_PORT))
    )
    return f"""set -eu
export LC_ALL=C
{header}
emit() {{ printf '%s=%s\\n' "$1" "$2"; }}
extracted=no
web_ready=no
broker_started=no
control_socket_present=no
server_started=no
health=no
hid=unavailable
power=unavailable
https=no

if [ "$(uname -m)" = aarch64 ]; then
  umask 022
  if tar -xzf "$archive" -C "$root" --strip-components=1 \
    && [ -x "$root/libexec/bli-kvm-server" ] \
    && [ "$(sha256sum "$root/libexec/bli-kvm-server" | cut -d ' ' -f 1)" = "$expected_server_sha" ] \
    && [ "$(sha256sum "$root/libexec/bli-kvm-control-broker" | cut -d ' ' -f 1)" = "$expected_broker_sha" ]; then
    extracted=yes
    if grep -rq 'hid/lease' "$root/share/web" && grep -rq 'application-power' "$root/share/web"; then
      web_ready=yes
    fi
  fi
fi

if [ "$extracted" = yes ]; then
  chmod 0755 "$root"
  install -d -m 0750 -o hardened-blikvm-control -g hardened-blikvm "$root/state"
  install -d -m 0700 -o hardened-blikvm-server -g hardened-blikvm "$root/server-state"
  if [ -f /var/lib/hardened-blikvm/account.json ]; then
    install -m 0600 -o hardened-blikvm-server -g hardened-blikvm /var/lib/hardened-blikvm/account.json "$root/server-state/account.json"
  fi
  install -d -m 0755 -o www-data -g www-data "$root/nginx"
  install -d -m 0700 -o www-data -g www-data "$root/nginx/client" "$root/nginx/proxy"

  cat > /run/systemd/system/hardened-blikvm-rc-broker.service <<'UNIT'
{broker_unit}UNIT
  cat > /run/systemd/system/hardened-blikvm-rc-server.service <<'UNIT'
{server_unit}UNIT
  systemctl daemon-reload
  rm -f "$root/state/control.sock"
  systemctl reset-failed hardened-blikvm-rc-broker.service >/dev/null 2>&1 || true
  systemctl start hardened-blikvm-rc-broker.service >"$root/broker.log" 2>&1 || true
  i=0
  while [ "$i" -lt 80 ]; do
    if [ -S "$root/state/control.sock" ]; then break; fi
    sleep 0.1
    i=$((i + 1))
  done
  if [ -S "$root/state/control.sock" ]; then control_socket_present=yes; fi
  if [ "$(systemctl is-active hardened-blikvm-rc-broker.service 2>/dev/null || true)" = active ]; then broker_started=yes; fi

  systemctl start hardened-blikvm-rc-server.service >>"$root/broker.log" 2>&1 || true
  i=0
  while [ "$i" -lt 80 ]; do
    if curl -sf "http://127.0.0.1:$port/api/health" >/dev/null 2>&1; then break; fi
    sleep 0.1
    i=$((i + 1))
  done
  if curl -sf "http://127.0.0.1:$port/api/health" >/dev/null 2>&1; then server_started=yes; health=ok; fi
  caps=$(curl -sf "http://127.0.0.1:$port/api/capabilities" 2>/dev/null || printf '{{}}')
  hid=$(printf '%s' "$caps" | python3 -c 'import sys,json
try:
    print(json.load(sys.stdin)["data"]["hid"]["status"]["availability"])
except Exception:
    print("unavailable")' 2>/dev/null || printf 'unavailable')
  power=$(printf '%s' "$caps" | python3 -c 'import sys,json
try:
    print(json.load(sys.stdin)["data"]["power"]["status"]["availability"])
except Exception:
    print("unavailable")' 2>/dev/null || printf 'unavailable')

  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes \
    -keyout "$root/nginx/server.key" -out "$root/nginx/server.crt" \
    -days 2 -subj "/CN=blikvm-rc-test" >/dev/null 2>&1 || true
  chown root:www-data "$root/nginx/server.key" "$root/nginx/server.crt" 2>/dev/null || true
  chmod 0640 "$root/nginx/server.key" 2>/dev/null || true
  chmod 0644 "$root/nginx/server.crt" 2>/dev/null || true
  cat > "$root/nginx/nginx.conf" <<'NGINXCONF'

{nginx_conf}NGINXCONF
  cat > /run/systemd/system/hardened-blikvm-rc-nginx.service <<'UNIT'
{nginx_unit}UNIT
  systemctl daemon-reload
  systemctl start hardened-blikvm-rc-nginx.service >>"$root/broker.log" 2>&1 || true
  i=0
  while [ "$i" -lt 80 ]; do
    if curl -skf "https://127.0.0.1:$https_port/api/health" >/dev/null 2>&1; then break; fi
    sleep 0.1
    i=$((i + 1))
  done
  if curl -skf "https://127.0.0.1:$https_port/api/health" >/dev/null 2>&1; then https=yes; fi
fi

emit extracted "$extracted"
emit web_ready "$web_ready"
emit broker_started "$broker_started"
emit control_socket_present "$control_socket_present"
emit server_started "$server_started"
emit health "$health"
emit hid_availability "$hid"
emit power_availability "$power"
emit https_ready "$https"
"""


def build_teardown_script(runtime_root: str) -> str:
    root = shlex.quote(runtime_root)
    return f"""set -eu
root={root}
emit() {{ printf '%s=%s\\n' "$1" "$2"; }}
for unit in hardened-blikvm-rc-nginx hardened-blikvm-rc-server hardened-blikvm-rc-broker; do
  systemctl stop "$unit.service" 2>/dev/null || true
done
broker_stopped=yes
server_stopped=yes
https_stopped=yes
for unit in hardened-blikvm-rc-broker hardened-blikvm-rc-server hardened-blikvm-rc-nginx; do
  if systemctl is-active "$unit.service" >/dev/null 2>&1; then
    case "$unit" in
      *broker) broker_stopped=no ;;
      *server) server_stopped=no ;;
      *nginx) https_stopped=no ;;
    esac
  fi
done
rm -f /run/systemd/system/hardened-blikvm-rc-broker.service \\
      /run/systemd/system/hardened-blikvm-rc-server.service \\
      /run/systemd/system/hardened-blikvm-rc-nginx.service
systemctl daemon-reload
emit broker_stopped "$broker_stopped"
emit server_stopped "$server_stopped"
emit https_stopped "$https_stopped"
if [ -e "$root" ]; then rm -rf -- "$root"; fi
if [ ! -e "$root" ]; then emit runtime_absent yes; else emit runtime_absent no; fi
emit units_removed yes
"""


def validate_apply(facts: dict[str, str]) -> None:
    if facts.get("extracted") != "yes":
        raise GateError("apply", "extraction_failed")
    if facts.get("web_ready") != "yes":
        raise GateError("apply", "web_assets_missing_restricted_gui")
    if facts.get("control_socket_present") != "yes" or facts.get("broker_started") != "yes":
        raise GateError("apply", "broker_did_not_start")
    if facts.get("server_started") != "yes" or facts.get("health") != "ok":
        raise GateError("apply", "server_did_not_start")
    if facts.get("hid_availability") != "read_write":
        raise GateError("apply", "hid_capability_not_advertised")
    if facts.get("power_availability") != "read_write":
        raise GateError("apply", "power_capability_not_advertised")
    if facts.get("https_ready") != "yes":
        raise GateError("apply", "https_proxy_not_ready")


def write_receipt(
    output: Path,
    bundle: ProfileBundle,
    *,
    stage: str,
    facts: dict[str, str],
    endpoint: str,
    failure_type: str,
    cleanup_confirmed: bool,
) -> None:
    receipt: Receipt = {
        "bundleSha256": bundle.sha256,
        "stage": stage,
        "facts": facts,
        "endpoint": endpoint,
        "cleanupConfirmed": cleanup_confirmed,
        "failureStage": "" if not failure_type else stage,
        "failureType": failure_type,
        "physicalActionsInvoked": False,
        "persistentChangeScope": "runtime-only",
    }
    write_json_0600_atomic(output, receipt)


def run_teardown(
    target: prototype.RemoteTarget, secret: bytearray, bundle: ProfileBundle, output: Path
) -> int:
    facts = prototype.parse_facts(
        prototype.run_ssh(target, secret, build_teardown_script(bundle.runtime_root), timeout=40).stdout,
        TEARDOWN_KEYS,
    )
    if facts.get("runtime_absent") != "yes":
        write_receipt(
            output,
            bundle,
            stage="teardown",
            facts=facts,
            endpoint="",
            failure_type="runtime_directory_remained",
            cleanup_confirmed=False,
        )
        return 1
    write_receipt(
        output,
        bundle,
        stage="teardown",
        facts=facts,
        endpoint="",
        failure_type="",
        cleanup_confirmed=True,
    )
    return 0


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    expected_sha256 = prototype.validate_sha256(options.expected_sha256)
    bundle = validate_profile_bundle(options.bundle, expected_sha256)
    host, ssh_port = prototype.parse_endpoint(options.endpoint)
    username = prototype.validate_username(options.username)
    known_hosts = prototype.validate_private_file(options.known_hosts)
    target = prototype.RemoteTarget(host, ssh_port, username, known_hosts)
    secret = prototype.read_secret_stdin()
    endpoint_public = f"https://{host}:{HTTPS_PORT}"

    if options.teardown:
        return run_teardown(target, secret, bundle, output)

    paths = prototype.StagePaths(
        run_id="rcontrol00",
        root=bundle.runtime_root,
        marker=f"{bundle.runtime_root}/.hardened-blikvm-owner",
        archive=f"{bundle.runtime_root}/bundle.tar.gz",
        payload=f"{bundle.runtime_root}/payload",
        server=f"{bundle.runtime_root}/payload/bin/bli-kvm-server",
    )
    facts: dict[str, str] = {}
    stage = "upload"
    try:
        if _exists(target, secret, bundle.runtime_root):
            _ = prototype.run_ssh(
                target, secret, build_teardown_script(bundle.runtime_root), timeout=40
            )
        install = prototype.run_ssh(
            target,
            secret,
            "set -eu\ninstall -d -m 0700 " + shlex.quote(bundle.runtime_root),
            timeout=20,
        )
        install.check_returncode()
        with bundle.path.open("rb") as bundle_stream:
            _ = prototype.run_ssh(
                target,
                secret,
                _copy_archive_script(bundle.runtime_root),
                stdin=bundle_stream,
                timeout=120,
            ).check_returncode()

        stage = "apply"
        apply_result = prototype.run_ssh(target, secret, build_apply_script(bundle), timeout=120)
        facts = prototype.parse_facts(apply_result.stdout, APPLY_KEYS)
        apply_result.check_returncode()
        validate_apply(facts)
    except (
        GateError,
        subprocess.SubprocessError,
        UnicodeError,
        ValueError,
    ) as error:
        failure_type = error.failure_type if isinstance(error, GateError) else "remote_command_failed"
        write_receipt(
            output,
            bundle,
            stage=stage,
            facts=facts,
            endpoint=endpoint_public,
            failure_type=failure_type,
            cleanup_confirmed=False,
        )
        return 1

    write_receipt(
        output,
        bundle,
        stage="running",
        facts=facts,
        endpoint=endpoint_public,
        failure_type="",
        cleanup_confirmed=False,
    )
    print(f"restricted-control test profile is running at {endpoint_public} (self-signed)")
    print(f"receipt: {output}")
    return 0


def _copy_archive_script(runtime_root: str) -> str:
    root = shlex.quote(runtime_root)
    return f"""set -eu
umask 077
root={root}
cat > "$root/bundle.tar.gz.part"
[ -s "$root/bundle.tar.gz.part" ]
mv "$root/bundle.tar.gz.part" "$root/bundle.tar.gz"
"""


def _exists(target: prototype.RemoteTarget, secret: bytearray, runtime_root: str) -> bool:
    script = (
        "if [ -e "
        + shlex.quote(runtime_root)
        + " ]; then printf 'present=yes\\n'; else printf 'present=no\\n'; fi"
    )
    result = prototype.run_ssh(target, secret, script, timeout=20)
    return b"present=yes" in (result.stdout or b"")


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError):
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
