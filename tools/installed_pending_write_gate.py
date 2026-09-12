#!/usr/bin/env python3
"""One held upload versus real restore; never complete an archive or activate."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
from typing import cast

sys.dont_write_bytecode = True
if not __package__:
    import installed_persistent_write as native
    import persistent_write_pending as pending
else:
    from tools import installed_persistent_write as native
    from tools import persistent_write_pending as pending

upload = native.temporary.upload
require = native.require


def profile_for(binding: native.contract.EvidenceBinding) -> upload.probe.UploadProfile:
    binding.validate()
    name = {8: 'seq8-publish', 9: 'seq9-retry'}.get(binding.baseline_floor)
    require(name is not None, 'unreviewed pending baseline')
    assert name is not None
    profile = upload.probe.PROFILES[name]
    require(profile.slot == binding.baseline_slot and binding.baseline_generation == {8: 40, 9: 43}[binding.baseline_floor],
            'pending profile binding differs')
    return profile


class Operations:
    def __init__(self, root: Path, source: Path) -> None:
        native.u.select_profile('dev7-dev9')
        self.phase = 'initialization'
        self.root, self.source = root, source
        self.client: subprocess.Popen[bytes] | None = None
        self.restore: subprocess.Popen[bytes] | None = None
        self.reference: upload.UploadUnitReference | None = None
        self.baseline = native.load(root)
        require(self.baseline['phase'] == 'wired', 'temporary policy is not wired')
        self.binding = native.binding(self.baseline)
        self.profile = profile_for(self.binding)

    def hold_upload(self) -> pending.Worker:
        self.phase = 'probe-start'
        previous = set(upload.workers())
        require(native.temporary.observer.live_workers() == 0,
                'pre-existing pipeline worker')
        self.client = subprocess.Popen(['runuser', '-u', 'hardened-blikvm-server', '--',
            'python3', '-B', str(self.source / 'probe.py'), 'idle', '--hold',
            '--archive', str(self.source / 'application.tar'), '--profile', self.profile.name],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, bufsize=0)
        assert self.client.stdout is not None
        self.phase = 'probe-handshake'
        upload.held_handshake(self.client.stdout)
        self.phase = 'worker-identification'
        selected = set(upload.workers()) - previous
        require(len(selected) == 1, 'cannot correlate one pending upload instance')
        unit = selected.pop()
        self.reference = upload.UploadUnitReference(unit)
        self.phase = 'worker-sandbox'
        live = upload.wait_live(unit)
        return pending.Worker(unit, cast(str, live['invocationId']), cast(int, live['pid']))

    def verify_live_spool(self, worker: pending.Worker) -> None:
        self.phase = 'held-worker-proof'
        live = upload.live_worker(worker.unit)
        require(live['invocationId'] == worker.invocation and live['pid'] == worker.pid
                and live['anonymousSpool'] is True, 'held worker identity/spool changed')

    def submit_restore(self) -> None:
        self.phase = 'restore-submission'
        require(self.restore is None, 'restore already submitted')
        self.restore = subprocess.Popen(['python3', '-B', str(self.root / 'controller.py'),
            'restore', '--guard-root', str(self.root)], stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)

    def await_closed_owned_fence(self) -> None:
        self.phase = 'fence-observation'
        deadline = time.monotonic() + 12
        while time.monotonic() < deadline:
            require(self.restore_still_running(), 'restore ended before live-fence observation')
            try:
                native.systemd.Admission(self.root).require_masked_and_closed()
                return
            except (ValueError, FileNotFoundError):
                time.sleep(0.1)
        raise TimeoutError('owned fence not observed while body held')

    def restore_still_running(self) -> bool:
        return self.restore is not None and self.restore.poll() is None

    def release_to_idle(self) -> None:
        require(self.client is not None and self.client.poll() is None, 'probe not held')
        assert self.client is not None and self.client.stdin is not None
        self.client.stdin.write(b'G')
        self.client.stdin.flush()

    def await_probe_idle_timeout(self) -> None:
        self.phase = 'probe-idle-timeout'
        assert self.client is not None
        raw, _ = self.client.communicate(timeout=25)
        require(self.client.returncode == 0 and len(raw) <= 1024 and
                json.loads(raw) == {'mode': 'idle', 'result': 'passed'}, 'idle probe did not pass')

    def natural_worker_exit(self, worker: pending.Worker) -> tuple[int, int]:
        self.phase = 'worker-natural-exit'
        value = upload.natural_exit(worker.unit, 'idle', worker.invocation)
        require(value['naturalExit'] is True and value['exitCode'] == 50, 'worker timeout not proved')
        return 1, 50  # natural_exit requires ExecMainCode=1 and exact InvocationID.

    def await_restore(self) -> dict[str, object]:
        self.phase = 'restore-completion'
        assert self.restore is not None
        raw, _ = self.restore.communicate(timeout=180)
        require(self.restore.returncode == 0 and len(raw) <= 65536, 'native restoration failed')
        value = native.object_value(json.loads(raw))
        require(value.get('guardRoot') == str(self.root), 'restoration root differs')
        return value

    def verify_preserved_terminal(self) -> None:
        self.phase = 'terminal-verification'
        upload.empty_spool()
        observation = native.temporary.observer.observe()
        terminal = native.pipeline.terminal(observation, native.pipeline.Counters(self.binding.baseline_floor, self.binding.baseline_generation))
        require(terminal is not None and terminal.slot == self.binding.baseline_slot
                and terminal.floor == self.binding.baseline_floor and terminal.generation == self.binding.baseline_generation,
                'pending gate terminal differs')
        require(observation.status.get('floorSequence') == self.binding.baseline_floor
                and observation.status.get('generation') == self.binding.baseline_generation,
                'pending gate counters changed')
        current = native.load(self.root)
        require(current['phase'] == 'restored', 'native plan not restored')
        # Native restore validates the retained immutable snapshot and settings;
        # Host independently repeats the selected baseline observation after cleanup.
        require(native.binding(current) == native.binding(self.baseline), 'transaction binding changed')

    def close_probe_client(self) -> None:
        try:
            if self.client is not None:
                if self.client.poll() is None:
                    if self.client.stdin is not None: self.client.stdin.close()
                    # Closing input makes await_release reject EOF and close only
                    # the client's socket. Never stop the installed worker unit.
                    self.client.wait(timeout=35)
                for stream in (self.client.stdin, self.client.stdout):
                    if stream is not None and not stream.closed: stream.close()
        finally:
            if self.reference is not None:
                self.reference.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--guard-root', required=True)
    parser.add_argument('--source', required=True)
    parser.add_argument('--output', required=True)
    args = parser.parse_args()
    root = Path(args.guard_root)
    native.u.select_profile('dev7-dev9')
    transaction = native.decisions.validate_root(root)
    profile = profile_for(native.binding(native.load(root)))
    source = Path(args.source)
    require(source == Path('/tmp') / ('blikvm-pending-probe-' + transaction)
            and not source.exists() and not source.is_symlink(), 'fresh owned probe source required')
    stage = Path(__file__).resolve().parent
    require(stage.stat().st_uid == 0 and stage.stat().st_mode & 0o777 == 0o700, 'unsafe helper staging')
    payloads = {'probe.py': Path(upload.probe.__file__).read_bytes(), 'application.tar': (stage / 'application.tar').read_bytes()}
    require(len(payloads['application.tar']) == upload.probe.BUNDLE_BYTES and
            hashlib.sha256(payloads['application.tar']).hexdigest() == profile.bundle_sha,
            'only the baseline-bound archive may be used')
    output = Path(args.output)
    require(output.parent == Path(__file__).resolve().parent and not output.exists(), 'fresh staged receipt required')
    receipt: dict[str, object] = {'result': 'failed', 'guardRoot': str(root)}
    def save() -> None:
        part = output.with_suffix('.part')
        fd = os.open(part, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, 'w') as stream:
            json.dump(receipt, stream, sort_keys=True); stream.flush(); os.fsync(stream.fileno())
        os.replace(part, output)
        native.u.sync_directory(output.parent)
    created = False
    ops: Operations | None = None
    try:
        source.mkdir(mode=0o755)
        created = True
        for name, data in payloads.items():
            fd = os.open(source / name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o444)
            with os.fdopen(fd, 'wb') as stream:
                stream.write(data); stream.flush(); os.fsync(stream.fileno())
        native.u.sync_directory(source)
        ops = Operations(root, source)
        pending.exercise(ops, receipt, save)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        receipt.update(result='failed', errorType=type(error).__name__)
        receipt['phase'] = ops.phase if ops is not None else 'source-preparation'
        save()
    finally:
        if created:
            try:
                require(not source.is_symlink() and source.stat().st_uid == 0 and
                        set(p.name for p in source.iterdir()) <= set(payloads), 'foreign probe source')
                for name, data in payloads.items():
                    path = source / name
                    if path.exists() or path.is_symlink():
                        require(not path.is_symlink() and path.stat().st_uid == 0
                                and path.stat().st_nlink == 1 and path.read_bytes() == data, 'probe source changed')
                for name in payloads: (source / name).unlink(missing_ok=True)
                source.rmdir()
                receipt['probeSourceRemoved'] = True
            except (OSError, ValueError) as error:
                receipt.update(result='failed', sourceCleanupErrorType=type(error).__name__)
            save()
    print(json.dumps(receipt, sort_keys=True))
    return 0 if receipt['result'] == 'passed' else 1


if __name__ == '__main__':
    raise SystemExit(main())
