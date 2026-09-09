"""Native admission fence adapter; no CLI or permission to retain policy.

Only sockets and the systemd manager are operated here. The caller owns the
transaction lock, durable phase/backup, complete runtime validation and guard
installation. This adapter does not stop workers or restart the application.
"""

from pathlib import Path
import subprocess

if not __package__:
    import application_write_observer as observer
    import persistent_write_contract as contract
    import persistent_write_decision as decisions
    import persistent_write_fence as fence
    import persistent_write_files as files
else:
    from tools import application_write_observer as observer
    from tools import persistent_write_contract as contract
    from tools import persistent_write_decision as decisions
    from tools import persistent_write_fence as fence
    from tools import persistent_write_files as files

require = contract.temporary.require


class Admission:
    def __init__(self, root: Path):
        self.root = root
        contract.guard_units(root)

    def require_revoked(self) -> None:
        decision = decisions.read(self.root)
        require(decision is not None and decision.outcome == "revoked", "admission revocation is not durable")

    def reload_manager(self) -> None:
        observer.upload.checked(["systemctl", "daemon-reload"], timeout=15)

    def stop_sockets(self, *, boot: bool) -> None:
        # A blocking stop can deadlock an early-boot guard against downstream
        # jobs. Runtime settlement verifies the completed stop separately.
        argv = ["systemctl", "stop"] + (["--no-block"] if boot else []) + list(contract.SOCKETS)
        observer.upload.checked(argv, timeout=15 if boot else 20)

    def fence_admission(self, *, boot: bool) -> None:
        self.require_revoked()
        self.install_owned_fence(boot=boot)

    def install_owned_fence(self, *, boot: bool) -> None:
        """Primitive for a revoked winner or locked emergency recovery.

        The coordinator must serialize emergency recovery against any native
        retention check/publication using the common controller lock. This
        primitive neither changes nor supplies a successful decision receipt.
        """
        first_error: Exception | None = None
        try:
            self.stop_sockets(boot=boot)
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            # Still attempt a durable-in-/run mask when the first stop fails.
            # Never treat a timeout as evidence that admission has closed.
            first_error = error
        try:
            fence.install(self.root)
            self.reload_manager()
        finally:
            # Also attempt closure after a partial mask install or reload error.
            # A failure leaves all owned masks/metadata in place for recovery.
            self.stop_sockets(boot=boot)
        if not boot:
            self.require_masked_and_closed()
        if first_error is not None:
            raise first_error

    def require_masked_and_closed(self) -> None:
        fence.validate_owned(self.root)
        for name in contract.SOCKETS:
            require(fence.exists(fence.UNIT_ROOT / name), "admission mask missing")
            require(observer.upload.s.property_value(name, "LoadState") == "masked", "runtime mask not loaded")
        observer.require_closed_admission()

    def require_preparer_quiescent(self) -> None:
        name, data = contract.preparer_unit(self.root)
        path = files.UNIT_ROOT / name
        files.safe_directory(files.UNIT_ROOT)
        files.safe_file(path, data)
        fields = observer.upload.properties(name, ("LoadState", "FragmentPath", "DropInPaths", "ActiveState",
            "MainPID", "ControlPID", "Job", "ControlGroup"))
        require(fields["LoadState"] == "loaded" and fields["FragmentPath"] == str(path)
                and fields["DropInPaths"] == "", "owned preparer loaded configuration differs")
        require(fields["ActiveState"] in ("inactive", "failed") and fields["MainPID"] == "0"
                and fields["ControlPID"] == "0" and fields["Job"] in ("", "0")
                and fields["ControlGroup"] == "", "owned preparer or its descendants remain")

    def release_admission_fence(self, backup: dict[str, object]) -> None:
        """Call under the common lock after complete closed-runtime validation."""
        self.require_revoked()
        self.require_preparer_quiescent()
        files.validate_backup(self.root, backup)
        require(all(not files.exists(path) for path in (*files.files(self.root), *files.links())),
                "owned write configuration still present")
        self.require_masked_and_closed()
        try:
            fence.remove(self.root)
            self.reload_manager()
            observer.require_closed_admission()
            observer.upload.u.write_transports_inert()
            self.require_preparer_quiescent()
        except (OSError, ValueError, subprocess.SubprocessError):
            # Do not leave an unsuccessful unmask attempt as the final state.
            self.fence_admission(boot=False)
            raise
