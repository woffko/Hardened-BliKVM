#!/usr/bin/env python3
"""What the packaged kernel module says about itself, and whether it fits.

The encoder driver of the video engine (kernel/hardened-blikvm-ve) is built
for one kernel release and loads on no other. A package and an image carry
it in a directory named after that release; this tool reads the module's
own `.modinfo` and refuses one that is not what its place says:

    ve_module.py release MODULE          print the release it was built for
    ve_module.py check MODULE RELEASE    the driver, for that release
    ve_module.py image ROOT              an image's tree: the driver it
                                         carries is for the image's kernel

It reads files and nothing else: no module is loaded and no tool is run.
"""

from __future__ import annotations

import struct
import sys
from pathlib import Path

# Where a package and an image keep the driver, below their root.
MODULE_ROOT = "usr/lib/hardened-blikvm/modules"
MODULE_FILE = "hardened-blikvm-ve-enc.ko"
MODULE_NAME = "hardened_blikvm_ve_enc"
# The unit that loads it, as an image has it when it is enabled.
ENABLED_UNIT = "etc/systemd/system/multi-user.target.wants/hardened-blikvm-ve.service"

ELF_MAGIC = b"\x7fELF"
ELF_CLASS_64 = 2
ELF_DATA_LITTLE = 1
ELF_TYPE_RELOCATABLE = 1
ELF_MACHINE_AARCH64 = 183


class ModuleError(Exception):
    pass


def modinfo(data: bytes) -> dict[str, list[str]]:
    """The `key=value` strings of an ELF object's `.modinfo` section."""
    if len(data) < 64 or data[:4] != ELF_MAGIC:
        raise ModuleError("not an ELF object")
    if data[4] != ELF_CLASS_64 or data[5] != ELF_DATA_LITTLE:
        raise ModuleError("not a 64-bit little-endian object")
    kind, machine = struct.unpack_from("<HH", data, 16)
    if kind != ELF_TYPE_RELOCATABLE or machine != ELF_MACHINE_AARCH64:
        raise ModuleError("not a relocatable aarch64 object")
    (section_offset,) = struct.unpack_from("<Q", data, 40)
    entry_size, count, names_index = struct.unpack_from("<HHH", data, 58)
    if entry_size != 64 or count == 0 or names_index >= count:
        raise ModuleError("no section table")
    if section_offset + count * entry_size > len(data):
        raise ModuleError("the section table is cut off")

    def section(index: int) -> tuple[int, int, int]:
        name, _kind, _flags, _address, offset, size = struct.unpack_from(
            "<IIQQQQ", data, section_offset + index * entry_size
        )
        return name, offset, size

    _, names_offset, names_size = section(names_index)
    if names_offset + names_size > len(data):
        raise ModuleError("the section names are cut off")
    names = data[names_offset : names_offset + names_size]
    for index in range(count):
        name, offset, size = section(index)
        end = names.find(b"\0", name)
        if name >= len(names) or end < 0 or names[name:end] != b".modinfo":
            continue
        if offset + size > len(data):
            raise ModuleError("the module's description is cut off")
        found: dict[str, list[str]] = {}
        for entry in data[offset : offset + size].split(b"\0"):
            key, separator, value = entry.partition(b"=")
            if separator:
                found.setdefault(key.decode("ascii", "replace"), []).append(
                    value.decode("ascii", "replace")
                )
        return found
    raise ModuleError("not a kernel module: it has no description")


def release_of(data: bytes) -> str:
    """The kernel release a module was built for: the first word of its
    version magic."""
    magic = modinfo(data).get("vermagic", [])
    if len(magic) != 1 or not magic[0].split():
        raise ModuleError("the module names no kernel release")
    return magic[0].split()[0]


def check(data: bytes, release: str) -> None:
    """The encoder driver, built for `release`."""
    info = modinfo(data)
    if info.get("name") != [MODULE_NAME]:
        raise ModuleError(f"not the encoder driver: {info.get('name')}")
    built_for = release_of(data)
    if built_for != release:
        raise ModuleError(f"built for kernel {built_for}, not for {release}")
    # The kernel asks every module that uses its GPL-only symbols for this.
    if info.get("license") != ["GPL"]:
        raise ModuleError(f"unexpected licence: {info.get('license')}")


def image_kernel(root: Path) -> str:
    """The release of the kernel an image boots."""
    link = root / "boot/Image"
    if link.is_symlink():
        target = Path(link.readlink()).name
        if target.startswith("vmlinuz-") and len(target) > len("vmlinuz-"):
            return target.removeprefix("vmlinuz-")
    raise ModuleError("the image does not say which kernel it boots")


def check_image(root: Path) -> str:
    """What an image's tree carries of the driver must be for the image's
    kernel. Returns a line that says what was found."""
    kernel = image_kernel(root)
    modules = root / MODULE_ROOT
    carried = sorted(path for path in modules.rglob("*") if not path.is_dir()) if modules.is_dir() else []
    expected = modules / kernel / MODULE_FILE
    for path in carried:
        if path != expected:
            raise ModuleError(
                f"{path.relative_to(root)} is not the encoder driver for the image's kernel {kernel}"
            )
    if not carried:
        if (root / ENABLED_UNIT).is_symlink() or (root / ENABLED_UNIT).exists():
            raise ModuleError(f"the driver's unit is enabled, and the image has no driver for kernel {kernel}")
        return f"no encoder driver in the image (kernel {kernel})"
    if expected.is_symlink() or not expected.is_file():
        raise ModuleError(f"{expected.relative_to(root)} is not a file")
    check(expected.read_bytes(), kernel)
    return f"encoder driver for kernel {kernel}"


def main(arguments: list[str]) -> int:
    try:
        if len(arguments) == 2 and arguments[0] == "release":
            print(release_of(Path(arguments[1]).read_bytes()))
        elif len(arguments) == 3 and arguments[0] == "check":
            check(Path(arguments[1]).read_bytes(), arguments[2])
        elif len(arguments) == 2 and arguments[0] == "image":
            print(check_image(Path(arguments[1])))
        else:
            print(__doc__, file=sys.stderr)
            return 2
    except (ModuleError, OSError) as error:
        print(f"ve_module: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
