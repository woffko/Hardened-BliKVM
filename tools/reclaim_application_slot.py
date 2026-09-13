#!/usr/bin/env python3
"""Reclaim exactly one inventoried unprotected slot through the native manager."""

import argparse
import hashlib
import json
import re
from pathlib import Path
import shlex
import sys
from typing import cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from tools import run_persistent_write_rehearsal as common
from tools.capture_pikvm_baseline import write_json_0600_atomic

PROFILES = {'seq1-for-seq9': (1, 8, 40), 'seq2-for-seq10': (2, 9, 43), 'seq3-for-seq11': (3, 10, 46)}


def reviewed_inventory(inventory: dict[str, object], profile: str) -> dict[str, int]:
    oldest, current, generation = PROFILES[profile]
    raw_slots = inventory['slots']
    common.require(isinstance(raw_slots, list), 'slot list required')
    values = cast(list[object], raw_slots)
    common.require(all(isinstance(value, str) for value in values), 'slot names required')
    slots = cast(list[str], values)
    status = common.native.object_value(inventory['status'])
    active, previous = status['activeSlotId'], status['previousSlotId']
    common.require(len(slots) == len(set(slots)) == 8 and all(
        re.fullmatch(r'app-[0-9]{20}-[a-f0-9]{64}', slot) for slot in slots), 'invalid slot set')
    common.require(sorted(int(slot.split('-')[1]) for slot in slots) == list(range(oldest, current + 1)),
                   'slot sequences differ from reviewed cycle')
    common.require(status['floorSequence'] == current and status['generation'] == generation
        and status['pending'] is None and active == status['confirmedSlotId']
        and isinstance(active, str) and active in slots and active.startswith(f'app-{current:020d}-')
        and isinstance(previous, str) and previous in slots and previous.startswith(f'app-{current - 1:020d}-'),
        'protected baseline differs from reviewed cycle')
    common.require(inventory['proposedDeletion'] == [min(slots)]
        and inventory['retainUnreferencedForOneDeletion'] == 5, 'not the reviewed single oldest slot')
    return {'oldest': oldest, 'current': current, 'generation': generation}

