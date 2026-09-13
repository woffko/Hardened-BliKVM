"""Parse a bounded host-gate receipt bound to the current write transaction."""

from collections.abc import Mapping
from dataclasses import dataclass
import hashlib
import json

if not __package__:
    import persistent_write_activation_receipt as common
    import persistent_write_contract as contract
else:
    from tools import persistent_write_activation_receipt as common
    from tools import persistent_write_contract as contract

require = common.require
SUITES = ("tools.test_persistent_write_contract", "tools.test_persistent_write_files",
          "tools.test_persistent_write_decision", "tools.test_persistent_write_lifecycle",
          "tools.test_persistent_write_fence", "tools.test_persistent_write_pipeline",
          "tools.test_persistent_write_systemd", "tools.test_persistent_write_snapshot",
          "tools.test_persistent_write_evidence_store", "tools.test_persistent_write_evidence_provider",
          "tools.test_persistent_write_gui_failure_receipt", "tools.test_persistent_write_commit",
          "tools.test_installed_persistent_write", "tools.test_run_persistent_write_rehearsal")
COMMAND = ["python3", "-B", "-m", "unittest", *SUITES]
MINIMUM_TESTS = 100


def source_paths() -> tuple[str, ...]:
    return tuple(module.replace(".", "/") + ".py" for module in SUITES)


@dataclass(frozen=True)
class Contents:
    original_bytes: bytes
    receipt_sha256: str
    binding: contract.EvidenceBinding


def parse(raw: bytes, *, expected_python_sources: Mapping[str, str], expected_tool: str) -> Contents:
    """Validate one bounded host suite receipt against pinned producers."""
    require(0 < len(raw) <= 1048576, 'receipt size outside bounds')
    common.digest(expected_tool)
    require(set(expected_python_sources) == set(source_paths()), 'incomplete reviewed host producer')
    for value in expected_python_sources.values():
        common.digest(value)
    try:
        value = common.object_value(json.loads(raw, object_pairs_hook=common.unique,
                                               parse_constant=common.reject_nonfinite))
    except (UnicodeDecodeError, RecursionError) as error:
        raise ValueError('invalid receipt encoding/depth') from error
    require(set(value) == {'probe', 'schemaVersion', 'result', 'binding', 'suite', 'toolSha256'},
            'invalid host receipt fields')
    require(value.get('probe') == 'persistent-write-host-gate' and type(value.get('schemaVersion')) is int
            and value.get('schemaVersion') == 1 and value.get('result') == 'passed',
            'not a completed host gate')
    require(value.get('toolSha256') == expected_tool, 'unreviewed host gate tool')
    binding = common.read_binding(value.get('binding'))
    suite = common.object_value(value.get('suite'))
    require(set(suite) == {'command', 'tests', 'result', 'sourceHashes'}, 'invalid host suite fields')
    require(suite.get('command') == COMMAND and suite.get('result') == 'passed', 'host suite command/result differs')
    require(set(common.object_value(suite.get('sourceHashes'))) == set(expected_python_sources)
            and suite.get('sourceHashes') == dict(expected_python_sources), 'unreviewed host sources')
    tests = common.number(suite.get('tests'))
    require(tests >= MINIMUM_TESTS, 'host suite coverage below bound')
    return Contents(raw, hashlib.sha256(raw).hexdigest(), binding)
