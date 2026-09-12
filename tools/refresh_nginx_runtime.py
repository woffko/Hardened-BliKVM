#!/usr/bin/env python3
"""Restart only custom Nginx after checking the reviewed configuration/runtime."""

import argparse
import json
from pathlib import Path
import shlex
import subprocess
import sys

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import run_persistent_write_rehearsal as common
from tools.capture_pikvm_baseline import write_json_0600_atomic

REMOTE = r'''
import hashlib, json, pathlib, subprocess, sys
def require(value, message):
    if not value: raise ValueError(message)
def sha(path):
    with open(path, 'rb') as f:
        h = hashlib.sha256()
        for data in iter(lambda: f.read(1048576), b''): h.update(data)
    return h.hexdigest()
def run(argv):
    return subprocess.run(argv, capture_output=True, timeout=60, check=True)
def state(unit):
    r = run(['systemctl', 'show', unit, '--property=ActiveState,SubState,MainPID'])
    return dict(line.split('=', 1) for line in r.stdout.decode().splitlines() if '=' in line)
unit = 'hardened-blikvm-nginx.service'
config = '/etc/hardened-blikvm/nginx.conf'
if sys.argv[1] == 'inspect':
    services = {name: state(name) for name in (unit, 'ssh.service', 'kvmd.service',
        'hardened-blikvm-server-current.service', 'hardened-blikvm-platform-broker.service',
        'hardened-blikvm-video-proxy.service', 'hardened-blikvm-update-upload.socket',
        'hardened-blikvm-update-activation.socket')}
    pid = int(services[unit].get('MainPID', '0'))
    print(json.dumps({'result': 'passed', 'readOnly': True, 'services': services,
        'configurationSha256': sha(config), 'installedSha256': sha('/usr/sbin/nginx'),
        'runningSha256': sha('/proc/' + str(pid) + '/exe') if pid > 1 else None}, sort_keys=True))
    raise SystemExit(0)
expected = 'a1bf9d746c15988c3806b50f5dc7a3f7a30660d2a7ead7e95d7b0b32340deb16'
require(sha(config) == expected, 'configuration differs from approved streaming policy')
before = state(unit)
require(before['ActiveState'] == 'active' and int(before['MainPID']) > 1, 'nginx not active')
disk_hash = sha('/usr/sbin/nginx')
require(disk_hash == '8c9a61fc3185bb31cd5e9b82b0640ca416fadb8b1764c293d3cce6472787e0a6', 'unreviewed installed executable')
protected = ('ssh.service', 'kvmd.service', 'hardened-blikvm-server-current.service',
    'hardened-blikvm-platform-broker.service', 'hardened-blikvm-video-proxy.service',
    'hardened-blikvm-update-upload.socket', 'hardened-blikvm-update-activation.socket')
baseline = {name: state(name) for name in protected}
require(all(baseline[name]['ActiveState'] == 'inactive' for name in protected[-2:]), 'write admission is not closed')
require(all(baseline[name]['ActiveState'] == 'active' for name in protected[:-2]), 'required runtime not active')
run(['/usr/sbin/nginx', '-t', '-c', config])
already_current = sha('/proc/' + before['MainPID'] + '/exe') == disk_hash
if not already_current:
    run(['systemctl', 'restart', unit])
after = state(unit)
require(after['ActiveState'] == 'active' and int(after['MainPID']) > 1, 'nginx restart failed')
require(sha('/proc/' + after['MainPID'] + '/exe') == disk_hash, 'running executable still differs')
require(already_current or before['MainPID'] != after['MainPID'], 'master was not replaced')
require(sha(config) == expected and sha('/usr/sbin/nginx') == disk_hash, 'configuration or executable changed')
require({name: state(name) for name in protected} == baseline, 'protected runtime changed')
print(json.dumps({'result': 'passed', 'restarted': not already_current, 'oldPid': int(before['MainPID']),
    'newPid': int(after['MainPID']), 'runningSha256': disk_hash, 'configurationUnchanged': True,
    'protectedRuntimeUnchanged': True, 'writeAdmissionClosed': True}, sort_keys=True))
'''


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--endpoint', required=True)
    parser.add_argument('--output', required=True)
    parser.add_argument('--inspect', action='store_true')
    args = parser.parse_args()
    output = common.transport.validate_output_path(args.output)
    common.require(not output.exists(), 'fresh runtime refresh receipt required')
    host, port = common.transport.parse_endpoint(args.endpoint)
    target = common.transport.RemoteTarget(host, port, 'root', common.transport.validate_private_file('evidence/private/ssh-known-hosts'))
    receipt: dict[str, object] = {'result': 'failed', 'submissionAttempted': False}
    write_json_0600_atomic(output, receipt)
    secret = common.transport.read_secret_stdin()
    try:
        receipt['submissionAttempted'] = True
        write_json_0600_atomic(output, receipt)
        result = common.transport.run_ssh(target, secret,
            shlex.join(['python3', '-B', '-c', REMOTE, 'inspect' if args.inspect else 'refresh']), timeout=180)
        common.require(result.returncode == 0 and len(result.stdout) < 16384, 'refresh failed or ambiguous; inspect before retry')
        value = common.native.object_value(json.loads(result.stdout))
        common.require(value.get('result') == 'passed', 'refresh not verified')
        receipt.update(result='passed', device=value)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt.update(errorType=type(error).__name__, independentInspectionRequired=True)
    finally:
        secret[:] = b'\0' * len(secret)
        write_json_0600_atomic(output, receipt)
    return 0 if receipt['result'] == 'passed' else 1


if __name__ == '__main__':
    raise SystemExit(main())
