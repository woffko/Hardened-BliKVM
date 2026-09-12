#!/usr/bin/env python3
"""Remove hash-bound pending-test staging after verified native cleanup."""

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
data = json.loads(sys.argv[1]); stage = pathlib.Path(data['stage'])
def require(value):
    if not value: raise ValueError('staging ownership not proved')
require(stage.parent == pathlib.Path('/tmp') and re.fullmatch(r'blikvm-persistent-write-[a-f0-9]{24}', stage.name))
transaction = stage.name.rsplit('-', 1)[1]
archive = pathlib.Path('/var/lib/hardened-blikvm-write-restored-' + transaction)
require(str(archive) == data['archive'] and archive.resolve(strict=True) == archive)
plan = json.loads((archive / 'plan.json').read_text())
require(plan['phase'] == 'restored' and plan['cleanupStarted'] is True)
require(stage.resolve(strict=True) == stage and stage.stat().st_uid == 0 and stat.S_IMODE(stage.stat().st_mode) == 0o700)
names = set(data['hashes']) | {'pending-result.json'}
require(len(names) <= 64 and all(re.fullmatch(r'[A-Za-z0-9_.-]+', n) for n in names))
require(set(p.name for p in stage.iterdir()) == names)
for name in sorted(names):
    path = stage / name; meta = path.lstat()
    require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1 and stat.S_IMODE(meta.st_mode) == 0o600)
    raw = path.read_bytes()
    if name == 'pending-result.json': require(json.loads(raw) == data['pending'])
    else: require(hashlib.sha256(raw).hexdigest() == data['hashes'][name])
for name in sorted(names): (stage / name).unlink()
stage.rmdir()
print(json.dumps({'result': 'passed', 'stagingRemoved': True, 'filesRemoved': len(names), 'recoveryArchivePreserved': archive.is_dir()}))
'''


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('endpoint', 'receipt', 'recovery', 'output'): parser.add_argument('--' + name, required=True)
    args = parser.parse_args()
    raw = common.transport.validate_private_file(args.receipt).read_bytes()
    source = json.loads(raw)
    recovery = json.loads(common.transport.validate_private_file(args.recovery).read_bytes())
    common.require(recovery.get('result') == 'passed' and recovery.get('runtimeWritesClosed') is True
        and recovery.get('sourceReceiptSha256') == hashlib.sha256(raw).hexdigest()
        and recovery.get('guardRoot') == source.get('guardRoot'), 'bound successful recovery required')
    cleanup = common.validate_result(recovery['cleanup'], action='cleanup', root=source['guardRoot'])
    data = {'stage': source['stageRoot'], 'hashes': source['sourceHashes'], 'pending': source['pending'], 'archive': cleanup['archiveRoot']}
    output = common.transport.validate_output_path(args.output)
    common.require(not output.exists(), 'fresh cleanup receipt required')
    host, port = common.transport.parse_endpoint(args.endpoint)
    target = common.transport.RemoteTarget(host, port, 'root', common.transport.validate_private_file('evidence/private/ssh-known-hosts'))
    receipt: dict[str, object] = {'result': 'failed', 'submitted': True, 'stageRoot': source['stageRoot']}
    write_json_0600_atomic(output, receipt)
    secret = common.transport.read_secret_stdin()
    try:
        result = common.transport.run_ssh(target, secret, shlex.join(['python3', '-B', '-c', REMOTE, json.dumps(data)]), timeout=60)
        common.require(result.returncode == 0 and len(result.stdout) < 4096, 'cleanup failed; inspect before retry')
        value = common.native.object_value(json.loads(result.stdout))
        common.require(value.get('result') == 'passed', 'cleanup unverified')
        receipt.update(result='passed', device=value)
    finally:
        secret[:] = b'\0' * len(secret)
        write_json_0600_atomic(output, receipt)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
