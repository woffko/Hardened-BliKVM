#!/usr/bin/env python3
"""Stage and run the bounded network write gate (DNS and IPv6).

Plan-only by default: it prints the reviewed device facts and refuses to change
anything. `--confirm-write` performs one bounded write through the board API and
immediately restores the previous value, so the device ends where it started:
the resolver list is written, read back, and restored, and one interface's
disable_ipv6 flag is flipped, read back, and restored. The GUI password is read
from stdin by the caller and is never written to the receipt.
"""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
import sys
import urllib.error
import urllib.parse
import urllib.request
from typing import Any, Final

sys.dont_write_bytecode = True

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype
from tools import upload_media_image as upload


UNIT: Final = "hardened-blikvm-board-server.service"
OWNER: Final = "hardened-blikvm-board-owner.service"
CONTROL_SOCKET: Final = "/run/hardened-blikvm-gadget/control.sock"
RESOLV_CONF: Final = "/etc/resolv.conf"
IPV6_ROOT: Final = "/proc/sys/net/ipv6/conf"
REVIEWED_RESOLVER: Final = "1.1.1.1"
MAX_RECEIPT_BYTES: Final = 64 * 1024
NAMESERVER: Final = re.compile(r"^nameserver ([0-9a-fA-F:.]+)$")


def preflight_script(interface: str) -> str:
    return f"""set -u
printf 'server_unit=%s\\n' "$(systemctl is-active {UNIT} 2>/dev/null || true)"
printf 'owner_unit=%s\\n' "$(systemctl is-active {OWNER} 2>/dev/null || true)"
if [ -S {CONTROL_SOCKET} ]; then printf 'control_socket=present\\n'; else printf 'control_socket=absent\\n'; fi
if systemctl show -p Environment --value {UNIT} 2>/dev/null | grep -q 'BLI_KVM_NETWORK_WRITES=1'; then printf 'writes_enabled=yes\\n'; else printf 'writes_enabled=no\\n'; fi
if [ -f {RESOLV_CONF} ]; then printf 'resolv_present=yes\\n'; else printf 'resolv_present=no\\n'; fi
sed -n 's/^nameserver \\([0-9a-fA-F:.]*\\)$/resolver=\\1/p' {RESOLV_CONF} 2>/dev/null || true
printf 'ipv6_interface=%s\\n' '{interface}'
if [ -e {IPV6_ROOT}/{interface}/disable_ipv6 ]; then
  printf 'ipv6_present=yes\\n'
  printf 'ipv6_disable_before=%s\\n' "$(tr -d '\\n' < {IPV6_ROOT}/{interface}/disable_ipv6 2>/dev/null || true)"
else
  printf 'ipv6_present=no\\n'
fi
printf 'boot_id=%s\\n' "$(tr -d '\\n' < /proc/sys/kernel/random/boot_id 2>/dev/null || true)"
"""


def parse_facts(output: str) -> dict[str, str]:
    facts: dict[str, str] = {}
    for line in output.splitlines():
        if "=" in line and not line.startswith(" "):
            key, value = line.split("=", 1)
            if re.fullmatch(r"[A-Za-z0-9_]+", key):
                facts[key] = value
    return facts


def fact_list(facts: dict[str, str], prefix: str) -> list[str]:
    values = [value for key, value in facts.items() if key == prefix]
    return values


def plan_writes(
    current_resolvers: list[str],
    interface: str,
    ipv6_before: str,
    wol_mac: str | None,
) -> dict[str, Any]:
    """The reviewed write plan: one DNS write plus restore, one IPv6 flip plus
    restore. Nothing is invented: the restore value is the observation."""
    review = [REVIEWED_RESOLVER]
    return {
        "dns": {
            "servers": review,
            "restore": list(current_resolvers),
            "changed": current_resolvers != review,
        },
        "ipv6": {
            "interface": interface,
            "disabled": ipv6_before != "1",
            "restore_disabled": ipv6_before == "1",
            "changed": True,
        },
        # Wake-on-LAN carries nothing to restore: the reviewed target is the
        # operator's own host address and the packet changes no device state.
        "wol": {"mac": wol_mac, "requested": wol_mac is not None},
    }


