"""Installed-system observation and admission closure for the HTTP gate.

These helpers never open admission, stop workers, or change application state.
The controller must independently validate the installed unit contract first.
"""

from pathlib import Path

if not __package__:
    import application_write_contract as contract
    import application_write_pipeline as pipeline
    import installed_upload_gate as upload
else:
    from tools import application_write_contract as contract
    from tools import application_write_pipeline as pipeline
    from tools import installed_upload_gate as upload


def unit_busy(unit: str) -> bool:
    value = upload.properties(unit, ("ActiveState", "MainPID", "ControlPID", "Job"))
    contract.require(value["MainPID"].isascii() and value["MainPID"].isdecimal()
                     and value["ControlPID"].isascii() and value["ControlPID"].isdecimal(),
                     "invalid pipeline process observation")
    # Unknown/new states are busy, never interpreted as idle.
    return (value["ActiveState"] not in ("inactive", "failed")
            or int(value["MainPID"]) != 0 or int(value["ControlPID"]) != 0
            or value["Job"] not in ("", "0"))


def live_workers() -> int:
    raw = upload.checked(["systemctl", "list-units", "--all", "--full", "--plain", "--no-legend", "--no-pager",
                          "hardened-blikvm-update-upload@*.service",
                          "hardened-blikvm-update-activation@*.service",
                          "hardened-blikvm-update-confirm@*.service",
                          "hardened-blikvm-update-activate@*.service"], timeout=5)
    contract.require(len(raw) <= 128 * 1024, "oversize worker inventory")
    names: set[str] = set()
    for line in raw.decode().splitlines():
        fields = line.split()
        contract.require(len(fields) >= 4 and pipeline.WORKER.fullmatch(fields[0]) is not None,
                         "invalid pipeline worker inventory")
        contract.require(fields[0] not in names, "duplicate pipeline worker")
        names.add(fields[0])
    return sum(unit_busy(name) for name in sorted(names))


def require_closed_admission() -> None:
    for unit, endpoint in ((contract.UPLOAD, "upload.sock"), (contract.ACTIVATION, "activation.sock")):
        value = upload.properties(unit, ("ActiveState", "SubState", "Job"))
        contract.require((value["ActiveState"], value["SubState"]) in (("inactive", "dead"), ("failed", "failed"))
                         and value["Job"] in ("", "0"), "write admission not closed")
        contract.require(not upload.application.infrastructure.exists(Path("/run/hardened-blikvm-update-api") / endpoint),
                         "write socket endpoint remains")


def close_admission() -> None:
    # Accept=yes instances have independent lifetimes; do not stop/kill their
    # service units. Their installed deadlines and OnSuccess chain must run.
    upload.checked(["systemctl", "stop", contract.ACTIVATION, contract.UPLOAD], timeout=20)
    require_closed_admission()


def observe() -> pipeline.Observation:
    require_closed_admission()
    workers = live_workers()
    raw = upload.checked(["systemctl", "list-jobs", "--full", "--no-legend", "--no-pager", "--plain"], timeout=5)
    contract.require(len(raw) <= 128 * 1024, "oversize systemd job observation")
    jobs = pipeline.queued_pipeline_jobs(raw.decode())
    # A oneshot can still be executing after its start job has disappeared.
    jobs += sum(unit_busy(unit) for unit in sorted(pipeline.APPLY_UNITS - {contract.CURRENT, contract.CANDIDATE}))
    candidate = unit_busy(contract.CANDIDATE)
    runtime = Path("/run/hardened-blikvm-update")
    health = upload.application.infrastructure.exists(runtime / "health-token")
    apply = upload.application.infrastructure.exists(runtime / "confirmed-apply-request.json")
    status = upload.application.infrastructure.current_status()
    require_closed_admission()
    return pipeline.Observation(status, workers, jobs, candidate, health, apply)
