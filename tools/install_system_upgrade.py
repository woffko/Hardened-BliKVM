#!/usr/bin/env python3
"""Install an already downloaded reviewed plan in a detached device service."""

import argparse
import hashlib
import json
from pathlib import Path
import shlex
import subprocess
import sys

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import run_persistent_write_rehearsal as common
from tools.capture_pikvm_baseline import write_json_0600_atomic

WORKER = r'''
import hashlib, json, os, pathlib, re, subprocess, sys
root = pathlib.Path(__file__).resolve().parent
policy = pathlib.Path('/usr/sbin/policy-rc.d')
policy_text = ('#!/bin/sh\n# BLiKVM scoped upgrade ' + root.name + '\n'
    'for arg in "$@"; do\ncase "$arg" in\n'
    'nginx|nginx.service|NetworkManager|NetworkManager.service|network-manager) exit 101;;\nesac\ndone\nexit 0\n')
def require(value, message):
    if not value: raise ValueError(message)
def parse_simulation(raw):
    actual = {}
    for line in raw.splitlines():
        require(not line.startswith(('Remv ', 'Purg ')), 'removal proposed')
        if line.startswith('Inst '):
            match = re.match(r'Inst (\S+) \[([^\]]+)\] \((\S+)', line)
            require(match, 'new package proposed')
            require(match[1] not in actual, 'duplicate package in simulation')
            actual[match[1]] = match[3]
    return actual
def sha(path):
    h = hashlib.sha256()
    with path.open('rb') as f:
        for b in iter(lambda: f.read(1048576), b''): h.update(b)
    return h.hexdigest()
def save(name, value):
    path = root / name
    part = root / (name + '.part')
    fd = os.open(part, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'w') as f:
        json.dump(value, f, sort_keys=True); f.flush(); os.fsync(f.fileno())
    os.replace(part, path)
    fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
    try: os.fsync(fd)
    finally: os.close(fd)
def run(argv, timeout=60):
    return subprocess.run(argv, capture_output=True, timeout=timeout,
        env={**os.environ, 'LC_ALL': 'C'})
def installed():
    r = run(['dpkg-query', '-W', '-f=${binary:Package}\t${Version}\t${db:Status-Abbrev}\n'])
    require(r.returncode == 0, 'inventory failed')
    return {f[0]: f[1] for line in r.stdout.decode().splitlines()
            for f in [line.split('\t')] if len(f) == 3 and len(f[2]) > 1 and f[2][1] == 'i'}
def state(unit):
    r = run(['systemctl', 'show', unit, '--property=ActiveState,SubState,UnitFileState'])
    require(r.returncode == 0, 'service query failed')
    return dict(line.split('=', 1) for line in r.stdout.decode().splitlines() if '=' in line)
def cleanup():
    baseline = root / 'install-baseline.json'
    if baseline.exists() and json.loads(baseline.read_text()).get('stockNginxWasDisabled'):
        require(run(['systemctl', 'disable', 'nginx.service']).returncode == 0, 'stock nginx disable cleanup failed')
    if (root / 'policy-owned.json').exists() and (policy.exists() or policy.is_symlink()):
        require(not policy.is_symlink() and policy.read_text() == policy_text, 'policy ownership changed')
        policy.unlink()
    save('policy-cleanup.json', {'removed': not policy.exists()})
if len(sys.argv) > 1 and sys.argv[1] == '--cleanup':
    cleanup(); raise SystemExit(0)
facts = {'result': 'failed', 'phase': 'preflight', 'installationStarted': False}
save('install-result.json', facts)
try:
    require(os.geteuid() == 0 and root.parent == pathlib.Path('/mnt') and
        re.fullmatch(r'blikvm-apt-plan-[a-f0-9]{24}', root.name), 'invalid installation root')
    require(run(['dpkg', '--print-architecture']).stdout.strip() == b'arm64', 'arm64 target required')
    require(os.path.ismount('/mnt') and not os.statvfs('/').f_flag & os.ST_RDONLY, 'mounts unavailable')
    plan = json.loads((root / 'reviewed-plan.json').read_text())
    archives = json.loads((root / 'archives-verified.json').read_text())
    expected = {p['package']: p['candidate'] for p in plan['changes']}
    require(len(expected) == 214 and len(archives) == len(expected), 'reviewed set changed')
    require(not policy.exists() and not policy.is_symlink(), 'existing service policy requires review')
    before = installed()
    def key(name):
        return name if name in before else name + ':arm64'
    for p in [*plan['changes'], *plan['protectedCandidates']]:
        require(before.get(key(p['package'])) == p['installed'], 'installed baseline drift')
    for item in archives:
        path = root / 'archives' / item['file']
        require(path.parent == root / 'archives' and not path.is_symlink() and
            item['package'] in expected and expected[item['package']] == item['version'] and
            path.stat().st_size == item['bytes'] and sha(path) == item['sha256'], 'archive changed')
    conf = (root / 'download.conf').read_text()
    # Restore ordinary dpkg hooks; needrestart is put in list-only mode below.
    conf = '\n'.join(line for line in conf.splitlines() if not line.startswith('#clear DPkg::'))
    conf += '\nDpkg::Options { "--force-confdef"; "--force-confold"; };\n'
    fd = os.open(root / 'install.conf', os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'w') as f: f.write(conf)
    base = ['apt-get', '-c', str(root / 'install.conf'), '--no-remove', '--no-download']
    simulation = run([*base, '-s', 'upgrade'], timeout=300)
    require(simulation.returncode == 0, 'installation simulation failed')
    actual = parse_simulation(simulation.stdout.decode())
    require(actual == expected, 'installation simulation differs')
    v = os.statvfs('/')
    require(v.f_bavail * v.f_frsize > 629145600, 'root headroom below 600 MiB')
    audit = run(['dpkg', '--audit'])
    require(audit.returncode == 0 and not audit.stdout.strip(), 'dpkg not clean')
    important = ('ssh.service', 'NetworkManager.service', 'kvmd.service', 'hardened-blikvm-nginx.service',
        'hardened-blikvm-server-current.service', 'hardened-blikvm-platform-broker.service',
        'hardened-blikvm-video-proxy.service')
    require(all(state(u)['ActiveState'] == 'active' for u in important), 'required service not active')
    nginx_before = state('nginx.service')
    require(nginx_before['ActiveState'] == 'inactive' and nginx_before['UnitFileState'] == 'disabled', 'stock nginx baseline drift')
    config_hashes = {p: sha(pathlib.Path(p)) for p in ('/etc/ssh/sshd_config', '/etc/hardened-blikvm/nginx.conf')}
    auto_before = run(['apt-mark', '-c', str(root / 'install.conf'), 'showauto'])
    require(auto_before.returncode == 0, 'auto package state unavailable')
    holds_before = run(['apt-mark', '-c', str(root / 'install.conf'), 'showhold'])
    require(holds_before.returncode == 0, 'holds unavailable')
    save('install-baseline.json', {'installed': before, 'configurationHashes': config_hashes,
        'stockNginxWasDisabled': True,
        'automaticPackages': auto_before.stdout.decode().splitlines(), 'holds': holds_before.stdout.decode().splitlines()})
    save('policy-owned.json', {'sha256': hashlib.sha256(policy_text.encode()).hexdigest()})
    fd = os.open(policy, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o755)
    with os.fdopen(fd, 'w') as f:
        f.write(policy_text); f.flush(); os.fsync(f.fileno())
    facts.update(phase='installing', installationStarted=True)
    save('install-result.json', facts)
    fd = os.open(root / 'install.log', os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'wb') as log:
        # No execution timeout: killing dpkg mid-transaction is worse than a
        # delayed result. The host observer has a bounded timeout instead.
        result = subprocess.run([*base, '-y', 'upgrade'], stdin=subprocess.DEVNULL, stdout=log,
            stderr=subprocess.STDOUT, env={**os.environ, 'LC_ALL': 'C', 'DEBIAN_FRONTEND': 'noninteractive',
                'NEEDRESTART_MODE': 'l', 'UCF_FORCE_CONFFOLD': '1'})
        log.flush(); os.fsync(log.fileno())
    facts['aptExitCode'] = result.returncode
    facts['phase'] = 'validation'
    save('install-result.json', facts)
    require(result.returncode == 0, 'APT installation failed; do not automatically retry')
    after = installed()
    predicted = {**before, **{key(n): v for n, v in expected.items()}}
    require(after == predicted, 'installed package set differs from plan')
    audit = run(['dpkg', '--audit'])
    require(audit.returncode == 0 and not audit.stdout.strip(), 'post-upgrade dpkg audit failed')
    auto_after = run(['apt-mark', '-c', str(root / 'install.conf'), 'showauto'])
    holds_after = run(['apt-mark', '-c', str(root / 'install.conf'), 'showhold'])
    require(auto_after.returncode == 0 and auto_before.stdout == auto_after.stdout, 'automatic flags changed')
    require(holds_after.returncode == 0 and holds_before.stdout == holds_after.stdout, 'holds changed')
    require(all(sha(pathlib.Path(p)) == h for p, h in config_hashes.items()), 'important configuration changed')
    require(run(['/usr/sbin/sshd', '-t']).returncode == 0, 'sshd config validation failed')
    require(run(['/usr/sbin/nginx', '-t', '-c', '/etc/hardened-blikvm/nginx.conf']).returncode == 0, 'nginx validation failed')
    require(state('nginx.service')['ActiveState'] == 'inactive', 'stock nginx unexpectedly active')
    if state('nginx.service')['UnitFileState'] != 'disabled':
        require(run(['systemctl', 'disable', 'nginx.service']).returncode == 0, 'stock nginx disable failed')
    facts['services'] = {u: state(u) for u in important}
    require(all(s['ActiveState'] == 'active' for s in facts['services'].values()), 'required service stopped')
    facts.update(result='passed', phase='completed', upgradedPackages=len(expected),
        automaticFlagsUnchanged=True, holdsUnchanged=True, importantConfigurationsUnchanged=True,
        rebootRequired=pathlib.Path('/run/reboot-required').exists(),
        rootAvailableBytes=os.statvfs('/').f_bavail * os.statvfs('/').f_frsize)
except Exception as error:
    facts['errorType'] = type(error).__name__
    # Only our fixed guard messages are retained, never subprocess output.
    if isinstance(error, ValueError): facts['guardFailure'] = str(error)
finally:
    try: cleanup()
    except Exception as error:
        facts.update(result='failed', cleanupErrorType=type(error).__name__)
    save('install-result.json', facts)
raise SystemExit(0 if facts['result'] == 'passed' else 1)
'''

