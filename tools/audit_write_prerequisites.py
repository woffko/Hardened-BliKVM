#!/usr/bin/env python3
"""Read archived rehearsal provenance; never qualify or commit a write policy."""

import argparse
import hashlib
import json
from pathlib import Path
import shlex
import sys

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from tools import run_persistent_write_rehearsal as common
from tools.capture_pikvm_baseline import write_json_0600_atomic

REMOTE = r'''
import hashlib, json, os, pathlib, re, stat, sys
sources = json.loads(sys.argv[1]); results = []
def require(value):
    if not value: raise ValueError('archive provenance mismatch')
def read(path):
    meta = path.lstat()
    require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1
        and stat.S_IMODE(meta.st_mode) == 0o600 and meta.st_size <= 33554432)
    return path.read_bytes()
machine = hashlib.sha256(pathlib.Path('/etc/machine-id').read_bytes().strip()).hexdigest()
require(1 <= len(sources) <= 8)
for source in sources:
    root = pathlib.Path(source['archiveRoot'])
    require(root.parent == pathlib.Path('/var/lib') and re.fullmatch(r'hardened-blikvm-write-restored-[a-f0-9]{24}', root.name)
        and root.resolve(strict=True) == root and root.stat().st_uid == 0 and stat.S_IMODE(root.stat().st_mode) == 0o700)
    raw = read(root / 'plan.json'); plan = json.loads(raw)
    require(plan['phase'] == 'restored' and plan['cleanupStarted'] is True
        and plan['binding'] == source['binding'] and plan['binding']['device_fingerprint'] == machine)
    require(root.name.endswith(plan['binding']['transaction_id']))
    matches = {}
    for name, expected in source['nativeSourceHashes'].items():
        require(re.fullmatch(r'[a-zA-Z0-9_.-]+', name) and re.fullmatch(r'[a-f0-9]{64}', expected))
        archived = 'controller.py' if name == 'installed_persistent_write.py' else name
        matches[name] = hashlib.sha256(read(root / archived)).hexdigest() == expected
    decision_raw = read(root / 'decision.json')
    before = plan['before']
    runtime = before['runtime']
    fields = ('account', 'nginxConfig', 'certificate', 'tlsKey', 'trust')
    runtime_hashes = {name: runtime[name] for name in fields}
    require(all(isinstance(v, str) and re.fullmatch(r'[a-f0-9]{64}', v) for v in runtime_hashes.values()))
    require(re.fullmatch(r'[a-f0-9]{64}', before['serving']['executableSha256']))
    results.append({'action': source['action'], 'sourceReceiptSha256': source['sourceReceiptSha256'],
        'sourceBinding': plan['binding'], 'archivePlanSha256': hashlib.sha256(raw).hexdigest(),
        'archiveDecisionSha256': hashlib.sha256(decision_raw).hexdigest(), 'nativeSourceMatches': matches,
        'packageSha256': hashlib.sha256(read(root / 'new.deb')).hexdigest(),
        'protectedRuntimeHashes': runtime_hashes,
        'servingExecutableSha256': before['serving']['executableSha256']})
facts = {'result': 'audited', 'archives': results, 'retentionApproved': False,
    'commitQualificationPerformed': False, 'readOnly': True}
if len(sys.argv) > 2 and sys.argv[2] == 'slots':
    # Import only the archived helper set whose complete hashes were checked.
    require(all(all(v['nativeSourceMatches'].values()) for v in results))
    sys.path.insert(0, str(root))
    import enable_update_services as infra
    infra.upgrade.select_profile('dev7-dev9')
    status = infra.current_status()
    require(status.get('pending') is None)
    for unit in ('hardened-blikvm-update-upload.socket', 'hardened-blikvm-update-activation.socket'):
        require(infra.s.property_value(unit, 'ActiveState') == 'inactive')
    slots_root = pathlib.Path('/var/lib/hardened-blikvm-slots')
    entries = list(slots_root.iterdir())
    slots = sorted(p.name for p in entries if re.fullmatch(r'app-[0-9]{20}-[a-f0-9]{64}', p.name))
    require(set(p.name for p in entries) == set(slots) | {'current'})
    protected = {status.get(k) for k in ('activeSlotId', 'confirmedSlotId', 'previousSlotId')} - {None}
    require(protected <= set(slots))
    trees = {}
    for name in slots:
        path = slots_root / name
        require(path.is_dir() and not path.is_symlink())
        tree = infra.upgrade.tree_fingerprint((path,))
        trees[name] = hashlib.sha256(json.dumps(tree, sort_keys=True).encode()).hexdigest()
    unreferenced = [name for name in slots if name not in protected]
    require(status == infra.current_status())
    facts['slotInventory'] = {'status': status, 'slots': slots, 'protectedSlots': sorted(protected),
        'unreferencedSlotsOldestFirst': unreferenced, 'treeSha256': trees,
        'proposedDeletion': unreferenced[:1], 'retainUnreferencedForOneDeletion': max(0, len(unreferenced) - 1)}
print(json.dumps(facts, sort_keys=True))
'''


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--endpoint', required=True)
    parser.add_argument('--receipt', action='append', required=True)
    parser.add_argument('--output', required=True)
    parser.add_argument('--slot-inventory', action='store_true')
    args = parser.parse_args()
    names = ('installed_persistent_write.py', *common.native.HELPERS)
    current = {name: hashlib.sha256(Path(__file__).with_name(name).read_bytes()).hexdigest() for name in names}
    sources: list[dict[str, object]] = []
    for value in args.receipt:
        raw = common.transport.validate_private_file(value).read_bytes()
        receipt = json.loads(raw)
        common.require(receipt.get('result') == 'passed' and receipt.get('runtimeWritesClosed') is True
            and receipt.get('guardRetained') is False and receipt.get('controllerStagingRemoved') is True,
            'completed restored rehearsal required')
        cleanup = common.validate_result(receipt['cleaned'], action='cleanup', root=receipt['guardRoot'])
        common.require(all(receipt['sourceHashes'].get(name) == sha for name, sha in current.items()), 'native source changed since receipt')
        sources.append({'action': receipt['action'], 'archiveRoot': cleanup['archiveRoot'],
            'binding': receipt['prepared']['binding'], 'sourceReceiptSha256': hashlib.sha256(raw).hexdigest(),
            'nativeSourceHashes': current})
    output = common.transport.validate_output_path(args.output)
    common.require(not output.exists(), 'fresh provenance audit receipt required')
    host, port = common.transport.parse_endpoint(args.endpoint)
    target = common.transport.RemoteTarget(host, port, 'root', common.transport.validate_private_file('evidence/private/ssh-known-hosts'))
    result: dict[str, object] = {'result': 'failed', 'readOnly': True}
    secret = common.transport.read_secret_stdin()
    try:
        response = common.transport.run_ssh(target, secret,
            shlex.join(['python3', '-B', '-c', REMOTE, json.dumps(sources), 'slots' if args.slot_inventory else 'archives']), timeout=180)
        common.require(response.returncode == 0 and len(response.stdout) <= 262144, 'archive audit failed')
        result = common.native.object_value(json.loads(response.stdout))
        common.require(result.get('result') == 'audited', 'archive audit unverified')
    finally:
        secret[:] = b'\0' * len(secret)
        write_json_0600_atomic(output, result)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
