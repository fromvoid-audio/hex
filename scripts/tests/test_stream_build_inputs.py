"""Run the actual source-observation script with pinned files and config refusals."""
import importlib.util
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from test_local_ci import install_delivery_source_fixture

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("stream_input_ci", ROOT / "scripts/local-ci.py")
ci = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = ci
source = (ROOT / "scripts/local-ci.py").read_bytes()
ci._DELIVERY_RUNNER_SOURCE_SHA256 = hashlib.sha256(source).hexdigest()
exec(compile(source, str(ROOT / "scripts/local-ci.py"), "exec", dont_inherit=True), ci.__dict__)


class StreamBuildInputsTests(unittest.TestCase):
    def test_actual_pins_guarded_missing_paths_and_all_existing_config_refusals(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            compiler = root / "compiler"
            (compiler / "src").mkdir(parents=True)
            install_delivery_source_fixture(compiler)
            (compiler / ".gitignore").write_text('/target/\n/artifacts/\n')
            (compiler / "Cargo.toml").write_text('[package]\nname="stream-build-observer"\nversion="0.1.0"\nedition="2024"\n[dependencies]\nsha2="=0.10.9"\n')
            (compiler / "src/main.rs").write_bytes((ROOT / "crates/hex-stream/build.rs").read_bytes())
            env = {**os.environ, "CARGO_TARGET_DIR": str(compiler / "target")}
            def command(*argv):
                return subprocess.check_output(argv, cwd=compiler, env=env).decode().strip()
            command("git", "init", "-q")
            command("git", "config", "user.name", "stream fixture")
            command("git", "config", "user.email", "fixture@invalid")
            command("cargo", "generate-lockfile", "--offline")
            command("git", "add", ".")
            command("git", "commit", "-qm", "actual observer source")
            runner = ci.LocalCiRunner(compiler, mode="component", run_id="observer", env=env, min_free_bytes=0,
                                      steps=(ci.Step("compile", ("cargo", "build", "--offline", "--locked")),))
            self.assertEqual(runner.run(), 0, runner.summary)
            home = root / "cargo-home"
            registry = home / "registry/src/fixture/rtrb-0.3.4/src"
            registry.mkdir(parents=True)
            actual_home = Path(os.environ.get("CARGO_HOME", str(Path.home() / ".cargo")))
            source = list((actual_home / "registry/src").glob("*/rtrb-0.3.4/src"))
            self.assertEqual(len(source), 1, "required pinned rtrb source unavailable")
            for name in ("lib.rs", "cache_padded.rs", "chunks.rs"):
                shutil.copyfile(source[0] / name, registry / name)
            observed = root / "observed"
            cwd = observed / "crates/source"
            cwd.mkdir(parents=True)
            (observed / "Cargo.lock").write_bytes((ROOT / "Cargo.lock").read_bytes())
            binary = compiler / "target/debug/stream-build-observer"
            def observe(*, guarded=True, changes=None):
                selected = {k: v for k, v in env.items() if not k.startswith("CARGO_SOURCE_")}
                selected.pop("CARGO_REGISTRIES_CRATES_IO_INDEX", None)
                selected.pop("HEX_GUARDED_CARGO_CONFIG_FINGERPRINT", None)
                selected["CARGO_HOME"] = str(home)
                if guarded:
                    selected["HEX_GUARDED_CARGO_CONFIG_FINGERPRINT"] = "a" * 64
                selected.update(changes or {})
                return subprocess.check_output([str(binary)], cwd=cwd, env=selected).decode()
            marker = "cargo:rustc-cfg=hex_stream_rtrb_source_observation_matches"
            self.assertIn(marker, observe())
            self.assertIn(marker, observe(guarded=False))
            missing = "cargo:rerun-if-changed=" + str(home / "config.toml")
            self.assertNotIn(missing, observe())
            self.assertIn(missing, observe(guarded=False))
            for path in (home / "config.toml", observed / ".cargo/config", observed.parent / ".cargo/config.toml"):
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("unsupported config")
                self.assertNotIn(marker, observe())
                self.assertNotIn(marker, observe(guarded=False))
                path.unlink()
            for name in ("CARGO_SOURCE_CRATES_IO_REPLACE_WITH", "CARGO_REGISTRIES_CRATES_IO_INDEX", "CARGO_SOURCE_OTHER"):
                self.assertNotIn(marker, observe(changes={name: "unsupported"}))
            (registry / "lib.rs").write_text("tampered")
            self.assertNotIn(marker, observe())
            print("ACTUAL_STREAM_INPUTS: pins PASS; guarded/raw missing watches differ; ancestor/home configs, source overrides and tamper all refused")


if __name__ == "__main__":
    unittest.main()
