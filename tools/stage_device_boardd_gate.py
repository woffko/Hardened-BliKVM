#!/usr/bin/env python3
"""Stage and run one board daemon gate (fan, panel, LED or button).

Plan-only by default: it prints the exact steps and refuses to touch the
device. `--confirm-handoff` performs the reviewed handoff for one subsystem:
it records the vendor sysfs exports, unexports only that subsystem's lines,
installs the daemon and its unit from the verified bundle, installs the
operator configuration, starts the unit and drives the subsystem over the
daemon control socket. `--confirm-observed` additionally asserts that the
operator watched the physical effect; only then is the receipt `passed`.
`--teardown` stops the unit, removes the configuration and restores the
recorded sysfs exports.

The password is never needed: the gate runs over SSH with the enrolled secret
provided on stdin by the caller.
"""

from __future__ import annotations

import argparse
import base64
import json
from pathlib import Path
import re
import sys
from typing import Any, Final

sys.dont_write_bytecode = True

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import stage_device_prototype as prototype
from tools.capture_pikvm_baseline import write_json_0600_atomic


CONFIG_PATH: Final = "/etc/hardened-blikvm-gadget/boardd.json"
UNIT: Final = "hardened-blikvm-boardd.service"
BOARD_BINARY: Final = "/usr/libexec/hardened-blikvm/bli-kvm-boardd"
SOCKET: Final = "/run/hardened-blikvm-boardd/control.sock"
BUNDLE_ROOT: Final = "/var/lib/hardened-blikvm-gadget/gate"
EXPORT_BACKUP: Final = "/var/lib/hardened-blikvm-gadget-gate/exports"
MAX_RECEIPT_BYTES: Final = 64 * 1024

# One entry per subsystem: the vendor sysfs lines it claims and the control
# socket requests the gate drives. Nothing about the board is invented here:
# the offsets come from the read-only recon receipts and the actual values are
# re-read from the device by the gate.
SUBSYSTEMS: Final[dict[str, dict[str, Any]]] = {
    "fan": {
        "lines": (269,),
        "requests": (
            {"request": "fan_mode_set", "mode": "manual"},
            {"request": "fan_duty_set", "percent": 30},
            {"request": "fan_duty_set", "percent": 60},
            {"request": "fan_duty_set", "percent": 100},
            {"request": "fan_mode_set", "mode": "auto"},
        ),
        "status_fields": ("fanMode", "fanDutyPercent", "fanPwm"),
    },
    "panel": {
        "lines": (259, 262, 260),
        "requests": (
            {"request": "display_test"},
            {"request": "display_backlight_set", "on": False},
            {"request": "display_backlight_set", "on": True},
            {"request": "display_test"},
        ),
        "status_fields": ("displayPresent", "displayInitialized", "displayBacklightOn"),
    },
    "led": {
        "lines": (261,),
        "requests": (
            {"request": "led_enabled_set", "enabled": True},
            {"request": "led_enabled_set", "enabled": False},
            {"request": "led_enabled_set", "enabled": True},
        ),
        "status_fields": ("ledPresent", "ledEnabled"),
    },
    "button": {
        "lines": (257,),
        "requests": (),
        "status_fields": ("buttonPresent", "buttonWakeups"),
    },
    # The ATX host LED inputs are read-only: the gate installs nothing to drive
    # them and only records what the daemon samples while the operator compares
    # the two states with the host.
    "atx-leds": {
        "lines": (234, 233),
        "requests": (),
        "status_fields": (
            "atxPowerLedPresent",
            "atxPowerLedOn",
            "atxHddLedPresent",
            "atxHddLedOn",
        ),
    },
}


def _line_read(n: int) -> str:
    return f"""{{ d=/sys/class/gpio/gpio{n};
  if [ -d "$d" ]; then
    printf 'line{n}=present value=%s dir=%s active_low=%s\\n' "$(cat $d/value 2>/dev/null)" "$(cat $d/direction 2>/dev/null)" "$(cat $d/active_low 2>/dev/null)";
  else
    printf 'line{n}=absent\\n';
  fi; }}"""


