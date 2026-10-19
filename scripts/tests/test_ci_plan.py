"""Scope boundaries, input freshness, and zero-match qualification regressions."""
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


SCRIPTS = Path(__file__).resolve().parents[1]


def module(name, file):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / file)
    value = importlib.util.module_from_spec(spec)
    sys.modules[name] = value
    spec.loader.exec_module(value)
    return value


plan = module("scope_fixture", "ci_plan.py")
inputs = module("build_inputs_fixture", "build_inputs.py")
ci = module("runner_scope_fixture", "local-ci.py")


class ScopeTests(unittest.TestCase):
    def test_current_review_future_timestamp_content_change_is_unproven(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text("fixture")
            source = root / "lib.rs"
            source.write_text("old")
            import time
            future = time.time_ns() + 3600 * 10**9
            os.utime(source, ns=(future, future))
            def git(*argv):
                return subprocess.check_output(["git", *argv], cwd=root).decode().strip()
            git("init", "-q");git("config", "user.name", "timestamp fixture");git("config", "user.email", "fixture@invalid")
            git("add", ".");git("commit", "-qm", "old")
            metadata = {"workspace_members": ["fixture"], "packages": [
                {"id": "fixture", "manifest_path": str(root / "Cargo.toml")}]}
            before = inputs.workspace_snapshot(root, os.environ, metadata)
            before.update(checked=True, commit=git("rev-parse", "HEAD"), dirty=False)
            source.write_text("new")
            os.utime(source, ns=(future, future))
            git("add", ".");git("commit", "-qm", "new content same future mtime")
            after = inputs.workspace_snapshot(root, os.environ, metadata)
            self.assertNotEqual(before["packages"][0]["sha256"], after["packages"][0]["sha256"])
            self.assertEqual(before["packages"][0]["timestamps"], after["packages"][0]["timestamps"])
            self.assertEqual(inputs.stale_workspace_packages(root, os.environ, before, after), ["fixture"])

    def test_current_review_native_build_environment_variants_are_semantic(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            metadata = {"workspace_members": [], "packages": [], "target_directory": str(root / "target")}
            env = {"CARGO_HOME": str(root / "home")}
            families = ("CC", "CXX", "AR", "RANLIB", "NVCC", "CFLAGS", "CPPFLAGS", "CXXFLAGS", "ARFLAGS", "RANLIBFLAGS", "LDFLAGS", "CXXSTDLIB", "CXXSTDLIB_STATIC")
            names = [key for name in families for key in (name, "HOST_" + name, "TARGET_" + name,
                name + "_aarch64-apple-darwin", name + "_aarch64_apple_darwin")]
            names += ["CRATE_CC_NO_DEFAULTS", "CROSS_COMPILE", "CC_SHELL_ESCAPED_FLAGS", "CC_KNOWN_WRAPPER_CUSTOM",
                      "SYSROOT", "CLIPPY_ARGS"]
            for name in names:
                with self.subTest(name=name):
                    self.assertNotEqual(inputs.fingerprint(root, {**env, name: "FIRST"}, metadata, {}),
                                        inputs.fingerprint(root, {**env, name: "SECOND"}, metadata, {}))
            self.assertEqual(inputs.fingerprint(root, {**env, "CARGO_TARGET_DIR": "first"}, metadata, {}),
                             inputs.fingerprint(root, {**env, "CARGO_TARGET_DIR": "second"}, metadata, {}))

    def test_nested_single_component_member_owns_its_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "a").mkdir()
            for path in ("Cargo.toml", "a/Cargo.toml", "a/lib.rs"):
                (root / path).write_text("original")
            subprocess.run(["git", "init", "-q"], cwd=root, check=True)
            subprocess.run(["git", "add", "."], cwd=root, check=True)
            metadata = {"workspace_members": ["root", "child"], "packages": [
                {"id": "root", "manifest_path": str(root / "Cargo.toml")},
                {"id": "child", "manifest_path": str(root / "a/Cargo.toml")}]}
            before = inputs.workspace_snapshot(root, os.environ, metadata)
            (root / "a/lib.rs").write_text("changed")
            after = inputs.workspace_snapshot(root, os.environ, metadata)
            changed = {p["spec"]: p["sha256"] != q["sha256"] for p, q in zip(before["packages"], after["packages"])}
            self.assertEqual(changed, {"child": True, "root": False})

    def test_review_target_directory_is_ownership_not_semantic_config(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            metadata = {"workspace_members": [], "packages": [], "target_directory": str(root / "ci-target")}
            env = {"CARGO_HOME": str(root / "home"), "CARGO_TARGET_DIR": str(root / "ci-target")}
            before = inputs.fingerprint(root, env, metadata, {})
            moved = {**env, "CARGO_TARGET_DIR": str(root / "release-target")}
            self.assertEqual(before, inputs.fingerprint(root, moved, {**metadata, "target_directory": moved["CARGO_TARGET_DIR"]}, {}))
            self.assertNotEqual(before, inputs.fingerprint(root, {**env, "RUSTFLAGS": "--cfg actually_changed"}, metadata, {}))

    def test_review_workspace_restored_mtime_runs_current_test_and_normal_edit_stays_incremental(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "src").mkdir()
            (root / "Cargo.toml").write_text('[package]\nname="workspace-source-fixture"\nversion="0.1.0"\nedition="2021"\n')
            (root / ".gitignore").write_text('/target/\n/artifacts/\n')
            source = root / "src/lib.rs"
            source.write_text('#[test] fn old_case() {}\n')
            def command(*argv):
                return subprocess.check_output(argv, cwd=root).decode().strip()
            command("git", "init", "-q");command("git", "config", "user.name", "workspace fixture");command("git", "config", "user.email", "fixture@invalid")
            command("cargo", "generate-lockfile", "--offline");command("git", "add", ".");command("git", "commit", "-qm", "old test")
            def run(name):
                runner = ci.LocalCiRunner(root, mode="component", run_id=name, min_free_bytes=0,
                    env={**os.environ, "CARGO_TARGET_DIR": str(root / "target")},
                    steps=(ci.Step("list", ("cargo", "test", "--offline", "--locked", "--", "--list")),))
                self.assertEqual(runner.run(), 0)
                return runner, (runner.output_dir / "01-list.log").read_text()
            first, log = run("old")
            self.assertIn("old_case: test", log)
            stamp = source.stat()
            source.write_text('#[test] fn new_case() {}\n')
            os.utime(source, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
            command("git", "add", ".");command("git", "commit", "-qm", "same mtime new test")
            second, log = run("restored")
            self.assertIn("new_case: test", log)
            self.assertNotIn("old_case: test", log)
            source.write_text('#[test] fn now_case() {}\n')
            command("git", "add", ".");command("git", "commit", "-qm", "ordinary later edit")
            third, log = run("ordinary")
            self.assertIn("now_case: test", log)
            self.assertEqual(third.summary["target_cache"]["cleanup_status"], "not_needed")

    def test_review_metadata_report_refused_by_locked_release_receiver_before_pending(self):
        delivery = module("metadata_receiver_fixture", "hex-test-build.py")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "src").mkdir()
            (root / "src/lib.rs").write_text("pub fn value() {}\n")
            (root / "Cargo.toml").write_text('[package]\nname="metadata-receiver"\nversion="0.1.0"\nedition="2021"\n')
            (root / ".gitignore").write_text('/target/\n/artifacts/\n')
            (root / "scripts").mkdir()
            (root / "scripts/ci_plan.py").write_bytes((SCRIPTS / "ci_plan.py").read_bytes())
            def command(*argv):
                return subprocess.check_output(argv, cwd=root).decode().strip()
            command("git", "init", "-q");command("git", "config", "user.name", "receiver fixture");command("git", "config", "user.email", "fixture@invalid")
            command("cargo", "generate-lockfile", "--offline");command("git", "add", ".");command("git", "commit", "-qm", "runtime")
            base = command("git", "rev-parse", "HEAD")
            (root / "README.md").write_text("Prose-only change.\n")
            command("git", "add", ".");command("git", "commit", "-qm", "metadata")
            candidate = command("git", "rev-parse", "HEAD")
            metadata = json.loads(command("cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"))
            resolved = plan.resolve(root, profile="merge", base=base, source=candidate, metadata=metadata)
            self.assertEqual(resolved["scope"], "metadata")
            env = {**os.environ, "CARGO_TARGET_DIR": str(root / "target")}
            checked = delivery.ci.LocalCiRunner(root, mode="merge", plan=resolved, env=env, run_id="metadata", min_free_bytes=0)
            self.assertEqual(checked.run(), 0)
            self.assertNotIn("target_cache", checked.summary)
            receiver = delivery.ci.LocalCiRunner(root, env=env, run_id="receiver", min_free_bytes=0)
            def validate():
                self.assertIsNotNone(receiver.target_lock)
                delivery.validate_ci(checked.summary_path, receiver, candidate, allow_dev_profile=True)
            with self.assertRaisesRegex(delivery.DeliveryError, "toolchain/configuration"):
                with receiver.guarded_release(requested_build_argv=["cargo", "build", "--release", "--locked"], verification_check=validate):
                    self.fail("metadata-only report reached PENDING/build")
            self.assertIsNone(receiver.target_lock)
            self.assertNotIn("release_target_cache", receiver.summary)
            self.assertFalse((root / "target").exists())

    def test_review_changed_wrapper_rebuilds_unchanged_dependency(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for folder in ("app/src", "vendor/src"):
                (root / folder).mkdir(parents=True)
            (root / "Cargo.toml").write_text('[workspace]\nmembers=["app"]\nexclude=["vendor"]\nresolver="2"\n')
            (root / "app/Cargo.toml").write_text('[package]\nname="tool-consumer"\nversion="0.1.0"\nedition="2021"\n[dependencies]\ntool-dependency={path="../vendor"}\n')
            (root / "vendor/Cargo.toml").write_text('[package]\nname="tool-dependency"\nversion="0.1.0"\nedition="2021"\n')
            (root / "vendor/src/lib.rs").write_text('pub fn value()->u32 { 7 }\n')
            (root / "app/src/main.rs").write_text('fn main(){println!("{}",tool_dependency::value());}\n')
            (root / ".gitignore").write_text('/target/\n/release-target/\n/artifacts/\n/wrapper\n/compiler.log\n')
            wrapper = root / "wrapper"
            log = root / "compiler.log"
            def write_wrapper(label):
                wrapper.write_text('#!/usr/bin/env python3\nimport os,sys\n'
                    'if "--crate-name" in sys.argv:\n'
                    f' with open({str(log)!r},"a") as f:f.write({label!r}+":"+sys.argv[sys.argv.index("--crate-name")+1]+"\\n")\n'
                    'os.execv(sys.argv[1],sys.argv[1:])\n')
                wrapper.chmod(0o755)
            write_wrapper("first")
            env = {**os.environ, "CARGO_TARGET_DIR": str(root / "target"), "RUSTC_WRAPPER": str(wrapper)}
            def command(*argv):
                return subprocess.check_output(argv, cwd=root, env=env).decode().strip()
            command("git", "init", "-q");command("git", "config", "user.name", "compiler fixture");command("git", "config", "user.email", "fixture@invalid")
            command("cargo", "generate-lockfile", "--offline");command("git", "add", ".");command("git", "commit", "-qm", "source")
            def run(name):
                runner = ci.LocalCiRunner(root, mode="component", run_id=name, env=env, min_free_bytes=0,
                    steps=(ci.Step("build", ("cargo", "build", "--offline", "--locked")),))
                self.assertEqual(runner.run(), 0, runner.summary)
                return runner
            run("old-tool")
            self.assertIn("first:tool_dependency", log.read_text())
            log.write_text("")
            write_wrapper("second")
            run("new-tool")
            self.assertIn("second:tool_dependency", log.read_text())
            log.write_text("")
            run("same-tool")
            self.assertNotIn(":tool_dependency", log.read_text())
            self.assertNotIn(":tool_consumer", log.read_text())
            delivery = module("compiler_release_fixture", "hex-test-build.py")
            def release(name):
                runner = delivery.ci.LocalCiRunner(root, env={**env, "CARGO_TARGET_DIR": str(root / "release-target")}, run_id=name, min_free_bytes=0)
                with delivery.optimized_artifact(runner, package="tool-consumer") as (binary, ownership):
                    self.assertEqual(command(str(binary)), "7")
                return ownership
            release("old-release-tool")
            log.write_text("")
            write_wrapper("third")
            release("new-release-tool")
            self.assertIn("third:tool_dependency", log.read_text())
            log.write_text("")
            reused = release("same-release-tool")
            self.assertNotIn(":tool_dependency", log.read_text())
            self.assertNotIn(":tool_consumer", log.read_text())
            self.assertEqual(reused["cleanup_status"], "not_needed")

    def test_review_default_tool_relative_and_empty_path_uses_cargo_cwd(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            root, caller = base / "repo", base / "caller"
            for parent in (root, caller):
                (parent / "bin").mkdir(parents=True)
                for name in ("cargo", "rustc"):
                    for folder in (parent, parent / "bin"):
                        path = folder / name
                        path.write_text("#!/bin/sh\nprintf 'same stable version\\n'\n")
                        path.chmod(0o755)
            original_cwd = Path.cwd()
            try:
                os.chdir(caller)
                for selected in ("bin", ""):
                    runner = ci.LocalCiRunner(root, env={"PATH": selected, "CARGO_HOME": str(base / "home")})
                    runner._release_metadata = {"workspace_members": [], "packages": [], "target_directory": str(base / "target")}
                    before = runner._release_build_inputs(["fixture"])
                    actual = root / selected / "rustc"
                    with self.subTest(selected=selected):
                        self.assertEqual(runner.summary["toolchain"]["rustc"]["path"], str(actual))
                    actual.write_text(actual.read_text() + "# replaced actual repo compiler\n")
                    with self.subTest(mutation=selected):
                        self.assertNotEqual(before, runner._release_build_inputs(["fixture"]))
            finally:
                os.chdir(original_cwd)

    def test_review_selected_tool_override_bytes_participate_in_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tool = root / "selected-compiler"
            tool.write_text("#!/bin/sh\nexit 0\n")
            tool.chmod(0o755)
            metadata = {"workspace_members": [], "packages": [], "target_directory": str(root / "target")}
            for name in ("RUSTC", "CARGO_BUILD_RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER",
                         "CARGO_BUILD_RUSTC_WRAPPER", "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER"):
                env = {"CARGO_HOME": str(root / "home"), name: str(tool)}
                before = inputs.fingerprint(root, env, metadata, {})
                tool.write_text(tool.read_text() + "# replaced executable\n")
                self.assertNotEqual(before, inputs.fingerprint(root, env, metadata, {}), name)
            env = {"CARGO_HOME": str(root / "home"), "RUSTC": tool.name, "PATH": "."}
            before = inputs.fingerprint(root, env, metadata, {})
            tool.write_text(tool.read_text() + "# relative PATH replacement\n")
            self.assertNotEqual(before, inputs.fingerprint(root, env, metadata, {}))
            tool.chmod(0o644)
            with self.assertRaisesRegex(ValueError, "executable"):
                inputs.fingerprint(root, env, metadata, {})

    def test_review_relative_cargo_home_is_rooted_at_cargo_cwd(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            metadata = {"workspace_members": [], "packages": [], "target_directory": str(root / "target")}
            env = {"CARGO_HOME": "relative-home"}
            before = inputs.fingerprint(root, env, metadata, {})
            home = root / "relative-home"
            home.mkdir()
            (home / "config.toml").write_text("unsupported config")
            self.assertNotEqual(before, inputs.fingerprint(root, env, metadata, {}))
            (home / "config.toml").write_text('[build]\nrustc="untracked-compiler"\n')
            with self.assertRaisesRegex(ValueError, "compiler/wrapper"):
                inputs.fingerprint(root, env, metadata, {})

    def test_review_qualifying_executable_source_symlinks_refuse(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "repo"
            root.mkdir()
            def git(*args):
                return subprocess.check_output(["git", *args], cwd=root).decode().strip()
            git("init", "-q"); git("config", "user.name", "source fixture"); git("config", "user.email", "fixture@invalid")
            (root / "Cargo.toml").write_text("fixture")
            (root / ".gitignore").write_text("/artifacts/\n")
            (root / "runtime.rs").write_text("base")
            git("add", "."); git("commit", "-qm", "base")
            base = git("rev-parse", "HEAD")
            external = root.parent / "external.rs"
            external.write_text("external input")
            (root / "runtime.rs").unlink()
            (root / "runtime.rs").symlink_to(external)
            git("add", "."); git("commit", "-qm", "linked candidate")
            metadata = {"workspace_members": ["fixture"], "packages": [dict(id="fixture", name="fixture",
                        manifest_path=str(root / "Cargo.toml"), features={}, targets=[], dependencies=[])]}
            with self.assertRaisesRegex(ValueError, "symlink"):
                plan.resolve(root, profile="merge", base=base, source="HEAD", metadata=metadata)
            runner = ci.LocalCiRunner(root, mode="full", min_free_bytes=0,
                                      steps=(ci.Step("must-not-run", (sys.executable, "-c", "raise AssertionError()")),))
            self.assertEqual(runner.run(), 2)
            self.assertIn("symlink", runner.summary["repository_error"])
            self.assertTrue(all(s["status"] == "not_run" for s in runner.summary["steps"]))

    def inventory(self):
        return [dict(name=name, manifest=f"crates/{name}/Cargo.toml", features={},
                     dependencies=deps, targets=[]) for name, deps in (
                         ("hex-widgets", []),
                         ("hex-ui", [{"name": "hex-widgets", "kind": "dev", "optional": True}]),
                         ("hex-desktop", [{"name": "hex-ui", "kind": None}]))]

    def test_known_scopes_keep_prose_and_all_dev_reverse_dependencies(self):
        inventory = self.inventory()
        scope, affected, _ = plan.classify(["crates/hex-widgets/src/a.rs", "docs/a.md"], inventory)
        self.assertEqual(scope, "ui")
        self.assertEqual(affected, ["hex-desktop", "hex-ui", "hex-widgets"])
        self.assertEqual(plan.classify(["launch", "docs/a.md"], inventory)[0], "tooling")
        self.assertEqual(plan.classify(["docs/a.md"], inventory)[0], "metadata")
        inventory.append(dict(name="hex-engine", manifest="crates/hex-engine/Cargo.toml",
                              dependencies=[{"name": "hex-ui", "kind": "dev"}]))
        self.assertEqual(plan.classify(["crates/hex-ui/src/a.rs"], inventory)[0], "full")

    def test_high_risk_unknown_and_mixed_cannot_be_scoped_away(self):
        inventory = self.inventory()
        for name in ("Cargo.lock", "crates/hex-ui/Cargo.toml", "crates/hex-ui/build.rs",
                     "third_party/gpui/src/a.rs", "crates/hex-engine/src/a.rs",
                     "rust-toolchain.toml", "scripts/ci_plan.py", "AGENTS.md", "unknown.rs"):
            with self.subTest(name=name):
                self.assertEqual(plan.classify([name], inventory)[0], "full")
        self.assertEqual(plan.classify(["launch", "crates/hex-ui/src/a.rs"], inventory)[0], "full")

    def test_immutable_rename_deletion_untracked_and_prose_digest(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def git(*args):
                return subprocess.check_output(["git", *args], cwd=root).decode().strip()
            git("init", "-q")
            git("config", "user.name", "scope fixture")
            git("config", "user.email", "fixture@invalid")
            (root / "runtime.rs").write_text("source")
            (root / "docs").mkdir()
            (root / "docs/a.md").write_text("first")
            git("add", ".")
            git("commit", "-qm", "base")
            base = git("rev-parse", "HEAD")
            before = plan.runtime_inputs(root)["sha256"]
            (root / "docs/a.md").write_text("second")
            self.assertEqual(before, plan.runtime_inputs(root)["sha256"])
            git("mv", "runtime.rs", "docs/renamed.md")
            git("add", ".")
            git("commit", "-qm", "move")
            (root / "unknown.rs").write_text("untracked")
            self.assertEqual(plan.changed_paths(root, base, "HEAD"),
                             ["docs/a.md", "docs/renamed.md", "runtime.rs", "unknown.rs"])
            self.assertNotEqual(before, plan.runtime_inputs(root)["sha256"])
            git("rm", "docs/renamed.md")
            git("commit", "-qm", "delete")
            self.assertIn("docs/renamed.md", plan.changed_paths(root, "HEAD^", "HEAD"))

    def test_profiles_preserve_default_and_feature_configurations(self):
        selected = ci.planned_steps({"scope": "ui"})
        argv = [s.argv for s in selected]
        self.assertTrue(any(a[:2] == ("cargo", "test") and "--features" not in a for a in argv))
        self.assertTrue(any("ui-test-support" in a for a in argv))
        self.assertTrue(any("layout_review" in a for a in argv))
        self.assertTrue(any(a[:2] == ("cargo", "clippy") and "--features" not in a for a in argv))
        self.assertTrue(ci.planned_steps({"scope": "tooling"})[0].uses_cargo_target)
        full = ci.planned_steps({"scope": "full"})
        self.assertEqual([s.argv for s in full], [s.argv for s in ci.FULL_STEPS])

    def test_prose_continuation_requires_complete_original_runtime_certificate(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def git(*args):
                return subprocess.check_output(["git", *args], cwd=root).decode().strip()
            git("init", "-q")
            git("config", "user.name", "certificate fixture")
            git("config", "user.email", "fixture@invalid")
            (root / ".gitignore").write_text('/artifacts/\n')
            (root / "Cargo.toml").write_text("fixture manifest")
            (root / "runtime.rs").write_text("original")
            (root / "docs").mkdir()
            (root / "docs/a.md").write_text("original")
            git("add", ".")
            git("commit", "-qm", "base")
            base = git("rev-parse", "HEAD")
            (root / "runtime.rs").write_text("candidate")
            git("add", ".")
            git("commit", "-qm", "runtime candidate")
            previous = git("rev-parse", "HEAD")
            metadata = {"workspace_members": ["fixture"], "packages": [dict(
                id="fixture", name="fixture", manifest_path=str(root / "Cargo.toml"),
                features={}, targets=[], dependencies=[])]}
            # Deliberately fabricated unit-test data exercises schema/refusal;
            # these counts are never claimed as executed product tests.
            tree = {"commit": previous, "dirty": False, "dirty_files": [], "content_sha256": "fixture"}
            evidence = {"schema_version": 2, "mode": "full", "status": "passed", "exit_code": 0,
                        "commit": previous, "dirty": False, "stale": False,
                        "start_tree": tree, "end_tree": tree, "runtime_inputs": plan.runtime_inputs(root),
                        "target_cache": {"dependency_inputs": {"version": 1, "sha256": "a" * 64, "packages": []}},
                        "steps": [{"step_id": s.step_id, "argv": list(s.argv), "status": "passed",
                                   "test_results": {"rust_passed_by_binary": [1]}} for s in ci.FULL_STEPS]}
            (root / "docs/a.md").write_text("metadata continuation")
            git("add", ".")
            git("commit", "-qm", "prose")
            current = git("rev-parse", "HEAD")
            resolved = plan.resolve(root, profile="merge", base=base, source=current, metadata=metadata)
            report = root / "artifacts/report.json"
            report.parent.mkdir()
            report.write_text(json.dumps(evidence))
            reused = ci.reusable_plan(root, resolved, report)
            self.assertEqual([s.step_id for s in ci.planned_steps(reused)], ["metadata-continuation"])
            self.assertEqual(reused["reuse"]["source"], previous)
            (root / "linked-runtime.rs").symlink_to(root / "runtime.rs")
            with self.assertRaisesRegex(ValueError, "symlink"):
                plan.resolve(root, profile="merge", base=base, source=current, metadata=metadata)
            (root / "linked-runtime.rs").unlink()
            for field, value in (("steps", evidence["steps"][:-1]), ("status", "failed"),
                                 ("mode", "edit"), ("schema_version", 1),
                                 ("runtime_inputs", {"sha256": "changed"}), ("target_cache", {})):
                report.write_text(json.dumps({**evidence, field: value}))
                with self.subTest(field=field), self.assertRaises(ValueError):
                    ci.reusable_plan(root, resolved, report)
            report.write_text(json.dumps(evidence))
            (root / "runtime.rs").write_text("uncommitted executable change")
            changed = plan.resolve(root, profile="merge", base=base, source=current, metadata=metadata)
            with self.assertRaises(ValueError):
                ci.reusable_plan(root, changed, report)
            runner = ci.LocalCiRunner(root, mode="merge", plan=changed, min_free_bytes=0,
                                      steps=(ci.Step("must-not-run", (sys.executable, "-c", "raise AssertionError()")),))
            self.assertEqual(runner.run(), 2)
            self.assertTrue(runner.summary["start_tree"]["dirty"])
            self.assertTrue(all(s["status"] == "not_run" for s in runner.summary["steps"]))

    def test_requested_zero_match_and_all_skipped_refuse_but_metadata_needs_no_rust(self):
        with tempfile.TemporaryDirectory() as directory:
            runner = ci.LocalCiRunner(Path(directory), mode="component", steps=(ci.Step("unused", ("true",)),))
            runner.output_dir.mkdir(parents=True)
            for index, output in enumerate(("test result: ok. 0 passed; 0 failed; 0 ignored;", "Ran 2 tests in 0.1s\nOK (skipped=2)"), 1):
                step = ci.Step("requested", (sys.executable, "-c", f"print({output!r})"), requires_tests=True)
                result = runner._run_step(step, index)
                self.assertEqual(result["status"], "failed")
                self.assertIn("no passing tests", result["error"])
            result = runner._run_step(ci.Step("metadata", (sys.executable, "-c", "print('checked prose')")), 3)
            self.assertEqual(result["status"], "passed")


class CacheIdentityTests(unittest.TestCase):
    def test_actual_cargo_restored_mtime_dependency_api_is_invalidated(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "app/src").mkdir(parents=True)
            (root / "vendor/src").mkdir(parents=True)
            (root / "Cargo.toml").write_text('[workspace]\nmembers=["app"]\nexclude=["vendor"]\nresolver="2"\n')
            (root / ".gitignore").write_text('/target/\n/artifacts/\n')
            (root / "app/Cargo.toml").write_text('[package]\nname="cache-consumer"\nversion="0.1.0"\nedition="2021"\n[dependencies]\ncache-dependency={path="../vendor"}\n')
            (root / "vendor/Cargo.toml").write_text('[package]\nname="cache-dependency"\nversion="0.1.0"\nedition="2021"\n')
            source = root / "vendor/src/lib.rs"
            source.write_text('pub fn value()->u32 { 7 }\n')
            main = root / "app/src/main.rs"
            main.write_text('fn main(){let value:u32=cache_dependency::value();println!("{value}");}\n')
            env = {**os.environ, "CARGO_TARGET_DIR": str(root / "target")}
            def command(*argv):
                return subprocess.check_output(argv, cwd=root, env=env).decode().strip()
            command("git", "init", "-q")
            command("git", "config", "user.name", "cache fixture")
            command("git", "config", "user.email", "fixture@invalid")
            command("cargo", "generate-lockfile", "--offline")
            command("git", "add", ".")
            command("git", "commit", "-qm", "first")
            def runner(name):
                return ci.LocalCiRunner(root, mode="component", run_id=name, env=env, min_free_bytes=0,
                                        steps=(ci.Step("build", ("cargo", "build", "--offline", "--locked")),))
            first = runner("first")
            self.assertEqual(first.run(), 0)
            stamp = source.stat()
            source.write_text('pub fn value()->&\'static str { "new API" }\n')
            os.utime(source, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
            main.write_text('fn main(){let value:&str=cache_dependency::value();println!("{value}");}\n')
            command("git", "add", ".")
            command("git", "commit", "-qm", "restored mtime changed API")
            stale = runner("stale-control")
            # Simulate the old workspace-only witness under the same actual
            # target lock. Cargo itself sees the restored timestamp, not a fake
            # compiler/error/event, and must demonstrate the stale API failure.
            def old_dependency_witness():
                stale._release_scope()
                stale._release_build_inputs(["guarded-dev-inputs-v1"])
                return first._dependency_inputs
            with patch.object(stale, "_dependency_context", side_effect=old_dependency_witness):
                self.assertEqual(stale.run(), 1)
            stale_log = (stale.output_dir / "01-build.log").read_text()
            self.assertIn("mismatched types", stale_log)
            print("ACTUAL_RESTORED_MTIME_STALE_CONTROL_EXIT1\n" + stale_log)
            fixed = runner("fixed")
            self.assertEqual(fixed.run(), 0, fixed.summary)
            self.assertIn("cache-dependency", " ".join(fixed.summary["target_cache"]["cleanup_argv"]))
            self.assertEqual(command(str(root / "target/debug/cache-consumer")), "new API")

    def test_final_dependency_signal_releases_lock_and_restores_handlers(self):
        # Reuse the existing isolated fake-tool fixture, not the real app/target.
        fixtures = module("scope_runner_fixtures", "tests/test_local_ci.py")
        for signum in (signal.SIGINT, signal.SIGTERM):
            fixture = fixtures.LocalCiRunnerTests()
            fixture.setUp()
            try:
                runner = fixture.runner("final-signal")
                previous = {s: signal.getsignal(s) for s in (signal.SIGINT, signal.SIGTERM)}
                identity = {"version": 1, "sha256": "a" * 64, "packages": []}
                calls = 0
                original_observe = runner._dependency_context
                def observe():
                    nonlocal calls
                    calls += 1
                    if calls == 2:
                        os.kill(os.getpid(), signum)
                    return original_observe()
                with patch.object(runner, "_dependency_context", side_effect=observe):
                    self.assertEqual(runner.run(), 130)
                self.assertIsNone(runner.target_lock)
                for s, handler in previous.items():
                    self.assertEqual(signal.getsignal(s), handler)
                self.assertEqual(fixture.runner("after-signal").run(), 0)
            finally:
                fixture.tearDown()

    def test_final_dependency_validation_errors_preserve_owner_release_lock_and_restore_handlers(self):
        fixtures = module("observer_error_fixtures", "tests/test_local_ci.py")
        for error in (ValueError("unsupported dependency FIFO"), RuntimeError("symlink loop")):
            fixture = fixtures.LocalCiRunnerTests()
            fixture.setUp()
            try:
                runner = fixture.runner("final-validation")
                previous = {s: signal.getsignal(s) for s in (signal.SIGINT, signal.SIGTERM)}
                identity = {"version": 1, "sha256": "a" * 64, "packages": []}
                owner_before = None
                calls = 0
                original_observe = runner._dependency_context
                def observe():
                    nonlocal calls, owner_before
                    calls += 1
                    if calls == 2:
                        owner_before = runner.target_owner_path.read_bytes() if runner.target_owner_path.exists() else None
                        raise error
                    return original_observe()
                with patch.object(runner, "_dependency_context", side_effect=observe):
                    self.assertEqual(runner.run(), 2)
                self.assertEqual(runner.summary["status"], "incomplete")
                self.assertIn(type(error).__name__, runner.summary["target_owner_update_error"])
                owner_after = runner.target_owner_path.read_bytes() if runner.target_owner_path.exists() else None
                self.assertEqual(owner_before, owner_after)
                self.assertEqual(json.loads((runner.output_dir / "summary.json").read_text())["exit_code"], 2)
                self.assertIsNone(runner.target_lock)
                for s, handler in previous.items():
                    self.assertEqual(signal.getsignal(s), handler)
                self.assertEqual(fixture.runner("after-validation").run(), 0)
            finally:
                fixture.tearDown()

    def test_source_changes_stay_incremental_but_dependency_config_mode_and_membership_do_not(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            workspace = root / "app"
            dependency = root / "vendor"
            workspace.mkdir()
            dependency.mkdir()
            (workspace / "Cargo.toml").write_text("manifest")
            (workspace / "lib.rs").write_text("old")
            (dependency / "empty").mkdir()
            executable = dependency / "helper"
            executable.write_bytes(b"same")
            link = dependency / "link"
            link.symlink_to("helper")
            metadata = {"workspace_members": ["app"], "target_directory": str(root / "target"),
                        "packages": [dict(id="app", manifest_path=str(workspace / "Cargo.toml"), source=None),
                                     dict(id="vendor", manifest_path=str(dependency / "Cargo.toml"), source=None)]}
            env = {"CARGO_HOME": str(root / "cargo-home")}
            def key():
                return inputs.fingerprint(root, env, metadata, {"rustc": "pinned", "cargo": "pinned"})
            initial = key()
            (workspace / "lib.rs").write_text("new")
            self.assertEqual(initial, key())
            executable.chmod(0o755)
            self.assertNotEqual(initial, key())
            mode_key = key()
            (dependency / "empty/new").write_text("addition")
            self.assertNotEqual(mode_key, key())
            (root / ".cargo").mkdir()
            old = key()
            (root / ".cargo/config.toml").write_text("[build]\njobs=1")
            self.assertNotEqual(old, key())
            old = key()
            env["RUSTFLAGS"] = "--cfg changed"
            self.assertNotEqual(old, key())

    def test_member_and_intermediate_absent_configs_change_guarded_witness(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            member = root / "crates/hex-stream"
            member.mkdir(parents=True)
            (member / "Cargo.toml").write_text("manifest")
            metadata = {"workspace_members": ["stream"], "target_directory": str(root / "target"),
                        "packages": [dict(id="stream", name="hex-stream", source=None, dependencies=[],
                                          manifest_path=str(member / "Cargo.toml"))]}
            def key():
                return inputs.fingerprint(root, {"CARGO_HOME": str(root / "cargo-home")}, metadata, {})
            for parent in (member, member.parent, root):
                for name in ("config", "config.toml"):
                    path = parent / ".cargo" / name
                    path.parent.mkdir(exist_ok=True)
                    before = key()
                    path.write_text('[source.crates-io]\nreplace-with="unsupported"\n')
                    self.assertNotEqual(before, key(), str(path))
                    path.unlink()
                    self.assertEqual(before, key())


if __name__ == "__main__":
    unittest.main()
