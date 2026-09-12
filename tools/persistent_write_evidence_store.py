"""Private hash-addressed receipt storage; sealing never approves a policy.

Callers hold the controller lock before entering this store when qualifying a
commit. Partial writes fail closed and remain for inspection, never overwritten.
Reading a seal fsyncs verified contents before a decision may reference them.
"""

from contextlib import contextmanager
from collections.abc import Generator, Mapping
from dataclasses import asdict
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import stat

if not __package__:
    import persistent_write_contract as contract
else:
    from tools import persistent_write_contract as contract

PARENT = Path('/var/lib')
PREFIX = 'hardened-blikvm-write-evidence-'
OWNER_UID = 0
MAX_BYTES = 1048576
MAX_OBJECTS = 64
require = contract.temporary.require


def encoded(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(',', ':'), allow_nan=False).encode()


class Store:
    def __init__(self, binding: contract.EvidenceBinding):
        binding.validate()
        self.binding = binding
        self.root = PARENT / (PREFIX + binding.transaction_id)
        self.header = encoded({'schemaVersion': 1, 'binding': asdict(binding)})

    def directory(self, path: Path, *, private: bool) -> None:
        meta = path.lstat()
        require(path.resolve(strict=True) == path and stat.S_ISDIR(meta.st_mode) and meta.st_uid == OWNER_UID
                and (stat.S_IMODE(meta.st_mode) == 0o700 if private else not meta.st_mode & 0o022), 'unsafe evidence directory')

    @contextmanager
    def locked(self) -> Generator[int, None, None]:
        self.directory(PARENT, private=False)
        self.directory(self.root, private=True)
        fd = os.open(self.root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            fcntl.flock(fd, fcntl.LOCK_EX)
            require(os.fstat(fd).st_ino == self.root.stat().st_ino, 'evidence root changed')
            require(self.read('binding.json') == self.header, 'evidence binding differs')
            self.names()
            yield fd
        finally:
            os.close(fd)

    @classmethod
    def create(cls, binding: contract.EvidenceBinding) -> 'Store':
        store = cls(binding)
        store.directory(PARENT, private=False)
        store.root.mkdir(mode=0o700)  # Exclusive namespace: no existing data overwritten.
        store.root.chmod(0o700)
        store.write_new('binding.json', store.header)
        for path in (store.root, PARENT):
            fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            try: os.fsync(fd)
            finally: os.close(fd)
        return store

    def names(self) -> set[str]:
        names = {p.name for p in self.root.iterdir()}
        require(len(names) <= MAX_OBJECTS + 2 and all(n in {'binding.json', 'seal.json'}
            or re.fullmatch(r'[a-f0-9]{64}\.json', n) for n in names), 'foreign/oversize evidence store')
        return names

    def read(self, name: str, *, durable: bool = False) -> bytes:
        require(name in {'binding.json', 'seal.json'} or re.fullmatch(r'[a-f0-9]{64}\.json', name) is not None,
                'invalid evidence object name')
        fd = os.open(self.root / name, os.O_RDONLY | os.O_NOFOLLOW)
        with os.fdopen(fd, 'rb') as stream:
            meta = os.fstat(stream.fileno())
            require(stat.S_ISREG(meta.st_mode) and meta.st_uid == OWNER_UID and meta.st_nlink == 1
                    and stat.S_IMODE(meta.st_mode) == 0o400 and 0 < meta.st_size <= MAX_BYTES, 'unsafe evidence object')
            raw = stream.read(MAX_BYTES + 1)
            require(len(raw) == meta.st_size, 'evidence changed during read')
            if name not in {'binding.json', 'seal.json'}:
                require(hashlib.sha256(raw).hexdigest() == name[:-5], 'evidence hash differs')
            if durable: os.fsync(stream.fileno())
            return raw

    def write_new(self, name: str, raw: bytes) -> None:
        require(name in {'binding.json', 'seal.json'} or re.fullmatch(r'[a-f0-9]{64}\.json', name) is not None,
                'invalid evidence object name')
        require(0 < len(raw) <= MAX_BYTES, 'evidence size outside bounds')
        fd = os.open(self.root / name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o400)
        with os.fdopen(fd, 'wb') as stream:
            os.fchmod(stream.fileno(), 0o400)
            stream.write(raw); stream.flush(); os.fsync(stream.fileno())

    def put(self, raw: bytes) -> str:
        require(type(raw) is bytes and 0 < len(raw) <= MAX_BYTES, 'invalid receipt bytes')
        digest = hashlib.sha256(raw).hexdigest()
        name = digest + '.json'
        with self.locked() as fd:
            require('seal.json' not in self.names(), 'evidence store is sealed')
            if name in self.names():
                require(self.read(name, durable=True) == raw, 'existing receipt differs')
            else:
                require(len(self.names() - {'binding.json'}) < MAX_OBJECTS, 'evidence object quota exceeded')
                self.write_new(name, raw)
            os.fsync(fd)
        return digest

    def get(self, digest: str) -> bytes:
        require(re.fullmatch(r'[a-f0-9]{64}', digest) is not None, 'invalid receipt digest')
        with self.locked():
            return self.read(digest + '.json')

    def seal(self, gates: Mapping[str, str]) -> str:
        gates = dict(gates)
        require(set(gates) == set(contract.REQUIRED_GATES), 'complete gate references required')
        require(all(re.fullmatch(r'[a-f0-9]{64}', v) is not None for v in gates.values()), 'invalid gate reference')
        raw = encoded({'schemaVersion': 1, 'binding': asdict(self.binding), 'gates': dict(gates)})
        with self.locked() as fd:
            self.read('binding.json', durable=True)
            for digest in gates.values(): self.read(digest + '.json', durable=True)
            for name in self.names() - {'binding.json', 'seal.json'}: self.read(name, durable=True)
            if 'seal.json' in self.names():
                require(self.read('seal.json', durable=True) == raw, 'sealed qualification differs')
            else:
                self.write_new('seal.json', raw)
            os.fsync(fd)
            self.sync_parent()
        return hashlib.sha256(raw).hexdigest()

    def verify_seal(self, gates: Mapping[str, str], expected_sha256: str) -> None:
        gates = dict(gates)
        require(set(gates) == set(contract.REQUIRED_GATES)
                and re.fullmatch(r'[a-f0-9]{64}', expected_sha256) is not None, 'invalid expected seal')
        expected = encoded({'schemaVersion': 1, 'binding': asdict(self.binding), 'gates': gates})
        with self.locked() as fd:
            raw = self.read('seal.json', durable=True)  # Never create a missing seal.
            require(raw == expected and hashlib.sha256(raw).hexdigest() == expected_sha256, 'seal differs')
            self.read('binding.json', durable=True)
            for digest in gates.values():
                require(re.fullmatch(r'[a-f0-9]{64}', digest) is not None, 'invalid gate digest')
                self.read(digest + '.json', durable=True)
            for name in self.names() - {'binding.json', 'seal.json'}: self.read(name, durable=True)
            os.fsync(fd)
            self.sync_parent()

    def sync_parent(self) -> None:
        self.directory(PARENT, private=False)
        fd = os.open(PARENT, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try: os.fsync(fd)
        finally: os.close(fd)