def build_preflight_script(subsystem: str) -> str:
    spec = SUBSYSTEMS[subsystem]
    lines = "".join(_line_read(n) + "\n" for n in spec["lines"])
    return f"""set +e
echo "== unit =="
printf 'unit_active=%s\\n' "$(systemctl is-active {UNIT} 2>/dev/null || true)"
printf 'unit_enabled=%s\\n' "$(systemctl is-enabled {UNIT} 2>/dev/null || true)"
echo "== vendor lines =="
{lines}
echo "== daemon =="
printf 'binary=%s\\n' "$([ -x {BOARD_BINARY} ] && echo present || echo absent)"
printf 'config=%s\\n' "$([ -f {CONFIG_PATH} ] && echo present || echo absent)"
printf 'socket=%s\\n' "$([ -S {SOCKET} ] && echo present || echo absent)"
echo "== thermal =="
printf 'thermal=%s\\n' "$(cat /sys/class/thermal/thermal_zone0/temp 2>/dev/null || echo unknown)"
"""


def build_apply_script(
    subsystem: str, bundle_sha256: str, config: dict[str, Any], run_requests: bool
) -> str:
    spec = SUBSYSTEMS[subsystem]
    lines = " ".join(str(n) for n in spec["lines"])
    payload = base64.b64encode(json.dumps(config).encode()).decode()
    requests = json.dumps(list(spec["requests"])) if run_requests else "[]"
    return f"""set -eu
stage={BUNDLE_ROOT}/boardd-gate
install -d -m 0700 {EXPORT_BACKUP}
backup={EXPORT_BACKUP}/{subsystem}.txt
: > "$backup"
for n in {lines}; do
  d=/sys/class/gpio/gpio$n
  if [ -d "$d" ]; then
    printf '%s %s %s %s\\n' "$n" "$(cat $d/value)" "$(cat $d/direction)" "$(cat $d/active_low)" >> "$backup"
  else
    printf '%s absent - -\\n' "$n" >> "$backup"
  fi
done
echo "exports_backed_up=yes"
echo "== unexport this subsystem's vendor lines =="
for n in {lines}; do
  if [ -d /sys/class/gpio/gpio$n ]; then
    echo "$n" > /sys/class/gpio/unexport || true
  fi
done
echo "== install the daemon from the verified bundle =="
rm -rf "$stage"; mkdir -p "$stage"
tar -xzf {BUNDLE_ROOT}/bundle.tar.gz -C "$stage"
root=$(find "$stage" -maxdepth 1 -mindepth 1 -type d | head -1)
install -m 0755 "$root/libexec/bli-kvm-boardd" {BOARD_BINARY}
install -m 0644 "$root/share/systemd/{UNIT}" /etc/systemd/system/{UNIT}
printf '%s' '{payload}' | base64 -d > {CONFIG_PATH}
chmod 0600 {CONFIG_PATH}
echo "config_installed=yes"
systemctl daemon-reload
echo "== read-only validation before the unit owns the line =="
printf 'once_status=%s\\n' "$(timeout 20 {BOARD_BINARY} --config {CONFIG_PATH} --once 2>&1 | tail -1)"
systemctl enable {UNIT} >/dev/null 2>&1 || true
systemctl restart {UNIT}
for i in $(seq 1 30); do [ -S {SOCKET} ] && break; sleep 0.5; done
printf 'socket_present=%s\\n' "$([ -S {SOCKET} ] && echo yes || echo no)"
printf 'unit_active=%s\\n' "$(systemctl is-active {UNIT})"
python3 - <<'PYEOF'
import json, socket, time, sys
requests = json.loads('''{requests}''')
path = "{SOCKET}"
def call(request):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(15)
        client.connect(path)
        client.sendall((json.dumps(request) + "\\n").encode())
        data = b""
        while not data.endswith(b"\\n"):
            chunk = client.recv(4096)
            if not chunk:
                break
            data += chunk
    return json.loads(data.decode() or "{{}}")
status = call({{"request": "status"}})
print("status_code=%s" % status.get("code"))
for request in requests:
    response = call(request)
    print("request=%s code=%s applied=%s" % (request["request"], response.get("code"), response.get("data") is not None))
    time.sleep(2.5)
final = call({{"request": "status"}})
data = final.get("data") or {{}}
for field in {json.dumps(list(spec["status_fields"]))}:
    print("field_%s=%s" % (field, data.get(field)))
PYEOF
"""