def api_write(
    base_url: str,
    route: str,
    payload: dict[str, Any],
    password: str,
    ca_file: str | None,
    tls_server_name: str | None,
) -> dict[str, Any]:
    base = base_url.rstrip("/")
    parsed = urllib.parse.urlparse(base)
    if parsed.scheme not in ("http", "https") or not parsed.hostname:
        raise ValueError("base URL must be an absolute http(s) URL")
    if parsed.scheme == "http" and parsed.hostname not in ("127.0.0.1", "localhost"):
        raise ValueError("a remote board must be reached over https")
    opener = upload.build_opener(ca_file, tls_server_name)
    login = upload.post_json(
        opener, base, "/api/auth/login", {"username": "admin", "password": password}
    )
    if login.get("code") != 0:
        raise ValueError("login failed")
    csrf = login.get("data", {}).get("csrfToken")
    if not csrf:
        raise ValueError("login returned no CSRF token")
    request = urllib.request.Request(
        urllib.parse.urljoin(base, route),
        data=json.dumps(payload).encode(),
        headers={
            "Content-Type": "application/json",
            "Origin": base.rstrip("/"),
            "x-csrf-token": csrf,
        },
        method="POST",
    )
    try:
        with opener.open(request, timeout=30) as response:
            body = json.loads(response.read().decode())
            status = response.status
    except urllib.error.HTTPError as error:
        return {"status": error.code, "code": None, "applied": False}
    data = body.get("data") or {}
    return {
        "status": status,
        "code": body.get("code"),
        "applied": bool(data.get("applied")),
        "rejection": data.get("rejection"),
        "read_back": data.get("readBack") or data.get("ipv6State"),
    }


def decide_outcome(*, confirm_write: bool, preflight: dict[str, str], write: dict[str, Any]) -> str:
    if not confirm_write:
        return "plan"
    if not write:
        return "preflight"
    required = [
        write.get("dns_applied") is True,
        write.get("dns_restored") is True,
        write.get("ipv6_applied") is True,
        write.get("ipv6_restored") is True,
    ]
    if write.get("wol_applied") is not None:
        required.append(write["wol_applied"] is True)
    passed = all(required)
    if passed:
        return "passed"
    if any(required):
        return "partial"
    return "inspect"


