#!/usr/bin/env python3
"""Run the in-window native upload gate and bind its vectors to a transaction."""

import argparse
from collections.abc import Callable
from dataclasses import asdict
import hashlib
import json
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
from typing import cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from tools import capture_persistent_write_host_gate as host_gate
from tools import persistent_write_contract as contract
from tools import persistent_write_evidence_pins as pins
from tools import persistent_write_upload_receipt as receipt_module
from tools import probe_installed_upload as probe
from tools import run_persistent_write_rehearsal as common
from tools.capture_pikvm_baseline import write_json_0600_atomic

TOOL = Path(__file__).resolve()
SPOOL = '/var/lib/hardened-blikvm-upload'
Runner = Callable[..., subprocess.CompletedProcess[bytes]]


def profile_for(binding: contract.EvidenceBinding) -> probe.UploadProfile:
    expected = receipt_module.PROFILES.get(binding.baseline_floor)
    common.require(expected is not None, 'no reviewed upload gate profile for this baseline')
    assert expected is not None
    return probe.PROFILES[expected[0]]


def mode_command(mode: str, root: Path, archive: str, profile: str, uid: int, gid: int) -> list[str]:
    common.require(mode in receipt_module.MODES, 'unknown upload gate mode')
    command = ['python3', '-B', str(root / 'probe_installed_upload.py'), mode,
               '--archive', archive, '--profile', profile]
    if mode == 'unauthorized':
        return command
    return ['setpriv', f'--reuid={uid}', f'--regid={gid}', '--clear-groups', *command]


def validate_mode_output(raw: bytes, mode: str) -> None:
    value = common.native.object_value(json.loads(raw))
    common.require(value.get('mode') == mode and value.get('result') == 'passed', 'upload vector did not pass')


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('endpoint', 'guard-root', 'bundle', 'binding-receipt', 'output'):
        parser.add_argument('--' + name, required=True)
    args = parser.parse_args()
    binding = host_gate.read_binding(json.loads(common.transport.validate_private_file(args.binding_receipt).read_bytes()))
    upload_sources = cast(dict[str, str], pins.CURRENT_PINS['upload_sources'])
    root = Path(args.guard_root)
    contract.guard_units(root)
    profile = profile_for(binding)
    archive = Path(args.bundle).read_bytes()
    common.require(len(archive) == probe.BUNDLE_BYTES and hashlib.sha256(archive).hexdigest() == profile.bundle_sha,
                   'upload gate archive differs from the reviewed profile')
    output = common.transport.validate_output_path(args.output)
    common.require(not output.exists(), 'new upload gate receipt required')
    host, port = common.transport.parse_endpoint(args.endpoint)
    target = common.transport.RemoteTarget(host, port, 'root',
        common.transport.validate_private_file('evidence/private/ssh-known-hosts'))
    receipt: dict[str, object] = {'probe': 'persistent-write-upload-gate', 'schemaVersion': 1, 'result': 'failed',
        'binding': asdict(binding), 'guardRoot': str(root), 'profile': profile.name,
        'archiveSha256': profile.bundle_sha, 'modes': {}, 'spoolEmpty': False,
        'sourceHashes': upload_sources,
        'toolSha256': hashlib.sha256(TOOL.read_bytes()).hexdigest()}
    write_json_0600_atomic(output, receipt)
    secret = common.transport.read_secret_stdin()
    q = shlex.quote
    staged: str | None = None

    def ssh(script: str, timeout: float = 60) -> subprocess.CompletedProcess[bytes]:
        return common.transport.run_ssh(target, secret, script, timeout=timeout)

    try:
        uid = int(ssh('id -u hardened-blikvm-server').stdout.decode().strip())
        gid = int(ssh('id -g hardened-blikvm-server').stdout.decode().strip())
        common.require(0 < uid and uid != 0, 'invalid upload server identity')
        staged = str(root / 'upload-gate-application.tar')
        with tempfile.TemporaryFile() as stream:
            stream.write(archive)
            stream.seek(0)
            upload = (f"set -eu\numask 077\ncat > {q(staged)}\n"
                      f"test \"$(sha256sum {q(staged)} | cut -d ' ' -f 1)\" = {profile.bundle_sha}")
            common.require(common.transport.run_ssh(target, secret, upload, stdin=stream, timeout=90).returncode == 0,
                           'upload gate archive staging failed')
        digest = ssh(f"sha256sum {q(str(root / 'probe_installed_upload.py'))} | cut -d ' ' -f 1").stdout.decode().strip()
        common.require(digest == upload_sources['probe_installed_upload.py'],
                       'device upload probe differs from reviewed pins')
        modes: dict[str, object] = {}
        for mode in receipt_module.MODES:
            response = ssh(shlex.join(mode_command(mode, root, staged, profile.name, uid, gid)), timeout=90)
            common.require(response.returncode == 0 and len(response.stdout) <= 65536, 'upload vector failed: ' + mode)
            validate_mode_output(response.stdout, mode)
            modes[mode] = 'passed'
        spool = ssh(f"set -eu\ntest ! -e {q(SPOOL)} || test -z \"$(ls -A {q(SPOOL)})\"").returncode == 0
        common.require(spool, 'upload spool is not empty')
        receipt.update(result='passed', modes=modes, spoolEmpty=True)
        receipt_module.parse(json.dumps(receipt, sort_keys=True, separators=(',', ':')).encode(),
                             expected_sources=upload_sources, expected_tool=str(receipt['toolSha256']))
    finally:
        if staged is not None:
            ssh(f"rm -f -- {q(staged)}")
        secret[:] = b'\0' * len(secret)
        write_json_0600_atomic(output, receipt)
    return 0 if receipt['result'] == 'passed' else 1


if __name__ == '__main__':
    raise SystemExit(main())