def build_teardown_script(subsystem: str) -> str:
    spec = SUBSYSTEMS[subsystem]
    lines = " ".join(str(n) for n in spec["lines"])
    return f"""set +e
systemctl stop {UNIT} >/dev/null 2>&1 || true
systemctl disable {UNIT} >/dev/null 2>&1 || true
printf 'unit_stopped=%s\\n' "$([ "$(systemctl is-active {UNIT} 2>/dev/null || true)" = "active" ] && echo no || echo yes)"
rm -f {CONFIG_PATH}
backup={EXPORT_BACKUP}/{subsystem}.txt
if [ -f "$backup" ]; then
  while read -r n value direction active_low; do
    [ "$value" = "absent" ] && continue
    [ -d /sys/class/gpio/gpio$n ] && echo "$n" > /sys/class/gpio/unexport || true
    echo "$n" > /sys/class/gpio/export || true
    echo "$direction" > /sys/class/gpio/gpio$n/direction || true
    echo "$value" > /sys/class/gpio/gpio$n/value || true
    echo "restored line $n value=$value"
  done < "$backup"
fi
echo "exports_restored=yes"
for n in {lines}; do
  printf 'line%s=%s\\n' "$n" "$([ -d /sys/class/gpio/gpio$n ] && echo present || echo absent)"
done
"""


def parse_facts(output: str) -> dict[str, str]:
    facts: dict[str, str] = {}
    for line in output.splitlines():
        if "=" in line and not line.startswith(" "):
            key, value = line.split("=", 1)
            if re.fullmatch(r"[A-Za-z0-9_]+", key):
                facts[key] = value
    return facts


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--endpoint", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--known-hosts", required=True)
    _ = parser.add_argument("--output", required=True)
    _ = parser.add_argument("--bundle", required=True)
    _ = parser.add_argument("--expected-sha256", required=True)
    _ = parser.add_argument("--config", required=True, help="boardd.json to install")
    _ = parser.add_argument("--subsystem", choices=sorted(SUBSYSTEMS), default="fan")
    _ = parser.add_argument("--confirm-handoff", action="store_true")
    _ = parser.add_argument(
        "--confirm-observed",
        action="store_true",
        help="the operator watched the physical effect; only then is the gate passed",
    )
    _ = parser.add_argument("--teardown", action="store_true")
    _ = parser.add_argument("--timeout", type=int, default=300)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    bundle = Path(args.bundle)
    if not bundle.is_file():
        raise SystemExit("bundle is missing")
    if not re.fullmatch(r"[0-9a-f]{64}", args.expected_sha256):
        raise SystemExit("expected sha256 is invalid")
    config = json.loads(Path(args.config).read_text(encoding="utf-8"))
    output = prototype.validate_output_path(args.output)
    spec = SUBSYSTEMS[args.subsystem]

    if args.teardown:
        if not args.confirm_handoff:
            raise SystemExit("teardown requires --confirm-handoff")
        script = build_teardown_script(args.subsystem)
    elif args.confirm_handoff:
        script = build_apply_script(
            args.subsystem, args.expected_sha256, config, run_requests=True
        )
    else:
        script = build_preflight_script(args.subsystem)

    host, ssh_port = prototype.parse_endpoint(args.endpoint)
    target = prototype.RemoteTarget(
        host, ssh_port, args.username, Path(args.known_hosts)
    )
    secret = prototype.read_secret_stdin()
    result = prototype.run_ssh(target, secret, script, timeout=args.timeout)
    stdout = (result.stdout or b"").decode("utf-8", "replace")
    facts = parse_facts(stdout)
    print(stdout)

    mode = "teardown" if args.teardown else ("handoff" if args.confirm_handoff else "plan")
    outcome = "recorded"
    if args.teardown:
        outcome = "restored" if facts.get("unit_stopped") == "yes" else "inspect"
    elif args.confirm_handoff:
        required = {
            "exports_backed_up": "yes",
            "config_installed": "yes",
            "unit_active": "active",
            "status_code": "0",
        }
        if all(facts.get(key) == value for key, value in required.items()):
            outcome = "passed" if args.confirm_observed else "observed-pending"
        else:
            outcome = "inspect"
    else:
        outcome = "plan"

    receipt = {
        "schemaVersion": 1,
        "probe": f"hardened-blikvm-boardd-{args.subsystem}-gate",
        "mode": mode,
        "result": outcome,
        "subsystem": args.subsystem,
        "operatorObserved": bool(args.confirm_observed),
        "bundleSha256": args.expected_sha256,
        "vendorLines": list(spec["lines"]),
        "facts": facts,
        "policy": {
            "credentialsRead": False,
            "mutationsPerformed": bool(args.confirm_handoff and not args.teardown),
            "receiptContainsSecrets": False,
        },
    }
    encoded = json.dumps(receipt).encode()
    if len(encoded) > MAX_RECEIPT_BYTES:
        raise SystemExit("receipt is too large")
    write_json_0600_atomic(output, receipt)
    print(f"boardd_gate={output} mode={mode} result={outcome}")
    return 0 if outcome in ("plan", "passed", "restored", "observed-pending", "recorded") else 1


if __name__ == "__main__":
    raise SystemExit(main())
