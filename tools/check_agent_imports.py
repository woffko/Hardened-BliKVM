#!/usr/bin/env python3
"""The Windows agent's search for DLLs (security review of 2026-10-18, HIGH-08).

Windows looks for a program's statically imported DLLs in the program's own
folder first, unless the DLL is one of Windows' known DLLs, an API set, or
ntdll. The agent may run from a folder others can write to (a downloads
folder), so every DLL it imports statically must be one of those, and its
load configuration must ask that DLLs it loads be searched in the system
folder only (DependentLoadFlags = LOAD_LIBRARY_SEARCH_SYSTEM32). Any other
DLL is loaded at run time from the system folder by the agent itself.

usage: check_agent_imports.py AGENT.EXE [...]
"""

from __future__ import annotations

import struct
import sys
from pathlib import Path

# Windows 10 and 11's KnownDLLs that the agent may import statically; the
# loader takes them from the system folder whatever the program's folder holds.
KNOWN_DLLS = frozenset(
    {
        "advapi32.dll",
        "combase.dll",
        "gdi32.dll",
        "kernel32.dll",
        "ole32.dll",
        "oleaut32.dll",
        "rpcrt4.dll",
        "sechost.dll",
        "setupapi.dll",
        "shell32.dll",
        "shlwapi.dll",
        "user32.dll",
        "ws2_32.dll",
    }
)
LOAD_LIBRARY_SEARCH_SYSTEM32 = 0x800


class PeError(Exception):
    pass


def _sections(data: bytes, header: int, count: int, optional_size: int):
    first = header + 24 + optional_size
    for index in range(count):
        offset = first + index * 40
        virtual_size, virtual_address, raw_size, raw_pointer = struct.unpack_from(
            "<IIII", data, offset + 8
        )
        yield virtual_address, max(virtual_size, raw_size), raw_pointer


def _file_offset(sections, rva: int) -> int:
    for virtual_address, size, raw_pointer in sections:
        if virtual_address <= rva < virtual_address + size:
            return raw_pointer + rva - virtual_address
    raise PeError(f"RVA {rva:#x} lies in no section")


def _name(data: bytes, offset: int) -> str:
    end = data.index(b"\0", offset)
    return data[offset:end].decode("ascii")


def inspect(path: Path) -> tuple[list[str], list[str], int | None]:
    """The DLLs imported, those imported with delay, and DependentLoadFlags."""
    data = path.read_bytes()
    if data[:2] != b"MZ":
        raise PeError("not a PE file")
    header = struct.unpack_from("<I", data, 0x3C)[0]
    if data[header : header + 4] != b"PE\0\0":
        raise PeError("no PE signature")
    count, optional_size = struct.unpack_from("<H12xH", data, header + 6)
    optional = header + 24
    if struct.unpack_from("<H", data, optional)[0] != 0x20B:
        raise PeError("not a 64-bit image")
    directories = optional + 112
    sections = list(_sections(data, header, count, optional_size))

    def directory(index: int) -> tuple[int, int]:
        return struct.unpack_from("<II", data, directories + index * 8)

    imports = []
    rva, size = directory(1)
    if rva:
        descriptor = _file_offset(sections, rva)
        while True:
            fields = struct.unpack_from("<IIIII", data, descriptor)
            if not any(fields):
                break
            imports.append(_name(data, _file_offset(sections, fields[3])).lower())
            descriptor += 20
    delayed = []
    rva, size = directory(13)
    if rva:
        descriptor = _file_offset(sections, rva)
        while True:
            fields = struct.unpack_from("<IIIIIIII", data, descriptor)
            if not any(fields):
                break
            delayed.append(_name(data, _file_offset(sections, fields[1])).lower())
            descriptor += 32
    flags = None
    rva, size = directory(10)
    if rva:
        config = _file_offset(sections, rva)
        config_size = struct.unpack_from("<I", data, config)[0]
        if config_size >= 80:
            flags = struct.unpack_from("<H", data, config + 78)[0]
    return imports, delayed, flags


def problems(imports: list[str], delayed: list[str], flags: int | None) -> list[str]:
    found = []
    for name in imports + delayed:
        if name in KNOWN_DLLS or name == "ntdll.dll" or name.startswith(("api-ms-win-", "ext-ms-win-")):
            continue
        found.append(f"{name} is searched in the program's folder first")
    if flags is None or flags & LOAD_LIBRARY_SEARCH_SYSTEM32 == 0:
        found.append("the load configuration does not limit DLL loads to the system folder")
    return found


def main(arguments: list[str]) -> int:
    if not arguments:
        print(__doc__.strip().splitlines()[-1], file=sys.stderr)
        return 2
    failed = False
    for argument in arguments:
        imports, delayed, flags = inspect(Path(argument))
        for problem in problems(imports, delayed, flags):
            print(f"{argument}: {problem}", file=sys.stderr)
            failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
