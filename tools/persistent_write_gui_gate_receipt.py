"""Parse an in-transaction negative GUI gate receipt for the three fresh gates."""

from collections.abc import Mapping
from dataclasses import dataclass
import hashlib
import json

if not __package__:
    import persistent_write_activation_receipt as common
    import persistent_write_contract as contract
    import persistent_write_gui_failure_receipt as gui_failure
else:
    from tools import persistent_write_activation_receipt as common
    from tools import persistent_write_contract as contract
    from tools import persistent_write_gui_failure_receipt as gui_failure

require = common.require
FIELDS = {'probe', 'schemaVersion', 'result', 'binding', 'guardRoot', 'profile', 'selection', 'browserSha256',
          'browser', 'sourceHashes', 'toolSha256'}


@dataclass(frozen=True)
class Contents:
    original_bytes: bytes
    receipt_sha256: str
    binding: contract.EvidenceBinding
    browser: dict[str, object]


def parse(raw: bytes, *, expected_sources: Mapping[str, str], expected_tool: str) -> Contents:
    """Validate one negative GUI gate captured inside a wired transaction."""
    require(0 < len(raw) <= 1048576, 'receipt size outside bounds')
    common.digest(expected_tool)
    require(set(expected_sources) == set(gui_failure.NEGATIVE_BROWSER_SOURCES), 'incomplete reviewed browser producer')
    for value in expected_sources.values():
        common.digest(value)
    try:
        value = common.object_value(json.loads(raw, object_pairs_hook=common.unique,
                                               parse_constant=common.reject_nonfinite))
    except (UnicodeDecodeError, RecursionError) as error:
        raise ValueError('invalid receipt encoding/depth') from error
    require(set(value) == FIELDS, 'invalid in-transaction GUI gate fields')
    require(value.get('probe') == 'persistent-write-gui-gate' and type(value.get('schemaVersion')) is int
            and value.get('schemaVersion') == 1 and value.get('result') == 'passed', 'not a completed GUI gate')
    require(value.get('toolSha256') == expected_tool and value.get('sourceHashes') == dict(expected_sources),
            'unreviewed GUI gate producer')
    binding = common.read_binding(value.get('binding'))
    root = '/var/lib/' + contract.PREFIX + binding.transaction_id
    require(value.get('guardRoot') == root, 'GUI gate guard root differs')
    profile, selection = gui_failure.browser_profile(binding)
    require(value.get('profile') == profile and value.get('selection') == selection, 'GUI gate profile differs')
    common.digest(value.get('browserSha256'))
    browser = common.object_value(value.get('browser'))
    require(browser.get('profile') == profile and browser.get('selectionVerified') == selection,
            'GUI gate browser profile differs')
    gui_failure.validate_browser(browser)
    return Contents(raw, hashlib.sha256(raw).hexdigest(), binding, browser)


def validate_gate(gate: str, contents: Contents) -> None:
    """Assert the per-gate facts from one shared in-transaction receipt."""
    gui_failure.assert_gate(gate, contents.browser)
