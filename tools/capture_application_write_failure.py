#!/usr/bin/env python3
"""Read bounded, redacted service failure facts; never reopen HTTP admission."""

import argparse
import json
from pathlib import Path
import shlex
import subprocess
import sys

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import run_application_write_gate as runner
from tools.capture_pikvm_baseline import write_json_0600_atomic

REMOTE = r'''
import hashlib, json, pathlib, re, subprocess
units = ("hardened-blikvm-nginx.service", "hardened-blikvm-server-current.service", "hardened-blikvm-update-upload@*.service")
patterns = ("Permission denied", "No space left on device", "Connection refused", "client intended to send too large body", "upstream prematurely closed", "Failed", "failed", "status=", "quota", "invalid_archive")
patterns += ("recv() failed", "send() failed", "Connection reset by peer", "while sending request to upstream", "while reading response header from upstream")
facts = {}
config = pathlib.Path("/etc/hardened-blikvm/nginx.conf").read_bytes()
if len(config) > 65536:
    raise ValueError("oversize proxy configuration")
directives = b"\n".join(line.split(b"#", 1)[0] for line in config.splitlines())
# Return only constrained directive values, never private configuration text.
facts["proxyPolicy"] = {
    "configurationSha256": hashlib.sha256(config).hexdigest(),
    "exactUploadLocations": len(re.findall(rb"\blocation\s*=\s*/api/application-update/upload\s*\{", directives)),
    "requestBuffering": [value.decode() for value in re.findall(rb"\bproxy_request_buffering\s+(on|off)\s*;", directives)],
    "responseBuffering": [value.decode() for value in re.findall(rb"\bproxy_buffering\s+(on|off)\s*;", directives)],
    "includesPresent": re.search(rb"\binclude\s", directives) is not None,
}
version = subprocess.run(["/usr/sbin/nginx", "-v"], capture_output=True, timeout=5, check=True)
version_bytes = version.stdout + version.stderr
if len(version_bytes) > 4096:
    raise ValueError("oversize nginx version output")
matched = re.search(rb"nginx/([0-9]+\.[0-9]+\.[0-9]+)", version_bytes)
facts["nginxVersion"] = matched[1].decode() if matched else None
for unit in units:
    result = subprocess.run(["journalctl", "--no-pager", "--output=cat", "--since=-20min", "--lines=100", "--unit=" + unit], capture_output=True, timeout=10)
    if result.returncode != 0 or len(result.stdout) > 262144:
        raise ValueError("bounded journal read failed")
    text = result.stdout.decode(errors="replace")
    facts[unit] = {"lines": len(text.splitlines()), "counts": {pattern: text.count(pattern) for pattern in patterns}}
    facts[unit]["systemErrors"] = [{"operation": operation, "errno": int(number)}
        for operation, number in re.findall(r"\b([a-z_]+)\(\) failed \(([0-9]{1,3}):", text)
        if operation in ("writev", "readv", "send", "recv", "connect", "read", "write", "open")]
for unit in ("hardened-blikvm-update-upload.socket", "hardened-blikvm-update-activation.socket"):
    result = subprocess.run(["systemctl", "show", unit, "--property=ActiveState,SubState,Result,Job"], capture_output=True, check=True, timeout=5)
    if len(result.stdout) > 4096:
        raise ValueError("oversize unit result")
    facts[unit] = dict(line.split("=", 1) for line in result.stdout.decode().splitlines())
print(json.dumps(facts, sort_keys=True))
'''


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    output = runner.transport.validate_output_path(args.output)
    runner.require(not output.exists(), "new failure evidence required")
    host, port = runner.transport.parse_endpoint(args.endpoint)
    target = runner.transport.RemoteTarget(host, port, "root", runner.transport.validate_private_file("evidence/private/ssh-known-hosts"))
    secret = runner.transport.read_secret_stdin()
    receipt: dict[str, object] = {"probe": "application-http-failure-facts", "result": "failed", "readOnly": True}
    try:
        result = runner.transport.run_ssh(target, secret, shlex.join(["python3", "-B", "-c", REMOTE]), timeout=60)
        runner.require(result.returncode == 0 and len(result.stdout) <= 16384, "failure fact capture failed")
        receipt["facts"] = json.loads(result.stdout)
        receipt["result"] = "passed"
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt["errorType"] = type(error).__name__
    finally:
        secret[:] = b"\0" * len(secret)
        write_json_0600_atomic(output, receipt)
    return 0 if receipt["result"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
