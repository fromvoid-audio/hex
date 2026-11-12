"""Exercise the actual UI build script using a tiny real Cargo consumer."""
import importlib.util
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from test_ci_target_storage import reuse_fixed_scratch_observations
from test_local_ci import install_delivery_source_fixture

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("identity_ci", ROOT / "scripts/local-ci.py")
ci = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = ci
source = (ROOT / "scripts/local-ci.py").read_bytes()
ci._DELIVERY_RUNNER_SOURCE_SHA256 = hashlib.sha256(source).hexdigest()
exec(compile(source, str(ROOT / "scripts/local-ci.py"), "exec", dont_inherit=True), ci.__dict__)
DELIVERY_SPEC = importlib.util.spec_from_file_location("identity_delivery", ROOT / "scripts/hex-test-build.py")
delivery = importlib.util.module_from_spec(DELIVERY_SPEC)
sys.modules[DELIVERY_SPEC.name] = delivery
DELIVERY_SPEC.loader.exec_module(delivery)


class UiBuildIdentityTests(unittest.TestCase):
    __hex_shard_early__ = True

    def setUp(self):
        reuse_fixed_scratch_observations(self, ci, delivery.ci)
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = self.root = Path(temporary.name).resolve()
        install_delivery_source_fixture(root)
        package = root / "crates/ui"
        (package / "src").mkdir(parents=True)
        (root / "changelog").mkdir()
        (root / "assets/fonts").mkdir(parents=True)
        font = self.font = root / "assets/fonts/fixture-font.data"
        font.write_text("old-font")
        (root / "Cargo.toml").write_text(
            '[workspace]\nmembers=["crates/ui"]\nresolver="2"\n'
            '[profile.dev]\ndebug=0\n')
        (root / ".gitignore").write_text('/target/\n/release-target/\n/artifacts/\n')
        (package / "Cargo.toml").write_text('[package]\nname="identity-fixture"\nversion="0.1.0"\nedition="2024"\n')
        script = (ROOT / "crates/hex-ui/build.rs").read_text()
        (package / "build.rs").write_text(script)
        (package / "src/main.rs").write_text(
            'fn main() {\n'
            '    let font = include_bytes!("../../../assets/fonts/fixture-font.data");\n'
            '    let notes = include_bytes!(concat!(env!("OUT_DIR"), "/changelog.json"));\n'
            '    println!("{}", env!("HEX_DISPLAY_VERSION"));\n'
            '    println!("{}", String::from_utf8_lossy(font));\n'
            '    println!("{}|{}", env!("HEX_BUILD_IDENTITY"), String::from_utf8_lossy(notes));\n'
            '}\n')

        self.command("git", "init", "-q")
        self.command("git", "config", "user.name", "identity fixture")
        self.command("git", "config", "user.email", "fixture@invalid")
        self.env = {**os.environ, "CARGO_TARGET_DIR": str(root / "target")}
        for key in ("HEX_GUARDED_BUILD_SOURCE", "HEX_RELEASE_CHANGELOG", "HEX_RELEASE_LABEL"):
            self.env.pop(key, None)
        subprocess.run(["cargo", "generate-lockfile", "--offline"], cwd=root, env=self.env,
                       check=True, capture_output=True)
        self.commit("fixture")
        self.count = 0
        self.notes = root / "changelog/generated.json"

    def command(self, *args):
        return subprocess.check_output(args, cwd=self.root, text=True).strip()

    def commit(self, message):
        self.command("git", "add", ".")
        self.command("git", "commit", "-qm", message)

    def inspect_artifact(self, text):
        events = (json.loads(line) for line in text.splitlines() if line.startswith("{"))
        artifact = next(e for e in events if e.get("reason") == "compiler-artifact" and e.get("executable"))
        self.display_version, embedded_font, identity = self.command(artifact["executable"]).split("\n", 2)
        self.assertEqual(embedded_font, self.font.read_text())
        return artifact["fresh"], identity

    def build(self, *, raw=False, optimized=False):
        self.count += 1
        argv = ("cargo", "build", "--offline", "--locked", "--message-format=json")
        if raw:
            result = subprocess.run(argv, cwd=self.root, env=self.env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            return self.inspect_artifact(result.stdout)
        if optimized:
            runner = delivery.ci.LocalCiRunner(self.root, run_id=f"optimized-identity-{self.count}",
                env={**self.env, "CARGO_TARGET_DIR": str(self.root / "release-target")}, min_free_bytes=0)
            with delivery.optimized_artifact(runner, package="identity-fixture") as (_, ownership):
                result = self.inspect_artifact(Path(ownership["build_log"]).read_text())
            self.cleanup = ownership["cleanup_status"]
            self.release_owner = Path(ownership["owner_path"])
            return result
        runner = ci.LocalCiRunner(self.root, mode="component", run_id=f"identity-{self.count}",
            env=self.env, min_free_bytes=0, steps=(ci.Step("build", argv, uses_cargo_target=True),))
        self.assertEqual(runner.run(), 0, runner.summary)
        self.cleanup = runner.summary["target_cache"]["cleanup_status"]
        return self.inspect_artifact((runner.output_dir / "01-build.log").read_text())

    def assert_dirty_optimized_refused(self):
        # A refused dirty build preserves both the READY owner and artifacts.
        owner_before = self.release_owner.read_bytes()

        def artifacts():
            return {str(p.relative_to(self.root)): (hashlib.sha256(p.read_bytes()).hexdigest(),
                    p.stat().st_mtime_ns) for p in (self.root / "release-target").rglob("*") if p.is_file()}

        before = artifacts()
        with self.assertRaisesRegex(delivery.ci.TargetCacheError, "requires exact clean source"):
            self.build(optimized=True)
        self.assertEqual(self.release_owner.read_bytes(), owner_before)
        self.assertEqual(artifacts(), before)

    @staticmethod
    def rewrite_with_old_mtime(path, content):
        stamp = path.stat()
        path.write_text(content)
        os.utime(path, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))

    def test_real_cargo_freshness_source_untracked_changelog_and_raw_fallback(self):
        fresh, output = self.build()
        self.assertFalse(fresh)
        self.assertEqual(output, self.command("git", "rev-parse", "--short=8", "HEAD") + "|")
        self.assertTrue(self.build()[0], "identical guarded source must not recompile UI consumer")
        (self.root / "untracked-input").write_text("new input")
        fresh, output = self.build()
        self.assertFalse(fresh)
        self.assertIn(" dirty|", output)
        self.assertTrue(self.build()[0])
        dirty_head = self.command("git", "rev-parse", "HEAD")

        # Same-HEAD dirty changes with restored mtime still reach actual build.rs
        # and the Rust asset embedding.
        self.rewrite_with_old_mtime(self.font, "new-font")
        self.assertFalse(self.build()[0])
        self.assertEqual(self.cleanup, "not_needed")
        self.assertEqual(self.command("git", "rev-parse", "HEAD"), dirty_head)
        self.notes.write_text('{"batches":[]}')
        fresh, output = self.build()
        self.assertFalse(fresh)
        self.assertTrue(output.endswith('|{"batches":[]}'))
        self.commit("new HEAD and notes")
        fresh, output = self.build()
        self.assertFalse(fresh)
        self.assertEqual(output, self.command("git", "rev-parse", "--short=8", "HEAD") + '|{"batches":[]}')
        self.assertTrue(self.build()[0])
        self.rewrite_with_old_mtime(self.notes, '{"batches": []}')
        self.assertTrue(self.build()[1].endswith('|{"batches": []}'))
        self.assertEqual(self.cleanup, "not_needed")
        self.commit("notes changed with retained mtime")

        (self.root / "another-input").write_text("dirty")
        self.assertIn(" dirty|", self.build(raw=True)[1])
        embedded = list((self.root / "target/debug/build").glob("identity-fixture-*/out/changelog.json"))
        before = {str(p): p.stat().st_mtime_ns for p in embedded}
        raw_fresh, _ = self.build(raw=True)
        self.assertEqual(before, {str(p): p.stat().st_mtime_ns for p in embedded})
        print("ACTUAL_UI_BUILD raw_repeat_fresh=", raw_fresh)
        # Missing notes are unavailable and malformed bytes retain their
        # embed behavior. The changelog guard separately validates the bytes.
        self.notes.unlink()
        self.assertTrue(self.build()[1].endswith("|"))
        self.notes.write_text("not-json")
        self.assertTrue(self.build()[1].endswith("|not-json"))

    def test_optimized_identity_refuses_dirty_source_and_embeds_frozen_changelog(self):
        self.assertFalse(self.build(optimized=True)[0])
        self.assertTrue(self.build(optimized=True)[0])
        (self.root / "untracked-input").write_text("new input")
        self.assert_dirty_optimized_refused()
        self.rewrite_with_old_mtime(self.font, "new-font")
        self.assert_dirty_optimized_refused()
        self.notes.write_text('{"batches":[]}')
        self.commit("new HEAD and notes")
        self.assertFalse(self.build(optimized=True)[0])
        self.assertEqual(self.cleanup, "not_needed")
        self.assertTrue(self.build(optimized=True)[0])
        self.rewrite_with_old_mtime(self.notes, '{"batches": []}')
        self.assert_dirty_optimized_refused()
        self.commit("notes changed with retained mtime")
        self.assertTrue(self.build(optimized=True)[1].endswith('|{"batches": []}'))
        self.assertEqual(self.cleanup, "not_needed")

        # A frozen release projection is a compiler input, including a
        # same-path content change with the original timestamp.
        frozen = self.root / "artifacts/release-notes.json"
        frozen.write_text('{"format":1,"batches":[]}')
        self.env["HEX_RELEASE_CHANGELOG"] = str(frozen)
        self.env["HEX_RELEASE_LABEL"] = "0.1.0-beta.1"
        fresh, output = self.build(optimized=True)
        self.assertFalse(fresh)
        self.assertEqual(self.display_version, "0.1.0-beta.1")
        self.assertTrue(output.startswith(self.command("git", "rev-parse", "--short=8", "HEAD")))
        self.assertTrue(output.endswith('|{"format":1,"batches":[]}'))
        self.rewrite_with_old_mtime(frozen, '{"format":1, "batches":[]}')
        fresh, output = self.build(optimized=True)
        self.assertFalse(fresh)
        self.assertTrue(output.endswith('|{"format":1, "batches":[]}'))


if __name__ == "__main__":
    unittest.main()
