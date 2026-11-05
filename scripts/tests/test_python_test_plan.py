"""App-only self-test selection and conservative refusal, without Cargo builds."""
import copy
from contextlib import redirect_stderr
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

# Module import only: never expose an imported TestCase class in this namespace.
import test_ci_plan as existing

SPEC = importlib.util.spec_from_file_location(
    "python_impact_fixture", Path(__file__).resolve().parents[1] / "python_test_plan.py")
impact = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(impact)
SHARDS_SPEC = importlib.util.spec_from_file_location(
    "python_shards_fixture", Path(__file__).resolve().parents[1] / "python_shards.py")
shards = importlib.util.module_from_spec(SHARDS_SPEC)
SHARDS_SPEC.loader.exec_module(shards)
ROOT = Path(__file__).resolve().parents[2]
INVENTORY = [
    {"name": "hex-ui", "manifest": "crates/hex-ui/Cargo.toml", "dependencies": []},
    {"name": "hex-engine", "manifest": "crates/hex-engine/Cargo.toml", "dependencies": []},
]


def selection(paths, profile="merge", modules=None, env=None):
    return impact.select(profile, paths, INVENTORY, {"hex-ui", "hex-widgets"},
                         sorted(impact.REVIEWED_MODULES) if modules is None else modules,
                         env={} if env is None else env)


def cases(suite):
    for item in suite:
        if isinstance(item, unittest.TestSuite):
            yield from cases(item)
        else:
            yield item.id()


