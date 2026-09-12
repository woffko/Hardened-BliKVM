"""Ordering contract for a held-upload restoration gate; no device I/O here."""

from collections.abc import Callable
from dataclasses import dataclass
import re
from typing import Protocol

if not __package__:
    import persistent_write_contract as contract
else:
    from tools import persistent_write_contract as contract

require = contract.temporary.require


@dataclass(frozen=True)
class Worker:
    unit: str
    invocation: str
    pid: int

    def validate(self) -> None:
        require(re.fullmatch(r'hardened-blikvm-update-upload@[^/\s]+\.service', self.unit) is not None,
                'not an upload instance')
        require(re.fullmatch(r'[a-f0-9]{32}', self.invocation) is not None,
                'invalid worker invocation')
        require(type(self.pid) is int and self.pid > 1, 'invalid worker pid')


class Operations(Protocol):
    def hold_upload(self) -> Worker: ...
    def verify_live_spool(self, worker: Worker) -> None: ...
    def submit_restore(self) -> None: ...
    def await_closed_owned_fence(self) -> None: ...
    def restore_still_running(self) -> bool: ...
    def release_to_idle(self) -> None: ...
    def await_probe_idle_timeout(self) -> None: ...
    def natural_worker_exit(self, worker: Worker) -> tuple[int, int]: ...
    def await_restore(self) -> dict[str, object]: ...
    def verify_preserved_terminal(self) -> None: ...
    def close_probe_client(self) -> None: ...


def exercise(ops: Operations, receipt: dict[str, object], save: Callable[[], None]) -> dict[str, object]:
    """Submit restore once, never signal workers, never infer an idle fixture passed.

    Native adapters must bound every wait and pin the actual worker invocation.
    ``restoreSubmissionAttempted`` is durable *before* submission; exceptions
    deliberately propagate without another restore. The existing native guard
    remains responsible for recovery after an ambiguous reply.
    """
    require(not receipt.get('restoreSubmissionAttempted'), 'restore was already attempted')
    receipt.update(result='failed', fixture='held-upload-idle-timeout', completeArchiveSent=False)
    save()
    try:
        worker = ops.hold_upload()
        worker.validate()
        ops.verify_live_spool(worker)
        receipt['worker'] = {'unit': worker.unit, 'invocation': worker.invocation, 'pid': worker.pid}
        receipt['restoreSubmissionAttempted'] = True
        save()
        ops.submit_restore()
        ops.await_closed_owned_fence()
        # Both checks precede release. An already-finished restore or a worker
        # replaced/killed before fencing cannot pass as pending-work recovery.
        ops.verify_live_spool(worker)
        require(ops.restore_still_running(), 'restore completed before held worker settled')
        receipt['fencedWhileWorkerLive'] = True
        save()
        ops.release_to_idle()
        ops.await_probe_idle_timeout()
        code, status = ops.natural_worker_exit(worker)
        require(type(code) is int and type(status) is int and (code, status) == (1, 50),
                'worker did not exit through natural upload timeout')
        receipt['workerNaturalExit'] = {'code': code, 'status': status}
        restored = ops.await_restore()
        require(restored.get('result') == 'restored' and restored.get('runtimeVerified') is True
                and restored.get('boot') is False and restored.get('retentionApproved') is False,
                'native restoration not verified')
        ops.verify_preserved_terminal()
        receipt.update(restored=restored, terminalPreserved=True)
        save()
    finally:
        # Only the client belongs to this fixture. No worker-stop/kill method
        # is exposed, and no second restore is issued during teardown.
        ops.close_probe_client()
    receipt['result'] = 'passed'
    save()
    return receipt