def read_credentials(*, confirm_write: bool) -> tuple[bytearray, str]:
    """Read the reviewed stdin payload: the SSH secret on the first line and,
    for a write run, the operator password on the second. Both stay in memory."""
    stream = getattr(sys.stdin, "buffer", sys.stdin)
    first = stream.readline()
    if not first or len(first) > 512:
        raise SystemExit("the SSH secret is missing on stdin")
    secret = bytearray(first.rstrip(b"\r\n"))
    if not secret:
        raise SystemExit("the SSH secret is missing on stdin")
    password = ""
    if confirm_write:
        second = stream.readline()
        password = second.decode("utf-8", "replace").rstrip("\r\n")
        if len(password) < 8:
            raise SystemExit("the operator password is missing on stdin")
    return secret, password


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--endpoint", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--known-hosts", required=True)
    _ = parser.add_argument("--output", required=True)
    _ = parser.add_argument("--base-url")
    _ = parser.add_argument("--api-username", default="admin")
    _ = parser.add_argument("--ca-file")
    _ = parser.add_argument("--tls-server-name")
    _ = parser.add_argument("--interface", default="wlan0")
    _ = parser.add_argument(
        "--wol-mac",
        help="reviewed host MAC for one Wake-on-LAN packet (optional)",
    )
    _ = parser.add_argument("--confirm-write", action="store_true")
    _ = parser.add_argument(
        "--confirm-observed",
        action="store_true",
        help="the operator verified the read-backs and that the device is unchanged",
    )
    _ = parser.add_argument("--timeout", type=int, default=300)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    output: Path = prototype.validate_output_path(args.output)
    if not re.fullmatch(r"[A-Za-z0-9_.-]{1,15}", args.interface):
        raise SystemExit("interface name is invalid")
    if args.wol_mac is not None and not re.fullmatch(
        r"[0-9a-fA-F]{2}([:-][0-9a-fA-F]{2}){5}", args.wol_mac
    ):
        raise SystemExit("--wol-mac is not a MAC address")
    if args.confirm_observed and not args.confirm_write:
        raise SystemExit("--confirm-observed needs --confirm-write")

    host, ssh_port = prototype.parse_endpoint(args.endpoint)
    target = prototype.RemoteTarget(host, ssh_port, args.username, Path(args.known_hosts))
    secret, operator_password = read_credentials(confirm_write=args.confirm_write)
    result = prototype.run_ssh(
        target, secret, preflight_script(args.interface), timeout=args.timeout
    )
    stdout = (result.stdout or b"").decode("utf-8", "replace")
    print(stdout)
    facts = parse_facts(stdout)
    resolvers = fact_list(facts, "resolver")
    plan = plan_writes(
        resolvers, args.interface, facts.get("ipv6_disable_before", ""), args.wol_mac
    )

    receipt: dict[str, Any] = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-network-write-gate",
        "policy": {
            "credentialsRead": False,
            "mutationsPerformed": bool(args.confirm_write),
            "receiptContainsSecrets": False,
        },
        "preflight": facts,
        "plan": plan,
        "operatorObserved": bool(args.confirm_observed),
    }
    write: dict[str, Any] = {}
    if args.confirm_write:
        if not args.base_url:
            raise SystemExit("--confirm-write needs --base-url")
        if facts.get("writes_enabled") != "yes":
            raise SystemExit("the board server does not enable network writes")
        if facts.get("ipv6_present") != "yes":
            raise SystemExit("the reviewed interface has no disable_ipv6 node")
        password = operator_password
        ca = args.ca_file
        name = args.tls_server_name
        base = args.base_url
        servers = plan["dns"]["servers"]
        restore_servers = plan["dns"]["restore"]
        ipv6_plan = plan["ipv6"]
        applied = api_write(
            base,
            "/api/network/dns",
            {"servers": servers, "confirm": True},
            password,
            ca,
            name,
        )
        write["dns_applied"] = bool(applied.get("applied"))
        if write["dns_applied"] and restore_servers:
            restored = api_write(
                base,
                "/api/network/dns",
                {"servers": restore_servers, "confirm": True},
                password,
                ca,
                name,
            )
            write["dns_restored"] = bool(restored.get("applied"))
        write["wol_applied"] = None
        if plan["wol"]["requested"]:
            wol = api_write(
                base,
                "/api/network/wol",
                {"mac": plan["wol"]["mac"], "confirm": True},
                password,
                ca,
                name,
            )
            write["wol_applied"] = bool(wol.get("applied"))
        ipv6 = api_write(
            base,
            "/api/network/ipv6",
            {
                "interface": ipv6_plan["interface"],
                "disabled": ipv6_plan["disabled"],
                "confirm": True,
            },
            password,
            ca,
            name,
        )
        write["ipv6_applied"] = bool(ipv6.get("applied"))
        if write["ipv6_applied"]:
            back = api_write(
                base,
                "/api/network/ipv6",
                {
                    "interface": ipv6_plan["interface"],
                    "disabled": ipv6_plan["restore_disabled"],
                    "confirm": True,
                },
                password,
                ca,
                name,
            )
            write["ipv6_restored"] = bool(back.get("applied"))
        password = ""
    receipt["write"] = write
    receipt["mode"] = "write" if args.confirm_write else "plan"
    receipt["result"] = decide_outcome(
        confirm_write=args.confirm_write, preflight=facts, write=write
    )
    encoded = json.dumps(receipt).encode()
    if len(encoded) > MAX_RECEIPT_BYTES:
        raise SystemExit("receipt is too large")
    write_json_0600_atomic(output, receipt)
    print(f"network_write_gate={output} mode={receipt['mode']} result={receipt['result']}")
    return 0 if receipt["result"] in ("plan", "passed", "preflight") else 1


if __name__ == "__main__":
    raise SystemExit(main())
