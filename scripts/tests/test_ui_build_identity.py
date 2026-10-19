"""Exercise the actual UI build script using a tiny real Cargo consumer."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("identity_ci", ROOT / "scripts/local-ci.py")
ci = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = ci
SPEC.loader.exec_module(ci)


class UiBuildIdentityTests(unittest.TestCase):
    def test_real_cargo_freshness_source_untracked_changelog_and_raw_fallback(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            package = root / "crates/ui"
            (package / "src").mkdir(parents=True)
            (root / "changelog").mkdir()
            (root / "Cargo.toml").write_text('[workspace]\nmembers=["crates/ui"]\nresolver="2"\n')
            (root / ".gitignore").write_text('/target/\n/artifacts/\n')
            (package / "Cargo.toml").write_text('[package]\nname="identity-fixture"\nversion="0.1.0"\nedition="2024"\n')
            script = (ROOT / "crates/hex-ui/build.rs").read_text()
            (package / "build.rs").write_text(script)
            (package / "src/main.rs").write_text('fn main(){println!("{}|{}", env!("HEX_BUILD_IDENTITY"), String::from_utf8_lossy(include_bytes!(concat!(env!("OUT_DIR"),"/changelog.json"))));}\n')
            def command(*args):
                return subprocess.check_output(args, cwd=root).decode().strip()
            command("git", "init", "-q")
            command("git", "config", "user.name", "identity fixture")
            command("git", "config", "user.email", "fixture@invalid")
            # Lockfile resolution does not compile or use a Cargo target.
            env = {**os.environ, "CARGO_TARGET_DIR": str(root / "target")}
            env.pop("HEX_GUARDED_BUILD_SOURCE", None)
            subprocess.run(["cargo", "generate-lockfile", "--offline"], cwd=root, env=env, check=True, capture_output=True)
            command("git", "add", ".")
            command("git", "commit", "-qm", "fixture")
            count = 0
            def build(*, raw=False):
                nonlocal count
                count += 1
                argv = ("cargo", "build", "--offline", "--locked", "--message-format=json")
                if raw:
                    argv = (sys.executable, "-c", "import os,subprocess; e=dict(os.environ); e.pop('HEX_GUARDED_BUILD_SOURCE',None); raise SystemExit(subprocess.run(" + repr(argv) + ",env=e).returncode)")
                runner = ci.LocalCiRunner(root, mode="component", run_id=f"identity-{count}", env=env,
                                          min_free_bytes=0, steps=(ci.Step("build", argv, uses_cargo_target=True),))
                self.assertEqual(runner.run(), 0, runner.summary)
                text = (runner.output_dir / "01-build.log").read_text()
                events = [json.loads(line) for line in text.splitlines() if line.startswith("{")]
                artifact = next(e for e in events if e.get("reason") == "compiler-artifact" and e.get("executable"))
                return artifact["fresh"], command(artifact["executable"])
            fresh, output = build()
            self.assertFalse(fresh)
            self.assertEqual(output, command("git", "rev-parse", "--short=8", "HEAD") + "|")
            self.assertTrue(build()[0], "identical guarded source must not recompile UI consumer")
            (root / "untracked-input").write_text("new input")
            fresh, output = build()
            self.assertFalse(fresh)
            self.assertIn(" dirty|", output)
            self.assertTrue(build()[0])
            (root / "changelog/generated.json").write_text('{"batches":[]}')
            fresh, output = build()
            self.assertFalse(fresh)
            self.assertTrue(output.endswith('|{"batches":[]}'))
            command("git", "add", ".")
            command("git", "commit", "-qm", "new HEAD and notes")
            fresh, output = build()
            self.assertFalse(fresh)
            self.assertEqual(output, command("git", "rev-parse", "--short=8", "HEAD") + '|{"batches":[]}')
            self.assertTrue(build()[0])
            # Raw Cargo still refreshes truthfully, including untracked dirt.
            (root / "another-input").write_text("dirty")
            self.assertIn(" dirty|", build(raw=True)[1])
            embedded = list((root / "target/debug/build").glob("identity-fixture-*/out/changelog.json"))
            before = {str(p): p.stat().st_mtime_ns for p in embedded}
            raw_fresh, _ = build(raw=True)
            self.assertEqual(before, {str(p): p.stat().st_mtime_ns for p in embedded})
            print("ACTUAL_UI_BUILD raw_repeat_fresh=", raw_fresh)
            # Missing notes remain unavailable; malformed bytes retain their
            # existing embed behavior (the separate changelog guard validates).
            (root / "changelog/generated.json").unlink()
            self.assertTrue(build()[1].endswith("|"))
            (root / "changelog/generated.json").write_text("not-json")
            self.assertTrue(build()[1].endswith("|not-json"))


if __name__ == "__main__":
    unittest.main()
