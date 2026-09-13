#!/usr/bin/env python3
"""Prove committed retained write admission survives one software reboot."""

import argparse
import json
from pathlib import Path
import shlex
import subprocess
import sys
import time

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from tools import run_persistent_write_rehearsal as common
from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools.stage_operational_reboot import REBOOT_COMMAND

REBOOT_DEADLINE_SEC = 360
VERIFY_SCRIPT = (
    "import importlib.util, json, sys\n"
    "root = sys.argv[1]\n"
    "sys.path.insert(0, root)\n"
    "spec = importlib.util.spec_from_file_location('installed_persistent_write', root + '/controller.py')\n"
    "native = importlib.util.module_from_spec(spec)\n"
    "sys.modules['installed_persistent_write'] = native\n"
    "spec.loader.exec_module(native)\n"
    "import persistent_write_retained_snapshot as retained\n"
    "from pathlib import Path\n"
    "path = Path(root)\n"
    "plan = native.load(path)\n"
    "bound = native.binding(plan)\n"
    "terminal_value = plan.get('terminal')\n"
    "if terminal_value is None:\n"
    "    terminal = native.contract.temporary.TerminalState(bound.baseline_slot, bound.baseline_floor, bound.baseline_generation)\n"
    "else:\n"
    "    value = native.object_value(terminal_value)\n"
    "    terminal = native.contract.temporary.TerminalState(str(value['slot']), int(value['floor']), int(value['generation']))\n"
    "after = retained.capture(path, native.object_value(plan['backup']), terminal)\n"
    "native.snapshot.preserve(native.object_value(plan['before']), after, new_boot=plan['boot'] != native.boot_identity(), allow_transition=False)\n"
    "print(json.dumps({'result': 'retained', 'guardRoot': root, 'bootIdSha256': native.boot_identity(), 'retentionApproved': True, 'snapshot': after}, sort_keys=True))\n"
)



def validate_retained(value: object) -> dict[str, object]:
    result = common.native.object_value(value)
    common.require(result.get('result') == 'retained' and result.get('retentionApproved') is True,
                   'retained policy was not verified')
    boot = result.get('bootIdSha256')
    common.require(isinstance(boot, str) and common.native.re.fullmatch(r'[a-f0-9]{64}', boot) is not None,
                   'invalid retained boot identity')
    return result


def survived(before: dict[str, object], after: dict[str, object]) -> bool:
    return after.get('bootIdSha256') != before.get('bootIdSha256') and after.get('result') == 'retained'


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('endpoint', 'guard-root', 'output'):
        parser.add_argument('--' + name, required=True)
    args = parser.parse_args()
    root = Path(args.guard_root)
    common.native.contract.guard_units(root)
    output = common.transport.validate_output_path(args.output)
    common.require(not output.exists(), 'new retained reboot receipt required')
    host, port = common.transport.parse_endpoint(args.endpoint)
    target = common.transport.RemoteTarget(host, port, 'root',
        common.transport.validate_private_file('evidence/private/ssh-known-hosts'))
    receipt: dict[str, object] = {'probe': 'persistent-write-retained-reboot', 'result': 'failed',
        'guardRoot': str(root), 'rebootRequested': False, 'targetPowerControl': False}
    write_json_0600_atomic(output, receipt)
    secret = common.transport.read_secret_stdin()

    def ssh(script: str, timeout: float = 60) -> subprocess.CompletedProcess[bytes]:
        return common.transport.run_ssh(target, secret, script, timeout=timeout)

    try:
        command = shlex.join(['python3', '-B', '-c', VERIFY_SCRIPT, str(root)])
        before = validate_retained(json.loads(ssh(command).stdout))
        receipt['before'] = before
        receipt['rebootRequested'] = True
        write_json_0600_atomic(output, receipt)  # Persist intent before an unclear reboot reply.
        try:
            request = ssh(REBOOT_COMMAND, 20)
            common.require(request.returncode in (0, 255), 'reboot request rejected')
            receipt['rebootReplyCode'] = request.returncode
        except subprocess.TimeoutExpired:
            receipt['rebootReplyUnavailable'] = True
        deadline = time.monotonic() + REBOOT_DEADLINE_SEC
        attempts = 0
        while time.monotonic() < deadline:
            attempts += 1
            try:
                after = validate_retained(json.loads(ssh(command, 45).stdout))
                if survived(before, after):
                    receipt.update(result='passed', after=after, bootProbes=attempts, runtimeWritesClosed=False)
                    return 0
            except (OSError, ValueError, subprocess.SubprocessError):
                pass
            time.sleep(2)
        receipt['bootProbes'] = attempts
        raise TimeoutError('retained policy reboot survival was not proven')
    finally:
        secret[:] = b'\0' * len(secret)
        write_json_0600_atomic(output, receipt)
    return 1


if __name__ == '__main__':
    raise SystemExit(main())
