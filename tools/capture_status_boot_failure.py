#!/usr/bin/env python3
"""Bounded read-only diagnosis of one existing persistent status boot guard."""

import argparse
import json
from pathlib import Path
import shlex
import sys

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import stage_device_prototype as transport
from tools.stage_status_api_wiring import validate_guard
from tools.capture_pikvm_baseline import write_json_0600_atomic


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--guard-root", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    guard = validate_guard(args.guard_root, persistent=True)
    output = transport.validate_output_path(args.output)
    if output.exists():
        raise ValueError("new diagnostic receipt required")
    host, port = transport.parse_endpoint(args.endpoint)
    target = transport.RemoteTarget(host, port, "root", transport.validate_private_file("evidence/private/ssh-known-hosts"))
    code = r'''
import hashlib,json,pathlib,subprocess,sys
root=pathlib.Path(sys.argv[1])
result={"guardExists":root.is_dir(),"bootIdSha256":hashlib.sha256(pathlib.Path("/proc/sys/kernel/random/boot_id").read_bytes().strip()).hexdigest()}
result["uptimeSeconds"]=int(float(pathlib.Path("/proc/uptime").read_text().split()[0]))
for name in ("plan.json","boot-recovery.json"):
 p=root/name
 if p.is_file() and not p.is_symlink() and p.stat().st_size<2097152:
  v=json.loads(p.read_bytes());result[name]={k:v[k] for k in ("phase","bootIdSha256","beforeBootIdSha256","afterBootIdSha256","finishedMonotonicUsec") if k in v}
units=(root.name+"-boot.service",root.name+"-deadline.timer","hardened-blikvm-server-current.service","hardened-blikvm-update-status.socket","hardened-blikvm-update-recover.service","hardened-blikvm-nginx.service")
result["units"]={}
for unit in units:
 p=subprocess.run(["systemctl","show",unit,"--property=LoadState,ActiveState,SubState,Result,ExecMainStatus,ExecMainStartTimestampMonotonic"],capture_output=True,timeout=10)
 result["units"][unit]=p.stdout.decode().splitlines()
p=subprocess.run(["journalctl","-b","--no-pager","-o","cat","-n","80","-u",root.name+"-boot.service"],capture_output=True,timeout=10)
known=("not executing in the bound boot guard","boot recovery requested without a new boot","persistent application state changed","protected configuration changed","Nginx unit changed","foreign server override directory","foreign status override","command failed: systemctl","PermissionError","FileNotFoundError","ModuleNotFoundError","NameError","KeyError")
result["bootErrorClasses"]=[label for label in known if label in p.stdout.decode(errors="replace")]
result["dropinsPresent"]={name:(pathlib.Path("/etc/systemd/system")/(name+".d")/"85-status-api-rehearsal.conf").exists() for name in ("hardened-blikvm-server-current.service","hardened-blikvm-server-candidate.service")}
print(json.dumps(result,sort_keys=True))
'''
    secret = transport.read_secret_stdin()
    try:
        observed = transport.run_ssh(target, secret, shlex.join(["python3", "-B", "-c", code, guard]), timeout=90)
        receipt = {"probe": "status-boot-read-only-diagnosis", "sshExitCode": observed.returncode,
                   "observation": json.loads(observed.stdout) if observed.returncode == 0 else None}
        write_json_0600_atomic(output, receipt)
        if observed.returncode != 0:
            raise SystemExit(1)
    finally:
        secret[:] = b"\0" * len(secret)


if __name__ == "__main__":
    main()