REMOTE = r'''
import hashlib, json, os, pathlib, re, secrets, stat, subprocess, sys
data = json.loads(sys.argv[1]); root = pathlib.Path(data['archive']); baseline = data['inventory']
def require(value, reason):
    if not value: raise ValueError(reason)
require(root.parent == pathlib.Path('/var/lib') and re.fullmatch(r'hardened-blikvm-write-restored-[a-f0-9]{24}', root.name)
    and root.resolve(strict=True) == root and root.stat().st_uid == 0 and stat.S_IMODE(root.stat().st_mode) == 0o700, 'unsafe helper archive')
for name, digest in data['helpers'].items():
    require(re.fullmatch(r'[A-Za-z0-9_.-]+', name), 'unsafe helper name')
    path = root / ('controller.py' if name == 'installed_persistent_write.py' else name)
    require(not path.is_symlink() and path.stat().st_uid == 0 and stat.S_IMODE(path.stat().st_mode) == 0o600
        and hashlib.sha256(path.read_bytes()).hexdigest() == digest, 'helper hash changed')
sys.path.insert(0, str(root))
import enable_update_services as infra
import application_write_observer as observer
infra.upgrade.select_profile('dev7-dev9')
infra.upgrade.installed(root, 'new')
observer.require_closed_admission()
observation = observer.observe()
require(observation.live_workers == 0 and observation.queued_jobs == 0 and not observation.candidate_active
    and not observation.health_token_present and not observation.confirmed_apply_present, 'pipeline is busy')
require(infra.current_status() == baseline['status'] and baseline['status']['pending'] is None, 'status drift')
slots_root = pathlib.Path('/var/lib/hardened-blikvm-slots')
require(set(p.name for p in slots_root.iterdir()) == set(baseline['slots']) | {'current'}, 'slot inventory drift')
protected = {baseline['status'][k] for k in ('activeSlotId', 'confirmedSlotId', 'previousSlotId')} - {None}
unreferenced = sorted(set(baseline['slots']) - protected)
target = baseline['proposedDeletion']
require(len(baseline['slots']) == 8 and target == unreferenced[:1] and len(target) == 1
    and target[0].startswith('app-%020d-' % data['profile']['oldest']), 'not the reviewed single oldest slot')
profile = data['profile']
require((profile['oldest'], profile['current'], profile['generation']) in ((1, 8, 40), (2, 9, 43), (3, 10, 46))
    and sorted(int(n.split('-')[1]) for n in baseline['slots']) == list(range(profile['oldest'], profile['current'] + 1))
    and baseline['status']['floorSequence'] == profile['current']
    and baseline['status']['generation'] == profile['generation'], 'unreviewed GC baseline')
retain = len(unreferenced) - 1
require(retain == baseline['retainUnreferencedForOneDeletion'] == 5, 'retention calculation differs')
def tree_hash(name):
    return hashlib.sha256(json.dumps(infra.upgrade.tree_fingerprint((slots_root / name,)), sort_keys=True).encode()).hexdigest()
require({n: tree_hash(n) for n in baseline['slots']} == baseline['treeSha256'], 'immutable tree changed')
unit = 'blikvm-reclaim-one-' + secrets.token_hex(12) + '.service'
argv = ['systemd-run', '--quiet', '--wait', '--pipe', '--collect', '--unit=' + unit,
    '--property=User=hardened-blikvm-update', '--property=Group=hardened-blikvm', '--property=UMask=0077',
    '--property=NoNewPrivileges=yes', '--property=ProtectSystem=strict', '--property=TimeoutStartSec=infinity',
    '--property=ReadWritePaths=/var/lib/hardened-blikvm-slots /var/lib/hardened-blikvm-update /run/hardened-blikvm-update',
    '--property=LoadCredential=update-supervisor-config:' + str(infra.s.bootstrap.CONFIG),
    str(infra.s.bootstrap.SUPERVISOR), '--config', '/run/credentials/' + unit + '/update-supervisor-config', 'gc', str(retain)]
result = subprocess.run(argv, capture_output=True, timeout=180)
require(result.returncode == 0 and len(result.stdout) < 16384, 'GC result ambiguous; never blindly retry')
value = json.loads(result.stdout)
require(value.get('result') == 'completed' and value.get('deletedSlotIds') == target
    and value.get('abandonedStagingRemoved') == 0 and value.get('slotDeletionsCompleted') == 0, 'unexpected GC result')
remaining = sorted(set(baseline['slots']) - set(target))
require(set(p.name for p in slots_root.iterdir()) == set(remaining) | {'current'}, 'unexpected remaining slots')
require(all(tree_hash(n) == baseline['treeSha256'][n] for n in remaining), 'retained slot changed')
require(infra.current_status() == baseline['status'], 'journal/protected pointers changed')
observer.require_closed_admission()
print(json.dumps({'result': 'passed', 'deletedSlotIds': target, 'remainingSlots': remaining,
    'retainedTreesUnchanged': True, 'statusUnchanged': True, 'writeAdmissionClosed': True}, sort_keys=True))
'''


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile', choices=tuple(PROFILES), default='seq1-for-seq9')
    for name in ('endpoint', 'inventory', 'source-receipt', 'output'): parser.add_argument('--' + name, required=True)
    args = parser.parse_args()
    inventory = json.loads(common.transport.validate_private_file(args.inventory).read_bytes())
    source_raw = common.transport.validate_private_file(args.source_receipt).read_bytes()
    source = json.loads(source_raw)
    common.require(inventory.get('result') == 'audited' and len(inventory['archives']) == 1
        and inventory['archives'][0]['sourceReceiptSha256'] == hashlib.sha256(source_raw).hexdigest(), 'bound inventory required')
    archive = common.validate_result(source['cleaned'], action='cleanup', root=source['guardRoot'])['archiveRoot']
    helpers = {name: hashlib.sha256(Path(__file__).with_name(name).read_bytes()).hexdigest()
        for name in ('installed_persistent_write.py', *common.native.HELPERS)}
    profile = reviewed_inventory(inventory['slotInventory'], args.profile)
    data = {'archive': archive, 'helpers': helpers, 'inventory': inventory['slotInventory'], 'profile': profile}
    output = common.transport.validate_output_path(args.output)
    common.require(not output.exists(), 'fresh reclaim receipt required')
    host, port = common.transport.parse_endpoint(args.endpoint)
    target = common.transport.RemoteTarget(host, port, 'root', common.transport.validate_private_file('evidence/private/ssh-known-hosts'))
    receipt: dict[str, object] = {'result': 'failed', 'submissionAttempted': True, 'profile': args.profile,
        'proposedDeletion': inventory['slotInventory']['proposedDeletion'], 'inventorySha256': hashlib.sha256(json.dumps(inventory, sort_keys=True).encode()).hexdigest()}
    write_json_0600_atomic(output, receipt)
    secret = common.transport.read_secret_stdin()
    try:
        response = common.transport.run_ssh(target, secret, shlex.join(['python3', '-B', '-c', REMOTE, json.dumps(data)]), timeout=240)
        common.require(response.returncode == 0 and len(response.stdout) < 16384, 'reclamation failed or ambiguous; inspect before retry')
        value = common.native.object_value(json.loads(response.stdout))
        common.require(value.get('result') == 'passed', 'reclamation unverified')
        receipt.update(result='passed', device=value)
    finally:
        secret[:] = b'\0' * len(secret)
        write_json_0600_atomic(output, receipt)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
