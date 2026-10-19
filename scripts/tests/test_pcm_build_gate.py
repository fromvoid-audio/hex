"""Exercise the real build script with isolated output and explicit compiler inputs."""
from pathlib import Path
import os
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
UNSUPPORTED = (
    "RUSTC_WRAPPER", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS",
    "RUSTC_BOOTSTRAP", "RUST_SYSROOT", "RUSTC_SYSROOT", "SYSROOT", "RUSTC_STAGE",
    "CARGO_UNSTABLE_BUILD_STD", "CARGO_UNSTABLE_BUILD_STD_FEATURES",
    "CARGO_BUILD_RUSTC_WRAPPER", "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
)
STRICT_ARGS = "-D__CLIPPY_HACKERY__warnings__CLIPPY_HACKERY__"


class PcmBuildGateTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.scratch = tempfile.TemporaryDirectory(prefix="hex-pcm-build-gate-")
        cls.base = Path(cls.scratch.name)
        cls.sysroot = Path(subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip())
        cls.compiler = cls.sysroot / "bin/rustc"
        cls.driver = cls.sysroot / "bin/clippy-driver"
        cls.verbose = subprocess.check_output([str(cls.compiler), "-Vv"], text=True)
        cls.host = next(s.removeprefix("host: ") for s in cls.verbose.splitlines() if s.startswith("host: "))
        cls.supported = cls.host in {"x86_64-unknown-linux-gnu", "aarch64-apple-darwin"} and "commit-hash: 4a4ef493e3a1488c6e321570238084b38948f6db\n" in cls.verbose
        cls.binary = cls.base / "build-gate"
        subprocess.run([str(cls.compiler), "--edition=2024", "--deny=warnings", str(ROOT / "crates/hex-engine/build.rs"), "-o", str(cls.binary)], check=True)

    @classmethod
    def tearDownClass(cls):
        cls.scratch.cleanup()

    def run_gate(self, changes=None, *, writable=True):
        env = os.environ.copy()
        for key in (*UNSUPPORTED, "RUSTC_WORKSPACE_WRAPPER", "CLIPPY_ARGS", "CARGO_TARGET_" + self.host.replace("-", "_").upper() + "_RUSTFLAGS"):
            env.pop(key, None)
        with tempfile.TemporaryDirectory(dir=self.base) as out:
            env.update(RUSTC=str(self.compiler), HOST=self.host, TARGET=self.host, OUT_DIR=out if writable else str(Path(out)/"missing"))
            env.update(changes or {})
            result = subprocess.run([str(self.binary)], env=env, cwd=ROOT, text=True, capture_output=True, check=True)
            evidence = Path(out)/"pcm-arc-build-evidence.txt"
            return result.stdout, evidence.read_text() if evidence.exists() else ""

    def test_real_installed_ordinary_and_official_clippy_default_and_strict(self):
        for args in [None, "", STRICT_ARGS]:
            changes = {} if args is None else {"RUSTC_WORKSPACE_WRAPPER":str(self.driver), "CLIPPY_ARGS":args}
            output, evidence = self.run_gate(changes)
            self.assertIn("HEX_PCM_ARC_LAYOUT_KNOWN=" + str(int(self.supported)), output)
            if self.supported:
                self.assertNotIn("cargo:warning=", output)
                self.assertIn("decision: Ok(())", evidence)
                if args is not None:
                    self.assertIn("installed Clippy files unchanged after probes", evidence)
            else:
                self.assertIn("cargo:warning=PCM declared Arc layout Unknown:", output)

    def test_every_unsupported_input_still_refuses_even_with_official_driver(self):
        target_flags="CARGO_TARGET_" + self.host.replace("-", "_").upper() + "_RUSTFLAGS"
        for name in (*UNSUPPORTED, target_flags):
            with self.subTest(name=name):
                output, evidence = self.run_gate({"RUSTC_WORKSPACE_WRAPPER":str(self.driver), "CLIPPY_ARGS":STRICT_ARGS, name:"unsupported"})
                self.assertIn("HEX_PCM_ARC_LAYOUT_KNOWN=0", output)
                self.assertIn("cargo:warning=PCM declared Arc layout Unknown:", output)
                self.assertIn("decision: Err(", evidence)

    def test_arbitrary_clippy_args_lookalike_nested_and_response_files_refuse(self):
        for args in ["@args", STRICT_ARGS+"--sysroot=foreign", "--cfg__CLIPPY_HACKERY__different", "-Zrandomize-layout", "-Dwarnings"]:
            with self.subTest(args=args):
                output, _ = self.run_gate({"RUSTC_WORKSPACE_WRAPPER":str(self.driver), "CLIPPY_ARGS":args})
                self.assertIn("HEX_PCM_ARC_LAYOUT_KNOWN=0", output)
        for wrapper in [str(self.base/"clippy-driver"), str(self.compiler)]:
            output, _ = self.run_gate({"RUSTC_WORKSPACE_WRAPPER":wrapper})
            self.assertIn("HEX_PCM_ARC_LAYOUT_KNOWN=0", output)

    def test_cross_target_and_evidence_write_failure_never_qualify(self):
        output, _ = self.run_gate({"TARGET":"unsupported-target"})
        self.assertIn("HEX_PCM_ARC_LAYOUT_KNOWN=0", output)
        output, _ = self.run_gate(writable=False)
        self.assertIn("HEX_PCM_ARC_LAYOUT_KNOWN=0", output)
        self.assertIn("cargo:warning=PCM declared Arc layout Unknown:", output)