START = r'''
import hashlib, json, os, pathlib, re, subprocess, sys
root = pathlib.Path(sys.argv[1]); worker = sys.argv[2]; expected_hash = sys.argv[3]
if root.parent != pathlib.Path('/mnt') or not re.fullmatch(r'blikvm-apt-plan-[a-f0-9]{24}', root.name):
    raise ValueError('invalid plan root')
if root.resolve(strict=True) != root or root.stat().st_uid != 0 or root.stat().st_mode & 0o022:
    raise ValueError('unsafe plan root')
if hashlib.sha256((root / 'reviewed-plan.json').read_bytes()).hexdigest() != expected_hash:
    raise ValueError('reviewed plan changed')
path = root / 'install-worker.py'
fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
with os.fdopen(fd, 'w') as f:
    f.write(worker); f.flush(); os.fsync(f.fileno())
unit = 'blikvm-userspace-upgrade-' + root.name.rsplit('-', 1)[1]
result = subprocess.run(['systemd-run', '--unit=' + unit, '--no-block', '--property=Type=oneshot',
    '--property=RemainAfterExit=yes', '--property=TimeoutStartSec=infinity', '--property=UMask=0077',
    '--property=StandardOutput=null', '--property=StandardError=null',
    '--property=ExecStopPost=/usr/bin/python3 -B ' + str(path) + ' --cleanup',
    '/usr/bin/python3', '-B', str(path)], capture_output=True, timeout=30)
print(json.dumps({'submitted': result.returncode == 0, 'unit': unit, 'exitCode': result.returncode}))
'''

