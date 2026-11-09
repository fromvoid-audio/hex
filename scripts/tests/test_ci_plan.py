"""Scope boundaries, input freshness, and zero-match qualification regressions."""
import importlib.util
from contextlib import contextmanager
import json
import os
import re
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
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
    def test_copied_host_transport_probe_runs_at_its_owning_package(self):
        inventory = [{"name": "hex-plugin-host", "features": {"allocation-probe": [], "test-support": []}}]
        selected = ci.boundary_steps({
            "affected_packages": ["hex-plugin-host"], "cargo_inventory": inventory,
            "paths": ["crates/hex-plugin-host/src/transport.rs"],
            "boundary": {"kind": "packages", "lint_packages": ["hex-plugin-host"],
                         "test_packages": ["hex-plugin-host"], "tests": []},
        })
        probes = [s for s in selected if s.step_id == "audio-allocation-probe"]
        self.assertEqual(len(probes), 1)
        self.assertEqual(probes[0].argv, ("cargo", "test", "-p", "hex-plugin-host",
                                         "--features", "allocation-probe", "--locked"))
        lint = next(s for s in selected if s.step_id == "affected-feature-clippy")
        self.assertEqual(lint.argv[:4], ("cargo", "clippy", "-p", "hex-plugin-host"))
        self.assertIn("hex-plugin-host/allocation-probe", lint.argv[lint.argv.index("--features") + 1])
        self.assertFalse(any(s.step_id.startswith("feature-") and "allocation-probe" in s.argv
                             for s in selected), "the real owning probe runs once")

    def test_edit_target_cli_requires_edit_plan_and_exclusive_selection(self):
        for arguments in (["--lib"], ["--profile", "full", "--lib"],
                          ["--quick", "--test", "roundtrip"], ["--lib", "--test", "roundtrip"]):
            with self.subTest(arguments=arguments), patch.object(ci, "LocalCiRunner") as runner:
                with self.assertRaises(SystemExit) as error:
                    ci.main(arguments)
                self.assertEqual(error.exception.code, 2)
                runner.assert_not_called()

    def test_edit_target_validation_and_required_features(self):
        inventory = [{"name": "fixture", "features": {"default": ["enabled"], "enabled": [], "extra": []},
                      "targets": [{"name": "fixture", "kind": ["lib"]},
                                  {"name": "roundtrip", "kind": ["test"], "required-features": ["enabled"]},
                                  {"name": "optional", "kind": ["test"], "required-features": ["extra"]}]}]
        def select(**kwargs):
            return ci.edit_selection(inventory, **{"package": "fixture", "test_filter": "case", **kwargs})
        self.assertNotIn("target", select())
        self.assertEqual(select(lib=True)["target"], {"kind": "lib", "name": "fixture"})
        self.assertEqual(select(test="roundtrip")["target"]["name"], "roundtrip")
        self.assertEqual(select(test="optional", features="extra")["target"]["name"], "optional")
        for options in ({"package": "unknown"}, {"test_filter": " "}, {"features": "unknown"},
                        {"test": "missing"}, {"test": "optional"}, {"test": ""},
                        {"lib": True, "test": "roundtrip"}):
            with self.subTest(options=options), self.assertRaises(ValueError):
                select(**options)
        with self.assertRaises(ValueError):
            ci.edit_selection(inventory * 2, package="fixture", test_filter="case", lib=True)
        inventory[0]["targets"].append({"name": "other", "kind": ["lib"]})
        with self.assertRaises(ValueError):
            select(lib=True)

    def test_edit_target_runs_only_owner_and_rejects_zero_matches_or_regression(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            (root / "src").mkdir()
            (root / "tests").mkdir()
            (root / "Cargo.toml").write_text('[package]\nname="edit-target-fixture"\nversion="0.1.0"\nedition="2021"\n')
            (root / ".gitignore").write_text('/target/\n/artifacts/\n')
            source = root / "src/lib.rs"
            source.write_text('#[test] fn library_case() { assert_eq!(2 + 2, 4); }\n')
            (root / "tests/roundtrip.rs").write_text('#[test] fn integration_case() {}\n')
            env = {**os.environ, "CARGO_TARGET_DIR": str(root / "target")}
            def command(*argv):
                return subprocess.check_output(argv, cwd=root, env=env).decode().strip()
            command("git", "init", "-q")
            command("git", "config", "user.name", "target fixture")
            command("git", "config", "user.email", "fixture@invalid")
            command("cargo", "generate-lockfile", "--offline")
            command("cargo", "fmt", "--all")
            command("git", "add", ".")
            command("git", "commit", "-qm", "fixture")
            metadata = json.loads(command("cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"))
            inventory = plan.cargo_inventory(metadata, root)
            def run(name, *, test_filter="case", **target):
                edit = ci.edit_selection(inventory, package="edit-target-fixture", test_filter=test_filter, **target)
                selected = {"scope": "full", "qualifies_merge": False, "edit": edit}
                runner = ci.LocalCiRunner(root, mode="edit", plan=selected, env=env, run_id=name, min_free_bytes=0)
                code = runner.run()
                log = (runner.output_dir / "01-edit-tests.log").read_text()
                return code, log, runner.summary
            code, log, summary = run("package")
            self.assertEqual(code, 0)
            self.assertIn("test library_case ... ok", log)
            self.assertIn("test integration_case ... ok", log)
            code, log, summary = run("library", lib=True)
            self.assertEqual(code, 0)
            self.assertEqual(summary["status"], "partial")
            self.assertIn("test library_case ... ok", log)
            self.assertNotIn("integration_case", log)
            self.assertNotIn("tests/roundtrip.rs", log)
            code, log, summary = run("integration", test="roundtrip")
            self.assertEqual(code, 0)
            self.assertIn("test integration_case ... ok", log)
            self.assertNotIn("library_case", log)
            code, log, summary = run("no-match", lib=True, test_filter="absent_case")
            self.assertEqual(code, 1)
            self.assertIn("no passing tests", summary["steps"][0]["error"])
            self.assertTrue(all(s["status"] == "not_run" for s in summary["steps"][1:]))
            source.write_text('#[test] fn library_case() { assert_eq!(2 + 2, 5); }\n')
            code, log, summary = run("real-regression", lib=True)
            self.assertEqual(code, 1)
            self.assertIn("library_case ... FAILED", log)

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
            (root / ".gitignore").write_text('/target/\n/artifacts/\n__pycache__/\n')
            (root / "scripts").mkdir()
            for name in ("ci_plan.py", "python_test_plan.py"):
                (root / "scripts" / name).write_bytes((SCRIPTS / name).read_bytes())
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
            base = Path(directory).resolve()
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
        inventory[0]["features"] = {"new-configuration": []}
        self.assertEqual(plan.classify(["crates/hex-widgets/src/a.rs"], inventory)[0], "rust")
        inventory[0]["features"] = {}
        inventory.append(dict(name="hex-engine", manifest="crates/hex-engine/Cargo.toml",
                              dependencies=[{"name": "hex-ui", "kind": "dev"}]))
        self.assertEqual(plan.classify(["crates/hex-ui/src/a.rs"], inventory)[0], "rust")

    def test_high_risk_unknown_and_mixed_cannot_be_scoped_away(self):
        inventory = self.inventory()
        for name in ("Cargo.lock",
                     "third_party/gpui/src/a.rs", "crates/hex-engine/src/a.rs",
                     "rust-toolchain.toml", "unknown.rs"):
            with self.subTest(name=name):
                self.assertEqual(plan.classify([name], inventory)[0], "full")
        self.assertEqual(plan.classify(["launch", "crates/hex-ui/src/a.rs"], inventory)[0], "rust")
        for name in ("crates/hex-ui/Cargo.toml", "crates/hex-ui/build.rs"):
            self.assertEqual(plan.classify([name], inventory)[0], "rust")
        for path in ("scripts/ci_plan.py", "scripts/ci_target_storage.py"):
            self.assertEqual(plan.classify([path], inventory)[0], "tooling")
        self.assertEqual(plan.classify(["AGENTS.md"], inventory)[0], "metadata")

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

    def test_cheap_guards_precede_self_tests_once_in_every_qualifying_scope(self):
        for scope in ("full", "tooling", "ui"):
            with self.subTest(scope=scope):
                steps = ci.planned_steps({"scope": scope})
                self.assertEqual(steps[:len(ci.QUICK_STEPS)], ci.QUICK_STEPS)
                ids = [s.step_id for s in steps]
                self.assertEqual(ids[len(ci.QUICK_STEPS)], "runner-tests")
                self.assertEqual(len(ids), len(set(ids)))

    def test_real_architecture_failure_stops_before_self_tests(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            (root / "src").mkdir()
            (root / "scripts").mkdir()
            (root / "src/lib.rs").write_text("")
            (root / "Cargo.toml").write_text(
                '[package]\nname="unregistered-boundary"\nversion="0.1.0"\nedition="2021"\n')
            (root / "scripts/check-architecture.py").write_bytes(
                (SCRIPTS / "check-architecture.py").read_bytes())
            (root / ".gitignore").write_text('/target/\n/artifacts/\n')
            for command in (("git", "init", "-q"), ("git", "config", "user.name", "guard fixture"),
                            ("git", "config", "user.email", "fixture@invalid"),
                            ("cargo", "generate-lockfile", "--offline"),
                            ("git", "add", "."), ("git", "commit", "-qm", "fixture")):
                subprocess.check_call(command, cwd=root, stdout=subprocess.DEVNULL)
            marker = "open('artifacts/self-tests-started', 'w').close(); print('Ran 1 test in 0.001s\\nOK')"
            steps = tuple(ci.Step(s.step_id,
                                  (sys.executable, "-c", "pass") if s.step_id == "changelog" else
                                  (sys.executable, "-c", marker)
                                  if s.step_id == "runner-tests" else s.argv,
                                  s.uses_cargo_target, s.requires_tests)
                          for s in ci.planned_steps({"scope": "tooling"}))
            runner = ci.LocalCiRunner(root, steps=steps, mode="custom", min_free_bytes=0,
                                      env={**os.environ, "CARGO_TARGET_DIR": str(root / "target")})
            self.assertEqual(runner.run(), 1)
            self.assertFalse((root / "artifacts/self-tests-started").exists())
            self.assertEqual([(s["step_id"], s["status"]) for s in runner.summary["steps"]],
                             [("changelog", "passed"), ("architecture", "failed")]
                             + [(s.step_id, "not_run") for s in steps[2:]])
            failed = runner.summary["steps"][1]
            self.assertIn("register a purpose and dependency policy", (root / failed["log"]).read_text())
            self.assertFalse(runner.summary.get("qualifies_merge", False))

    def test_profiles_preserve_default_and_feature_configurations(self):
        selected = ci.planned_steps({"scope": "ui"})
        argv = [s.argv for s in selected]
        self.assertTrue(any(a[:2] == ("cargo", "test") and "--features" not in a for a in argv))
        self.assertTrue(any("ui-test-support" in a for a in argv))
        self.assertTrue(any("layout_review" in a for a in argv))
        self.assertTrue(any(a[:2] == ("cargo", "clippy") and "--features" not in a for a in argv))
        self.assertTrue(next(s for s in ci.planned_steps({"scope": "tooling"})
                             if s.step_id == "runner-tests").uses_cargo_target)
        full = ci.planned_steps({"scope": "full"})
        self.assertEqual([s.argv for s in full], [s.argv for s in ci.FULL_STEPS])

    def test_runtime_contract_consumers_and_required_configurations(self):
        root = SCRIPTS.parent
        metadata = json.loads(subprocess.check_output(
            ["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"], cwd=root))
        live_inventory = plan.cargo_inventory(metadata, root)
        path = "crates/hex-devices/src/compressor.rs"
        # The copied/native client no longer depends on musical audio rendering.
        # Changing a compressor must not select the independent client. Adding
        # an unreviewed reverse consumer still broadens to package coverage.
        live_scope, live_affected, _ = plan.classify([path], live_inventory)
        self.assertNotIn("hex-plugin-host", live_affected)
        live_boundary = plan.boundary_selection(root, [path], live_inventory, live_affected)
        # Keep the narrow-case fixture independent of newly added features.
        # An unreviewed live feature must widen the plan, not break this test
        # or be silently added to the production allowlist.
        reviewed = {"hex-engine": {"instrument-test-support"},
                    "hex-export": {"instrument-test-support"},
                    "hex-audio": {"allocation-probe", "stream-test-support"},
                    "hex-desktop": {"allocation-probe"},
                    "hex-ui": {"ui-test-support"},
                    "hex-session": {"source-overview-test-support"},
                    "hex-stream": {"allocation-probe", "test-support"},
                    "hex-media": {"test-support"},
                    "hex-sandbox": {"ui-test-support"}}
        unreviewed = {p["name"]: set(p["features"]) - reviewed.get(p["name"], set())
                      for p in live_inventory if p["name"] in live_affected}
        if any(unreviewed.values()):
            self.assertEqual(live_boundary["kind"], "packages")
            self.assertEqual(live_boundary["lint_packages"], live_affected)
            live_steps = ci.planned_steps({"scope": live_scope, "affected_packages": live_affected,
                                           "boundary": live_boundary, "cargo_inventory": live_inventory})
            lint_commands = [",".join(step.argv) for step in live_steps
                             if step.argv[:2] == ("cargo", "clippy")]
            for package, features in unreviewed.items():
                for feature in features - {"default"}:
                    self.assertTrue(any(f"{package}/{feature}" in command for command in lint_commands))
        else:
            # Other safe-widening causes, such as a new macro boundary, may
            # also make the real workspace require package coverage.
            self.assertIn(live_boundary["kind"], {"compressor", "packages"})
        reversed_inventory = [{**p, "dependencies": p["dependencies"] + [{"name": "hex-audio"}]}
                              if p["name"] == "hex-plugin-host" else p for p in live_inventory]
        _, reversed_affected, _ = plan.classify([path], reversed_inventory)
        self.assertIn("hex-plugin-host", reversed_affected)
        reversed_boundary = plan.boundary_selection(root, [path], reversed_inventory, reversed_affected)
        self.assertEqual(reversed_boundary["kind"], "packages")
        # Owner-boundary merges: the new consumer compiles and lints, but only
        # the owner and its reviewed runtime consumer run test suites.
        self.assertIn("hex-plugin-host", reversed_boundary["lint_packages"])
        self.assertEqual(reversed_boundary["test_packages"], ["hex-devices", "hex-storage"])
        inventory = [{**p, "features": {name: value for name, value in p["features"].items()
                                         if name in reviewed.get(p["name"], set())}}
                     for p in live_inventory]
        scope, affected, _ = plan.classify([path], inventory)
        self.assertEqual(scope, "rust")
        self.assertIn("hex-storage", affected)  # opaque state, no Cargo edge
        fixture_dir = tempfile.TemporaryDirectory()
        self.addCleanup(fixture_dir.cleanup)
        fixture = Path(fixture_dir.name)
        for name in (path, "crates/hex-devices/src/compressor/tests.rs"):
            source = fixture / name
            source.parent.mkdir(parents=True, exist_ok=True)
            source.write_text("// reviewed fixture source\n")
        boundary = plan.boundary_selection(fixture, [path], inventory, affected)
        self.assertEqual(boundary["kind"], "compressor")
        resolved = {"scope": scope, "affected_packages": affected,
                    "boundary": boundary, "cargo_inventory": inventory}
        steps = ci.planned_steps(resolved)
        # Quick checks keep their order; formatting is scoped to the changed owners.
        self.assertEqual([s.step_id for s in steps[:len(ci.QUICK_STEPS)]], [s.step_id for s in ci.QUICK_STEPS])
        self.assertNotIn("--all", steps[[s.step_id for s in steps].index("format")].argv)
        self.assertEqual(steps[len(ci.QUICK_STEPS)].step_id, "runner-tests")
        commands = [s.argv for s in steps]
        self.assertTrue(all(s.requires_tests for s in steps if s.argv[:2] == ("cargo", "test")))
        self.assertTrue(any("device_chain" in a for a in commands))
        self.assertTrue(any("allocation_probe_calibration" in a for a in commands))
        self.assertTrue(any("hex-storage" in a and "compressor" in a for a in commands))
        self.assertTrue(any("allocation-probe" in a and "compressor" in a for a in commands))
        self.assertTrue(any("ui-test-support" in a and "compressor" in a for a in commands))
        self.assertFalse(any("--workspace" in a for a in commands))
        self.assertFalse(any("operator" in a for a in commands))
        # Reviewed stream hooks are linted, never silently skipped, but their
        # unrelated full suites are not run for a compressor-only change.
        stream = [a for a in commands if any("stream-test-support" in arg for arg in a)]
        self.assertTrue(stream)
        self.assertTrue(all(a[:2] == ("cargo", "clippy") for a in stream))
        self.assertTrue(any("hex-stream/test-support" in ",".join(a) and a[1] == "clippy" for a in commands))
        self.assertTrue(any("hex-media/test-support" in ",".join(a) and a[1] == "clippy" for a in commands))
        self.assertIn("hex-midi", boundary["excluded_packages"])
        with tempfile.TemporaryDirectory() as directory:
            fixture_root = Path(directory)
            for name in (path, "crates/hex-devices/src/compressor/tests.rs"):
                source = fixture_root / name
                source.parent.mkdir(parents=True, exist_ok=True)
                source.write_text("// unchanged fixture source\n")
            (fixture_root / path).write_text("#[macro_export] macro_rules! external { () => {}; }\n")
            widened = plan.boundary_selection(fixture_root,
                ["crates/hex-devices/src/compressor/tests.rs"], inventory, affected)
            self.assertEqual(widened["kind"], "packages")
        self.assertEqual(plan.boundary_selection(root, [path], inventory, [*affected, "new-consumer"])["kind"], "packages")
        new_feature = [{**p, "features": {"new-configuration": []}} if p["name"] == "hex-devices" else p
                       for p in inventory]
        self.assertEqual(plan.boundary_selection(root, [path], new_feature, affected)["kind"], "packages")
        new_media_feature = [{**p, "features": {**p["features"], "unreviewed": []}}
                             if p["name"] == "hex-media" else p for p in inventory]
        self.assertEqual(plan.boundary_selection(root, [path], new_media_feature, affected)["kind"], "packages")
        with self.assertRaisesRegex(ValueError, "runtime consumers missing"):
            plan.boundary_selection(root, [path], [p for p in inventory if p["name"] != "hex-storage"], affected)
        for change in ("crates/hex-devices/src/lib.rs", "crates/hex-lattice-contract/src/lib.rs",
                       "crates/hex-devices/Cargo.toml", "crates/hex-devices/build.rs"):
            with self.subTest(change=change):
                _, consumers, _ = plan.classify([change], inventory)
                wider = plan.boundary_selection(root, [change], inventory, consumers)
                self.assertEqual(wider["kind"], "packages")
                selected = ci.planned_steps({**resolved, "affected_packages": consumers, "boundary": wider})
                self.assertTrue(any(s.step_id == "affected-tests" for s in selected))
                # Consumers' allocation probes run only for build inputs; source
                # changes keep the probe configuration compiling via clippy.
                build_input = change.endswith(("Cargo.toml", "build.rs"))
                self.assertEqual(any(s.step_id == "audio-allocation-probe" for s in selected), build_input)
                self.assertTrue(any(s.step_id == "affected-feature-clippy"
                                    and "hex-audio/allocation-probe" in ",".join(s.argv) for s in selected))
        for broken in ({**boundary, "lint_packages": []},
                       {**boundary, "lint_packages": ["missing"]},
                       {**boundary, "tests": []},
                       {**boundary, "tests": [{"package": "hex-engine", "test": "missing", "filter": ""}]}):
            with self.subTest(broken=broken), self.assertRaises(ValueError):
                ci.planned_steps({**resolved, "boundary": broken})

    def test_real_cargo_negative_controls_and_matched_selection_timing(self):
        """Run real selected binaries: affected failures fail; excluded failure stays excluded.

        A tiny generated workspace keeps this independent of native frameworks.
        Its timings characterize selection overhead only, not production savings.
        """
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text(
                '[workspace]\nmembers = ["crates/owner", "crates/consumer", "crates/unrelated"]\nresolver = "2"\n')
            sources = {
                "owner": 'include!(concat!(env!("OUT_DIR"), "/value.rs"));\n#[macro_export] macro_rules! value { () => { $crate::VALUE }; }\n#[test] fn owner_behavior() { assert_eq!(VALUE, 7); }\n',
                "consumer": '#[test] fn real_contract() { assert_eq!(owner::value!(), 7); }\n#[cfg(feature="checked")] #[test] fn feature_contract() { assert_eq!(owner::value!(), 7); }\n',
                "unrelated": '#[test] fn unrelated_behavior() { assert_eq!(2 + 2, 4); }\n',
            }
            build = 'fn main() { std::fs::write(std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("value.rs"), "pub const VALUE: u32 = 7;").unwrap(); }\n'
            for name, source in sources.items():
                folder = root / "crates" / name
                (folder / "src").mkdir(parents=True)
                manifest = f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2021"\n'
                if name == "consumer":
                    manifest += '[dependencies]\nowner = { path = "../owner" }\n[features]\nchecked = []\n'
                (folder / "Cargo.toml").write_text(manifest)
                (folder / "src/lib.rs").write_text(source)
            (root / "crates/owner/build.rs").write_text(build)
            env = {**os.environ, "CARGO_TARGET_DIR": str(root / "target"), "CARGO_NET_OFFLINE": "true"}
            metadata = json.loads(subprocess.check_output(
                ["cargo", "metadata", "--no-deps", "--format-version", "1"], cwd=root, env=env))
            subprocess.run(["cargo", "generate-lockfile", "--offline"], cwd=root, env=env, check=True)
            inventory = plan.cargo_inventory(metadata, root)

            def selection(path):
                scope, affected, _ = plan.classify([path], inventory)
                if scope == "full":
                    return next(s.argv for s in ci.FULL_STEPS if s.step_id == "workspace-tests")
                boundary = plan.boundary_selection(root, [path], inventory, affected)
                steps = ci.planned_steps({"scope": scope, "affected_packages": affected,
                                          "boundary": boundary, "cargo_inventory": inventory})
                return next(s.argv for s in steps if s.step_id == "affected-tests")

            def run(argv, passed=True):
                started = time.monotonic()
                result = subprocess.run(argv, cwd=root, env=env, text=True, capture_output=True)
                elapsed = time.monotonic() - started
                self.assertEqual(result.returncode == 0, passed, result.stdout + result.stderr)
                return {"seconds": elapsed, "timing_estimates": ci.log_timing_estimates(
                    result.stdout + result.stderr, elapsed)}

            selected = selection("crates/owner/src/lib.rs")
            self.assertNotIn("unrelated", selected)
            baseline = next(s.argv for s in ci.FULL_STEPS if s.step_id == "workspace-tests")
            warmup = [run(baseline), run(selected)]
            pairs = []
            for index in range(3):
                order = [("before", baseline), ("after", selected)]
                if index % 2:
                    order.reverse()
                pairs.append({label: run(argv) for label, argv in order})
            print("boundary selection matched fixture timing: " + json.dumps({
                "workload": "three tiny Rust crates; owner build script/exported macro; real consumer; unrelated crate",
                "cache": "same warm temporary target, offline, no dependencies; sequential alternating pairs",
                "before_argv": baseline, "after_argv": selected, "warmup": warmup, "pairs": pairs,
                "qualification": "fixture command/coverage timing only; not production merge wall savings"}, sort_keys=True))
            # Public contract/build output and macro expansion must reach the real consumer.
            for path, content in (("crates/owner/build.rs", build.replace("= 7;", "= 8;")),
                                  ("crates/consumer/src/lib.rs", sources["consumer"].replace(", 7)", ", 8)")),
                                  ("crates/owner/src/lib.rs", sources["owner"].replace(", 7)", ", 8)"))):
                original = (root / path).read_text()
                (root / path).write_text(content)
                run(selection(path), passed=False)
                (root / path).write_text(original)
            # Relevant feature-only regression must fail its selected configuration.
            feature_path = root / "crates/consumer/src/lib.rs"
            feature_path.write_text(sources["consumer"].replace(
                'fn feature_contract() { assert_eq!(owner::value!(), 7); }',
                'fn feature_contract() { assert_eq!(owner::value!(), 8); }'))
            _, affected, _ = plan.classify(["crates/consumer/src/lib.rs"], inventory)
            feature_steps = ci.planned_steps({"scope": "rust", "affected_packages": affected,
                "boundary": plan.boundary_selection(root, ["crates/consumer/src/lib.rs"], inventory, affected),
                "cargo_inventory": inventory})
            run(next(s.argv for s in feature_steps if s.step_id == "feature-test-consumer-0"), passed=False)
            feature_path.write_text(sources["consumer"])
            # Real unrelated failure is omitted by a justified owner boundary;
            # unknown input conservatively selects workspace coverage and detects it.
            (root / "crates/unrelated/src/lib.rs").write_text(sources["unrelated"].replace(", 4)", ", 5)"))
            run(selected)
            run(selection("unknown.input"), passed=False)

    @patch.dict(os.environ, {}, clear=True)
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
                        "native_acceptance_opt_in": False,
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
            self.assertIs(reused["reuse"]["native_acceptance_opt_in"], False)
            original_report = report.read_bytes()
            original_plan = json.dumps(resolved, sort_keys=True)
            # Full reports without a plan and scoped reports must both carry an
            # explicit false observation. Missing historical state is unknown.
            for old_plan in (None, {"scope": "tooling", "qualifies_merge": True}):
                candidate = {**evidence, "plan": old_plan,
                             "mode": "full" if old_plan is None else "merge",
                             "steps": [{"step_id": s.step_id, "argv": list(s.argv), "status": "passed",
                                        "test_results": {"rust_passed_by_binary": [1]}}
                                       for s in ci.planned_steps(old_plan or {"scope": "full"})]}
                for flag in ("missing", True, None, 0, 1, "false", [], {}):
                    rejected = {**candidate, "native_acceptance_opt_in": flag}
                    if flag == "missing":
                        rejected.pop("native_acceptance_opt_in")
                    raw = json.dumps(rejected).encode()
                    report.write_bytes(raw)
                    with self.subTest(scope=old_plan, flag=flag), self.assertRaisesRegex(ValueError, "native/external"):
                        ci.reusable_plan(root, resolved, report)
                    self.assertEqual(report.read_bytes(), raw)
                    self.assertEqual(json.dumps(resolved, sort_keys=True), original_plan)
                report.write_text(json.dumps(candidate))
                self.assertIs(ci.reusable_plan(root, resolved, report)["reuse"]["native_acceptance_opt_in"], False)
            report.write_bytes(original_report)
            for key in ("HEX_VST3_SDK_DIR", "HEX_VST3_FUTURE_FIXTURE", "HEX_TEST_SIGNING_IDENTITY"):
                with self.subTest(current_input=key), self.assertRaisesRegex(ValueError, "native/external"):
                    plan.reuse_checks(root, resolved, report, env={key: "explicit-input"})
                self.assertEqual(report.read_bytes(), original_report)
                self.assertEqual(json.dumps(resolved, sort_keys=True), original_plan)
            # A path string and repository tree can stay identical while an
            # external plugin changes. Native evidence remains non-reusable.
            with tempfile.TemporaryDirectory() as external:
                plugin = Path(external) / "fixture.plugin"
                report.write_text(json.dumps({**evidence, "native_acceptance_opt_in": True}))
                native_report = report.read_bytes()
                for content in (b"original plugin", b"replacement at the same path"):
                    plugin.write_bytes(content)
                    with self.assertRaisesRegex(ValueError, "native/external"):
                        plan.reuse_checks(root, resolved, report, env={})
                    self.assertEqual(report.read_bytes(), native_report)
                self.assertEqual(json.dumps(resolved, sort_keys=True), original_plan)
            report.write_bytes(original_report)
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


class TestShardTests(unittest.TestCase):
    shards = module("test_shards_fixture", "test_shards.py")

    def test_real_cargo_empty_selections_fail_without_rejecting_empty_targets(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "src").mkdir()
            (root / "Cargo.toml").write_text(
                '[package]\nname = "emptyshardfixture"\nversion = "0.1.0"\nedition = "2021"\n')
            env = {**os.environ, "CARGO_TARGET_DIR": str(root / "target")}
            command = [sys.executable, str(SCRIPTS / "test_shards.py"), "--shards", "3"]

            def run(*options):
                return subprocess.run([*command, *options, "--", "cargo", "test", "--offline"],
                                      cwd=root, env=env, capture_output=True, text=True)

            for source, options in (
                ("", ()),
                ("#[test] fn present() {}\n", ("--filter", "typo_no_match")),
                ("#[test] fn present() {}\n", ("--skip", "present")),
            ):
                with self.subTest(options=options):
                    (root / "src/lib.rs").write_text(source)
                    result = run(*options)
                    self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertIn("no tests matched", result.stderr)
                    self.assertNotIn("test result: ok", result.stdout)
                    allowed = run("--allow-empty", *options)
                    self.assertEqual(allowed.returncode, 0, allowed.stdout + allowed.stderr)
                    self.assertIn("no tests matched", allowed.stderr)
                    self.assertNotIn("test result: ok", allowed.stdout)

            # Cargo may build an empty integration target alongside selected
            # library tests. Refuse only a completely empty combined selection.
            (root / "tests").mkdir()
            (root / "tests/empty.rs").write_text("")
            selected = run("--filter", "present")
            self.assertEqual(selected.returncode, 0, selected.stdout + selected.stderr)
            self.assertEqual(selected.stdout.count("present ... ok"), 1)
            self.assertIn("test result: ok. 1 passed;", selected.stdout)

    def test_partition_is_disjoint_complete_and_deterministic(self):
        names = [f"t{i:02d}" for i in range(11)]
        groups = self.shards.partition(names, 4)
        self.assertEqual(sorted(n for g in groups for n in g), names)
        self.assertEqual(len(groups), 4)
        self.assertEqual(groups, self.shards.partition(names, 4))
        self.assertEqual(self.shards.partition(["only"], 4), [["only"]])
        for argv in (["--shards", "0", "--", "cargo", "test"], ["--shards", "2", "--", "cargo", "build"],
                     ["--shards", "2", "--", "cargo", "test", "--", "--ignored"]):
            with self.subTest(argv=argv), self.assertRaises(SystemExit):
                self.shards.main(argv)

    def test_real_cargo_shards_run_every_test_once_and_fail_on_any_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "src").mkdir()
            (root / "Cargo.toml").write_text('[package]\nname = "shardfixture"\nversion = "0.1.0"\nedition = "2021"\n')
            tests = "".join(f"#[test] fn case_{i}() {{ assert!(std::path::Path::new(\"Cargo.toml\").exists()); }}\n"
                            for i in range(5))
            (root / "src/lib.rs").write_text("#[cfg(test)] mod tests {\n" + tests + "}\n")
            env = {**os.environ, "CARGO_TARGET_DIR": str(root / "target")}
            command = [sys.executable, str(SCRIPTS / "test_shards.py"), "--shards", "3", "--",
                       "cargo", "test", "--offline"]
            run = subprocess.run(command, cwd=root, env=env, capture_output=True, text=True)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
            passed = [int(n) for n in re.findall(r"test result: ok\. (\d+) passed;", run.stdout)]
            self.assertEqual((len(passed), sum(passed)), (3, 5))  # cwd is the package directory
            (root / "src/lib.rs").write_text(
                "#[cfg(test)] mod tests {\n" + tests
                + "mod child { #[test] fn nested() {} }\n}\n")
            filtered = subprocess.run(
                [sys.executable, str(SCRIPTS / "test_shards.py"), "--shards", "3",
                 "--filter", "tests::", "--", "cargo", "test", "--offline"],
                cwd=root, env=env, capture_output=True, text=True)
            self.assertEqual(filtered.returncode, 0, filtered.stdout + filtered.stderr)
            passed = [int(n) for n in re.findall(r"test result: ok\. (\d+) passed;", filtered.stdout)]
            self.assertEqual(sum(passed), 6)
            self.assertIn("tests::child::nested ... ok", filtered.stdout)
            (root / "src/lib.rs").write_text("#[cfg(test)] mod tests {\n" + tests + "#[test] fn broken() { panic!() }\n}\n")
            failed = subprocess.run(command, cwd=root, env=env, capture_output=True, text=True)
            self.assertEqual(failed.returncode, 1)
            self.assertIn("broken", failed.stdout)


class TestOnlyScopeTests(unittest.TestCase):
    FILES = {
        "crates/core/src/lib.rs": ('mod prod;\n#[cfg(test)]\nmod a_tests;\n'
                                   '#[cfg(any(test, feature = "x"))]\npub mod support;\n'
                                   '#[cfg(test)]\n#[path = "custom/p.rs"]\nmod p;\n'
                                   '#[cfg(all(test, not(feature = "x")))]\nmod negated;\n'
                                   '#[cfg(all(test, feature = "x"))]\nmod featured;\n'
                                   'mod outer {\n    #[cfg(test)]\n    mod inline_tests;\n}\n'),
        "crates/core/src/prod.rs": "#[cfg(test)]\nmod t;\npub fn f() {}\n",
        "crates/core/src/prod/t.rs": "// test\n",
        "crates/core/src/a_tests.rs": "mod helper;\n",
        "crates/core/src/a_tests/helper.rs": "// helper\n",
        "crates/core/src/support.rs": "// support\n",
        "crates/core/src/custom/p.rs": "// path test\n",
        "crates/core/src/negated.rs": "// negated\n",
        "crates/core/src/featured.rs": "// featured\n",
        "crates/core/src/outer/inline_tests.rs": "// inline\n",
        "crates/core/tests/integration.rs": "// integration\n",
        "crates/ui/src/lib.rs": "pub fn g() {}\n",
    }

    def write(self, root):
        for name, text in self.FILES.items():
            (root / name).parent.mkdir(parents=True, exist_ok=True)
            (root / name).write_text(text)

    def test_only_cfg_test_declarations_and_owner_targets_are_owner_only(self):
        inventory = BaseAdvanceCarryTests.INVENTORY
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.write(root)
            owner = lambda name: plan.test_only_owner(root, name, inventory)
            for name in ("src/a_tests.rs", "src/a_tests/helper.rs", "src/prod/t.rs", "src/custom/p.rs",
                         "src/featured.rs", "tests/integration.rs"):
                with self.subTest(name=name):
                    self.assertEqual(owner("crates/core/" + name), "core")
            for name in ("src/lib.rs", "src/prod.rs", "src/support.rs", "src/negated.rs",
                         "src/outer/inline_tests.rs", "src/missing.rs", "build.rs"):
                with self.subTest(name=name):
                    self.assertIsNone(owner("crates/core/" + name))
            (root / "crates/core/src/prod.rs").write_text('include!("a_tests/helper.rs");\n')
            self.assertIsNone(owner("crates/core/src/a_tests.rs"))

    def test_resolved_plan_tests_only_owners_and_mixed_inputs_widen(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            git = BaseAdvanceCarryTests.repository(self, root)
            self.write(root)
            git("add", ".")
            git("commit", "-qm", "fixture")
            base = git("rev-parse", "HEAD")
            metadata = BaseAdvanceCarryTests.metadata(self, root)
            (root / "crates/core/src/a_tests.rs").write_text("mod helper;\n// changed\n")
            git("commit", "-qam", "test only")
            resolved = plan.resolve(root, profile="merge", base=base, source=git("rev-parse", "HEAD"),
                                    metadata=metadata)
            self.assertEqual((resolved["scope"], resolved["affected_packages"]), ("rust", ["core"]))
            self.assertTrue(any("test-only" in r and "ui" in r for r in resolved["reasons"]))
            steps = [s.argv for s in ci.planned_steps(resolved)]
            self.assertIn(("cargo", "test", "-p", "core", "--locked"), steps)
            self.assertIn(("cargo", "clippy", "-p", "core", "--all-targets", "--locked", "--", "-D", "warnings"), steps)
            (root / "crates/core/src/prod.rs").write_text("#[cfg(test)]\nmod t;\npub fn f() -> u8 { 1 }\n")
            git("commit", "-qam", "production too")
            mixed = plan.resolve(root, profile="merge", base=base, source=git("rev-parse", "HEAD"),
                                 metadata=metadata)
            self.assertEqual(mixed["affected_packages"], ["core", "ui"])


class SingleRunFeatureTests(unittest.TestCase):
    def live(self):
        root = SCRIPTS.parent
        metadata = json.loads(subprocess.check_output(
            ["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"], cwd=root))
        return root, plan.cargo_inventory(metadata, root)

    def test_reviewed_additive_features_test_once_and_keep_doctests_and_lint(self):
        root, live_inventory = self.live()
        path = "crates/hex-session/src/lib.rs"
        _, live_affected, _ = plan.classify([path], live_inventory)
        live_boundary = plan.boundary_selection(root, [path], live_inventory, live_affected)
        session = next(p for p in live_inventory if p["name"] == "hex-session")
        extra = set(session["features"]) - {"default", "source-overview-test-support"}
        if extra:
            self.assertNotIn("hex-session", live_boundary["single_run"])
            live_steps = ci.planned_steps({"scope": "rust", "affected_packages": live_affected,
                                           "boundary": live_boundary, "cargo_inventory": live_inventory})
            self.assertTrue(any(s.step_id == "affected-tests" for s in live_steps))
            for feature in extra:
                self.assertTrue(any(s.argv[1] == "test" and "hex-session" in s.argv
                                    and feature in s.argv for s in live_steps))
        # The reviewed one-feature case is a controlled fixture; live packages
        # may acquire extra features that correctly require separate runs.
        inventory = [{**p, "features": {name: value for name, value in p["features"].items()
                                         if name in {"default", "source-overview-test-support"}}}
                     if p["name"] == "hex-session" else p for p in live_inventory]
        _, affected, _ = plan.classify([path], inventory)
        fixture_dir = tempfile.TemporaryDirectory()
        self.addCleanup(fixture_dir.cleanup)
        fixture = Path(fixture_dir.name)
        (fixture / "crates/hex-session/src").mkdir(parents=True)
        (fixture / path).write_text("// additive feature fixture\n")
        boundary = plan.boundary_selection(fixture, [path], inventory, affected)
        # Only the owner tests; its additive feature still runs once.
        self.assertEqual(boundary["test_packages"], ["hex-session"])
        self.assertEqual(boundary["single_run"], {"hex-session": "source-overview-test-support"})
        steps = {s.step_id: s for s in ci.planned_steps({
            "scope": "rust", "affected_packages": affected, "boundary": boundary, "cargo_inventory": inventory})}
        self.assertNotIn("affected-tests", steps)  # the owner's only run is its feature run
        self.assertIn("hex-session", steps["single-run-doctests"].argv)
        self.assertTrue(any("hex-session" in s.argv and "source-overview-test-support" in s.argv
                            and s.argv[1] == "test" for s in steps.values()))
        # Every consumer still compiles in every configuration, without running its suites.
        for name in ("hex-session", "hex-ui", "hex-desktop"):
            self.assertIn(name, steps["affected-clippy"].argv)
        self.assertNotIn("desktop-allocation-probe", steps)
        features = steps["affected-feature-clippy"].argv[steps["affected-feature-clippy"].argv.index("--features") + 1]
        self.assertIn("hex-desktop/allocation-probe", features)
        self.assertIn("hex-ui/ui-test-support", features)
        self.assertFalse(any(s.argv[1] == "test" and "hex-ui" in s.argv for s in steps.values()))
        self.assertFalse(steps["single-run-doctests"].requires_tests)
        # A build input keeps every consumer's suites.
        manifest = "crates/hex-session/Cargo.toml"
        self.assertEqual(plan.boundary_selection(root, [manifest], inventory, affected)["test_packages"],
                         sorted(affected))

    def test_cfg_not_tripwire_and_unreviewed_features_keep_both_runs(self):
        inventory = [dict(name="hex-ui", manifest="crates/hex-ui/Cargo.toml", features={"ui-test-support": []},
                          dependencies=[], targets=[]),
                     dict(name="hex-session", manifest="crates/hex-session/Cargo.toml",
                          features={"source-overview-test-support": [], "new-configuration": []},
                          dependencies=[], targets=[])]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "crates/hex-ui/src").mkdir(parents=True)
            self.assertEqual(plan.single_run_packages(root, ["hex-ui", "hex-session"], inventory),
                             {"hex-ui": "ui-test-support"})
            inventory[0]["features"]["ui-unit-tier"] = []
            self.assertEqual(plan.single_run_packages(root, ["hex-ui", "hex-session"], inventory),
                             {"hex-ui": "ui-test-support"})
            ui_source = root / "crates/hex-ui/src/lib.rs"
            ui_source.write_text('#[cfg(all(test, feature = "ui-unit-tier"))]\nmod tests {}\n')
            self.assertEqual(plan.single_run_packages(root, ["hex-ui", "hex-session"], inventory),
                             {"hex-ui": "ui-test-support"})
            ui_source.write_text('#[cfg(feature = "ui-unit-tier")]\nfn runtime() {}\n')
            self.assertEqual(plan.single_run_packages(root, ["hex-ui", "hex-session"], inventory), {})
            ui_source.write_text("")
            inventory[0]["features"]["unknown"] = []
            self.assertEqual(plan.single_run_packages(root, ["hex-ui", "hex-session"], inventory), {})
            del inventory[0]["features"]["unknown"]
            (root / "crates/hex-ui/src/lib.rs").write_text('#[cfg(not(feature = "ui-test-support"))]\nfn f() {}\n')
            self.assertEqual(plan.single_run_packages(root, ["hex-ui", "hex-session"], inventory), {})


class BaseAdvanceCarryTests(unittest.TestCase):
    """Fabricated reports exercise carry policy; no product test counts are claimed."""

    INVENTORY = [dict(name=name, manifest=f"crates/{name}/Cargo.toml", features={}, targets=[],
                      dependencies=[{"name": d, "kind": None} for d in deps])
                 for name, deps in (("core", []), ("ui", ["core"]), ("desktop", ["ui"]), ("tool", []))]

    def entry(self, step, count=1):
        return {"step_id": step.step_id, "argv": list(step.argv), "status": "passed", "log": "old.log",
                "test_results": {"rust_passed_by_binary": [count]}}

    def test_steps_carry_only_outside_the_changed_dependency_closure(self):
        shared = {"mode": "shared", "test_selectors": ["test_python_test_plan"], "reasons": ["app-only"]}
        steps = (ci.Step("format", ("cargo", "fmt", "--all", "--check")),
                 ci.Step("runner-tests", ci.ci_plan.python_tests.COMPREHENSIVE_ARGV, True, True),
                 ci.Step("affected-tests", ("cargo", "test", "-p", "core", "-p", "tool", "--locked"), requires_tests=True),
                 ci.Step("tool-clippy", ("cargo", "clippy", "-p", "tool", "--locked")),
                 ci.Step("desktop-build", ("cargo", "build", "-p", "desktop", "--locked")),
                 ci.Step("workspace-tests", ("cargo", "test", "--workspace", "--locked"), requires_tests=True),
                 ci.Step("new-step", ("cargo", "test", "-p", "tool", "--features", "x"), requires_tests=True),
                 ci.Step("zero-match", ("cargo", "test", "-p", "tool", "--lib"), requires_tests=True))
        evidence = {"steps": [self.entry(s, 0 if s.step_id == "zero-match" else 1)
                              for s in steps if s.step_id != "new-step"]}
        actions = lambda owners, python=shared: [d["action"] for d in ci.carry_decisions(
            steps, evidence, {"delta_owners": owners, "python_tests": python}, self.INVENTORY)]
        # A leaf change reruns its consumers only; guards always rerun.
        self.assertEqual(actions(["desktop"]),
                         ["run", "narrow", "carry", "carry", "run", "run", "run", "run"])
        # A shared dependency reruns every step whose closure reaches it.
        self.assertEqual(actions(["core"]),
                         ["run", "narrow", "run", "carry", "run", "run", "run", "run"])
        self.assertEqual(actions([], {"mode": "none", "reasons": []}),
                         ["run", "carry", "carry", "carry", "carry", "carry", "run", "run"])
        comprehensive = {"mode": "comprehensive", "reasons": ["tooling"]}
        self.assertEqual(actions(["tool"], comprehensive)[1:4], ["run", "run", "run"])
        narrowed = ci.carry_decisions(steps, evidence, {"delta_owners": ["desktop"], "python_tests": shared},
                                      self.INVENTORY)[1]
        self.assertEqual(narrowed["argv"], ci.ci_plan.python_tests.argv(shared))
        # A narrowed or carried entry still matches by its planned configuration.
        chained = {"steps": [{**self.entry(steps[1]), "argv": list(narrowed["argv"]),
                              "carry": {"planned_argv": list(steps[1].argv)}},
                             {**self.entry(steps[3]), "status": "carried"}]}
        self.assertEqual([d["action"] for d in ci.carry_decisions(
            steps[1:4:2], chained, {"delta_owners": ["desktop"], "python_tests": shared}, self.INVENTORY)],
            ["narrow", "carry"])

    def repository(self, root):
        def git(*args):
            return subprocess.check_output(["git", *args], cwd=root).decode().strip()
        git("init", "-q", "-b", "main")
        git("config", "user.name", "carry fixture")
        git("config", "user.email", "fixture@invalid")
        (root / ".gitignore").write_text("/artifacts/\n")
        for name in ("core", "ui"):
            (root / f"crates/{name}/src").mkdir(parents=True)
            (root / f"crates/{name}/Cargo.toml").write_text(f"[package]\nname='{name}'\n")
            (root / f"crates/{name}/src/lib.rs").write_text("// base\n")
        git("add", ".")
        git("commit", "-qm", "base")
        return git

    def metadata(self, root):
        return {"workspace_members": ["core", "ui"], "packages": [
            dict(id=name, name=name, manifest_path=str(root / f"crates/{name}/Cargo.toml"), features={},
                 targets=[], dependencies=[{"name": "core", "kind": None}] if name == "ui" else [])
            for name in ("core", "ui")]}

    def evidence(self, root, resolved):
        tree = {"commit": resolved["source"], "dirty": False, "dirty_files": [], "content_sha256": "fixture"}
        return {"schema_version": 2, "mode": "merge", "status": "passed", "exit_code": 0,
                "commit": resolved["source"], "dirty": False, "stale": False,
                "native_acceptance_opt_in": False, "start_tree": tree, "end_tree": tree,
                "plan": resolved, "target_cache": {"dependency_inputs": {"version": 1}},
                "steps": [{"step_id": "affected-tests", "argv": ["cargo", "test", "-p", "ui"], "status": "passed",
                           "test_results": {"rust_passed_by_binary": [1]}}]}

    @contextmanager
    def carry_fixture(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            git = self.repository(root)
            base = git("rev-parse", "HEAD")
            git("checkout", "-qb", "pr")
            (root / "crates/ui/src/a.rs").write_text("// pr\n")
            git("add", ".")
            git("commit", "-qm", "pr")
            evidence = self.evidence(root, plan.resolve(root, profile="merge", base=base,
                                                        source=git("rev-parse", "HEAD"), metadata=self.metadata(root)))
            candidate = git("rev-parse", "HEAD")

            def advance(paths, *, then=None):
                git("checkout", "-q", "--detach", base)
                for path, text in paths.items():
                    (root / path).parent.mkdir(parents=True, exist_ok=True)
                    (root / path).write_text(text)
                git("add", ".")
                git("commit", "-qm", "advance")
                advanced = git("rev-parse", "HEAD")
                git("checkout", "-q", "--detach", candidate)
                git("merge", "-q", "--no-edit", advanced)
                if then:
                    (root / then).write_text("// edited after merge\n")
                    git("add", ".")
                    git("commit", "-qm", "extra")
                resolved = plan.resolve(root, profile="merge", base=advanced,
                                        source=git("rev-parse", "HEAD"), metadata=self.metadata(root))
                return plan.carry_basis(root, resolved, evidence, env={})

            yield root, base, candidate, evidence, advance

    def test_carry_basis_accepts_disjoint_ordinary_advance(self):
        with self.carry_fixture() as (_, _, candidate, _, advance):
            basis = advance({"crates/core/src/lib.rs": "// advanced\n", "docs/a.md": "prose"})
            self.assertEqual(basis["delta_owners"], ["core"])
            self.assertEqual(basis["report_commit"], candidate)

    def assert_carry_advance_refused(self, paths, then, message):
        with self.carry_fixture() as (_, _, _, _, advance):
            with self.assertRaisesRegex(ValueError, message):
                advance(paths, then=then)

    def test_carry_basis_rejects_nonverbatim_advance(self):
        self.assert_carry_advance_refused({"crates/core/src/lib.rs": "// advanced\n"},
                                          "crates/core/src/lib.rs", "verbatim")

    def test_carry_basis_rejects_candidate_own_path_change(self):
        self.assert_carry_advance_refused({"crates/core/src/lib.rs": "// advanced\n"},
                                          "crates/ui/src/a.rs", "own path")

    def test_carry_basis_rejects_toolchain_change(self):
        self.assert_carry_advance_refused({"rust-toolchain.toml": "[toolchain]\n"}, None, "toolchain")

    def test_carry_basis_rejects_dependency_change(self):
        self.assert_carry_advance_refused({"crates/core/Cargo.toml": "[package]\nname='core'\nversion='2'\n"},
                                          None, "dependency")

    def test_carry_basis_rejects_tooling_change(self):
        self.assert_carry_advance_refused({"scripts/ci_plan.py": "# tool\n"}, None, "tooling")

    def test_carry_basis_rejects_invalid_report_and_native_inputs(self):
        with self.carry_fixture() as (root, base, candidate, evidence, _):
            current = plan.resolve(root, profile="merge", base=base, source=candidate, metadata=self.metadata(root))
            for field, value in (("status", "failed"), ("stale", True), ("mode", "full"),
                                 ("native_acceptance_opt_in", None), ("steps", [])):
                with self.subTest(field=field), self.assertRaisesRegex(ValueError, "not a complete passed"):
                    plan.carry_basis(root, current, {**evidence, field: value}, env={})
            with self.assertRaisesRegex(ValueError, "native/external"):
                plan.carry_basis(root, current, evidence, env={"HEX_VST3_SDK_DIR": "x"})
            # A report whose base is newer than the current base cannot carry.
            newer = {**evidence["plan"], "base": candidate}
            with self.assertRaisesRegex(ValueError, "not an ancestor"):
                plan.carry_basis(root, current, {**evidence, "plan": newer}, env={})

    def test_runner_records_carried_origin_and_falls_back_to_fresh_runs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            report = root / "old.json"
            step = ci.Step("tool-tests", ("cargo", "test", "-p", "tool"), requires_tests=True)
            report.write_text(json.dumps({"steps": [self.entry(step)],
                                          "target_cache": {"dependency_inputs": {"version": 1}}}))
            resolved = {"cargo_inventory": self.INVENTORY, "profile": "merge"}
            runner = ci.LocalCiRunner(root, mode="merge", plan=resolved, min_free_bytes=0,
                                      steps=(step,), carry_report=report)
            runner.output_dir.mkdir(parents=True)  # run() creates it before any step.
            basis = {"report_commit": "c0", "delta_paths": ["crates/core/src/lib.rs"],
                     "delta_owners": ["core"], "python_tests": {"mode": "none", "reasons": []}}
            with patch.object(ci.ci_plan, "carry_basis", return_value=basis):
                runner._prepare_carry()
            self.assertEqual(runner.summary["carry"]["decisions"][0]["action"], "carry")
            runner._record_carried(step, 1, runner._carry[0])
            carried = runner.summary["steps"][0]
            self.assertEqual((carried["status"], carried["execution"], carried["log"]), ("carried", "carried", None))
            self.assertEqual(carried["carried_from"], {"report": str(report.resolve()), "commit": "c0", "log": "old.log"})
            # A chained carry keeps the run that actually executed the step.
            self.assertEqual(runner._carry_origin({"carried_from": {"report": "first.json"}}), {"report": "first.json"})
            runner._drop_carry("toolchain changed")
            self.assertIsNone(runner._carry)
            self.assertIn("run fresh", runner.summary["carry"]["refused"])
            refused = ci.LocalCiRunner(root, mode="merge", plan=resolved, min_free_bytes=0,
                                       steps=(step,), carry_report=report)
            with patch.object(ci.ci_plan, "carry_basis", side_effect=ValueError("own path changed")):
                refused._prepare_carry()
            self.assertIsNone(refused._carry)
            self.assertIn("every step runs fresh", refused.summary["carry"]["refused"])


class MainHealthTests(unittest.TestCase):
    INVENTORY = BaseAdvanceCarryTests.INVENTORY

    def profile_steps(self, profile, extra=None, workspace_change=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            git = BaseAdvanceCarryTests.repository(self, root)
            manifest = root / "Cargo.toml"
            manifest.write_text('[workspace]\nmembers = ["crates/*"]\n')
            git("add", "Cargo.toml")
            git("commit", "-qm", "manifest base")
            git("checkout", "-qb", "profile")
            content = manifest.read_text()
            if workspace_change:
                content = content.replace('["crates/*"]', '["crates/*", "tools"]')
            manifest.write_text(content + "\n" + profile)
            if extra:
                (root / extra).write_text("changed\n")
            git("add", ".")
            git("commit", "-qm", "profile change")
            git("checkout", "-q", "main")
            git("merge", "-q", "--no-ff", "--no-edit", "profile")
            source = git("rev-parse", "HEAD")
            resolved = plan.resolve(root, profile="health", base=source + "^1", source=source,
                                    metadata=BaseAdvanceCarryTests.metadata(self, root))
            return [(step.step_id, step.argv) for step in ci.planned_steps(resolved)]

    def test_health_profile_codegen_selects_retuned_owner(self):
        steps = self.profile_steps('[profile.dev.package.core]\nopt-level = 1\n')
        self.assertIn(("health-tests", ("cargo", "test", "-p", "core", "--locked")), steps)

    def test_health_profile_codegen_and_other_owner_select_both(self):
        steps = self.profile_steps('[profile.dev.package.core]\nopt-level = 1\n',
                                   "crates/ui/src/lib.rs")
        self.assertIn(("health-tests", ("cargo", "test", "-p", "core", "-p", "ui", "--locked")), steps)

    def test_health_two_retuned_profiles_select_both_owners(self):
        steps = self.profile_steps('[profile.dev.package.core]\nopt-level = 1\n'
                                   '[profile.dev.package.ui]\nopt-level = 1\n')
        self.assertIn(("health-tests", ("cargo", "test", "-p", "core", "-p", "ui", "--locked")), steps)

    def assert_workspace_profile(self, profile):
        steps = self.profile_steps(profile)
        self.assertIn(("health-tests", ("cargo", "test", "--workspace", "--locked")), steps)

    def test_health_unsafe_package_profile_keeps_workspace(self):
        self.assert_workspace_profile('[profile.dev.package.core]\ndebug-assertions = false\n')

    def test_health_unknown_package_profile_keeps_workspace(self):
        self.assert_workspace_profile('[profile.dev.package.unknown]\nopt-level = 1\n')

    def test_health_workspace_profile_keeps_workspace(self):
        self.assert_workspace_profile('[profile.release]\nlto = true\n')

    def test_health_profile_with_lockfile_keeps_workspace(self):
        steps = self.profile_steps('[profile.dev.package.core]\nopt-level = 1\n', "Cargo.lock")
        self.assertIn(("health-tests", ("cargo", "test", "--workspace", "--locked")), steps)

    def test_health_profile_with_workspace_change_keeps_workspace(self):
        steps = self.profile_steps('[profile.dev.package.core]\nopt-level = 1\n',
                                   workspace_change=True)
        self.assertIn(("health-tests", ("cargo", "test", "--workspace", "--locked")), steps)

    def steps(self, paths):
        health = plan.health_selection(paths, self.INVENTORY)
        return [(s.step_id, s.argv) for s in ci.planned_steps(
            {"health": health, "base": "b", "source": "s", "scope": "rust",
             "python_tests": {"mode": "comprehensive"}})]

    def test_health_owned_tooling_resolves_to_selected_command(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            git = BaseAdvanceCarryTests.repository(self, root)
            (root / "scripts").mkdir()
            script = root / "scripts/ci_plan.py"
            script.write_text("# base\n")
            git("add", ".")
            git("commit", "-qm", "tool base")
            script.write_text("# change\n")
            git("commit", "-qam", "tool change")
            source = git("rev-parse", "HEAD")
            with patch.object(plan.python_tests, "available_modules",
                              return_value=sorted(plan.python_tests.REVIEWED_MODULES)):
                resolved = plan.resolve(root, profile="health", base=source + "^1", source=source,
                                        metadata=BaseAdvanceCarryTests.metadata(self, root))
            selection = resolved["python_tests"]
            self.assertEqual(selection["mode"], "shared")
            steps = {s.step_id: s for s in ci.planned_steps(resolved)}
            self.assertEqual(steps["health-runner-tests"].argv, plan.python_tests.argv(selection))
            self.assertTrue(steps["health-runner-tests"].requires_tests)

    def test_health_tooling_refuses_empty_python_selection(self):
        health = plan.health_selection(["scripts/ci_plan.py"], self.INVENTORY)
        with self.assertRaises(ValueError):
            ci.planned_steps({"health": health, "base": "b", "source": "s", "scope": "tooling",
                              "python_tests": {"mode": "none"}})

    def test_health_tests_only_the_merged_owners_and_widens_for_unknown_inputs(self):
        fmt = ("format", ("cargo", "fmt", "--all", "--check"))
        self.assertEqual(self.steps(["crates/core/src/a.rs", "crates/ui/tests/b.rs", "docs/a.md"]),
                         [fmt, ("health-tests", ("cargo", "test", "-p", "core", "-p", "ui", "--locked"))])
        self.assertEqual(self.steps(["scripts/ci_plan.py"]),
                         [fmt, ("health-runner-tests", ci.ci_plan.python_tests.COMPREHENSIVE_ARGV)])
        self.assertEqual(self.steps(["Cargo.lock", "crates/core/src/a.rs"])[1],
                         ("health-tests", ("cargo", "test", "--workspace", "--locked")))
        self.assertEqual(self.steps(["docs/a.md"])[1][0], "metadata")

    def test_health_requires_the_merge_commit_and_its_first_parent(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            git = BaseAdvanceCarryTests.repository(self, root)
            first = git("rev-parse", "HEAD")
            git("checkout", "-qb", "pr")
            (root / "crates/ui/src/a.rs").write_text("// pr\n")
            git("add", ".")
            git("commit", "-qm", "pr")
            git("checkout", "-q", "main")
            (root / "crates/core/src/b.rs").write_text("// main\n")
            git("add", ".")
            git("commit", "-qm", "main")
            git("merge", "-q", "--no-ff", "--no-edit", "pr")
            merge = git("rev-parse", "HEAD")
            metadata = BaseAdvanceCarryTests.metadata(self, root)
            resolved = plan.resolve(root, profile="health", base=merge + "^1", source=merge, metadata=metadata)
            self.assertEqual(resolved["health"]["packages"], ["ui"])
            self.assertFalse(resolved["qualifies_merge"])
            with self.assertRaisesRegex(ValueError, "first parent"):
                plan.resolve(root, profile="health", base=first, source=merge, metadata=metadata)


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
            # Target storage policy participates in the guarded CI inputs too.
            (root / "scripts").mkdir()
            helper = root / "scripts/ci_target_storage.py"
            old = key()
            helper.write_text("original storage guard")
            self.assertNotEqual(old, key())
            old = key()
            stamp = helper.stat()
            helper.write_text("modified storage guard")
            os.utime(helper, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
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

class MergeTestBudgetTests(unittest.TestCase):
    def entry(self, step_id, argv, duration, build=None, rust=None, carried=False):
        return {"step_id": step_id, "argv": list(argv), "duration_seconds": duration,
                "execution": "carried" if carried else "executed",
                "timing_estimates": {"cargo_build_phase_seconds": build, "rust_harness_seconds": rust,
                                     "python_harness_seconds": None, "unclassified_seconds": None}}

    def test_counts_test_execution_only_and_flags_the_one_minute_budget(self):
        steps = [
            self.entry("format", ("cargo", "fmt", "--all", "--check"), 9.0),
            self.entry("affected-clippy", ("cargo", "clippy", "-p", "a"), 80.0, build=80.0),
            self.entry("affected-tests", ("cargo", "test", "-p", "a"), 70.0, build=40.0, rust=25.0),
            self.entry("carried-tests", ("cargo", "test", "-p", "b"), 500.0, build=1.0, rust=499.0, carried=True),
        ]
        within = ci.merge_test_budget(steps, None)
        self.assertEqual(within["test_seconds"], 30.0)  # build phase and lint/format excluded
        self.assertFalse(within["exceeded"])
        steps.append(self.entry("ui-feature-tests", ("cargo", "test", "-p", "ui"), 45.0, build=5.0, rust=38.0))
        over = ci.merge_test_budget(steps, None)
        self.assertEqual(over["test_seconds"], 70.0)
        self.assertTrue(over["exceeded"])
        self.assertEqual(over["slowest"][0], {"step_id": "ui-feature-tests", "seconds": 40.0})
        self.assertEqual(ci.merge_test_budget(steps, "Pulse #1 slow ui suite")["over_budget_reason"],
                         "Pulse #1 slow ui suite")

class ModuleScopeTests(unittest.TestCase):
    """Module-scoped owner tests (user policy, 1 November 2026)."""

    def test_editor_unit_targets_follow_each_module_and_editor_ancestor(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        path = "crates/hex-ui/src/editor/pending_edit.rs"
        filters = plan.module_test_filters(root, [path], inventory, [owner])
        self.assertEqual(filters, {owner: ["editor::pending_edit::"]})
        viewport = "editor::viewport::"
        timeline = ["editor::musical_time::", "editor::seek::", "editor::signed_frame_viewport::", viewport]
        self.assertEqual(plan.module_test_filters(
            root, ["crates/hex-ui/src/editor/viewport.rs"], inventory, [owner]),
            {owner: timeline})
        disk = "editor::disk_overload::"
        self.assertEqual(plan.module_test_filters(
            root, ["crates/hex-ui/src/editor/disk_overload.rs"], inventory, [owner]),
            {owner: [disk]})
        cpu = "editor::cpu_meter::"
        self.assertEqual(plan.module_test_filters(
            root, ["crates/hex-ui/src/editor/cpu_meter.rs"], inventory, [owner]),
            {owner: [cpu]})
        thumbnail = "editor::midi_thumbnail::"
        self.assertEqual(plan.module_test_filters(
            root, ["crates/hex-ui/src/editor/midi_thumbnail.rs"], inventory, [owner]),
            {owner: [thumbnail]})
        scale = "editor::track_meters::scale::"
        self.assertEqual(plan.module_test_filters(
            root, ["crates/hex-ui/src/editor/track_meters/scale.rs"], inventory, [owner]),
            {owner: [scale]})
        admission = "editor::audio_admission::"
        self.assertEqual(plan.module_test_filters(
            root, ["crates/hex-ui/src/editor/audio_admission.rs"], inventory, [owner]),
            {owner: [admission]})
        navigation = "editor::clip_content_cache::profile::navigation::selection::"
        self.assertEqual(plan.module_test_filters(
            root, ["crates/hex-ui/src/editor/navigation_profile/selection.rs"], inventory, [owner]),
            {owner: [navigation]})
        recovery = "editor::recovery_state::"
        recovery_integration = "editor::recovery_state_integration_tests::"
        self.assertEqual(plan.module_test_filters(
            root, ["crates/hex-ui/src/editor/recovery_state.rs"], inventory, [owner]),
            {owner: [recovery, recovery_integration]})
        self.assertEqual(plan.module_test_filters(
            root, ["crates/hex-ui/src/editor/recovery_state_integration_tests.rs"], inventory, [owner]),
            {owner: [recovery, recovery_integration]})
        play_intent = "editor::play_intent::"
        self.assertEqual(plan.module_test_filters(
            root, ["crates/hex-ui/src/editor/play_intent.rs"], inventory, [owner]),
            {owner: [play_intent]})
        all_modules = sorted([admission, navigation, cpu, disk, thumbnail, "editor::pending_edit::",
                              play_intent, recovery, recovery_integration, scale, *timeline])
        self.assertEqual(plan.module_test_filters(
            root, ["crates/hex-ui/tests/unit_editor_state.rs"], inventory, [owner]),
            {owner: all_modules})
        # A removed test target remains unproven rather than silently selecting
        # a subset; this migration uses an explicitly reviewed check set.
        self.assertEqual(plan.module_test_filters(
            root, ["crates/hex-ui/tests/unit_pending_edit.rs"], inventory, [owner]), {})
        for names, expected in ((filters[owner], True), ([viewport], True), ([disk], True), ([cpu], True),
                                ([thumbnail], True), ([scale], True), ([admission], True), ([navigation], True),
                                ([play_intent], True),
                                ([recovery, recovery_integration], True),
                                (["editor::clip_content_cache::"], True),
                                (["editor::track_meters::"], True),
                                (all_modules, True), (["editor::"], True),
                                (["editor::save::"], False)):
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                        "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                        "module_filters": {owner: names}}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                      "boundary": boundary, "cargo_inventory": inventory,
                                      "paths": [path]})
            unit = [step for step in steps if "unit_editor_state" in step.argv]
            self.assertEqual(len(unit), int(expected))
            if unit:
                self.assertTrue(ci.module_scoped(unit[0].argv))
                for name in names:
                    if name != recovery_integration and name not in timeline[:3]:
                        self.assertIn(name, unit[0].argv)
                if recovery in names:
                    library = next(step for step in steps if step.step_id == "ui-feature-tests")
                    self.assertIn(recovery_integration, library.argv)
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                    "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                    "module_filters": {}}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                  "boundary": boundary, "cargo_inventory": inventory,
                                  "paths": ["crates/hex-ui/src/lib.rs"]})
        feature = next(step for step in steps if step.step_id == "ui-feature-tests")
        self.assertNotIn("--lib", feature.argv)
        self.assertNotIn("--test", feature.argv)
        self.assertTrue(any(step.step_id == "ui-piano-unit-tests" for step in steps))
        self.assertTrue(any(step.step_id == "ui-timeline-unit-tests" for step in steps))
        self.assertTrue(any(step.step_id == "ui-session-unit-tests" for step in steps))
        self.assertTrue(any(step.step_id == "ui-automation-unit-tests" for step in steps))
        self.assertTrue(any(step.step_id == "ui-device-unit-tests" for step in steps))
        self.assertTrue(any(step.step_id == "ui-section-unit-tests" for step in steps))

    def test_section_hit_unit_target_follows_its_owner(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        names = ["section_hit::"]
        self.assertEqual(plan.module_test_filters(root, ["crates/hex-ui/src/section_hit.rs"], inventory, [owner]),
                         {owner: names})
        reorder = ["section_appearance::", "section_reorder::"]
        for path in ("section_appearance.rs", "section_reorder.rs"):
            self.assertEqual(plan.module_test_filters(root, ["crates/hex-ui/src/" + path], inventory, [owner]),
                             {owner: reorder})
        for name in ("section_input", "section_resize", "section_selection"):
            self.assertEqual(plan.module_test_filters(root, ["crates/hex-ui/src/" + name + ".rs"], inventory, [owner]),
                             {owner: [name + "::"]})
        self.assertEqual(plan.module_test_filters(root, ["crates/hex-ui/tests/unit_section.rs"], inventory, [owner]),
                         {owner: ["section_appearance::", "section_hit::", "section_input::", "section_reorder::",
                                  "section_resize::", "section_selection::"]})
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner], "lint_packages": [owner],
                    "single_run": {owner: "ui-test-support"}, "module_filters": {owner: names}}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": []})
        self.assertEqual(sum("unit_section" in step.argv for step in steps), 1)
        for name in ("section_input::", "section_resize::", "section_selection::"):
            boundary["module_filters"] = {owner: [name]}
            scoped = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                       "cargo_inventory": inventory, "paths": []})
            unit = [step for step in scoped if "unit_section" in step.argv]
            self.assertEqual(len(unit), 1)
            self.assertIn(name, unit[0].argv)
        self.assertFalse(any("unit_device" in step.argv or "unit_automation" in step.argv or
                             "unit_session" in step.argv or "unit_timeline" in step.argv or
                             "unit_piano_roll" in step.argv for step in steps))
        self.assertTrue(any(step.step_id == "ui-section-unit-tests" for step in ci.planned_steps({"scope": "full"})))
        health = ci.planned_steps({"scope": "rust", "health": {"python": False, "workspace_inputs": True, "packages": []}})
        self.assertTrue(any(step.step_id == "health-ui-section-unit" for step in health))

    def test_device_display_unit_target_follows_its_owner(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        names = ["editor::device_panel::numeric_display::"]
        for path in ("crates/hex-ui/src/editor/device_panel/numeric_display.rs", "crates/hex-ui/tests/unit_device.rs"):
            self.assertEqual(plan.module_test_filters(root, [path], inventory, [owner]), {owner: names})
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner], "lint_packages": [owner],
                    "single_run": {owner: "ui-test-support"}, "module_filters": {owner: names}}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": []})
        self.assertEqual(sum("unit_device" in step.argv for step in steps), 1)
        self.assertFalse(any("unit_automation" in step.argv or "unit_session" in step.argv or
                             "unit_timeline" in step.argv or "unit_piano_roll" in step.argv for step in steps))
        self.assertTrue(any(step.step_id == "ui-device-unit-tests" for step in ci.planned_steps({"scope": "full"})))
        health = ci.planned_steps({"scope": "rust", "health": {"python": False, "workspace_inputs": True, "packages": []}})
        self.assertTrue(any(step.step_id == "health-ui-device-unit" for step in health))

    def test_automation_plot_unit_target_follows_its_owner(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        names = ["editor::device_automation::plot_geometry::"]
        for path in ("crates/hex-ui/src/editor/device_automation/plot_geometry.rs", "crates/hex-ui/tests/unit_automation.rs"):
            self.assertEqual(plan.module_test_filters(root, [path], inventory, [owner]), {owner: names})
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner], "lint_packages": [owner],
                    "single_run": {owner: "ui-test-support"}, "module_filters": {owner: names}}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": []})
        self.assertEqual(sum("unit_automation" in step.argv for step in steps), 1)
        self.assertFalse(any("unit_timeline" in step.argv or "unit_piano_roll" in step.argv or
                             "unit_session" in step.argv for step in steps))
        self.assertTrue(any(step.step_id == "ui-automation-unit-tests" for step in ci.planned_steps({"scope": "full"})))
        health = ci.planned_steps({"scope": "rust", "health": {"python": False, "workspace_inputs": True, "packages": []}})
        self.assertTrue(any(step.step_id == "health-ui-automation-unit" for step in health))

    def test_session_route_unit_target_follows_its_owner(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        names = ["editor::session_route_view::"]
        for path in ("crates/hex-ui/src/editor/session_route_view.rs", "crates/hex-ui/tests/unit_session.rs"):
            self.assertEqual(plan.module_test_filters(root, [path], inventory, [owner]), {owner: names})
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner], "lint_packages": [owner],
                    "single_run": {owner: "ui-test-support"}, "module_filters": {owner: names}}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": []})
        self.assertEqual(sum("unit_session" in step.argv for step in steps), 1)
        self.assertFalse(any("unit_timeline" in step.argv or "unit_piano_roll" in step.argv for step in steps))
        self.assertTrue(any(step.step_id == "ui-session-unit-tests" for step in ci.planned_steps({"scope": "full"})))
        health = ci.planned_steps({"scope": "rust", "health": {"python": False, "workspace_inputs": True, "packages": []}})
        self.assertTrue(any(step.step_id == "health-ui-session-unit" for step in health))

    def test_timeline_target_follows_its_viewport_dependencies(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        timeline = ["editor::musical_time::", "editor::seek::", "editor::signed_frame_viewport::", "editor::viewport::"]
        for path in ("musical_time.rs", "seek.rs", "signed_frame_viewport.rs", "viewport.rs"):
            self.assertEqual(plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + path], inventory, [owner]),
                             {owner: timeline})
        self.assertEqual(plan.module_test_filters(root, ["crates/hex-ui/tests/unit_timeline.rs"], inventory, [owner]),
                         {owner: timeline})
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner], "lint_packages": [owner],
                    "single_run": {owner: "ui-test-support"}, "module_filters": {owner: timeline}}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": []})
        self.assertEqual(sum("unit_timeline" in step.argv for step in steps), 1)
        self.assertEqual(sum("unit_editor_state" in step.argv for step in steps), 1)
        self.assertTrue(any(step.step_id == "ui-timeline-unit-tests" for step in ci.planned_steps({"scope": "full"})))
        health = ci.planned_steps({"scope": "rust", "health": {"python": False, "workspace_inputs": True, "packages": []}})
        self.assertTrue(any(step.step_id == "health-ui-timeline-unit" for step in health))

    def test_piano_unit_target_follows_owners_and_runs_once(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        piano = ["editor::midi_drag::", "editor::midi_group_drag::", "editor::midi_scene::", "editor::midi_selection::", "editor::midi_stroke::", "editor::midi_velocity::", "editor::midi_viewport_grid::"]
        for path in ("midi_drag.rs", "midi_group_drag.rs", "midi_scene.rs", "midi_selection.rs", "midi_stroke.rs", "midi_velocity.rs", "midi_viewport_grid.rs"):
            self.assertEqual(plan.module_test_filters(
                root, ["crates/hex-ui/src/editor/" + path], inventory, [owner]), {owner: piano})
        self.assertEqual(plan.module_test_filters(
            root, ["crates/hex-ui/tests/unit_piano_roll.rs"], inventory, [owner]),
            {owner: sorted(piano + ["editor::midi_source_controls::"])})
        self.assertEqual(plan.module_test_filters(
            root, ["crates/hex-ui/src/editor/midi_source_controls.rs"], inventory, [owner]),
            {owner: ["editor::midi_source_controls::"]})
        for names, expected in (([piano[0]], True), (piano, True),
                                (["editor::midi_source_controls::"], True), (["editor::"], True),
                                (["editor::save::"], False)):
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                        "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                        "module_filters": {owner: names}}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                      "boundary": boundary, "cargo_inventory": inventory,
                                      "paths": []})
            units = [step for step in steps if "unit_piano_roll" in step.argv]
            self.assertEqual(len(units), int(expected))
            if units:
                self.assertIn("ui-unit-tier", units[0].argv)
                for name in names:
                    self.assertIn(name, units[0].argv)
                self.assertTrue(ci.module_scoped(units[0].argv))
        full = ci.planned_steps({"scope": "full"})
        self.assertTrue(any(step.step_id == "ui-piano-unit-tests" for step in full))
        health = ci.planned_steps({"scope": "rust", "health": {"python": False, "workspace_inputs": True, "packages": []}})
        self.assertTrue(any(step.step_id == "health-ui-piano-unit" for step in health))

    def crate(self, root: Path) -> list[dict]:
        files = {
            "src/lib.rs": "mod parent;\nmod plain;\n#[cfg(test)]\n#[path = \"plain_tests.rs\"]\nmod plain_tests;\n",
            "src/parent.rs": "mod child;\nmod untested;\n#[test] fn parent_case() {}\n",
            "src/parent/child.rs": "#[cfg(test)] mod tests { #[test] fn child_case() {} }\n",
            "src/parent/untested.rs": "pub fn helper() {}\n",
            "src/plain.rs": "pub fn plain() {}\n",
            "src/plain_tests.rs": "#[test] fn plain_case() {}\n",
        }
        for name, text in files.items():
            path = root / "crates/owner" / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
        return [{"name": "owner", "manifest": "crates/owner/Cargo.toml", "features": {},
                 "dependencies": [], "targets": []}]

    def test_changed_modules_select_their_own_or_nearest_tested_subtree(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            inventory = self.crate(root)
            select = lambda *paths: plan.module_test_filters(root, list(paths), inventory, ["owner"])
            self.assertEqual(select("crates/owner/src/parent/child.rs"), {"owner": ["parent::child::"]})
            # No tests of its own: the nearest ancestor with tests.
            self.assertEqual(select("crates/owner/src/parent/untested.rs"), {"owner": ["parent::"]})
            # A broader selected module subsumes a narrower one.
            self.assertEqual(select("crates/owner/src/parent.rs", "crates/owner/src/parent/child.rs"),
                             {"owner": ["parent::"]})
            # #[path] files map to their declared module.
            self.assertEqual(select("crates/owner/src/plain_tests.rs"), {"owner": ["plain_tests::"]})
            # No tested ancestor below the root, the root itself, or non-src inputs: whole crate.
            self.assertEqual(select("crates/owner/src/plain.rs"), {})
            self.assertEqual(select("crates/owner/src/lib.rs", "crates/owner/src/parent/child.rs"), {})
            self.assertEqual(select("crates/owner/tests/it.rs"), {})
            (root / "crates/owner/src/plain.rs").write_text('include!("generated.rs");\n')
            self.assertEqual(select("crates/owner/src/parent/child.rs"), {})

    def test_changed_module_roots_run_the_complete_test_subtree(self):
        root = SCRIPTS.parent
        for owner, path, prefix, child in (
            ("hex-session", "crates/hex-session/src/service.rs", "service::", "format_upgrade_tests"),
            ("hex-ui", "crates/hex-ui/src/editor/save.rs", "editor::save::", "ui_tests"),
        ):
            with self.subTest(owner=owner):
                inventory = [{"name": owner, "manifest": f"crates/{owner}/Cargo.toml",
                              "features": {}, "dependencies": [], "targets": [{"kind": ["lib"]}]}]
                filters = plan.module_test_filters(root, [path], inventory, [owner])
                self.assertEqual(filters, {owner: [prefix]})
                modules = plan.module_paths(root / f"crates/{owner}/src")
                self.assertIn(prefix + child, modules.values())
                boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                            "lint_packages": [owner], "single_run": {}, "module_filters": filters}
                steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                          "boundary": boundary, "cargo_inventory": inventory,
                                          "paths": [path]})
                selected = [step for step in steps if ci.runs_rust_tests(step.argv)]
                self.assertTrue(selected)
                for step in selected:
                    self.assertIn(prefix, step.argv)
                    self.assertNotIn("--skip", step.argv)

    def test_reuse_rejects_a_report_that_skipped_a_child_subtree(self):
        inventory = [{"name": "hex-session", "manifest": "crates/hex-session/Cargo.toml",
                      "features": {}, "dependencies": [], "targets": [{"kind": ["lib"]}]}]
        boundary = {"kind": "packages", "tests": [], "test_packages": ["hex-session"],
                    "lint_packages": ["hex-session"], "single_run": {},
                    "module_filters": {"hex-session": ["service::"]}}
        old_plan = {"scope": "rust", "affected_packages": ["hex-session"],
                    "boundary": boundary, "cargo_inventory": inventory, "paths": []}
        steps = ci.planned_steps(old_plan)
        entries = [{"step_id": step.step_id, "argv": list(step.argv)} for step in steps]
        test = next(entry for entry, step in zip(entries, steps) if ci.runs_rust_tests(step.argv))
        test["argv"].extend(["--skip", "service::format_upgrade_tests::"])
        with tempfile.TemporaryDirectory() as directory:
            report = Path(directory) / "report.json"
            report.write_text(json.dumps({"plan": old_plan, "steps": entries}))
            with self.assertRaisesRegex(ValueError, "complete required configurations"):
                ci.reusable_plan(Path(directory), {}, report)

    def test_runtime_consumer_owner_gets_module_filters_and_doc_examples_gate_doctests(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            inventory = self.crate(root)
            inventory[0]["name"] = "hex-storage"  # a reviewed runtime consumer, here the owner
            path = "crates/owner/src/parent/child.rs"
            self.assertEqual(plan.module_test_filters(root, [path], inventory, ["hex-storage"]),
                             {"hex-storage": ["parent::child::"]})
            self.assertEqual(plan.doc_example_owners(root, [path], inventory), [])
            (root / path).write_text("/// ```\n/// assert!(true);\n/// ```\npub fn f() {}\n")
            self.assertEqual(plan.doc_example_owners(root, [path], inventory), ["hex-storage"])
        inventory = [{"name": "hex-session", "manifest": "crates/hex-session/Cargo.toml",
                      "features": {"source-overview-test-support": []}, "dependencies": [], "targets": []}]
        boundary = {"kind": "packages", "tests": [], "test_packages": ["hex-session"],
                    "lint_packages": ["hex-session"], "single_run": {"hex-session": "source-overview-test-support"},
                    "module_filters": {"hex-session": ["service::"]},
                    "doc_example_owners": []}
        steps = {s.step_id: s for s in ci.planned_steps({
            "scope": "rust", "affected_packages": ["hex-session"], "boundary": boundary,
            "cargo_inventory": inventory, "paths": []})}
        self.assertNotIn("module-doctests-hex-session", steps)
        feature = next(s for s in steps.values() if ci.runs_rust_tests(s.argv))
        self.assertEqual(feature.argv[-1], "service::")
        steps = {s.step_id: s for s in ci.planned_steps({
            "scope": "rust", "affected_packages": ["hex-session"],
            "boundary": {**boundary, "doc_example_owners": ["hex-session"]},
            "cargo_inventory": inventory, "paths": []})}
        self.assertIn("module-doctests-hex-session", steps)

    def test_scoped_steps_append_filters_skip_examples_and_drop_per_step_zero_guard(self):
        inventory = [{"name": "hex-desktop", "manifest": "crates/hex-desktop/Cargo.toml",
                      "features": {"allocation-probe": []}, "dependencies": [], "targets": []},
                     {"name": "hex-ui", "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": []}, "dependencies": [], "targets": []}]
        for owner, filters in (("hex-desktop", ["native_audio::"]), ("hex-ui", ["editor::x::"])):
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner], "lint_packages": [owner],
                        "single_run": {}, "module_filters": {owner: filters}}
            steps = {s.step_id: s for s in ci.planned_steps({
                "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                "cargo_inventory": inventory, "paths": []})}
            tests = [s for s in steps.values() if ci.runs_rust_tests(s.argv)]
            self.assertTrue(tests)
            for step in tests:
                with self.subTest(step=step.step_id):
                    self.assertIn(filters[0], step.argv)
                    self.assertFalse(step.requires_tests)
                    # Only the unit-test target can match a module path.
                    self.assertTrue({"--lib", "--bins"} & set(step.argv))
                    self.assertNotIn("--all-targets", step.argv)
                    if step.argv[:2] == ("python3", "scripts/test_shards.py"):
                        self.assertLess(step.argv.index("--allow-empty"), step.argv.index("--"))
                        self.assertLess(step.argv.index("--filter"), step.argv.index("--"))
            self.assertNotIn("device-gallery-tests", steps)
            self.assertTrue(any(s.argv[:2] == ("cargo", "clippy") for s in steps.values()))

class ProfileManifestTests(unittest.TestCase):
    """Profile-only root manifest changes test the retuned packages (policy v6)."""

    def repo(self, directory: Path, before: str, after: str) -> tuple[str, str]:
        run = lambda *args: subprocess.run(["git", *args], cwd=directory, check=True, capture_output=True)
        run("init", "-q")
        run("config", "user.email", "ci@example.invalid")
        run("config", "user.name", "ci")
        (directory / "Cargo.toml").write_text(before)
        run("add", "Cargo.toml")
        run("commit", "-qm", "base")
        base = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=directory, text=True).strip()
        (directory / "Cargo.toml").write_text(after)
        run("commit", "-qam", "change")
        return base, subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=directory, text=True).strip()

    def test_only_codegen_keys_of_named_members_narrow(self):
        inventory = [{"name": "hex-storage", "manifest": "crates/hex-storage/Cargo.toml", "features": {},
                      "dependencies": [], "targets": []}]
        before = ('[workspace]\nmembers = ["crates/*"] # all\n\n[profile.dev.package."*"]\nopt-level = 2\n')
        cases = {
            '[profile.dev.package.hex-storage]\nopt-level = 1 # faster large-document suites\n': ["hex-storage"],
            '[profile.dev.package."hex-storage"]\ndebug = 1\ncodegen-units = 16\n': ["hex-storage"],
            '[profile.dev.package.hex-storage]\ndebug-assertions = false\n': None,
            '[profile.dev.package.hex-storage]\noverflow-checks = false\n': None,
            '[profile.dev.package.unknown-crate]\nopt-level = 1\n': None,
            '[profile.release]\nlto = true\n': None,
        }
        for addition, expected in cases.items():
            with self.subTest(addition=addition), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                base, source = self.repo(root, before, before + "\n" + addition)
                self.assertEqual(plan.profile_only_packages(root, base, source, inventory), expected)
        for after in (before.replace("opt-level = 2", "opt-level = 1"),  # the "*" table
                      before.replace('["crates/*"]', '["crates/*", "tools"]')):
            with self.subTest(after=after), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                base, source = self.repo(root, before, after)
                self.assertIsNone(plan.profile_only_packages(root, base, source, inventory))
        self.assertEqual(plan.strip_toml_comment('name = "a#b" # note'), 'name = "a#b" ')


if __name__ == "__main__":
    unittest.main()
