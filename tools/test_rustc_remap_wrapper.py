import os
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parent.parent
WRAPPER = ROOT / "scripts/rustc-remap-wrapper.sh"


class RustcRemapWrapperTests(unittest.TestCase):
    def test_aarch64_build_enables_wrapper_and_elf_privacy_gate(self) -> None:
        build = (ROOT / "scripts/build-aarch64.sh").read_text()
        verifier = (ROOT / "scripts/verify-aarch64-elf.sh").read_text()
        self.assertIn('RUSTC_WRAPPER="$project_root/scripts/rustc-remap-wrapper.sh"', build)
        self.assertIn("cargo metadata --format-version 1 --locked", build)
        self.assertIn('BLI_KVM_REMAP_PROJECT_ROOT="$project_root"', build)
        self.assertIn('BLI_KVM_REMAP_RUST_SYSROOT="$rust_sysroot"', build)
        self.assertIn('BLI_KVM_REMAP_CARGO_HOME="$cargo_home"', build)
        self.assertIn("host-specific build path entered ELF", verifier)
        self.assertIn("/home/[^/]+/", verifier)
        self.assertIn("/mnt/[A-Za-z]/", verifier)

    def test_wrapper_appends_all_bounded_remaps(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            compiler = root / "compiler"
            compiler.write_text("#!/bin/sh\nprintf '%s\\n' \"$@\"\n")
            compiler.chmod(0o755)
            environment = os.environ.copy()
            environment.update(
                {
                    "BLI_KVM_REMAP_PROJECT_ROOT": "/build/project",
                    "BLI_KVM_REMAP_RUST_SYSROOT": "/toolchain",
                    "BLI_KVM_REMAP_CARGO_HOME": "/cargo",
                }
            )
            result = subprocess.run(
                [str(WRAPPER), str(compiler), "--crate-name", "fixture"],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                env=environment,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            arguments = result.stdout.decode().splitlines()
            self.assertEqual(arguments[:2], ["--crate-name", "fixture"])
            self.assertIn(
                "--remap-path-prefix=/build/project=/usr/src/hardened-blikvm",
                arguments,
            )
            self.assertIn(
                "--remap-path-prefix=/toolchain=/usr/src/rust-toolchain",
                arguments,
            )
            self.assertIn(
                "--remap-path-prefix=/cargo=/usr/src/cargo-home",
                arguments,
            )

    def test_wrapper_rejects_missing_or_relative_roots(self) -> None:
        environment = os.environ.copy()
        environment.update(
            {
                "BLI_KVM_REMAP_PROJECT_ROOT": "relative",
                "BLI_KVM_REMAP_RUST_SYSROOT": "/toolchain",
            }
        )
        result = subprocess.run(
            [str(WRAPPER), "/bin/true"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=environment,
            check=False,
        )
        self.assertEqual(result.returncode, 2)


if __name__ == "__main__":
    _ = unittest.main()
