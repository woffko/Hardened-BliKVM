#!/usr/bin/env python3
"""Reclaim one oldest unprotected slot under a committed retained policy."""

import argparse
import json
from pathlib import Path
import shlex
import sys
from typing import cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from tools import run_persistent_write_rehearsal as common
from tools.capture_pikvm_baseline import write_json_0600_atomic

EXPECTED_OLDEST = "app-00000000000000000003-"

REMOTE = r'''
import hashlib, importlib.util, json, pathlib, re, secrets, subprocess, sys
root = pathlib.Path(sys.argv[1])
sys.path.insert(0, str(root))
spec = importlib.util.spec_from_file_location('installed_persistent_write', str(root / 'controller.py'))
native = importlib.util.module_from_spec(spec)
sys.modules['installed_persistent_write'] = native
spec.loader.exec_module(native)
import enable_update_services as infra
import application_write_observer as observer
def require(value, reason):
    if not value:
        raise ValueError(reason)
infra.upgrade.select_profile('dev7-dev9')
infra.upgrade.installed(root, 'new')
plan = native.load(root)
decision = native.decisions.read(root)
require(decision is not None and decision.outcome == 'committed' and decision.binding == native.binding(plan),
        'committed retained policy required')
status = infra.current_status()
require(status.get('pending') is None and status.get('activeSlotId') == status.get('confirmedSlotId'),
        'status is not settled')
slots_root = pathlib.Path('/var/lib/hardened-blikvm-slots')
slots = sorted(p.name for p in slots_root.iterdir() if p.name != 'current')
require(len(slots) == 8 and all(re.fullmatch(r'app-[0-9]{20}-[a-f0-9]{64}', name) for name in slots),
        'slot inventory differs')
protected = {status[k] for k in ('activeSlotId', 'confirmedSlotId', 'previousSlotId')} - {None}
unreferenced = sorted(set(slots) - protected)
target = unreferenced[:1]
retain = len(unreferenced) - 1
require(len(target) == 1 and target[0].startswith(sys.argv[2]) and retain == 5,
        'not the reviewed single oldest slot')
def tree_hash(name):
    return hashlib.sha256(json.dumps(infra.upgrade.tree_fingerprint((slots_root / name,)), sort_keys=True).encode()).hexdigest()
before = {name: tree_hash(name) for name in slots}
for unit, endpoint in ((native.contract.SOCKETS[0], 'upload'), (native.contract.SOCKETS[1], 'activation')):
    require(observer.upload.s.property_value(unit, 'ActiveState') == 'active'
            and pathlib.Path('/run/hardened-blikvm-update-api', endpoint + '.sock').is_socket(),
            'retained admission is not active')
unit_name = 'blikvm-reclaim-one-' + secrets.token_hex(12) + '.service'
argv = ['systemd-run', '--quiet', '--wait', '--pipe', '--collect', '--unit=' + unit_name,
    '--property=User=hardened-blikvm-update', '--property=Group=hardened-blikvm', '--property=UMask=0077',
    '--property=NoNewPrivileges=yes', '--property=ProtectSystem=strict', '--property=TimeoutStartSec=infinity',
    '--property=ReadWritePaths=/var/lib/hardened-blikvm-slots /var/lib/hardened-blikvm-update /run/hardened-blikvm-update',
    '--property=LoadCredential=update-supervisor-config:' + str(infra.s.bootstrap.CONFIG),
    str(infra.s.bootstrap.SUPERVISOR), '--config', '/run/credentials/' + unit_name + '/update-supervisor-config',
    'gc', str(retain)]
result = subprocess.run(argv, capture_output=True, timeout=180)
require(result.returncode == 0 and len(result.stdout) < 16384, 'GC result ambiguous; never blindly retry')
value = json.loads(result.stdout)
require(value.get('result') == 'completed' and value.get('deletedSlotIds') == target
        and value.get('abandonedStagingRemoved') == 0 and value.get('slotDeletionsCompleted') == 0,
        'unexpected GC result')
remaining = sorted(set(slots) - set(target))
require(set(p.name for p in slots_root.iterdir()) == set(remaining) | {'current'}, 'unexpected remaining slots')
require(all(tree_hash(name) == before[name] for name in remaining), 'retained slot changed')
after = infra.current_status()
require(after == status, 'journal or protected pointers changed')
for unit, endpoint in ((native.contract.SOCKETS[0], 'upload'), (native.contract.SOCKETS[1], 'activation')):
    require(observer.upload.s.property_value(unit, 'ActiveState') == 'active'
            and pathlib.Path('/run/hardened-blikvm-update-api', endpoint + '.sock').is_socket(),
            'retained admission changed')
print(json.dumps({'result': 'passed', 'deletedSlotIds': target, 'remainingSlots': remaining,
    'retainedTreesUnchanged': True, 'statusUnchanged': True, 'admissionRetained': True,
    'decisionSha256': hashlib.sha256((root / 'decision.json').read_bytes()).hexdigest(),
    'planSha256': hashlib.sha256((root / 'plan.json').read_bytes()).hexdigest()}, sort_keys=True))
'''


