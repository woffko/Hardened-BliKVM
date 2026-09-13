"""Parse an in-window native upload gate receipt bound to the write transaction."""

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
MODES = ("retry", "invalid-archive", "short", "surplus", "malformed", "unauthorized")
SOURCES = ("probe_installed_upload.py", "upload_unit_reference.py")
PROFILES = {9: ("seq9-retry", "8f858ec697928f39bcfc6a7242aa2d1fdbb061b8cf9232048d9468d03dff247f"),
            10: ("seq10-retry", "85e06d1b68e54480bab89c3cdc5e0d9f80cc3ee007e7ba1e425432af4b250f1f")}


@dataclass(frozen=True)
class Contents:
    original_bytes: bytes
    receipt_sha256: str
    binding: contract.EvidenceBinding
    modes: dict[str, object]


def parse(raw: bytes, *, expected_sources: Mapping[str, str], expected_tool: str) -> Contents:
    """Validate one completed in-window upload gate against pinned producers."""
    require(0 < len(raw) <= 1048576, 'receipt size outside bounds')
    common.digest(expected_tool)
    require(set(expected_sources) == set(SOURCES), 'incomplete reviewed upload producer')
    for value in expected_sources.values():
        common.digest(value)
    try:
        value = common.object_value(json.loads(raw, object_pairs_hook=common.unique,
                                               parse_constant=common.reject_nonfinite))
    except (UnicodeDecodeError, RecursionError) as error:
        raise ValueError('invalid receipt encoding/depth') from error
    require(set(value) == {'probe', 'schemaVersion', 'result', 'binding', 'guardRoot', 'profile', 'archiveSha256',
                           'modes', 'spoolEmpty', 'sourceHashes', 'toolSha256'}, 'invalid upload gate fields')
    require(value.get('probe') == 'persistent-write-upload-gate' and type(value.get('schemaVersion')) is int
            and value.get('schemaVersion') == 1 and value.get('result') == 'passed', 'not a completed upload gate')
    require(value.get('toolSha256') == expected_tool, 'unreviewed upload gate tool')
    require(value.get('sourceHashes') == dict(expected_sources), 'unreviewed upload sources')
    binding = common.read_binding(value.get('binding'))
    root = '/var/lib/' + contract.PREFIX + binding.transaction_id
    require(value.get('guardRoot') == root, 'upload gate guard root differs')
    expected = PROFILES.get(binding.baseline_floor)
    require(expected is not None, 'upload gate has no reviewed profile for this baseline')
    assert expected is not None
    profile, archive_sha = expected
    require(value.get('profile') == profile and value.get('archiveSha256') == archive_sha,
            'upload gate profile/archive differs')
    modes = common.object_value(value.get('modes'))
    require(set(modes) == set(MODES) and all(modes[mode] == 'passed' for mode in MODES), 'upload vectors incomplete')
    require(value.get('spoolEmpty') is True, 'upload spool is not empty')
    return Contents(raw, hashlib.sha256(raw).hexdigest(), binding, modes)
