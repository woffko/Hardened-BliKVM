"""Architectural guard: gadget-profile builds must not link KVMD crates.

The gadget profiles replace KVMD, so their normal (non-dev) dependency closure
must contain no `bli-kvm-kvmd-*` crate. The matching KVMD profiles must still
pull one, otherwise the check would pass for the wrong reason.
"""

from __future__ import annotations

import subprocess
import unittest


def dependency_tree(package: str, args: list[str]) -> str:
    result = subprocess.run(
        ["cargo", "tree", "-p", package, "-e", "normal", "--prefix", "none", *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=True,
        text=True,
    )
    return result.stdout


def kvmd_crates(tree: str) -> list[str]:
    return [line for line in tree.splitlines() if "bli-kvm-kvmd" in line]


class GadgetProfileBoundaryTests(unittest.TestCase):
    def test_gadget_profiles_link_no_kvmd(self) -> None:
        cases = (
            ("bli-kvm-control-broker", ["--features", "gadget-media-profile,gadget-hid-profile"]),
            (
                "bli-kvm-media-transition",
                ["--no-default-features", "--features", "gadget-media-profile"],
            ),
            (
                "bli-kvm-hid-transition",
                ["--no-default-features", "--features", "gadget-hid-profile"],
            ),
            (
                "bli-kvm-atx-transition",
                ["--no-default-features", "--features", "gpio-atx-profile"],
            ),
        )
        for package, args in cases:
            with self.subTest(package=package):
                tree = dependency_tree(package, args)
                self.assertEqual(
                    kvmd_crates(tree),
                    [],
                    f"{package} gadget profile links KVMD: {kvmd_crates(tree)[:3]}",
                )

    def test_kvmd_profiles_still_link_kvmd(self) -> None:
        tree = dependency_tree(
            "bli-kvm-control-broker",
            ["--features", "hid-transition-profile,media-transition-profile"],
        )
        self.assertNotEqual(kvmd_crates(tree), [])


if __name__ == "__main__":
    _ = unittest.main()