def select_oldest(slots: list[str], status: dict[str, object], expected_oldest: str) -> list[str]:
    protected = {status[key] for key in ("activeSlotId", "confirmedSlotId", "previousSlotId")} - {None}
    unreferenced = sorted(set(slots) - protected)
    target = unreferenced[:1]
    common.require(len(slots) == 8 and len(target) == 1 and target[0].startswith(expected_oldest),
                   "not the reviewed single oldest slot")
    common.require(len(unreferenced) - 1 == 5, "retention calculation differs")
    return target


def validate_result(value: object, target: list[str]) -> dict[str, object]:
    result = common.native.object_value(value)
    common.require(result.get("result") == "passed" and result.get("deletedSlotIds") == target
                   and result.get("retainedTreesUnchanged") is True and result.get("statusUnchanged") is True
                   and result.get("admissionRetained") is True, "retained reclamation unverified")
    for name in ("decisionSha256", "planSha256"):
        digest = result.get(name)
        common.require(isinstance(digest, str) and common.native.re.fullmatch(r"[a-f0-9]{64}", digest) is not None,
                       "invalid retained evidence digest")
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("endpoint", "guard-root", "output"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args()
    root = Path(args.guard_root)
    common.native.contract.guard_units(root)
    output = common.transport.validate_output_path(args.output)
    common.require(not output.exists(), "fresh retained reclamation receipt required")
    host, port = common.transport.parse_endpoint(args.endpoint)
    target = common.transport.RemoteTarget(host, port, "root",
        common.transport.validate_private_file("evidence/private/ssh-known-hosts"))
    receipt: dict[str, object] = {"probe": "retained-slot-reclamation", "result": "failed",
        "submissionAttempted": True, "guardRoot": str(root)}
    write_json_0600_atomic(output, receipt)
    secret = common.transport.read_secret_stdin()
    try:
        response = common.transport.run_ssh(target, secret,
            shlex.join(["python3", "-B", "-c", REMOTE, str(root), EXPECTED_OLDEST]), timeout=240)
        common.require(response.returncode == 0 and len(response.stdout) < 16384,
                       "retained reclamation failed or ambiguous; inspect before retry")
        device = common.native.object_value(json.loads(response.stdout))
        raw_deleted = device.get("deletedSlotIds")
        common.require(isinstance(raw_deleted, list), "invalid deleted slot list")
        deleted = [item for item in cast(list[object], raw_deleted)]
        targets = [item for item in deleted if isinstance(item, str)]
        common.require(len(targets) == len(deleted), "invalid deleted slot list")
        receipt.update(result="passed", device=validate_result(device, targets))
    finally:
        secret[:] = b"\0" * len(secret)
        write_json_0600_atomic(output, receipt)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
