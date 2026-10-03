"""The packaged kernel module is the encoder driver for the kernel beside it
(tools/ve_module.py)."""

from __future__ import annotations

import struct
import subprocess
import sys
import tempfile
import unittest
from collections.abc import Callable
from pathlib import Path

import tools.ve_module as ve_module

ROOT = Path(__file__).resolve().parent.parent
RELEASE = "5.19.4-sunxi64"


def module(
    name: str = ve_module.MODULE_NAME,
    release: str = RELEASE,
    licence: str = "GPL",
    machine: int = ve_module.ELF_MACHINE_AARCH64,
) -> bytes:
    """The least an ELF object needs to be read as a module: a header, a
    `.modinfo` section, the section names, and the table of the three."""
    description = b"".join(
        entry.encode() + b"\0"
        for entry in (
            f"license={licence}",
            f"name={name}",
            f"vermagic={release} SMP mod_unload aarch64",
        )
    )
    names = b"\0.modinfo\0.shstrtab\0"
    body = description + names
    table = 64 + len(body)
    header = b"\x7fELF" + bytes([2, 1, 1, 0]) + bytes(8)
    header += struct.pack("<HHIQQQIHHHHHH", 1, machine, 1, 0, 0, table, 0, 64, 0, 0, 64, 3, 2)

    def section(name_at: int, kind: int, offset: int, size: int) -> bytes:
        return struct.pack("<IIQQQQIIQQ", name_at, kind, 0, 0, offset, size, 0, 0, 1, 0)

    sections = (
        bytes(64)
        + section(1, 1, 64, len(description))
        + section(10, 3, 64 + len(description), len(names))
    )
    return header + body + sections


def image(root: Path, kernel: str = RELEASE) -> None:
    (root / "boot").mkdir(parents=True)
    (root / f"boot/vmlinuz-{kernel}").write_bytes(b"kernel")
    (root / "boot/Image").symlink_to(f"vmlinuz-{kernel}")


def carry(root: Path, release: str, data: bytes, name: str = ve_module.MODULE_FILE) -> None:
    directory = root / ve_module.MODULE_ROOT / release
    directory.mkdir(parents=True, exist_ok=True)
    (directory / name).write_bytes(data)


class ModuleTests(unittest.TestCase):
    def test_a_module_says_what_it_is_and_which_kernel_it_is_for(self) -> None:
        data = module()
        self.assertEqual(ve_module.release_of(data), RELEASE)
        self.assertEqual(ve_module.modinfo(data)["name"], [ve_module.MODULE_NAME])
        ve_module.check(data, RELEASE)

    def test_another_kernel_another_module_and_another_licence_are_refused(self) -> None:
        for data, release in (
            (module(release="6.1.0-sunxi64"), RELEASE),
            (module(), "5.19.4-sunxi"),
            (module(name="hardened_blikvm_ve_lab"), RELEASE),
            (module(licence="Proprietary"), RELEASE),
            (module(machine=62), RELEASE),
        ):
            with self.assertRaises(ve_module.ModuleError):
                ve_module.check(data, release)

    def test_what_is_not_a_module_is_refused_without_a_crash(self) -> None:
        whole = module()
        for data in (b"", b"\x7fELF", whole[:70], whole[:-10], bytes(200), b"#!/bin/sh\n" * 20):
            with self.assertRaises(ve_module.ModuleError):
                ve_module.check(data, RELEASE)
        # Every length of a real module's beginning, and every byte of its
        # header and tables changed: refused or read, never an exception of
        # another kind.
        for length in range(len(whole)):
            try:
                ve_module.modinfo(whole[:length])
            except ve_module.ModuleError:
                pass
        for at in list(range(64)) + list(range(len(whole) - 192, len(whole))):
            changed = bytearray(whole)
            changed[at] ^= 0xFF
            try:
                ve_module.modinfo(bytes(changed))
            except ve_module.ModuleError:
                pass

    def test_an_image_carries_the_driver_for_its_own_kernel_or_none(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            image(root)
            self.assertIn("no encoder driver", ve_module.check_image(root))
            carry(root, RELEASE, module())
            self.assertEqual(ve_module.check_image(root), f"encoder driver for kernel {RELEASE}")

    def test_an_image_with_a_driver_for_another_kernel_is_refused(self) -> None:
        def another_directory(root: Path) -> None:
            carry(root, "6.1.0-sunxi64", module(release="6.1.0-sunxi64"))

        def another_release(root: Path) -> None:
            carry(root, RELEASE, module(release="6.1.0-sunxi64"))

        def lab_module(root: Path) -> None:
            carry(root, RELEASE, module(name="hardened_blikvm_ve_lab"), "hardened-blikvm-ve-lab.ko")

        def second_file(root: Path) -> None:
            carry(root, RELEASE, module())
            carry(root, RELEASE, b"x", "notes.txt")

        def no_module(root: Path) -> None:
            carry(root, RELEASE, b"not a module")

        cases: dict[str, Callable[[Path], None]] = {
            "the directory of another release": another_directory,
            "another release in the right directory": another_release,
            "the lab module": lab_module,
            "a second file beside the driver": second_file,
            "a file that is no module": no_module,
        }
        for name, put in cases.items():
            with self.subTest(name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                image(root)
                put(root)
                with self.assertRaises(ve_module.ModuleError):
                    ve_module.check_image(root)

    def test_an_image_that_enables_the_unit_without_a_driver_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            image(root)
            wants = root / ve_module.ENABLED_UNIT
            wants.parent.mkdir(parents=True)
            wants.symlink_to("/lib/systemd/system/hardened-blikvm-ve.service")
            with self.assertRaises(ve_module.ModuleError):
                ve_module.check_image(root)
            carry(root, RELEASE, module())
            ve_module.check_image(root)

    def test_an_image_that_does_not_name_its_kernel_is_refused(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "boot").mkdir()
            carry(root, RELEASE, module())
            with self.assertRaises(ve_module.ModuleError):
                ve_module.check_image(root)

    def test_the_command_line_says_yes_and_no_by_its_status(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / ve_module.MODULE_FILE
            path.write_bytes(module())
            tool = [sys.executable, str(ROOT / "tools/ve_module.py")]
            found = subprocess.run([*tool, "release", str(path)], capture_output=True, text=True, check=False)
            self.assertEqual((found.returncode, found.stdout), (0, RELEASE + "\n"))
            self.assertEqual(subprocess.run([*tool, "check", str(path), RELEASE], check=False).returncode, 0)
            refused = subprocess.run([*tool, "check", str(path), "6.1.0"], capture_output=True, text=True, check=False)
            self.assertEqual(refused.returncode, 1)
            self.assertIn("built for kernel", refused.stderr)
            self.assertEqual(subprocess.run([*tool], capture_output=True, check=False).returncode, 2)


if __name__ == "__main__":
    unittest.main()
