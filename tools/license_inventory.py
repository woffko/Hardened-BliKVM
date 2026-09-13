#!/usr/bin/env python3
"""Build a deterministic Rust license inventory from `cargo metadata`.

This is an input to the later SPDX release SBOM, not the SBOM itself: it lists
every package, its declared license and whether it is a workspace member, and
fails closed when any package has no declared license. It performs no network
access and never rewrites the lockfile.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from typing import TypedDict, cast


class InventoryPackage(TypedDict):
    name: str
    version: str
    license: str | None
    licenseFile: str | None
    workspace: bool


class Inventory(TypedDict):
    schemaVersion: int
    generatedFrom: str
    packageCount: int
    packages: list[InventoryPackage]
    missingLicense: list[str]


def build_inventory(metadata: dict[str, object]) -> Inventory:
    packages = metadata.get("packages")
    if not isinstance(packages, list):
        raise ValueError("cargo metadata has no package list")
    inventory: list[InventoryPackage] = []
    missing: list[str] = []
    for raw in cast(list[object], packages):
        if not isinstance(raw, dict):
            raise ValueError("cargo metadata package is invalid")
        package = cast(dict[str, object], raw)
        name = package.get("name")
        version = package.get("version")
        if not isinstance(name, str) or not isinstance(version, str):
            raise ValueError("cargo metadata package identity is invalid")
        license_value = package.get("license")
        if license_value is not None and not isinstance(license_value, str):
            raise ValueError("cargo metadata license is invalid")
        license_file = package.get("license_file")
        if license_file is not None and not isinstance(license_file, str):
            raise ValueError("cargo metadata license file is invalid")
        workspace = package.get("source") is None
        inventory.append(
            {
                "name": name,
                "version": version,
                "license": license_value,
                "licenseFile": license_file,
                "workspace": workspace,
            }
        )
        if not license_value and not license_file:
            missing.append(f"{name}@{version}")
    inventory.sort(key=lambda entry: (entry["name"], entry["version"]))
    return {
        "schemaVersion": 1,
        "generatedFrom": "cargo metadata --locked",
        "packageCount": len(inventory),
        "packages": inventory,
        "missingLicense": sorted(missing),
    }


def load_metadata() -> dict[str, object]:
    completed = subprocess.run(
        ["cargo", "metadata", "--locked", "--format-version", "1"],
        check=True,
        capture_output=True,
        text=True,
    )
    return cast(dict[str, object], json.loads(completed.stdout))


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--output", help="write the inventory JSON to this file")
    _ = parser.add_argument(
        "--allow-missing",
        action="store_true",
        help="record missing licenses without failing (audit only)",
    )
    return parser.parse_args()


def main() -> int:
    options = parse_args()
    try:
        inventory = build_inventory(load_metadata())
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"license inventory failed: {error}", file=sys.stderr)
        return 1
    payload = json.dumps(inventory, indent=2, sort_keys=True) + "\n"
    if options.output:
        with open(options.output, "w", encoding="utf-8") as stream:
            stream.write(payload)
    else:
        print(payload, end="")
    if inventory["missingLicense"] and not options.allow_missing:
        print(
            f"missing license: {', '.join(inventory['missingLicense'])}",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
