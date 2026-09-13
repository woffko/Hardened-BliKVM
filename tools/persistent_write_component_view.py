"""Derive comparable safety components from a controller plan or archived plan."""

from collections.abc import Mapping
import hashlib
import json
import re

if not __package__:
    import persistent_write_activation_receipt as common
else:
    from tools import persistent_write_activation_receipt as common

require = common.require
RECOVERY_HELPERS = ("persistent_write_contract.py", "persistent_write_decision.py", "persistent_write_fence.py",
                    "persistent_write_files.py", "persistent_write_lifecycle.py", "persistent_write_pipeline.py",
                    "persistent_write_snapshot.py", "persistent_write_systemd.py")
PLAN_FILES = ("new.deb", "bindings.json", "controller.py")


def read_plan(raw: bytes) -> dict[str, object]:
    """Read a live or archived plan without requiring current plan keys."""
    require(0 < len(raw) <= 4 * 1024 * 1024, 'plan size outside bounds')
    try:
        value = json.loads(raw, object_pairs_hook=common.unique, parse_constant=common.reject_nonfinite)
    except (UnicodeDecodeError, RecursionError) as error:
        raise ValueError('invalid plan encoding/depth') from error
    return common.object_value(value)


def _joined(values: Mapping[str, object]) -> str:
    entries: list[str] = []
    for name, value in sorted(values.items()):
        require(isinstance(value, str) and re.fullmatch(r'[a-f0-9]{64}', value) is not None,
                'invalid component digest')
        assert isinstance(value, str)
        entries.append(name + ':' + value)
    require(bool(entries), 'empty component set')
    return hashlib.sha256('\n'.join(entries).encode()).hexdigest()


def component_view(plan: object, *, test_harness: str) -> dict[str, str]:
    """Return the nine compatibility components for one transaction plan."""
    document = common.object_value(plan)
    common.digest(test_harness)
    files = common.object_value(document.get('files'))
    before = common.object_value(document.get('before'))
    runtime = common.object_value(before.get('runtime'))
    dropins = common.object_value(before.get('persistentDropinHashes'))
    recovery = {name: files.get(name) for name in RECOVERY_HELPERS}
    return {
        'updaterPackage': common.digest(files.get('new.deb')),
        'applicationPayload': common.digest(files.get('bindings.json')),
        'controller': common.digest(files.get('controller.py')),
        'recoveryPolicy': _joined(recovery),
        'testHarness': test_harness,
        'trustRoot': common.digest(runtime.get('trust')),
        'unitContracts': _joined(dropins),
        'proxyExecutable': common.digest(before.get('nginxUnit')),
        'proxyConfiguration': common.digest(runtime.get('nginxConfig')),
    }