class SelectionTests(unittest.TestCase):
    def test_health_owned_tooling_uses_owner_and_shared_checks(self):
        chosen = selection(["scripts/ci_plan.py"], profile="health")
        self.assertEqual(chosen["mode"], "shared")
        self.assertIn("test_ci_plan", chosen["test_selectors"])
        self.assertIn("test_python_test_plan", chosen["test_selectors"])
        self.assertIn("test_architecture", chosen["excluded_groups"])
        self.assertEqual(impact.argv(chosen)[:3], ("python3", "scripts/python_shards.py", "--"))

    def test_health_mixed_and_unknown_inputs_remain_comprehensive(self):
        for paths in (["scripts/ci_plan.py", "crates/hex-engine/src/lib.rs"],
                      ["scripts/ci_plan.py", "Cargo.lock"],
                      ["scripts/unknown-helper.py"]):
            with self.subTest(paths=paths):
                self.assertEqual(selection(paths, profile="health")["mode"], "comprehensive")

    def test_health_native_or_unregistered_inputs_remain_comprehensive(self):
        paths = ["scripts/ci_plan.py"]
        self.assertEqual(selection(paths, profile="health", env={"HEX_VST3_SDK_DIR": "fixture"})["mode"],
                         "comprehensive")
        modules = sorted(impact.REVIEWED_MODULES) + ["test_unknown"]
        self.assertEqual(selection(paths, profile="health", modules=modules)["mode"], "comprehensive")

    def test_shard_discovery_retains_bounded_marker_and_deduplicates(self):
        class Cases(unittest.TestCase):
            def test_plain(self):
                pass
            def test_bounded(self):
                pass
        Cases.test_bounded.__hex_shard_bounded__ = True
        plain, bounded = Cases("test_plain"), Cases("test_bounded")
        suite = unittest.TestSuite([plain, bounded, plain])
        with patch.object(unittest.defaultTestLoader, "loadTestsFromNames", return_value=suite):
            selected = shards.test_cases(["fixture"])
        self.assertEqual(dict(selected), {plain.id(): False, bounded.id(): True})

    def test_shard_scheduler_preserves_fast_parallelism_and_bounds_tagged_cases(self):
        lock = threading.Lock()
        barrier = threading.Barrier(3, timeout=0.5)
        active = {False: 0, True: 0}
        maximum = {False: 0, True: 0}
        events = []
        def run(name):
            bounded = name.startswith("bounded")
            with lock:
                active[bounded] += 1
                maximum[bounded] = max(maximum[bounded], active[bounded])
                events.append(name)
            if not bounded:
                barrier.wait()
            time.sleep(0.01)
            with lock:
                active[bounded] -= 1
            return (name, int(name == "bounded-1"), "result")
        cases = [(f"fast-{i}", False) for i in range(3)] + [(f"bounded-{i}", True) for i in range(5)]
        results = list(shards.run_cases(cases, 3, run))
        self.assertEqual([item[0] for item in results], [name for name, _ in cases])
        self.assertEqual([item[1] for item in results].count(1), 1)
        self.assertEqual(maximum[False], 3)
        self.assertLessEqual(maximum[True], 2)
        self.assertTrue(all(name.startswith("fast") for name in events[:3]))

    def test_shard_bounded_phase_respects_a_single_worker(self):
        active = 0
        maximum = 0
        def run(name):
            nonlocal active, maximum
            active += 1
            maximum = max(maximum, active)
            time.sleep(0.01)
            active -= 1
            return (name, 0, "")
        results = list(shards.run_cases([("a", True), ("b", True)], 1, run))
        self.assertEqual([item[0] for item in results], ["a", "b"])
        self.assertEqual(maximum, 1)

    def test_shard_cli_refuses_empty_or_missing_selection(self):
        output = io.StringIO()
        with patch.object(shards, "test_cases", return_value=[]), redirect_stderr(output):
            self.assertEqual(shards.main(["--", "fixture"]), 1)
        self.assertIn("no tests matched", output.getvalue())
        output = io.StringIO()
        with redirect_stderr(output):
            self.assertEqual(shards.main(["--", "missing_shard_fixture.no_case"]), 1)
        self.assertIn("exit 1", output.getvalue())

    def test_shard_cli_runs_both_groups_and_propagates_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "shard_fixture.py").write_text(
                "import unittest\n"
                "class Cases(unittest.TestCase):\n"
                "    def test_plain(self): pass\n"
                "    def test_bounded(self): self.fail('expected')\n"
                "    test_bounded.__hex_shard_bounded__ = True\n"
            )
            output = io.StringIO()
            with patch.dict(os.environ, {"PYTHONPATH": directory}), patch.object(sys, "path", [directory, *sys.path]):
                with redirect_stderr(output):
                    self.assertEqual(shards.main(["--workers", "3", "--", "shard_fixture"]), 1)
            self.assertIn("shard_fixture.Cases.test_plain (exit 0)", output.getvalue())
            self.assertIn("shard_fixture.Cases.test_bounded (exit 1)", output.getvalue())
            sys.modules.pop("shard_fixture", None)

    def test_ui_and_core_app_edits_have_the_same_reviewed_shared_checks(self):
        ui = selection(["crates/hex-ui/src/editor.rs"])
        core = selection(["crates/hex-engine/src/lib.rs"])
        self.assertEqual(ui["mode"], "shared")
        self.assertEqual(core["mode"], "shared")
        self.assertEqual(ui["test_selectors"], core["test_selectors"])
        self.assertIn("test_local_ci", ui["excluded_groups"])
        self.assertIn("test_launch_restart_compile", ui["excluded_groups"])
        self.assertIn("test_hex_update_entry", ui["excluded_groups"])
        self.assertIn("test_pending_update_exit", ui["excluded_groups"])
        self.assertIn("test_ci_plan:remaining-cases", ui["excluded_groups"])

    def test_current_repository_inventory_retains_scoped_feedback_and_impact_fallbacks(self):
        modules = impact.available_modules(ROOT)
        self.assertIn("test_ui_style_log_viewer", modules)
        self.assertEqual(selection(["scripts/check-ui-style.py"], modules=modules)["mode"], "comprehensive")
        for path in ("crates/hex-ui/src/editor.rs", "crates/hex-engine/src/lib.rs"):
            with self.subTest(path=path):
                chosen = selection([path], modules=modules)
                self.assertEqual(chosen["mode"], "shared")
                self.assertEqual(chosen["test_selectors"], [*impact.SHARED_TESTS, "test_python_test_plan"])
        # Tooling owner boundary: a script with its own test module selects that
        # module plus shared invariants, never unrelated tooling suites.
        for path, owner in (
            ("scripts/check-architecture.py", "test_check_architecture"),
            ("scripts/tests/test_architecture.py", "test_architecture"),
            ("scripts/hex-update-entry.py", "test_hex_update_entry"),
            ("scripts/pending_update_exit.py", "test_pending_update_exit"),
            ("scripts/tests/test_hex_update_entry.py", "test_hex_update_entry"),
            ("scripts/tests/test_pending_update_exit.py", "test_pending_update_exit"),
            ("scripts/tests/test_vst3_macos_bootstrap.py", "test_vst3_macos_bootstrap"),
        ):
            with self.subTest(path=path):
                chosen = selection([path], modules=modules)
                if owner not in modules:  # no module of its own: comprehensive
                    self.assertEqual(chosen["mode"], "comprehensive")
                    continue
                self.assertEqual(chosen["mode"], "shared")
                self.assertIn(owner, chosen["test_selectors"])
                self.assertIn("test_python_test_plan", chosen["test_selectors"])
                self.assertNotIn(owner, chosen["excluded_groups"])
        for path in (
            "AGENTS.md",
            "crates/hex-plugin-scan/native/macos_bootstrap.py",
            "crates/hex-plugin-scan/native/macos_helper.mm",
            "crates/hex-plugin-scan/native/macos_fixture.cpp",
            "crates/hex-plugin-scan/native/build.py", "crates/hex-plugin-scan/build.rs",
            "Cargo.toml", ".cargo/config.toml",
        ):
            with self.subTest(path=path):
                chosen = selection([path], modules=modules)
                self.assertEqual(chosen["mode"], "comprehensive")
                self.assertEqual(chosen["selected_groups"], modules)
        for profile in ("full", "release"):
            chosen = selection(["crates/hex-ui/src/editor.rs"], profile, modules)
            self.assertEqual(chosen["selected_groups"], modules)
            self.assertEqual(impact.argv(chosen), impact.COMPREHENSIVE_ARGV)

    def test_explicit_native_inputs_keep_comprehensive_discovery_and_native_cases(self):
        for key in ("HEX_VST3_MACOS_RECEIPT", "HEX_VST3_DISCOVERY_RECEIPT",
                    "HEX_VST3_PACKAGE_SCANNER", "HEX_VST3_PACKAGE_FIXTURES", "HEX_TEST_SIGNING_IDENTITY",
                    "HEX_VST3_INSTANCE_RECEIPT", "HEX_VST3_INSTANCE_TEST_APP",
                    "HEX_VST3_SDK_DIR", "HEX_VST3_FUTURE_FIXTURE"):
            with self.subTest(key=key):
                chosen = selection(["crates/hex-ui/src/editor.rs"], env={key: "explicit-input"})
                self.assertEqual(chosen["mode"], "comprehensive")
                self.assertEqual(impact.argv(chosen), impact.COMPREHENSIVE_ARGV)
                self.assertIn(key, chosen["reasons"][0])
        # Fresh process: the native suite's opt-in decorator is evaluated at
        # import time. Collect actual cases without executing any fake fixture.
        code = """
import sys, unittest
from pathlib import Path
sys.path[:0] = ['scripts', 'scripts/tests']
import python_test_plan as impact
chosen = impact.select('merge', ['crates/hex-ui/src/lib.rs'],
    [{'name': 'hex-ui', 'manifest': 'crates/hex-ui/Cargo.toml'}],
    {'hex-ui'}, impact.available_modules(Path.cwd()))
assert impact.argv(chosen) == impact.COMPREHENSIVE_ARGV
import test_vst3_macos_bootstrap as native
assert not getattr(native.NativeBootstrapTests, "__unittest_skip__", False)
suite = unittest.defaultTestLoader.loadTestsFromTestCase(native.NativeBootstrapTests)
assert any(case.id().endswith(".test_actual_native_identity_and_effect_instrument_pcm")
           for case in suite)
import test_vst3_instance_worker as instance
assert not getattr(instance.NativeInstanceTests, "__unittest_skip__", False)
assert unittest.defaultTestLoader.loadTestsFromTestCase(instance.NativeInstanceTests).countTestCases() > 0
"""
        result = subprocess.run([sys.executable, "-c", code], cwd=ROOT,
                                env={**os.environ, "HEX_VST3_MACOS_RECEIPT": "collection-only",
                                     "HEX_VST3_INSTANCE_RECEIPT": "collection-only"},
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_native_acceptance_inputs_are_sorted_truthy_names_including_sdk_and_future_inputs(self):
        env = {"HEX_VST3_SDK_DIR": "external-sdk", "HEX_VST3_Z_FUTURE": "fixture",
               "HEX_TEST_SIGNING_IDENTITY": "fixture identity", "HEX_VST3_EMPTY": "",
               "HEX_VST3": "unrelated", "HEX_TEST_SIGNING_IDENTITY_SUFFIX": "unrelated",
               "OTHER": "unrelated"}
        self.assertEqual(impact.native_acceptance_inputs(env), [
            "HEX_TEST_SIGNING_IDENTITY", "HEX_VST3_SDK_DIR", "HEX_VST3_Z_FUTURE"])
        self.assertEqual(impact.native_acceptance_inputs({"HEX_VST3_SDK_DIR": ""}), [])

    def test_tooling_configuration_vendor_and_unknown_inputs_stay_comprehensive(self):
        modules = sorted(impact.REVIEWED_MODULES)
        for path, owner in (
            ("scripts/launch-restart.py", "test_launch_restart"), ("scripts/local-ci.py", "test_local_ci"),
            ("scripts/ci_plan.py", "test_ci_plan"), ("scripts/python_test_plan.py", "test_python_test_plan"),
            ("scripts/hex-test-build.py", "test_hex_test_build"), ("scripts/test_shards.py", "test_ci_plan"),
            ("scripts/tests/test_ci_plan.py", "test_ci_plan"),
        ):
            with self.subTest(path=path):
                result = selection([path], modules=modules)
                self.assertEqual(result["mode"], "shared")
                self.assertIn(owner, result["test_selectors"])
                self.assertIn("test_architecture", result["excluded_groups"])
        for path in (
            "launch", "scripts/build_inputs.py", "scripts/hex-test-build.json", "scripts/bundle-macos.sh",
            "scripts/new-helper.py", "Cargo.toml", "Cargo.lock",
            ".cargo/config.toml", "rust-toolchain.toml", "AGENTS.md",
            "crates/hex-ui/Cargo.toml", "crates/hex-ui/build.rs",
            "crates/hex-ui/assets/not-source.rs", "third_party/gpui/src/lib.rs",
            "assets/fonts/font.data", "changelog/generated.json", "unknown.rs",
        ):
            with self.subTest(path=path):
                result = selection([path])
                self.assertEqual(result["mode"], "comprehensive")
                self.assertEqual(impact.argv(result), next(s for s in existing.ci.FULL_STEPS if s.step_id == "runner-tests").argv)
                self.assertEqual(result["excluded_groups"], [])

    def test_mixed_inputs_and_unregistered_rename_endpoint_cannot_take_fast_path(self):
        # Mixed UI/core Rust inputs share the same invariants as either alone.
        self.assertEqual(selection(["crates/hex-ui/src/a.rs", "crates/hex-engine/src/a.rs"])["mode"], "shared")
        for paths in (
            ["crates/hex-ui/src/a.rs", "launch"],
            ["crates/hex-engine/src/a.rs", "unknown/renamed.rs"],
        ):
            self.assertEqual(selection(paths)["mode"], "comprehensive")

    def test_new_or_missing_test_module_inventory_falls_back(self):
        known = sorted(impact.REVIEWED_MODULES)
        for modules in (known + ["test_new_safety_contract"], known[1:], []):
            self.assertEqual(selection(["crates/hex-ui/src/a.rs"], modules=modules)["mode"],
                             "comprehensive")

    def test_actual_nested_duplicate_basename_is_an_unregistered_module(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tests = root / "scripts/tests"
            tests.mkdir(parents=True)
            for name in impact.REVIEWED_MODULES:
                (tests / (name + ".py")).write_text("")
            self.assertEqual(selection(["crates/hex-ui/src/a.rs"],
                                       modules=impact.available_modules(root))["mode"], "shared")
            nested = tests / "nested"
            nested.mkdir()
            (nested / "__init__.py").write_text("")
            (nested / "test_ci_plan.py").write_text("import unittest\n")
            modules = impact.available_modules(root)
            self.assertIn("test_ci_plan", modules)
            self.assertIn("nested.test_ci_plan", modules)
            self.assertEqual(selection(["crates/hex-ui/src/a.rs"], modules=modules)["mode"],
                             "comprehensive")

    def test_explicit_full_release_and_nonmerge_stay_comprehensive(self):
        for profile in ("full", "release", "edit", "unknown"):
            self.assertEqual(selection(["crates/hex-ui/src/a.rs"], profile)["mode"],
                             "comprehensive")
        self.assertEqual(selection([], "full")["mode"], "comprehensive")
        self.assertEqual(selection([])["mode"], "none")

    def test_selected_collection_is_exactly_approved_methods_and_local_selector_cases(self):
        chosen = selection(["crates/hex-ui/src/a.rs"])
        loaded = list(cases(unittest.defaultTestLoader.loadTestsFromNames(chosen["test_selectors"])))
        expected = set(impact.SHARED_TESTS) | {
            "test_python_test_plan.SelectionTests." + name
            for name in unittest.defaultTestLoader.getTestCaseNames(SelectionTests)
        }
        self.assertEqual(set(loaded), expected)
        self.assertEqual(len(loaded), len(expected))
        self.assertFalse(any(name.startswith("test_local_ci.") for name in loaded))

    def test_empty_unknown_and_missing_named_selection_refuse(self):
        for chosen in ({"mode": "none"}, {"mode": "unclassified"},
                       {"mode": "shared", "test_selectors": []},
                       {"mode": "shared", "test_selectors": [""]}):
            with self.assertRaises(ValueError):
                impact.argv(chosen)
        missing = {"mode": "shared", "test_selectors": [
            "test_ci_plan.ScopeTests.this_case_does_not_exist"]}
        result = subprocess.run((sys.executable, *impact.argv(missing)[1:]), cwd=ROOT,
                                capture_output=True, text=True, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("FAILED", result.stderr)

    def test_selection_changes_only_python_command_not_rust_guards_or_ownership(self):
        for scope in ("ui", "full", "tooling"):
            before = existing.ci.planned_steps({"scope": scope})
            chosen = selection(["crates/hex-ui/src/a.rs"]) if scope == "ui" else selection(["launch"])
            after = existing.ci.planned_steps({"scope": scope, "python_tests": chosen})
            self.assertEqual([s for s in before if s.step_id != "runner-tests"],
                             [s for s in after if s.step_id != "runner-tests"])
            runner = next(s for s in after if s.step_id == "runner-tests")
            self.assertTrue(runner.uses_cargo_target)
            self.assertTrue(runner.requires_tests)
        core = existing.ci.planned_steps({"scope": "full", "python_tests": selection([
            "crates/hex-engine/src/lib.rs"])})
        original = existing.ci.planned_steps({"scope": "full"})
        self.assertEqual([s for s in core if s.step_id != "runner-tests"],
                         [s for s in original if s.step_id != "runner-tests"])
        self.assertNotEqual(next(s.argv for s in core if s.step_id == "runner-tests"),
                            next(s.argv for s in original if s.step_id == "runner-tests"))

    def test_real_git_planner_embeds_python_impact_and_hash_independent_of_rust_scope(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for module in impact.REVIEWED_MODULES:
                path = root / "scripts/tests" / (module + ".py")
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("")
            metadata = {"workspace_members": [], "packages": []}
            for package in INVENTORY:
                manifest = root / package["manifest"]
                manifest.parent.mkdir(parents=True, exist_ok=True)
                manifest.write_text("fixture metadata supplied without invoking Cargo")
                source = manifest.parent / "src/lib.rs"
                source.parent.mkdir();source.write_text("old")
                metadata["workspace_members"].append(package["name"])
                metadata["packages"].append({
                    "id": package["name"], "name": package["name"],
                    "manifest_path": str(manifest), "features": {}, "targets": [], "dependencies": [],
                })
            def git(*args):
                return subprocess.check_output(["git", *args], cwd=root, stderr=subprocess.DEVNULL).decode().strip()
            git("init", "-q");git("config", "user.name", "selection fixture")
            git("config", "user.email", "fixture@invalid")
            git("add", ".");git("commit", "-qm", "base")
            base = git("rev-parse", "HEAD")
            for name, expected_scope in (("hex-ui", "ui"), ("hex-engine", "rust")):
                (root / "crates" / name / "src/lib.rs").write_text("changed")
                git("add", ".");git("commit", "-qm", name)
                source = git("rev-parse", "HEAD")
                with patch.dict(os.environ, dict.fromkeys(impact.NATIVE_ACCEPTANCE_ENV, "")):
                    resolved = existing.plan.resolve(root, profile="merge", base=base,
                                                     source=source, metadata=metadata)
                self.assertEqual(resolved["scope"], expected_scope)
                self.assertEqual(resolved["python_tests"]["mode"], "shared")
                unhashed = copy.deepcopy(resolved)
                certificate = unhashed.pop("plan_sha256")
                self.assertEqual(certificate, hashlib.sha256(
                    json.dumps(unhashed, sort_keys=True).encode()).hexdigest())
                base = source
