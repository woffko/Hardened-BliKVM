#!/usr/bin/env python3
"""Capture the negative GUI gate inside a wired write transaction."""

import argparse
from dataclasses import asdict
import hashlib
import json
from pathlib import Path
import re
import shlex
import subprocess
import sys
from typing import cast
from urllib.parse import urlsplit

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from tools import capture_persistent_write_host_gate as host_gate
from tools import persistent_write_contract as contract
from tools import persistent_write_evidence_pins as pins
from tools import persistent_write_gui_failure_receipt as gui_failure
from tools import persistent_write_gui_gate_receipt as gui_gate
from tools import probe_installed_upload as probe
from tools import run_persistent_write_rehearsal as common
from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools.secret_pair_stdin import secret_pair_stdin

PLAYWRIGHT = "/home/w0w/.local/share/playwright/node_modules/playwright/index.mjs"
CHROMIUM = "/home/w0w/.cache/ms-playwright/chromium-1228/chrome-linux64/chrome"
WIRE_CHECK = ("import json,os,sys; p=json.load(open(sys.argv[1])); "
              "assert p['phase']=='wired' and os.stat(sys.argv[1]).st_uid==0; "
              "assert not os.path.exists(sys.argv[2]); "
              "assert p['binding']==json.loads(sys.argv[3]); print('wired')")
TOOL = Path(__file__).resolve()


def profile_for(binding: contract.EvidenceBinding) -> probe.UploadProfile:
    name, _selection = gui_failure.browser_profile(binding)
    return probe.PROFILES[name]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('endpoint', 'base-url', 'username', 'guard-root', 'binding-receipt', 'bundle', 'output'):
        parser.add_argument('--' + name, required=True)
    args = parser.parse_args()
    binding = host_gate.read_binding(json.loads(common.transport.validate_private_file(args.binding_receipt).read_bytes()))
    root = Path(args.guard_root)
    contract.guard_units(root)
    profile = profile_for(binding)
    browser_profile, selection = gui_failure.browser_profile(binding)
    archive = Path(args.bundle).read_bytes()
    common.require(len(archive) == probe.BUNDLE_BYTES
                   and hashlib.sha256(archive).hexdigest() == profile.bundle_sha,
                   'GUI gate archive differs from the reviewed profile')
    host, port = common.transport.parse_endpoint(args.endpoint)
    url = urlsplit(cast(str, args.base_url))
    common.require(url.scheme == 'https' and url.hostname == host and not url.username and not url.password
                   and not url.query and not url.fragment and url.path in ('', '/'), 'invalid browser endpoint')
    common.require(re.fullmatch(r'[a-z_][a-z0-9_-]{0,31}', args.username) is not None, 'invalid browser account')
    output = common.transport.validate_output_path(args.output)
    common.require(not output.exists(), 'new GUI gate receipt required')
    browser_receipt = output.with_name(output.stem + '-browser.json')
    common.require(not browser_receipt.exists(), 'new browser receipt required')
    target = common.transport.RemoteTarget(host, port, 'root',
        common.transport.validate_private_file('evidence/private/ssh-known-hosts'))
    sources = cast(dict[str, str], pins.CURRENT_PINS['failure_browser_sources'])
    receipt: dict[str, object] = {'probe': 'persistent-write-gui-gate', 'schemaVersion': 1, 'result': 'failed',
        'binding': asdict(binding), 'guardRoot': str(root), 'profile': browser_profile, 'selection': selection,
        'browserSha256': '', 'browser': {}, 'sourceHashes': sources,
        'toolSha256': hashlib.sha256(TOOL.read_bytes()).hexdigest()}
    write_json_0600_atomic(output, receipt)
    with secret_pair_stdin() as pair:
        check = shlex.join(['python3', '-B', '-c', WIRE_CHECK, str(root / 'plan.json'),
                            str(root / 'decision.json'), json.dumps(asdict(binding), sort_keys=True)])
        response = common.transport.run_ssh(target, pair['ssh'], check, timeout=30)
        common.require(response.returncode == 0 and response.stdout.strip() == b'wired',
                       'transaction is not wired and undecided')
        result = subprocess.run(['node', 'tools/playwright/device_application_upload_failures.mjs', PLAYWRIGHT,
                                 CHROMIUM, args.base_url.rstrip('/'), args.username, args.bundle, str(browser_receipt),
                                 browser_profile], input=pair['gui'], stdout=subprocess.DEVNULL,
                                stderr=subprocess.DEVNULL, timeout=240, check=False, close_fds=True)
        common.require(result.returncode == 0, 'negative GUI probe failed')
        raw = common.transport.validate_private_file(str(browser_receipt)).read_bytes()
        common.require(len(raw) <= 65536, 'oversize browser receipt')
        browser = common.native.object_value(json.loads(raw))
        common.require(browser.get('profile') == browser_profile and browser.get('selectionVerified') == selection,
                       'browser profile/selection differs')
        gui_failure.validate_browser(browser)
        receipt.update(result='passed', browserSha256=hashlib.sha256(raw).hexdigest(), browser=browser)
        gui_gate.parse(json.dumps(receipt, sort_keys=True, separators=(',', ':')).encode(),
                       expected_sources=sources, expected_tool=str(receipt['toolSha256']))
    write_json_0600_atomic(output, receipt)
    return 0 if receipt['result'] == 'passed' else 1


if __name__ == '__main__':
    raise SystemExit(main())
