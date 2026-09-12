#!/usr/bin/env python3
"""Refresh isolated APT metadata on /mnt and calculate only; never install."""

import argparse
import json
from pathlib import Path
import secrets
import shlex
import subprocess
import sys

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import run_persistent_write_rehearsal as common
from tools.capture_pikvm_baseline import write_json_0600_atomic

REMOTE = r'''
import contextlib, hashlib, io, json, os, pathlib, re, subprocess, sys
import apt, apt_pkg

def require(value, message):
    if not value: raise ValueError(message)

def space(path):
    v = os.statvfs(path)
    return v.f_bavail * v.f_frsize

def status_hash():
    return hashlib.sha256(pathlib.Path('/var/lib/dpkg/status').read_bytes()).hexdigest()

root = pathlib.Path(sys.argv[1])
require(os.geteuid() == 0 and root.parent == pathlib.Path('/mnt')
    and re.fullmatch(r'blikvm-apt-plan-[a-f0-9]{24}', root.name), 'invalid plan directory')
require(os.path.ismount('/mnt') and pathlib.Path('/mnt').resolve() == pathlib.Path('/mnt')
    and os.stat('/mnt').st_dev != os.stat('/').st_dev, 'separate data mount required')
require(space('/mnt') > 2147483648 and space('/') > 67108864, 'insufficient working space')
require(not root.exists() and not root.is_symlink(), 'fresh plan directory required')
before_hash = status_hash()
root.mkdir(mode=0o755)
for name in ('lists', 'archives', 'logs'):
    (root / name).mkdir(mode=0o755)
options = {
    'Dir::State::lists': str(root / 'lists'),
    'Dir::Cache::archives': str(root / 'archives'),
    'Dir::Cache::pkgcache': '', 'Dir::Cache::srcpkgcache': '',
    'Dir::Log': str(root / 'logs'),
    'Acquire::Retries': '1', 'Acquire::http::Timeout': '30',
    'Acquire::https::Timeout': '30', 'Acquire::Languages': 'none',
    'APT::Update::Error-Mode': 'any',
}
hooks = ('APT::Update::Pre-Invoke', 'APT::Update::Post-Invoke',
         'APT::Update::Post-Invoke-Success', 'DPkg::Pre-Invoke',
         'DPkg::Post-Invoke', 'DPkg::Pre-Install-Pkgs')
config = root / 'plan.conf'
with config.open('x') as f:
    f.write(''.join('#clear ' + key + ';\n' for key in hooks))
    f.write(''.join(key + ' ' + json.dumps(value) + ';\n' for key, value in options.items()))
# Preserve repository definitions, pins, signature and validity verification.
# Only metadata is fetched. No package archives, hooks or dpkg invocation.
result = subprocess.run(['apt-get', '-c', str(config), 'update'],
    capture_output=True, timeout=600, env={**os.environ, 'LC_ALL': 'C'})
facts = {'result': 'failed', 'planRoot': str(root), 'metadataExitCode': result.returncode,
    'packagesInstalled': False, 'packageDatabaseUnchanged': status_hash() == before_hash}
require(len(result.stdout) + len(result.stderr) < 8388608, 'oversize metadata output')
# Keep diagnostic output on the device, private; do not return repository URLs.
fd = os.open(root / 'metadata.log', os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(fd, 'wb') as f: f.write(result.stdout + result.stderr)
facts['metadataWarningCount'] = sum(line.startswith(b'W:') for line in result.stderr.splitlines())
if result.returncode == 0 and facts['packageDatabaseUnchanged']:
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        apt_pkg.init_config()
        for key, value in options.items(): apt_pkg.config.set(key, value)
        for key in hooks: apt_pkg.config.clear(key)
        apt_pkg.init_system()
        cache = apt.Cache()
        protected = [p for p in cache if p.is_installed and
            (p._pkg.selected_state == apt_pkg.SELSTATE_HOLD or
             re.match(r'^(linux-|armbian|u-boot)', p.name))]
        protected_names = {p.name for p in protected}
        facts['protectedCandidates'] = [{'package': p.name,
            'installed': p.installed.version, 'candidate': p.candidate.version if p.candidate else None}
            for p in protected]
        # Pin candidate to installed in memory only, before dependency resolution.
        for p in protected: p.candidate = p.installed
        cache.upgrade(dist_upgrade=False)
        changes = cache.get_changes()
        require(cache.broken_count == 0 and not cache.dpkg_journal_dirty, 'invalid package state')
        require(not any(p.marked_delete or p.name in protected_names for p in changes),
            'protected package change or removal in plan')
        facts['downloadBytes'] = cache.required_download
        facts['additionalInstalledBytes'] = cache.required_space
        facts['changes'] = [{'package': p.name, 'installed': p.installed.version if p.installed else None,
            'candidate': p.candidate.version, 'archiveBytes': p.candidate.size,
            'installedBytes': p.candidate.installed_size} for p in changes]
        facts['keptBack'] = [p.name for p in cache if p.is_upgradable and not p.marked_upgrade]
        facts['result'] = 'passed'
facts['rootAvailableBytes'] = space('/')
facts['dataAvailableBytes'] = space('/mnt')
facts['packageDatabaseUnchanged'] = status_hash() == before_hash
require(facts['packageDatabaseUnchanged'], 'package database changed during planning')
print(json.dumps(facts, sort_keys=True))
'''


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--endpoint', required=True)
    parser.add_argument('--output', required=True)
    args = parser.parse_args()
    output = common.transport.validate_output_path(args.output)
    common.require(not output.exists(), 'new plan receipt required')
    plan_root = '/mnt/blikvm-apt-plan-' + secrets.token_hex(12)
    receipt: dict[str, object] = {'result': 'failed', 'planRoot': plan_root, 'packagesInstalled': False}
    write_json_0600_atomic(output, receipt)
    host, port = common.transport.parse_endpoint(args.endpoint)
    target = common.transport.RemoteTarget(host, port, 'root', common.transport.validate_private_file('evidence/private/ssh-known-hosts'))
    secret = common.transport.read_secret_stdin()
    try:
        result = common.transport.run_ssh(target, secret,
            shlex.join(['python3', '-B', '-c', REMOTE, plan_root]), timeout=720)
        common.require(result.returncode == 0 and len(result.stdout) <= 1048576, 'package planning failed')
        value = common.native.object_value(json.loads(result.stdout))
        receipt['device'] = value
        common.require(value.get('result') == 'passed' and value.get('packageDatabaseUnchanged') is True,
                       'fresh package plan unavailable')
        receipt['result'] = 'passed'
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt['errorType'] = type(error).__name__
    finally:
        secret[:] = b'\0' * len(secret)
        write_json_0600_atomic(output, receipt)
    return 0 if receipt['result'] == 'passed' else 1


if __name__ == '__main__':
    raise SystemExit(main())
