#!/usr/bin/env python3
"""Download a reviewed same-release package plan; never install or change holds."""

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

REMOTE_INSPECT = r'''
import json, pathlib, re, subprocess, sys
plan = json.loads(sys.argv[1])
root = pathlib.Path(plan['planRoot'])
if root.parent != pathlib.Path('/mnt') or not re.fullmatch(r'blikvm-apt-plan-[a-f0-9]{24}', root.name):
    raise ValueError('invalid plan directory')
facts = {'result': 'passed', 'readOnly': True, 'files': {}}
for name in ('reviewed.pref', 'download.conf', 'reviewed-plan.json', 'download-simulation.log', 'download.log', 'archives-verified.json'):
    path = root / name
    entry = {'exists': path.is_file(), 'bytes': path.stat().st_size if path.is_file() else 0}
    if name.endswith('.log') and path.is_file() and path.stat().st_size < 8388608:
        raw = path.read_text(errors='replace')
        entry['lines'] = len(raw.splitlines())
        entry['classifications'] = {p: raw.count(p) for p in ('E:', 'W:', 'Err:', 'Get:', 'Inst ', 'Remv ',
            'Permission denied', 'No space left', 'Could not get lock', 'Failed to fetch', 'Hash Sum mismatch',
            'Temporary failure', 'Unable to locate', 'Download complete', 'Connection failed')}
    facts['files'][name] = entry
files = list((root / 'archives').glob('*.deb'))
facts['archiveCount'] = len(files)
facts['archiveBytes'] = sum(p.stat().st_size for p in files)
query = subprocess.run(['dpkg-query', '-W', '-f=${binary:Package}\t${Version}\n'], capture_output=True, check=True)
installed = dict(line.split('\t', 1) for line in query.stdout.decode().splitlines())
facts['reviewedInstalledVersionsUnchanged'] = all(installed.get(p['package'], installed.get(p['package'] + ':arm64')) == p['installed'] for p in plan['changes'])
print(json.dumps(facts, sort_keys=True))
'''

