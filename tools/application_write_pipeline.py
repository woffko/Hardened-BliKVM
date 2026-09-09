"""Read-only, bounded observation of the installed activation pipeline.

Admission must already be closed by the caller. No worker is killed and no
journal, token or apply receipt is modified by this module.
"""

from collections.abc import Callable
from dataclasses import dataclass
import re
import time

if not __package__:
    import application_write_contract as contract
else:
    from tools import application_write_contract as contract

WORKER = re.compile(r"hardened-blikvm-update-(?:activation|upload|confirm|activate)@[A-Za-z0-9_.:\\-]+\.service")
APPLY_UNITS = frozenset((
    "hardened-blikvm-update-apply-candidate.service",
    "hardened-blikvm-update-apply-confirmed.service",
    contract.CURRENT, contract.CANDIDATE,
))


def relevant_unit(name: str) -> bool:
    return name in APPLY_UNITS or WORKER.fullmatch(name) is not None


def queued_pipeline_jobs(raw: str) -> int:
    """Parse systemctl list-jobs --no-legend --no-pager --plain output.

The recurring watchdog timer is not itself an outstanding application apply.
Its effects are covered by the token, receipt and journal observations.
"""
    count = 0
    for line in raw.splitlines():
        if not line.strip():
            continue
        fields = line.split()
        contract.require(len(fields) == 4 and fields[0].isascii() and fields[0].isdecimal()
                         and fields[3] in ("waiting", "running"), "invalid systemd job observation")
        contract.require("…" not in fields[1] and "..." not in fields[1], "truncated systemd job unit")
        count += int(relevant_unit(fields[1]))
    return count


@dataclass(frozen=True)
class Observation:
    status: dict[str, object]
    live_workers: int
    queued_jobs: int
    candidate_active: bool
    health_token_present: bool
    confirmed_apply_present: bool

    def terminal(self, observed_floor: int) -> contract.TerminalState:
        contract.require_pipeline_idle(
            activation_workers=self.live_workers, queued_jobs=self.queued_jobs,
            candidate_active=self.candidate_active, health_token_present=self.health_token_present,
            confirmed_apply_present=self.confirmed_apply_present)
        return contract.terminal_state(self.status, observed_floor=observed_floor)


def await_terminal(read: Callable[[], Observation], remember_floor: Callable[[int], None], *,
                   observed_floor: int = 7, timeout: float = 90,
                   clock: Callable[[], float] = time.monotonic,
                   sleep: Callable[[float], None] = time.sleep) -> contract.TerminalState:
    """Require two identical terminal observations, retaining monotonic floor.

Persist an observed floor increase even when health/apply work is outstanding.
Read/parse failures propagate; only a valid but unsettled observation retries.
The caller retains its recovery guard on every failure.
"""
    contract.require(type(observed_floor) is int and observed_floor in (7, 8), "invalid observed floor")
    contract.require(0 < timeout <= 120, "invalid settlement timeout")
    deadline = clock() + timeout
    previous: contract.TerminalState | None = None
    while clock() < deadline:
        observation = read()
        floor = observation.status.get("floorSequence")
        contract.require(type(floor) is int and floor in (7, 8) and floor >= observed_floor,
                         "observed floor regressed or escaped gate")
        assert isinstance(floor, int)
        if floor > observed_floor:
            remember_floor(floor)
            observed_floor = floor
        try:
            terminal = observation.terminal(observed_floor)
        except ValueError:
            previous = None
        else:
            if previous == terminal:
                return terminal
            previous = terminal
        sleep(min(0.25, max(0, deadline - clock())))
    raise TimeoutError("application pipeline did not settle; recovery guard retained")
