#!/usr/bin/env python3
"""Select musl's self-relocating CRT objects for direct static-PIE links."""

from __future__ import annotations

import os
from pathlib import Path
import sys
from typing import NoReturn


CRT_REPLACEMENTS = {
    "crt1.o": "rcrt1.o",
    "crtbegin.o": "crtbeginS.o",
    "crtend.o": "crtendS.o",
}
STATIC_PIE_CRT = frozenset(CRT_REPLACEMENTS.values())


def _is_rust_self_contained_crt(path: Path) -> bool:
    return path.parent.name == "self-contained" and "rustlib" in path.parts


def rewrite_link_args(arguments: list[str]) -> list[str]:
    """Rewrite only a direct musl ``-static`` + ``-pie`` executable link."""

    rewritten = list(arguments)
    if "-static" not in rewritten or "-pie" not in rewritten:
        return rewritten

    selected: set[str] = set()
    for index, argument in enumerate(rewritten):
        path = Path(argument)
        name = path.name
        if name in CRT_REPLACEMENTS:
            if not _is_rust_self_contained_crt(path):
                raise ValueError("refusing to replace a non-rustc CRT object")
            replacement = path.with_name(CRT_REPLACEMENTS[name])
            if not replacement.is_file():
                raise ValueError("required static-PIE CRT object is unavailable")
            rewritten[index] = str(replacement)
            selected.add(replacement.name)
        elif name in STATIC_PIE_CRT:
            if not _is_rust_self_contained_crt(path):
                raise ValueError("unexpected non-rustc static-PIE CRT object")
            selected.add(name)

    if selected != set(STATIC_PIE_CRT):
        raise ValueError("rustc did not provide a complete static-PIE CRT sequence")
    if any(Path(argument).name in CRT_REPLACEMENTS for argument in rewritten):
        raise ValueError("a non-relocating CRT object remained in the static-PIE link")
    return rewritten


def main() -> NoReturn:
    if len(sys.argv) < 2:
        raise SystemExit("usage: rust_lld_static_pie.py RUST_LLD [ARG ...]")
    linker = Path(sys.argv[1]).resolve(strict=True)
    if not linker.is_file() or not os.access(linker, os.X_OK):
        raise SystemExit("rust-lld is not executable")
    try:
        arguments = rewrite_link_args(sys.argv[2:])
    except ValueError as error:
        raise SystemExit(str(error)) from error
    os.execv(linker, [str(linker), *arguments])


if __name__ == "__main__":
    main()
