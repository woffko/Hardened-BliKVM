"""Atomic commit-versus-revocation winner for one root-private transaction.

Publication uses an exclusive hard link to a fully written proposal. A reader
never observes an empty or partial decision, and a late deadline cannot replace
a committed winner. The controller still validates actual acceptance receipts.
"""

from dataclasses import asdict, dataclass
import json
import os
from pathlib import Path
import re
import secrets
import stat
from typing import cast

if not __package__:
    import persistent_write_contract as contract
    import status_api_wiring as status
else:
    from tools import persistent_write_contract as contract
    from tools import status_api_wiring as status

PARENT = Path("/var/lib")
OWNER_UID = 0
require = contract.temporary.require


@dataclass(frozen=True)
class Decision:
    binding: contract.EvidenceBinding
    outcome: str
    receipt_hashes: dict[str, str]

    def validate(self) -> None:
        self.binding.validate()
        require(self.outcome in ("committed", "revoked"), "invalid policy decision")
        expected: frozenset[str] = contract.REQUIRED_GATES if self.outcome == "committed" else frozenset()
        require(frozenset(self.receipt_hashes) == expected, "decision gate set differs")
        require(all(re.fullmatch(r"[a-f0-9]{64}", value) is not None for value in self.receipt_hashes.values()),
                "invalid decision receipt hash")

    def encode(self) -> bytes:
        self.validate()
        return json.dumps({"schemaVersion": 1, **asdict(self)}, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()


def validate_root(root: Path) -> str:
    require(root.parent == PARENT and re.fullmatch(contract.PREFIX + "[a-f0-9]{24}", root.name) is not None,
            "invalid decision root namespace")
    meta = root.lstat()
    require(stat.S_ISDIR(meta.st_mode) and meta.st_uid == OWNER_UID and stat.S_IMODE(meta.st_mode) == 0o700
            and root.resolve(strict=True) == root, "unsafe decision root")
    return root.name[len(contract.PREFIX):]


def read(root: Path) -> Decision | None:
    transaction = validate_root(root)
    path = root / "decision.json"
    try:
        meta = path.lstat()
    except FileNotFoundError:
        return None
    require(stat.S_ISREG(meta.st_mode) and meta.st_uid == OWNER_UID and meta.st_nlink in (1, 2)
            and stat.S_IMODE(meta.st_mode) == 0o600 and meta.st_size <= 65536, "unsafe policy decision")
    raw = path.read_bytes()
    value: object = json.loads(raw)
    require(isinstance(value, dict), "invalid decision object")
    document = cast(dict[str, object], value)
    require(set(document) == {"schemaVersion", "binding", "outcome", "receipt_hashes"}
            and type(document["schemaVersion"]) is int and document["schemaVersion"] == 1, "invalid decision schema")
    binding = document["binding"]
    require(isinstance(binding, dict), "invalid decision binding")
    fields = cast(dict[str, object], binding)
    require(set(fields) == {"transaction_id", "device_fingerprint", "configuration_sha256", "baseline_slot", "baseline_floor", "baseline_generation"},
            "invalid binding fields")
    require(all(isinstance(fields[name], str) for name in ("transaction_id", "device_fingerprint", "configuration_sha256", "baseline_slot"))
            and type(fields["baseline_floor"]) is int and type(fields["baseline_generation"]) is int, "invalid binding types")
    hashes = document["receipt_hashes"]
    require(isinstance(hashes, dict) and isinstance(document["outcome"], str), "invalid decision values")
    parsed_hashes = cast(dict[str, object], hashes)
    require(all(isinstance(value, str) for value in parsed_hashes.values()), "invalid decision hashes")
    result = Decision(contract.EvidenceBinding(cast(str, fields["transaction_id"]), cast(str, fields["device_fingerprint"]),
        cast(str, fields["configuration_sha256"]), cast(str, fields["baseline_slot"]), cast(int, fields["baseline_floor"]),
        cast(int, fields["baseline_generation"])), cast(str, document["outcome"]), cast(dict[str, str], parsed_hashes))
    require(result.binding.transaction_id == transaction and result.encode() == raw, "noncanonical or foreign decision")
    return result


def publish(root: Path, proposal: Decision) -> Decision:
    transaction = validate_root(root)
    require(proposal.binding.transaction_id == transaction, "proposal belongs to another transaction")
    data = proposal.encode()
    candidate = root / ("decision-" + secrets.token_hex(12) + ".part")
    status.u.new_file(candidate, data)
    try:
        try:
            os.link(candidate, root / "decision.json", follow_symlinks=False)
        except FileExistsError:
            pass
        status.u.sync_directory(root)
        winner = read(root)
        require(winner is not None and winner.binding == proposal.binding, "policy decision binding conflict")
        assert winner is not None
        return winner
    finally:
        candidate.unlink()
        status.u.sync_directory(root)
