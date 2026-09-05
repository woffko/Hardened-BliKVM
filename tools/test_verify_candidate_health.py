from pathlib import Path
import unittest

from tools import verify_candidate_health as gate


class CandidateHealthTests(unittest.TestCase):
    def test_renderer_retains_hardening_and_isolates_mutable_paths(self) -> None:
        template = (Path(__file__).resolve().parent.parent / "packaging/systemd/hardened-blikvm-server-candidate.service").read_text()
        root = Path("/var/lib/blikvm-candidate-health-" + "a" * 24)
        runtime = Path("/run") / root.name
        rendered = gate.render_candidate(template, root, runtime, alias=True)
        for setting in ("NoNewPrivileges=yes", "ProtectSystem=strict", "CapabilityBoundingSet=\n",
                        "MemoryDenyWriteExecute=yes", "SystemCallFilter=@system-service", "User=hardened-blikvm-server"):
            self.assertIn(setting, rendered)
        self.assertNotIn("Conflicts=", rendered)
        self.assertNotIn("StateDirectory=", rendered)
        self.assertNotIn("hardened-blikvm-update-confirm.socket", rendered)
        self.assertIn(f"BLI_KVM_STATE_DIR={root}/server-state", rendered)
        self.assertIn(f"ExecStart={root}/server", rendered)
        self.assertIn(f"--confirm-socket {runtime}/confirm.sock", rendered)
        self.assertIn("BindReadOnlyPaths=/run/credentials/%n:/run/hardened-blikvm-candidate-credentials/%n", rendered)
        negative = gate.render_candidate(template, root, runtime, alias=False)
        self.assertNotIn("BindReadOnlyPaths=", negative)
        self.assertIn("--credential-directory", negative)

    def test_missing_template_binding_is_rejected(self) -> None:
        with self.assertRaises(ValueError):
            gate.render_candidate("[Service]\nExecStart=/bin/true\n", Path("/var/lib/test"), Path("/run/test"), alias=True)


if __name__ == "__main__":
    unittest.main()
