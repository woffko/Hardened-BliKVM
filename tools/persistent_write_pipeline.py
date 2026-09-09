"""Monotonic natural settlement for persistent admission, without slot limits.

The fixed 7-to-8 acceptance observer is reusable, but its settlement policy is
not. This module never stops a worker, modifies the journal, or chooses a slot.
The caller closes admission first and durably records counter high-water marks.
"""

from collections.abc import Callable
from dataclasses import dataclass
import math
import re
import time

if not __package__:
    import application_write_pipeline as observations
else:
    from tools import application_write_pipeline as observations

require = observations.contract.require
MAX_COUNTER = (1 << 64) - 1


@dataclass(frozen=True)
class Counters:
    floor: int
    generation: int

    def validate(self) -> None:
        require(type(self.floor) is int and 0 < self.floor <= MAX_COUNTER
                and type(self.generation) is int and 0 < self.generation <= MAX_COUNTER,
                "invalid persistent pipeline counters")


def read_counters(status: dict[str, object], previous: Counters) -> Counters:
    previous.validate()
    floor, generation = status.get("floorSequence"), status.get("generation")
    require(type(floor) is int and type(generation) is int, "invalid status counters")
    assert isinstance(floor, int) and isinstance(generation, int)
    current = Counters(floor, generation)
    current.validate()
    require(current.floor >= previous.floor and current.generation >= previous.generation,
            "persistent pipeline counters regressed")
    return current


def terminal(observation: observations.Observation, counters: Counters) -> observations.contract.TerminalState | None:
    status = observation.status
    require("pending" in status, "missing pending status")
    require(type(observation.live_workers) is int and observation.live_workers >= 0
            and type(observation.queued_jobs) is int and observation.queued_jobs >= 0
            and all(type(flag) is bool for flag in (observation.candidate_active,
                observation.health_token_present, observation.confirmed_apply_present)), "invalid pipeline activity")
    if (status["pending"] is not None or observation.live_workers or observation.queued_jobs
            or observation.candidate_active or observation.health_token_present or observation.confirmed_apply_present):
        return None
    slot = status.get("confirmedSlotId")
    require(isinstance(slot, str) and re.fullmatch(r"app-[0-9]{20}-[a-f0-9]{64}", slot) is not None,
            "invalid confirmed slot")
    assert isinstance(slot, str)
    require(status.get("activeSlotId") == slot and int(slot.split("-", 2)[1]) == counters.floor,
            "settled slot and floor disagree")
    return observations.contract.TerminalState(slot, counters.floor, counters.generation)


def await_terminal(read: Callable[[], observations.Observation], remember: Callable[[Counters], None], *,
                   observed: Counters, timeout: float = 90,
                   clock: Callable[[], float] = time.monotonic,
                   sleep: Callable[[float], None] = time.sleep) -> observations.contract.TerminalState:
    observed.validate()
    require(type(timeout) in (int, float) and math.isfinite(timeout) and 0 < timeout <= 120,
            "invalid persistent settlement timeout")
    deadline = clock() + timeout
    previous: observations.contract.TerminalState | None = None
    while clock() < deadline:
        observation = read()
        current = read_counters(observation.status, observed)
        if current != observed:
            # Persist before accepting even a busy observation. A persistence
            # failure is fatal: never continue on a merely in-memory floor.
            remember(current)
            observed = current
        value = terminal(observation, observed)
        if value is not None and value == previous:
            return value
        previous = value
        sleep(min(0.25, max(0, deadline - clock())))
    raise TimeoutError("persistent application pipeline did not settle; admission fence retained")