OBSERVE = r'''
import json, pathlib, re, sys, time
root = pathlib.Path(sys.argv[1])
if root.parent != pathlib.Path('/mnt') or not re.fullmatch(r'blikvm-apt-plan-[a-f0-9]{24}', root.name):
    raise ValueError('invalid observer root')
deadline = time.monotonic() + 1500
while time.monotonic() < deadline:
    path = root / 'install-result.json'
    if path.exists():
        value = json.loads(path.read_text())
        if value.get('phase') == 'completed' or 'errorType' in value or 'cleanupErrorType' in value:
            print(json.dumps(value)); break
    time.sleep(5)
else:
    print(json.dumps({'result': 'pending', 'independentInspectionRequired': True}))
'''


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--endpoint', required=True)
    parser.add_argument('--plan', required=True)
    parser.add_argument('--output', required=True)
    parser.add_argument('--observe-only', action='store_true')
    args = parser.parse_args()
    output = common.transport.validate_output_path(args.output)
    common.require(not output.exists(), 'fresh installation receipt required')
    plan = json.loads(common.transport.validate_private_file(args.plan).read_text())['device']
    root = plan['planRoot']
    receipt: dict[str, object] = {'result': 'failed', 'planRoot': root, 'submissionAttempted': False}
    write_json_0600_atomic(output, receipt)
    host, port = common.transport.parse_endpoint(args.endpoint)
    target = common.transport.RemoteTarget(host, port, 'root', common.transport.validate_private_file('evidence/private/ssh-known-hosts'))
    secret = common.transport.read_secret_stdin()
    try:
        if not args.observe_only:
            receipt['submissionAttempted'] = True
            write_json_0600_atomic(output, receipt)
            digest = hashlib.sha256(json.dumps(plan, sort_keys=True).encode()).hexdigest()
            result = common.transport.run_ssh(target, secret,
                shlex.join(['python3', '-B', '-c', START, root, WORKER, digest]), timeout=50)
            common.require(result.returncode == 0 and len(result.stdout) < 16384, 'submission ambiguous; do not repeat')
            receipt['submission'] = json.loads(result.stdout)
            write_json_0600_atomic(output, receipt)
            common.require(common.native.object_value(receipt['submission']).get('submitted') is True, 'unit submission failed')
        result = common.transport.run_ssh(target, secret,
            shlex.join(['python3', '-B', '-c', OBSERVE, root]), timeout=1530)
        common.require(result.returncode == 0 and len(result.stdout) < 262144, 'observer disconnected; inspect existing unit')
        value = common.native.object_value(json.loads(result.stdout))
        receipt['device'] = value
        receipt['result'] = value.get('result', 'failed')
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt['errorType'] = type(error).__name__
        receipt['independentInspectionRequired'] = True
    finally:
        secret[:] = b'\0' * len(secret)
        write_json_0600_atomic(output, receipt)
    return 0 if receipt['result'] == 'passed' else 1


if __name__ == '__main__':
    raise SystemExit(main())