REMOTE = r'''
import hashlib, io, json, os, pathlib, re, stat, subprocess, sys, tarfile
plan = json.loads(sys.argv[1])
root = pathlib.Path(plan['planRoot'])
def require(value, message):
    if not value: raise ValueError(message)
def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest() if path.exists() else None
def run(argv, timeout=60):
    try:
        return subprocess.run(argv, capture_output=True, timeout=timeout,
            env={**os.environ, 'LC_ALL': 'C', 'DEBIAN_FRONTEND': 'noninteractive'})
    except subprocess.TimeoutExpired as error:
        return subprocess.CompletedProcess(argv, 124, error.stdout or b'',
            (error.stderr or b'') + b'\nBLiKVM bounded command timeout\n')
def private(path, content, reuse=False):
    if reuse and path.exists():
        require(not path.is_symlink() and path.stat().st_uid == 0 and
            not path.stat().st_mode & 0o077 and path.read_text() == content, 'prepared file differs')
        return
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'w') as f:
        f.write(content); f.flush(); os.fsync(f.fileno())
require(os.geteuid() == 0 and root.parent == pathlib.Path('/mnt') and
    re.fullmatch(r'blikvm-apt-plan-[a-f0-9]{24}', root.name), 'invalid plan root')
require(root.resolve(strict=True) == root and root.stat().st_uid == 0
    and not root.stat().st_mode & 0o022 and os.path.ismount('/mnt'), 'unsafe plan root')
require(plan['result'] == 'passed' and plan['packagesInstalled'] is False, 'unreviewed plan')
changes = plan['changes']
require(0 < len(changes) <= 300 and all(p['installed'] for p in changes), 'upgrade-only plan required')
expected = {p['package']: p['candidate'] for p in changes}
require(len(expected) == len(changes), 'duplicate package')
for p in [*changes, *plan['protectedCandidates']]:
    require(re.fullmatch(r'[a-z0-9][a-z0-9+.-]*(?::[a-z0-9]+)?', p['package']) and
        all(re.fullmatch(r'[A-Za-z0-9.+:~_-]+', p[k]) for k in ('installed', 'candidate')), 'unsafe package metadata')
before = {p: digest(pathlib.Path(p)) for p in ('/var/lib/dpkg/status', '/var/lib/apt/extended_states')}
audit = run(['dpkg', '--audit'])
require(audit.returncode == 0 and audit.stdout == b'', 'dpkg audit not clean')
pref = pathlib.Path('/etc/apt/preferences')
original = pref.read_text() if pref.exists() else ''
require(len(original) < 1048576, 'oversize preferences')
# Local override only; preserve the original preferences.d path and main entries.
pins = ''.join('Package: ' + p['package'] + '\nPin: version ' + version + '\nPin-Priority: 1001\n\n'
    for p, version in [(p, p['candidate']) for p in changes] +
                      [(p, p['installed']) for p in plan['protectedCandidates']])
private(root / 'reviewed.pref', pins + original, reuse=True)
conf = (root / 'plan.conf').read_text()
conf += '\nDir::Etc::preferences ' + json.dumps(str(root / 'reviewed.pref')) + ';\n'
conf += 'APT::Get::Upgrade-Allow-New "false";\nAPT::Get::AutomaticRemove "false";\n'
private(root / 'download.conf', conf, reuse=True)
private(root / 'reviewed-plan.json', json.dumps(plan, sort_keys=True), reuse=True)
base = ['apt-get', '-c', str(root / 'download.conf'), '--no-remove']
simulation = run([*base, '-s', 'upgrade'], timeout=300)
private(root / 'simulation-result.json', json.dumps({'exitCode': simulation.returncode}))
private(root / 'download-simulation.log', (simulation.stdout + simulation.stderr).decode(errors='replace'))
require(simulation.returncode == 0, 'APT simulation failed')
actual = {}
for line in simulation.stdout.decode().splitlines():
    require(not line.startswith(('Remv ', 'Purg ')), 'removal proposed')
    if line.startswith('Inst '):
        m = re.match(r'Inst (\S+) \[([^\]]+)\] \((\S+)', line)
        require(m, 'new package or unrecognized upgrade')
        actual[m[1]] = m[3]
require(actual == expected, 'APT simulation differs from reviewed plan')
v = os.statvfs('/mnt')
require(v.f_bavail * v.f_frsize > plan['downloadBytes'] + 1073741824, 'insufficient archive space')
download = run([*base, '--download-only', '-y', 'upgrade'], timeout=900)
private(root / 'download.log', (download.stdout + download.stderr).decode(errors='replace'))
require(download.returncode == 0, 'package download failed')
archives = []
scripts = {}
for path in sorted((root / 'archives').glob('*.deb')):
    meta = path.lstat()
    require(stat.S_ISREG(meta.st_mode) and not path.is_symlink(), 'unsafe archive')
    info = run(['dpkg-deb', '-f', str(path), 'Package', 'Version'])
    require(info.returncode == 0, 'archive metadata unavailable')
    fields = dict(line.split(': ', 1) for line in info.stdout.decode().splitlines())
    name, version = fields['Package'], fields['Version']
    require(expected.get(name) == version, 'unplanned archive')
    archives.append({'file': path.name, 'package': name, 'version': version,
        'bytes': meta.st_size, 'sha256': digest(path)})
    if name in ('nginx-common', 'nginx-core', 'systemd', 'udev', 'libc6', 'openssh-server', 'network-manager'):
        control = run(['dpkg-deb', '--ctrl-tarfile', str(path)])
        require(control.returncode == 0 and len(control.stdout) < 2097152, 'oversize control archive')
        with tarfile.open(fileobj=io.BytesIO(control.stdout), mode='r:') as tar:
            scripts[name] = {}
            for member in tar:
                short = member.name.removeprefix('./')
                if short in ('preinst', 'postinst', 'prerm', 'postrm') and member.isfile():
                    require(member.size < 131072, 'oversize maintainer script')
                    stream = tar.extractfile(member)
                    scripts[name][short] = stream.read().decode() if stream else ''
require(len(archives) == len(expected) and {p['package'] for p in archives} == set(expected), 'incomplete archive set')
require(sum(p['bytes'] for p in archives) == plan['downloadBytes'], 'download size changed')
require(before == {p: digest(pathlib.Path(p)) for p in before}, 'package state changed during download')
private(root / 'archives-verified.json', json.dumps(archives, sort_keys=True))
services = {}
for unit in ('nginx.service', 'ssh.service', 'NetworkManager.service', 'kvmd.service',
    'kvmd-nginx.service', 'hardened-blikvm-nginx.service', 'hardened-blikvm-server-current.service'):
    value = run(['systemctl', 'show', unit, '--property=LoadState,ActiveState,SubState,UnitFileState'])
    services[unit] = dict(line.split('=', 1) for line in value.stdout.decode().splitlines() if '=' in line)
policy = pathlib.Path('/usr/sbin/policy-rc.d')
print(json.dumps({'result': 'passed', 'planRoot': str(root), 'archiveCount': len(archives),
    'downloadBytes': sum(p['bytes'] for p in archives), 'packageStateUnchanged': True,
    'services': services, 'policyRcExists': policy.exists(), 'policyRcSha256': digest(policy),
    'maintainerScripts': scripts, 'packagesInstalled': False}, sort_keys=True))
'''


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--endpoint', required=True)
    parser.add_argument('--plan', required=True)
    parser.add_argument('--output', required=True)
    parser.add_argument('--inspect', action='store_true')
    args = parser.parse_args()
    output = common.transport.validate_output_path(args.output)
    common.require(not output.exists(), 'new download receipt required')
    source = common.transport.validate_private_file(args.plan)
    plan = json.loads(source.read_text())
    common.require(plan['result'] == 'passed', 'successful plan required')
    device = plan['device']
    host, port = common.transport.parse_endpoint(args.endpoint)
    target = common.transport.RemoteTarget(host, port, 'root', common.transport.validate_private_file('evidence/private/ssh-known-hosts'))
    receipt: dict[str, object] = {'result': 'failed', 'planRoot': device['planRoot'], 'packagesInstalled': False}
    write_json_0600_atomic(output, receipt)
    secret = common.transport.read_secret_stdin()
    try:
        result = common.transport.run_ssh(target, secret,
            shlex.join(['python3', '-B', '-c', REMOTE_INSPECT if args.inspect else REMOTE, json.dumps(device)]), timeout=1080)
        common.require(result.returncode == 0 and len(result.stdout) <= 2097152, 'package download failed')
        value = common.native.object_value(json.loads(result.stdout))
        receipt['device'] = value
        common.require(value.get('result') == 'passed' and
            (value.get('readOnly') is True if args.inspect else value.get('packageStateUnchanged') is True), 'unverified download')
        receipt['result'] = 'passed'
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt['errorType'] = type(error).__name__
    finally:
        secret[:] = b'\0' * len(secret)
        write_json_0600_atomic(output, receipt)
    return 0 if receipt['result'] == 'passed' else 1


if __name__ == '__main__':
    raise SystemExit(main())
