#!/usr/bin/env python3
"""Validate generated firewall rules using reviewed nft in a fresh net namespace."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--nft", required=True)
    parser.add_argument("--fixtures", required=True)
    parser.add_argument("--parent-netns")
    parser.add_argument("--runtime-gate")
    args = parser.parse_args()
    namespace = os.readlink("/proc/self/ns/net")
    if args.parent_netns is None:
        command = [
            "unshare", "--user", "--map-root-user", "--net", sys.executable,
            str(Path(__file__).resolve()), "--nft", str(Path(args.nft).resolve()),
            "--fixtures", str(Path(args.fixtures).resolve()), "--parent-netns", namespace,
        ]
        if args.runtime_gate:
            command.extend(["--runtime-gate", str(Path(args.runtime_gate).resolve())])
        subprocess.run(command, check=True, timeout=20)
        return
    if namespace == args.parent_netns:
        raise RuntimeError("refusing to run in the parent network namespace")

    def nft(*arguments: str) -> str:
        result = subprocess.run([args.nft, *arguments], check=False, capture_output=True,
                                text=True, timeout=3)
        if result.returncode:
            raise RuntimeError(f"nft failed: {result.stderr}")
        return result.stdout

    nft("add", "table", "inet", "unrelated")
    for mode in ("baseline", "moderate", "restricted", "paranoid"):
        fixture = str(Path(args.fixtures) / f"{mode}.nft")
        nft("--check", "--file", fixture)
        nft("--file", fixture)
        tables = json.loads(nft("-j", "list", "tables"))["nftables"]
        table = next(item["table"] for item in tables if item.get("table", {}).get("name") == "hardened-blikvm")
        assert table["comment"] == f"hardened-blikvm-owner-v2:{mode}", table
        assert table["handle"] > 0
        rules = json.loads(nft("-j", "list", "table", "inet", "hardened-blikvm"))["nftables"]
        (Path(args.fixtures) / f"{mode}.json").write_text(json.dumps({"nftables": rules}, indent=2) + "\n")
        chains = {item["chain"]["name"]: item["chain"] for item in rules if "chain" in item}
        assert chains["input"]["policy"] == ("accept" if mode == "baseline" else "drop")
        assert chains["forward"]["policy"] == ("accept" if mode == "baseline" else "drop")
        assert chains["output"]["policy"] == ("drop" if mode in ("restricted", "paranoid") else "accept")
        nft("delete", "table", "inet", "handle", str(table["handle"]))
        remaining = json.loads(nft("-j", "list", "tables"))["nftables"]
        assert [item["table"]["name"] for item in remaining if "table" in item] == ["unrelated"]
    print("nft isolated namespace: four modes parse/apply, ownership JSON and scoped delete passed")
    if args.runtime_gate:
        subprocess.run([args.runtime_gate, args.nft, args.parent_netns], check=True, timeout=10)
        remaining = json.loads(nft("-j", "list", "tables"))["nftables"]
        assert any(item.get("table", {}).get("name") == "unrelated" for item in remaining)


if __name__ == "__main__":
    main()
