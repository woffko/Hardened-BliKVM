"""Read the Longrun v1 two-secret envelope without printing its contents.

Only reviewed coordinators should use this helper. Decoded bytes are kept
exactly as staged; individual consumers apply their existing password rules.
Wiping mutable buffers is best effort, not secure erasure of Python memory.
"""

from __future__ import annotations

import base64
from contextlib import contextmanager
import json
import sys
from typing import BinaryIO, Generator, cast

MAX_ENVELOPE_BYTES = 65536
ROLES = {"ssh", "gui"}


def _object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for name, value in pairs:
        if name in result:
            raise ValueError("duplicate envelope field")
        result[name] = value
    return result


def _mapping(value: object) -> dict[str, object]:
    if not isinstance(value, dict):
        raise ValueError("invalid envelope object")
    return cast(dict[str, object], value)


def clear_secret_pair(values: dict[str, bytearray]) -> None:
    for value in values.values():
        value[:] = b"\0" * len(value)


@contextmanager
def secret_pair_stdin(stream: BinaryIO | None = None) -> Generator[dict[str, bytearray], None, None]:
    """Yield exactly SSH and GUI bytes, wiping decoded buffers on every exit."""
    values: dict[str, bytearray] = {}
    try:
        try:
            source = sys.stdin.buffer if stream is None else stream
            raw = source.read(MAX_ENVELOPE_BYTES + 1)
            if not raw or len(raw) > MAX_ENVELOPE_BYTES:
                raise ValueError("invalid envelope size")
            envelope = _mapping(json.loads(raw.decode("ascii"), object_pairs_hook=_object))
            if (set(envelope) != {"schemaVersion", "encoding", "secrets"}
                    or type(envelope["schemaVersion"]) is not int or envelope["schemaVersion"] != 1
                    or envelope["encoding"] != "base64"):
                raise ValueError("invalid envelope schema")
            secrets = _mapping(envelope["secrets"])
            if set(secrets) != ROLES:
                raise ValueError("invalid envelope roles")
            for role in sorted(ROLES):
                encoded = secrets[role]
                if not isinstance(encoded, str) or not encoded:
                    raise ValueError("invalid secret encoding")
                decoded = bytearray(base64.b64decode(encoded, validate=True))
                values[role] = decoded
                if not decoded or base64.b64encode(decoded).decode("ascii") != encoded:
                    raise ValueError("noncanonical secret encoding")
        except (OSError, ValueError, TypeError, KeyError, RecursionError):
            raise ValueError("invalid secret stdin pair") from None
        # Keep our own mapping so consumer pop/clear/reassignment cannot lose
        # the references needed to clear the original decoded buffers.
        yield dict(values)
    finally:
        clear_secret_pair(values)
