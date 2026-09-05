#!/usr/bin/env python3
"""Identity-bound publication and recoverable quarantine of initial update roots."""

from __future__ import annotations

import ctypes
from dataclasses import asdict, dataclass
import json
import os
from pathlib import Path
import stat
from typing import cast


ROOT_NAMES = {"slots": "hardened-blikvm-slots", "state": "hardened-blikvm-update"}
ROOT_MODES = {"slots": 0o750, "state": 0o700}


@dataclass(frozen=True)
class Identity:
    device: int
    inode: int
    uid: int
    gid: int
    mode: int


def identity(path: Path) -> Identity:
    metadata = path.lstat()
    if not stat.S_ISDIR(metadata.st_mode):
        raise ValueError("bootstrap root is not a real directory")
    return Identity(metadata.st_dev, metadata.st_ino, metadata.st_uid, metadata.st_gid,
                    stat.S_IMODE(metadata.st_mode))


def sync_directory(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def rename_owned(source: Path, destination: Path, expected: Identity) -> None:
    if identity(source) != expected:
        raise ValueError("bootstrap directory identity changed")
    libc = ctypes.CDLL(None, use_errno=True)
    rename = libc.renameat2
    rename.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
    rename.restype = ctypes.c_int
    source_fd = os.open(source.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    destination_fd = os.open(destination.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        # RENAME_NOREPLACE prevents replacing even an empty foreign directory.
        result = rename(source_fd, os.fsencode(source.name), destination_fd, os.fsencode(destination.name), 1)
        if result != 0:
            raise OSError(ctypes.get_errno(), "bootstrap no-replace rename failed")
        os.fsync(source_fd)
        os.fsync(destination_fd)
        if identity(destination) != expected:
            raise ValueError("bootstrap renamed directory failed identity verification")
    finally:
        os.close(destination_fd)
        os.close(source_fd)


class BootstrapRoots:
    """Caller holds the exclusive guard lock and stops its units before rollback.

    The production caller fixes parent=/var/lib and a root-owned private guard;
    explicit parent permits unprivileged filesystem regression tests.
    """

    def __init__(self, guard: Path, parent: Path, binding: str):
        if len(binding) != 64 or any(character not in "0123456789abcdef" for character in binding):
            raise ValueError("bootstrap binding must be SHA-256")
        if guard.resolve(strict=True) != guard or parent.resolve(strict=True) != parent:
            raise ValueError("bootstrap parents must be canonical")
        guard_identity = identity(guard)
        parent_identity = identity(parent)
        if (guard_identity.uid != os.geteuid() or guard_identity.mode != 0o700
                or parent_identity.uid != os.geteuid() or parent_identity.mode & 0o022
                or guard_identity.device != parent_identity.device):
            raise ValueError("bootstrap parent ownership, mode or filesystem is invalid")
        self.guard = guard
        self.parent = parent
        self.binding = binding
        self.marker = guard / "roots.json"
        self.guard_identity = guard_identity
        self.parent_identity = parent_identity

    def validate_parents(self) -> None:
        if identity(self.guard) != self.guard_identity or identity(self.parent) != self.parent_identity:
            raise ValueError("bootstrap parent was replaced or changed")

    def save(self, document: dict[str, object]) -> None:
        self.validate_parents()
        path = self.guard / "roots.json.part"
        if path.exists() or path.is_symlink():
            # Only a prior valid marker permits discarding an incomplete update.
            self.load()
            meta = path.lstat()
            if (not stat.S_ISREG(meta.st_mode) or meta.st_uid != os.geteuid() or meta.st_nlink != 1
                    or stat.S_IMODE(meta.st_mode) != 0o600 or meta.st_size > 4096):
                raise ValueError("unsafe incomplete bootstrap marker")
            path.unlink()
            sync_directory(self.guard)
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        with os.fdopen(descriptor, "w") as stream:
            json.dump(document, stream, sort_keys=True, separators=(",", ":"))
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(path, self.marker)
        sync_directory(self.guard)

    def load(self) -> dict[str, object]:
        self.validate_parents()
        descriptor = os.open(self.marker, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
        with os.fdopen(descriptor, "rb") as stream:
            meta = os.fstat(stream.fileno())
            if (not stat.S_ISREG(meta.st_mode) or meta.st_uid != os.geteuid() or meta.st_nlink != 1
                    or stat.S_IMODE(meta.st_mode) != 0o600 or not 0 < meta.st_size <= 4096):
                raise ValueError("unsafe bootstrap marker")
            decoded = cast(object, json.loads(stream.read(4097)))
        if not isinstance(decoded, dict):
            raise ValueError("invalid bootstrap marker root")
        document = cast(dict[str, object], decoded)
        if (set(document) != {"schemaVersion", "binding", "phase", "roots"}
                or type(document.get("schemaVersion")) is not int or document.get("schemaVersion") != 1
                or document.get("binding") != self.binding
                or document.get("phase") not in {"prepared", "published", "quarantined", "committed"}):
            raise ValueError("invalid bootstrap marker")
        roots = document.get("roots")
        if not isinstance(roots, dict) or set(cast(dict[str, object], roots)) != set(ROOT_NAMES):
            raise ValueError("invalid bootstrap identities")
        for name, value in cast(dict[str, object], roots).items():
            if not isinstance(value, dict) or set(cast(dict[str, object], value)) != {"device", "inode", "uid", "gid", "mode"}:
                raise ValueError("invalid bootstrap identity")
            if any(type(item) is not int or item < 0 for item in cast(dict[str, object], value).values()):
                raise ValueError("invalid bootstrap identity values")
            if value["mode"] != ROOT_MODES[name]:
                raise ValueError("invalid bootstrap root mode")
        return document

    def prepare(self, uid: int, gid: int) -> None:
        self.validate_parents()
        if self.marker.exists() or self.marker.is_symlink():
            raise ValueError("bootstrap already prepared")
        for name in ROOT_NAMES.values():
            path = self.parent / name
            if path.exists() or path.is_symlink():
                raise ValueError("production root already exists")
        identities: dict[str, object] = {}
        for name, mode in ROOT_MODES.items():
            path = self.guard / ("prepared-" + name)
            path.mkdir(mode=mode)
            path.chmod(mode)
            os.chown(path, uid, gid)
            sync_directory(path)
            identities[name] = asdict(identity(path))
        self.save({"schemaVersion": 1, "binding": self.binding, "phase": "prepared", "roots": identities})

    def locations(self, document: dict[str, object]) -> list[tuple[str, Path, Identity]]:
        self.validate_parents()
        found: list[tuple[str, Path, Identity]] = []
        roots = cast(dict[str, dict[str, int]], document["roots"])
        for name, live_name in ROOT_NAMES.items():
            expected = Identity(**roots[name])
            candidates = [self.guard / ("prepared-" + name), self.parent / live_name,
                          self.guard / ("quarantined-" + name)]
            present = [path for path in candidates if path.exists() or path.is_symlink()]
            if len(present) != 1 or identity(present[0]) != expected:
                raise ValueError("bootstrap root missing, foreign or duplicated")
            found.append((name, present[0], expected))
        return found

    def publish(self) -> None:
        document = self.load()
        if document["phase"] not in {"prepared", "published"}:
            raise ValueError("bootstrap cannot be published in this phase")
        locations = self.locations(document)
        if any(path.name.startswith("quarantined-") for _, path, _ in locations):
            raise ValueError("quarantined bootstrap cannot be republished")
        for name, path, expected in locations:
            target = self.parent / ROOT_NAMES[name]
            if path != target:
                rename_owned(path, target, expected)
        document["phase"] = "published"
        self.save(document)

    def quarantine(self) -> None:
        document = self.load()
        if document["phase"] == "committed":
            raise ValueError("committed bootstrap must not be rolled back")
        for name, path, expected in self.locations(document):
            target = self.guard / ("quarantined-" + name)
            if path != target:
                rename_owned(path, target, expected)
        document["phase"] = "quarantined"
        self.save(document)

    def commit(self) -> None:
        document = self.load()
        if document["phase"] not in {"published", "committed"}:
            raise ValueError("bootstrap is not published")
        locations = self.locations(document)
        if any(path != self.parent / ROOT_NAMES[name] for name, path, _ in locations):
            raise ValueError("bootstrap roots are not both live")
        document["phase"] = "committed"
        self.save(document)
