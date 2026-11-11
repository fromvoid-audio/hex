"""Scope boundaries, input freshness, and zero-match qualification regressions."""
import importlib.util
import hashlib
import py_compile
import shutil
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
from test_local_ci import install_delivery_source_fixture
from unittest.mock import patch


SCRIPTS = Path(__file__).resolve().parents[1]


def module(name, file):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / file)
    value = importlib.util.module_from_spec(spec)
    sys.modules[name] = value
    if file == "local-ci.py":
        source = (SCRIPTS / file).read_bytes()
        value._DELIVERY_RUNNER_SOURCE_SHA256 = hashlib.sha256(source).hexdigest()
        exec(compile(source, str(SCRIPTS / file), "exec", dont_inherit=True), value.__dict__)
    else:
        spec.loader.exec_module(value)
    return value


plan = module("scope_fixture", "ci_plan.py")
inputs = module("build_inputs_fixture", "build_inputs.py")
ci = module("runner_scope_fixture", "local-ci.py")

METER_BOUNDARY = ["editor::browser::console_drop_ui_test::console_column_paged_narrow_target_uses_painted_anchor_and_rejects_stale_source",
                  "editor::console_track_page::", "editor::output_meter_ui_test::", "editor::refresh_state::",
                  "editor::session_track_reorder_ui_test::console_page_turn_rejects_fold_and_window_changes",
                  "editor::session_track_reorder_ui_test::session_group_band_survives_paging_past_its_parent",
                  "editor::track_clipboard_ui_test::console_fold_hides_cells_navigation_and_invalidates_column_frame",
                  "editor::track_meters::decay::", "editor::track_meters::follower::",
                  "editor::track_meters::main_output::tests::", "editor::track_meters::scale::",
                  "editor::track_meters::selection::", "editor::track_meters::tests::",
                  "output_meter::logic::", "output_meter::tests::"]


WAVEFORM_PAINT_GEOMETRY_CONSUMERS = [
    "editor::audio_event_editor::free_placement_tests::finite_frame_drag_preserves_the_cycle_markers_and_root_waveform",
    "editor::audio_event_editor::tests::moved_and_pasted_waveform_positions_read_event_composed_pcm",
    "editor::audio_event_editor::workflow_tests::cut_publishes_clipboard_on_ack_and_paste_survives_save_reopen_with_original_media",
    "editor::fade_handles::ui_tests::released_two_axis_fade_stays_painted_until_real_worker_ack",
    "editor::fades::musical_tests::transformed_cropped_musical_fades_keep_readouts_reset_adjust_and_projected_curve",
]


GRID_CONSUMERS = [
    *WAVEFORM_PAINT_GEOMETRY_CONSUMERS,
    "editor::browser::ui_test::browser_hover_and_selection_keep_selected_waveform_and_surroundings_stable",
    "editor::overview_waveform::",
    "editor::sampler_parameter::tests::source_hints_and_paint_require_same_view_source_facts_and_context",
    "editor::waveform::", "editor::waveform_columns::", "editor::waveform_geometry::",
    "editor::waveform_loop::", "editor::waveform_source::", "editor::waveform_stretch::",
]

ARRANGEMENT_CONSUMERS = [
    "editor::arrangement_focus::visibility::",
    "editor::arrangement_main_track_ui_test::clips_and_main_sections_share_grid_endpoints_through_fractional_pan_and_resize",
    "editor::arrangement_map::model::",
    "editor::arrangement_map::tests::main_lane_paints_canonical_intervals_at_the_ruler_scale",
    "editor::arrangement_sections::tests::section_has_opaque_chamfered_fill_and_compact_header_across_palettes_and_layouts",
    "editor::arrangement_sections::tests::section_lower_body_paints_all_tracks_without_section_edit_even_when_focused",
    "editor::group_overview::model::",
    "editor::group_overview::tests::",
    "section_surface::geometry::",
]


class PlannerSourceLoaderTests(unittest.TestCase):
    def test_nested_policy_descriptors_close_when_wrapping_fails(self):
        spec = importlib.util.spec_from_file_location("planner_close_probe", SCRIPTS / "ci_plan.py")
        loaded = importlib.util.module_from_spec(spec)
        original_open = os.open
        opened = []

        def track_open(*args, **kwargs):
            descriptor = original_open(*args, **kwargs)
            opened.append(descriptor)
            return descriptor

        with patch.object(os, "open", side_effect=track_open), \
                patch.object(os, "fdopen", side_effect=OSError("fixture wrap failure")):
            with self.assertRaisesRegex(OSError, "fixture wrap failure"):
                spec.loader.exec_module(loaded)
        self.assertEqual(len(opened), 2)
        for descriptor in opened:
            with self.assertRaises(OSError):
                os.fstat(descriptor)

    def test_timestamp_valid_stale_bytecode_never_selects_either_planner(self):
        with tempfile.TemporaryDirectory() as directory:
            scripts = Path(directory) / "scripts"
            scripts.mkdir()
            for name in ("local-ci.py", "ci_plan.py", "python_test_plan.py", "build_inputs.py",
                         "ci_checkpoint.py", "ci_target_storage.py"):
                shutil.copyfile(SCRIPTS / name, scripts / name)

            replacement_version = plan.POLICY_VERSION ^ 1
            replacements = {
                "ci_plan.py": (f"POLICY_VERSION = {plan.POLICY_VERSION}".encode(),
                               f"POLICY_VERSION = {replacement_version}".encode()),
                "python_test_plan.py": (b'"test_changelog", "test_ci_plan"',
                                        b'"test_changemog", "test_ci_plan"'),
            }
            for name, (before, after) in replacements.items():
                path = scripts / name
                original = path.read_bytes()
                self.assertEqual(original.count(before), 1)
                self.assertEqual(len(before), len(after))
                # Timestamp-mode pyc remains valid after a same-size edit with
                # the original mtime restored; the loader must still see bytes.
                py_compile.compile(str(path), doraise=True,
                                   invalidation_mode=py_compile.PycInvalidationMode.TIMESTAMP)
                timestamp = path.stat()
                path.write_bytes(original.replace(before, after, 1))
                os.utime(path, ns=(timestamp.st_atime_ns, timestamp.st_mtime_ns))

            path = scripts / "local-ci.py"
            spec = importlib.util.spec_from_file_location("source_loader_regression", path)
            self.assertIsNotNone(spec)
            loaded = importlib.util.module_from_spec(spec)
            with patch.dict(sys.modules, {spec.name: loaded}):
                spec.loader.exec_module(loaded)
            self.assertEqual(loaded.ci_plan.POLICY_VERSION, replacement_version)
            self.assertIn("test_changemog", loaded.ci_plan.python_tests.REVIEWED_MODULES)
            self.assertNotIn("test_changelog", loaded.ci_plan.python_tests.REVIEWED_MODULES)
            self.assertEqual(loaded._PLAN_SOURCE_SHA256,
                             hashlib.sha256((scripts / "ci_plan.py").read_bytes()).hexdigest())
            self.assertEqual(loaded.ci_plan._PYTHON_SOURCE_SHA256,
                             hashlib.sha256((scripts / "python_test_plan.py").read_bytes()).hexdigest())
            self.assertEqual(Path(loaded.ci_plan.__file__), scripts / "ci_plan.py")
            self.assertEqual(Path(loaded.ci_plan.python_tests.__file__), scripts / "python_test_plan.py")


class ScopeTests(unittest.TestCase):
    def test_path_included_storage_text_selects_its_real_consumers(self):
        inventory = [
            {"name": "hex-storage", "manifest": "crates/hex-storage/Cargo.toml", "features": {},
             "dependencies": [], "targets": [{"kind": ["lib"]}]},
            {"name": "hex-ui", "manifest": "crates/hex-ui/Cargo.toml", "features": {"ui-test-support": [], "ui-unit-tier": []},
             "dependencies": [{"name": "hex-storage"}], "targets": [{"kind": ["lib"]}]},
            {"name": "hex-sandbox", "manifest": "crates/hex-sandbox/Cargo.toml", "features": {"ui-test-support": [], "ui-unit-tier": []},
             "dependencies": [], "targets": [{"kind": ["bin"]}]},
        ]
        path = "crates/hex-storage/src/text.rs"
        _, affected, _ = plan.classify([path], inventory)
        self.assertEqual(affected, ["hex-sandbox", "hex-storage", "hex-ui"])
        self.assertEqual(plan.owner_test_packages([path], inventory, affected), affected)
        self.assertEqual(plan.module_test_filters(SCRIPTS.parent, [path], inventory, affected),
                         {"hex-storage": ["text_tests::"], "hex-ui": ["editor::theme_settings::tests::", "theme::"],
                          "hex-sandbox": ["theme::tests::"]})
        self.assertEqual(plan.module_test_filters(SCRIPTS.parent, ["crates/hex-storage/src/text_tests.rs"],
                                                   inventory, ["hex-storage"]), {"hex-storage": ["text_tests::"]})
        self.assertEqual(plan.module_test_filters(SCRIPTS.parent, ["crates/hex-storage/src/lib.rs"],
                                                   inventory, ["hex-storage"]), {})
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / path
            source.parent.mkdir(parents=True)
            source.write_text("pub fn plain() {}\n")
            # A missing test owner must widen selection, not name a zero-match test.
            self.assertEqual(plan.module_test_filters(root, [path], inventory, ["hex-storage"]), {})
        boundary = plan.boundary_selection(SCRIPTS.parent, [path], inventory, affected)
        self.assertEqual(boundary["test_packages"], affected)
        self.assertEqual(boundary["module_filters"]["hex-sandbox"], ["theme::tests::"])
        steps = ci.boundary_steps({"affected_packages": affected, "cargo_inventory": inventory,
                                   "paths": [path], "boundary": boundary})
        self.assertTrue(any("text_tests::" in step.argv for step in steps))
        self.assertTrue(any("hex-sandbox" in step.argv and "theme::tests::" in step.argv for step in steps))
        self.assertTrue(any("hex-ui" in step.argv and "theme::" in step.argv for step in steps))
        _, other, _ = plan.classify(["crates/hex-storage/src/project_name.rs"], inventory)
        self.assertEqual(other, ["hex-storage", "hex-ui"])
        with self.assertRaisesRegex(ValueError, "path consumers missing"):
            plan.classify([path], inventory[:2])
        with self.assertRaisesRegex(ValueError, "path consumers missing"):
            plan.boundary_selection(SCRIPTS.parent, [path], inventory, ["hex-storage", "hex-ui"])

    def test_all_feature_gated_ui_unit_targets_are_scheduled_once(self):
        targets = set(ci.ci_plan.UI_UNIT_FEATURES)
        health = {"python": False, "workspace_inputs": True, "packages": []}
        for selected in (ci.planned_steps({"scope": "full"}),
                         ci.planned_steps({"scope": "ui"}),
                         ci.planned_steps({"scope": "rust", "health": health})):
            with self.subTest(scope=selected[0].step_id):
                unit_steps = [step for step in selected
                              if "--test" in step.argv and
                              step.argv[step.argv.index("--test") + 1] in targets]
                self.assertEqual(len(unit_steps), len(targets))
                self.assertEqual({step.argv[step.argv.index("--test") + 1]
                                  for step in unit_steps}, targets)
                for step in unit_steps:
                    target = step.argv[step.argv.index("--test") + 1]
                    self.assertEqual(step.argv[step.argv.index("--features") + 1],
                                     ci.ci_plan.UI_UNIT_FEATURES[target])


    def test_ui_library_and_midi_pressure_feature_targets_are_disjoint(self):
        for scope in ("full", "ui"):
            with self.subTest(scope=scope):
                steps = {step.step_id: step for step in ci.planned_steps({"scope": scope})}
                self.assertIn("--lib", steps["ui-feature-tests"].argv)
                self.assertNotIn("--test", steps["ui-feature-tests"].argv)
                self.assertEqual(steps["ui-midi-pressure-boundary-tests"].argv,
                                 ("cargo", "test", "-p", "hex-ui", "--features", "ui-test-support",
                                  "--test", "midi_pressure_boundary", "--locked"))
                self.assertTrue(steps["ui-midi-pressure-boundary-tests"].requires_tests)
                self.assertIn("--doc", steps["ui-feature-doctests"].argv)
                self.assertIn("--example", steps["device-gallery-tests"].argv)
                self.assertNotIn("ui-default-standalone-tests", steps)

        health = {"python": False, "workspace_inputs": False, "packages": ["hex-ui"]}
        steps = {step.step_id: step for step in ci.planned_steps({"scope": "rust", "health": health})}
        self.assertEqual(steps["health-tests"].argv, ("cargo", "test", "-p", "hex-ui", "--locked"))
        self.assertFalse(any("midi_pressure_boundary" in step.argv for step in steps.values()))

    def test_hex_ui_standalone_targets_require_the_reviewed_features(self):
        root = SCRIPTS.parent
        metadata = json.loads(subprocess.check_output(
            ["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"], cwd=root))
        ui = next(package for package in plan.cargo_inventory(metadata, root) if package["name"] == "hex-ui")
        by_feature = {}
        for target in ui["targets"]:
            if target["kind"] == ["test"]:
                features = tuple(target.get("required-features", ()))
                by_feature.setdefault(features, set()).add(target["name"])
        self.assertEqual(by_feature, {
            (): {"unit_editor_state", "track_group_focus", "browser_disclosure_assets",
                 "browser_place_assets", "cursor_assets", "midi_audition_icon", "transport_icon_assets"},
            ("ui-unit-tier",): set(plan.UI_UNIT_FEATURES),
            ("ui-test-support",): {"midi_pressure_boundary"},
        })

    def test_shared_theme_selects_unit_and_path_consumer_configurations(self):
        root = SCRIPTS.parent
        inventory = [
            {"name": "hex-ui", "manifest": "crates/hex-ui/Cargo.toml", "features": {"ui-test-support": [], "ui-unit-tier": []},
             "dependencies": [], "targets": [{"kind": ["lib"]}]},
            {"name": "hex-sandbox", "manifest": "crates/hex-sandbox/Cargo.toml", "features": {"ui-test-support": [], "ui-unit-tier": []},
             "dependencies": [], "targets": [{"kind": ["bin"]}]},
        ]
        for path, sandbox_filter in (("crates/hex-ui/src/theme.rs", "theme::"),
                                     ("crates/hex-ui/src/theme/device.rs", "theme::device::")):
            with self.subTest(path=path):
                _, affected, _ = plan.classify([path], inventory)
                self.assertEqual(affected, ["hex-sandbox", "hex-ui"])
                boundary = plan.boundary_selection(root, [path], inventory, affected)
                self.assertEqual(boundary["test_packages"], affected)
                ui_filters = ["editor::theme_settings::tests::", "theme::"]
                if path.endswith("/theme.rs"):
                    ui_filters.extend((
                        "output_meter::tests::unit_tests::",
                        "output_meter::tests::rendered::output_meter_uses_shared_level_colors_and_click_clear_clipping_caps",
                    ))
                self.assertEqual(boundary["module_filters"], {
                    "hex-ui": sorted(ui_filters),
                    "hex-sandbox": [sandbox_filter]})
                steps = ci.boundary_steps({"affected_packages": affected, "cargo_inventory": inventory,
                                           "paths": [path], "boundary": boundary})
                if path.endswith("/theme.rs"):
                    default_ui = next(step for step in steps if step.step_id == "ui-default-lib-unit-tests")
                    self.assertIn("output_meter::tests::unit_tests::", default_ui.argv)
                    feature_ui = next(step for step in steps if step.step_id == "ui-feature-tests")
                    self.assertIn("output_meter::tests::rendered::output_meter_uses_shared_level_colors_and_click_clear_clipping_caps", feature_ui.argv)
                ui_units = [step for step in steps if "unit_editor_logic" in step.argv]
                self.assertEqual(len(ui_units), 1)
                self.assertTrue(ui_units[0].requires_tests)
                self.assertTrue(any(step.step_id == "sandbox-unit-tests" and sandbox_filter in step.argv
                                    and step.requires_tests for step in steps))
                self.assertTrue(any(step.step_id == "ui-feature-tests" and "editor::theme_settings::tests::" in step.argv
                                    for step in steps))
                planned = ci.planned_steps({"scope": "rust", "affected_packages": affected,
                                            "cargo_inventory": inventory, "paths": [path], "boundary": boundary})
                self.assertTrue(next(step for step in planned if "unit_editor_logic" in step.argv).requires_tests)
                self.assertTrue(next(step for step in planned if step.step_id == "sandbox-unit-tests").requires_tests)
                health = plan.health_selection([path], inventory)
                health_steps = ci.planned_steps({"scope": "rust", "health": health})
                health_unit = next(step for step in health_steps if step.step_id == "health-sandbox-unit")
                self.assertEqual(health_unit.argv[-1], "theme::")
                self.assertTrue(health_unit.requires_tests)
        full = ci.planned_steps({"scope": "full"})
        full_unit = next(step for step in full if step.step_id == "sandbox-unit-tests")
        self.assertEqual(full_unit.argv[-1], "theme::")
        self.assertTrue(full_unit.requires_tests)

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
            install_delivery_source_fixture(root)
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
            install_delivery_source_fixture(root)
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
            for name in ("local-ci.py", "ci_plan.py", "python_test_plan.py", "build_inputs.py",
                         "ci_checkpoint.py", "ci_target_storage.py"):
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
            install_delivery_source_fixture(root)
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
            install_delivery_source_fixture(root)
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
            install_delivery_source_fixture(root)
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
                    "hex-sandbox": {"ui-test-support", "ui-unit-tier"}}
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
            install_delivery_source_fixture(root)
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

    def test_standalone_ui_unit_inputs_run_only_the_owning_targets(self):
        root, inventory = self.live()
        prefix = "crates/hex-ui/tests/"
        tracks = ["editor::tracks::clone_source_window::", "editor::tracks::group_overlap::"]
        cases = (
            (["browser_disclosure_assets.rs"], {"browser_disclosure_assets": []}),
            (["editor_logic_unit/tracks.rs"], {"unit_editor_logic": tracks}),
            (["editor_logic_unit/tracks.rs", "unit_editor_logic.rs"], {"unit_editor_logic": []}),
            (["unit_editor_state.rs", "unit_editor_logic.rs"], {"unit_editor_logic": [], "unit_editor_state": []}),
        )
        for inputs, expected in cases:
            with self.subTest(inputs=inputs):
                paths = [prefix + path for path in inputs]
                boundary = plan.boundary_selection(root, paths, inventory, ["hex-ui"])
                self.assertEqual(boundary["ui_standalone_unit_targets"], expected)
                self.assertEqual(boundary["module_filters"], {})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": ["hex-ui"], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": paths})}
                test_steps = {key: step for key, step in steps.items() if step.argv[:2] == ("cargo", "test")}
                self.assertEqual(set(test_steps), {f"ui-unit-{target}" for target in expected})
                for target, filters in expected.items():
                    step = test_steps[f"ui-unit-{target}"]
                    feature = ("--features", "ui-unit-tier") if target == "unit_editor_logic" else ()
                    self.assertEqual(step.argv, ("cargo", "test", "-p", "hex-ui", *feature,
                                                 "--test", target, "--locked", *(("--", *filters) if filters else ())))
                    self.assertTrue(step.requires_tests)
                self.assertIn("affected-clippy", steps)
                feature_lint = steps["affected-feature-clippy"].argv
                features = feature_lint[feature_lint.index("--features") + 1].split(",")
                self.assertIn("hex-ui/ui-test-support", features)
                self.assertIn("hex-ui/ui-unit-tier", features)

    def test_standalone_ui_unit_selection_preserves_other_routes(self):
        root, inventory = self.live()
        unit = "crates/hex-ui/tests/unit_editor_state.rs"
        prefix = "crates/hex-ui/"
        cases = (
            [unit, prefix + "src/editor/pending_edit.rs"],
            [unit, prefix + "tests/unreviewed_fixture.rs"],
            [unit, prefix + "tests/unreviewed_fixture.py"],
            [unit, prefix + "assets/unreviewed.svg"],
            [unit, prefix + "Cargo.toml"],
            [prefix + "tests/midi_pressure_boundary.rs"],
        )
        for paths in cases:
            with self.subTest(paths=paths):
                boundary = plan.boundary_selection(root, paths, inventory, ["hex-ui"])
                self.assertNotIn("ui_standalone_unit_targets", boundary)
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": ["hex-ui"], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": paths})}
                feature = steps["ui-feature-tests"].argv
                if prefix + "src/editor/pending_edit.rs" in paths:
                    for witness in (
                        "editor::clip_loop::audio::capture_tests::audio_loop_start_edge_keeps_input_and_pending_receipt_separate",
                        "editor::fade_handles::ui_tests::real_refusal_retires_frozen_curve_with_its_canonical_state",
                    ):
                        self.assertIn(witness, feature)
                elif paths == cases[-1]:
                    self.assertIn("editor::midi_recording::", feature)
                    self.assertIn("ui-test-support", steps["ui-consumer-midi_pressure_boundary"].argv)
                else:
                    self.assertNotIn("--", feature)

        # An unexpected Cargo target source or feature cannot narrow the plan.
        for override in ({"src_path": "tests/renamed_editor_state.rs"},
                         {"required-features": ["ui-test-support"]}):
            with self.subTest(override=override):
                wrong_target = [{**package, "targets": [
                    {**target, **override} if target.get("name") == "unit_editor_state" else target
                    for target in package["targets"]]}
                    if package["name"] == "hex-ui" else package for package in inventory]
                boundary = plan.boundary_selection(root, [unit], wrong_target, ["hex-ui"])
                self.assertNotIn("ui_standalone_unit_targets", boundary)

    def test_ui_default_lib_units_keep_scope_and_feature_consumers(self):
        root = SCRIPTS.parent
        path = "crates/hex-ui/src/editor/device_gesture.rs"
        unit = "editor::device_gesture::unit_tests::"
        inventory = [{"name": "hex-ui", "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        names = plan.module_test_filters(root, [path], inventory, ["hex-ui"])
        self.assertEqual(names, {"hex-ui": ["editor::device_gesture::"]})
        for single in ({}, {"hex-ui": "ui-test-support"}):
            for filters in (names, {}):
                boundary = {"kind": "packages", "tests": [], "test_packages": ["hex-ui"],
                            "lint_packages": ["hex-ui"], "single_run": single,
                            "module_filters": filters}
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": ["hex-ui"], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                feature = steps["ui-feature-tests"].argv
                self.assertIn("ui-test-support", feature)
                if filters:
                    self.assertIn("editor::device_gesture::", feature)
                    self.assertIn("--lib", feature)
                default_id = "module-tests-hex-ui" if filters else "affected-tests"
                if single:
                    self.assertTrue(steps["ui-default-lib-unit-tests"].requires_tests)
                    all_units = {prefix for modules in plan.UI_DEFAULT_LIB_UNIT_MODULES.values()
                                 for prefix in modules}
                    expected_units = [unit] if filters else sorted(all_units)
                    self.assertEqual(steps["ui-default-lib-unit-tests"].argv,
                                     ("cargo", "test", "-p", "hex-ui", "--lib", "--locked", "--", *expected_units))
                    self.assertNotIn(default_id, steps)
                else:
                    default = steps[default_id].argv
                    self.assertNotIn("--features", default)
                    if filters:
                        self.assertIn("editor::device_gesture::", default)
                        self.assertIn("--lib", default)
                    self.assertNotIn("ui-default-lib-unit-tests", steps)

        unrelated = {"kind": "packages", "tests": [], "test_packages": ["hex-ui"],
                     "lint_packages": ["hex-ui"], "single_run": {"hex-ui": "ui-test-support"},
                     "module_filters": {"hex-ui": ["editor::recent_projects::"]}}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": ["hex-ui"], "boundary": unrelated,
                                  "cargo_inventory": inventory, "paths": []})
        self.assertFalse(any(step.step_id == "ui-default-lib-unit-tests" for step in steps))

    def test_clip_selection_unit_tier_routes_owner_and_mixed_fixture_consumers(self):
        root = SCRIPTS.parent
        paths = (
            "crates/hex-ui/src/editor/clip_selection.rs",
            "crates/hex-ui/src/editor/clip_selection_unit_tests.rs",
            "crates/hex-ui/src/editor/clip_selection_test_fixture.rs",
        )
        inventory = [{"name": "hex-ui", "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []},
                      "dependencies": [], "targets": [{"kind": ["lib"]}]}]
        native_consumers = (
            "editor::clip_selection::tests::additive_clip_clicks_toggle_members_and_shift_on_other_track_starts_new_anchor",
            "editor::clip_selection::tests::same_primary_membership_change_repaints_retained_audio_and_midi",
            "editor::clip_selection::tests::shift_header_click_selects_five_mixed_clips_and_keeps_reverse_repeated_anchor",
        )
        for path in paths:
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, ["hex-ui"])
                self.assertEqual(boundary["single_run"], {"hex-ui": "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": ["hex-ui"], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                default = steps["ui-default-lib-unit-tests"]
                self.assertTrue(default.requires_tests)
                self.assertEqual(default.argv[default.argv.index("--") + 1:],
                                 ("editor::clip_selection::unit_tests::",))
                feature = steps["ui-feature-tests"].argv
                filters = feature[feature.index("--") + 1:]
                self.assertNotIn("editor::", filters)
                self.assertTrue(any(name.startswith("editor::clip_selection::") for name in filters))
                if path in (paths[0], paths[2]):
                    for consumer in native_consumers:
                        self.assertTrue(any(consumer.startswith(name) for name in filters),
                                        (path, consumer, filters))
        unit_source = (root / paths[1]).read_text()
        fixture_source = (root / paths[2]).read_text()
        self.assertIn("additive_range_on_another_track_preserves_toggle_fallback", unit_source)
        self.assertIn("Command::PlaceMidiClip", fixture_source)
        self.assertIn("Command::PlaceClip", fixture_source)

    def test_default_lib_keeps_multiple_unit_modules_from_one_source(self):
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        parent = "editor::shared::"
        first, second = parent + "first::", parent + "second::"
        relative_source = "src/editor/shared.rs"
        shared = "crates/hex-ui/" + relative_source
        first_source = "crates/hex-ui/src/editor/shared/first.rs"
        modules = {relative_source: (first, second), "src/editor/shared/first.rs": (first,)}
        with tempfile.TemporaryDirectory() as directory, \
                patch.dict(plan.UI_DEFAULT_LIB_UNIT_MODULES, modules, clear=True), \
                patch.dict(ci.ci_plan.UI_DEFAULT_LIB_UNIT_MODULES, modules, clear=True):
            root = Path(directory)
            source = root / "crates/hex-ui/src"
            source.joinpath("editor/shared").mkdir(parents=True)
            source.joinpath("lib.rs").write_text("mod editor;\n")
            source.joinpath("editor.rs").write_text("mod shared;\n")
            (root / shared).write_text(
                plan.UI_DEFAULT_LIB_UNIT_CFG + "\nmod first;\n"
                + plan.UI_DEFAULT_LIB_UNIT_CFG + "\nmod second;\n"
                '#[cfg(all(test, feature = "ui-test-support"))]\nmod rendered;\n')
            for child in ("first", "second", "rendered"):
                source.joinpath(f"editor/shared/{child}.rs").write_text("#[test]\nfn covers_case() {}\n")

            for path, scope, expected in (
                (shared, [parent], (first, second)),
                (first_source, [first], (first,)),
                (shared, [], (first, second)),
            ):
                with self.subTest(path=path, scope=scope):
                    boundary = plan.boundary_selection(root, [path], inventory, [owner])
                    self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                    if scope:
                        self.assertEqual(boundary["module_filters"], {owner: scope})
                    else:
                        boundary["module_filters"] = {}
                    steps = {step.step_id: step for step in ci.planned_steps({
                        "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                        "cargo_inventory": inventory, "paths": [path]})}
                    unit = steps["ui-default-lib-unit-tests"]
                    self.assertTrue(unit.requires_tests)
                    self.assertEqual(unit.argv, ("cargo", "test", "-p", owner, "--lib", "--locked",
                                                 "--", *expected))
                    self.assertEqual(steps["ui-feature-tests"].argv,
                                     ("cargo", "test", "-p", owner, "--lib", "--features",
                                      "ui-test-support", "--locked", *(("--", *scope) if scope else ())))

            source.joinpath("unreviewed.rs").write_text(plan.UI_DEFAULT_LIB_UNIT_CFG + "\nmod tests {}\n")
            self.assertEqual(plan.single_run_packages(root, [owner], inventory), {})

    def test_live_hex_ui_plans_keep_the_test_tier_split(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        def planned_for(path):
            boundary = plan.boundary_selection(root, [path], inventory, [owner])
            self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
            return {step.step_id: step for step in ci.planned_steps({
                "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                "cargo_inventory": inventory, "paths": [path]})}

        scoped = planned_for("crates/hex-ui/src/editor/device_panel/operator_panel/filter_response.rs")
        self.assertNotIn("module-tests-hex-ui", scoped)
        self.assertIn("editor::device_panel::operator_panel::filter_response::tests::",
                      scoped["ui-default-lib-unit-tests"].argv)
        self.assertIn("editor::device_panel::tests::operator_filter_response_tests::filter_curve_keeps_released_parameter_preview_across_completed_repaints",
                      scoped["ui-feature-tests"].argv)

        whole = planned_for("crates/hex-ui/Cargo.toml")
        self.assertNotIn("affected-tests", whole)
        for name in ("ui-default-lib-unit-tests", "ui-default-standalone-tests",
                     "ui-feature-tests", "ui-midi-pressure-boundary-tests", "ui-feature-doctests"):
            self.assertIn(name, whole)
        self.assertTrue(any("unit_editor_logic" in step.argv for step in whole.values()))

    def test_ui_test_cfg_exceptions_keep_unknown_gates_conservative(self):
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            editor = root / "crates/hex-ui/src/editor"
            editor.mkdir(parents=True)
            witness = editor / "clip_boundary.rs"
            witness.write_text(
                '#[cfg(not(any(test, feature = "ui-test-support")))]\nfn production_edge() {}\n')
            fixture = editor / "fixture.rs"
            fixture.write_text(
                '#[cfg(all(test, any(feature = "ui-test-support", feature = "ui-unit-tier")))]\n'
                'fn fixture() {}\n')
            self.assertEqual(plan.single_run_packages(root, [owner], inventory),
                             {owner: "ui-test-support"})
            unknown = editor / "unknown.rs"
            unknown.write_text(witness.read_text())
            self.assertEqual(plan.single_run_packages(root, [owner], inventory), {})
            unknown.unlink()
            fixture.write_text('#[cfg(feature = "ui-unit-tier")]\nfn production() {}\n')
            self.assertEqual(plan.single_run_packages(root, [owner], inventory), {})
            fixture.unlink()
            witness.write_text('#[cfg(not(feature = "ui-test-support"))]\nfn production_edge() {}\n')
            self.assertEqual(plan.single_run_packages(root, [owner], inventory), {})

    def test_ui_default_lib_unit_cfg_exception_is_exact_and_local(self):
        inventory = [{"name": "hex-ui", "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            src = root / "crates/hex-ui/src"
            (src / "editor").mkdir(parents=True)
            recognized = src / "editor/device_gesture.rs"
            unknown = src / "editor/other.rs"
            gate = '#[cfg(all(test, not(feature = "ui-test-support")))]'
            recognized.write_text(gate + "\nmod unit_tests {}\n")
            self.assertEqual(plan.single_run_packages(root, ["hex-ui"], inventory),
                             {"hex-ui": "ui-test-support"})
            unknown.write_text(gate + "\nmod unit_tests {}\n")
            self.assertEqual(plan.single_run_packages(root, ["hex-ui"], inventory), {})
            unknown.unlink()
            recognized.write_text('#[cfg(not(feature = "ui-test-support"))]\nfn runtime() {}\n')
            self.assertEqual(plan.single_run_packages(root, ["hex-ui"], inventory), {})


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
    @unittest.skipUnless(shutil.which("cargo") and shutil.which("git"), "requires real Cargo and Git")
    def test_selector_hidden_build_input_requires_guarded_cache_invalidation(self):
        # A build script can read workspace-root policy without registering it
        # as a Cargo watch. The selector is outside every member directory.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            install_delivery_source_fixture(root)
            (root / "app/src").mkdir(parents=True)
            (root / "Cargo.toml").write_text('[workspace]\nmembers=["app"]\nresolver="2"\n')
            (root / ".gitignore").write_text('/target/\n/artifacts/\n')
            (root / "app/Cargo.toml").write_text(
                '[package]\nname="selector-cache-fixture"\nversion="0.1.0"\nedition="2021"\n')
            (root / "app/build.rs").write_text(
                'fn main() {\n'
                '    println!("cargo:rerun-if-changed=build.rs");\n'
                '    let s = std::fs::read_to_string("../scripts/ci_plan.py").unwrap();\n'
                '    let v = if s.contains("# CACHE_PROBE=B") { "B" } else { "A" };\n'
                '    println!("cargo:rustc-env=SELECTOR_PROBE={v}");\n'
                '}\n')
            (root / "app/src/main.rs").write_text(
                'fn main() { println!("{}", env!("SELECTOR_PROBE")); }\n')
            selector = root / "scripts/ci_plan.py"
            selector.write_bytes(selector.read_bytes() + b"\n# CACHE_PROBE=A\n")
            env = {**os.environ, "CARGO_TARGET_DIR": str(root / "target")}

            def command(*argv):
                return subprocess.check_output(argv, cwd=root, env=env, text=True).strip()

            command("git", "init", "-q")
            command("git", "config", "user.name", "selector cache fixture")
            command("git", "config", "user.email", "fixture@invalid")
            command("cargo", "generate-lockfile", "--offline")
            command("git", "add", ".")
            command("git", "commit", "-qm", "selector A")

            def load_runner_module(name):
                path = root / "scripts/local-ci.py"
                spec = importlib.util.spec_from_file_location(name, path)
                module = importlib.util.module_from_spec(spec)
                sys.modules[name] = module
                self.addCleanup(sys.modules.pop, name, None)
                source = path.read_bytes()
                module._DELIVERY_RUNNER_SOURCE_SHA256 = hashlib.sha256(source).hexdigest()
                exec(compile(source, str(path), "exec", dont_inherit=True), module.__dict__)
                return module

            def runner(module, name):
                return module.LocalCiRunner(
                    root, mode="component", run_id=name, env=env, min_free_bytes=0,
                    steps=(module.Step("build", ("cargo", "build", "--offline", "--locked")),))

            first = runner(load_runner_module("selector_cache_a"), "selector-a")
            self.assertEqual(first.run(), 0, first.summary)
            binary = root / "target/debug/selector-cache-fixture"
            self.assertEqual(command(str(binary)), "A")
            unrelated = root / "target/release/unrelated-artifact"
            unrelated.parent.mkdir(parents=True, exist_ok=True)
            unrelated.write_bytes(b"unrelated release artifact")

            selector.write_bytes(selector.read_bytes().replace(b"# CACHE_PROBE=A", b"# CACHE_PROBE=B"))
            command("git", "add", ".")
            command("git", "commit", "-qm", "selector B")
            candidate = load_runner_module("selector_cache_b")
            control = runner(candidate, "selector-unsafe-control")
            observe = control._dependency_context

            def unsafe_previous_witness():
                # Test-only simulation of excluding the selector from the
                # invalidation and guarded Cargo config token. Retain all real
                # target ownership, compiler, metadata and Cargo behavior.
                observe()
                return first._dependency_inputs

            with patch.object(control, "_dependency_context", side_effect=unsafe_previous_witness):
                self.assertEqual(control.run(), 0, control.summary)
            self.assertEqual(control.summary["target_cache"]["action"], "reuse")
            self.assertIsNone(control.summary["target_cache"]["cleanup_argv"])
            self.assertEqual(command(str(binary)), "A", "unsafe reuse must demonstrate stale output")
            self.assertEqual(unrelated.read_bytes(), b"unrelated release artifact")

            guarded = runner(candidate, "selector-guarded")
            self.assertEqual(guarded.run(), 0, guarded.summary)
            self.assertNotEqual(guarded._dependency_inputs, first._dependency_inputs)
            self.assertEqual(guarded.summary["target_cache"]["action"], "dependency-input-handoff")
            self.assertEqual(guarded.summary["target_cache"]["cleanup_status"], "passed")
            self.assertEqual(command(str(binary)), "B")
            self.assertEqual(unrelated.read_bytes(), b"unrelated release artifact")

    def test_actual_cargo_restored_mtime_dependency_api_is_invalidated(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            install_delivery_source_fixture(root)
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
        pending = "editor::pending_edit::"
        receipt_consumers = [
            "editor::clip_loop::audio::capture_tests::audio_loop_start_edge_keeps_input_and_pending_receipt_separate",
            "editor::fade_handles::ui_tests::real_refusal_retires_frozen_curve_with_its_canonical_state",
        ]
        expected_receipt_filters = sorted([pending, *receipt_consumers])
        self.assertEqual(filters, {owner: expected_receipt_filters})
        boundary = plan.boundary_selection(root, [path], inventory, [owner])
        receipt_steps = {step.step_id: step for step in ci.planned_steps({
            "scope": "rust", "affected_packages": [owner], "boundary": boundary,
            "cargo_inventory": inventory, "paths": [path]})}
        receipt_unit = receipt_steps["ui-unit-unit_editor_state"].argv
        self.assertEqual(receipt_unit[receipt_unit.index("--") + 1:], (pending,))
        receipt_support = receipt_steps["ui-feature-tests"].argv
        self.assertEqual(receipt_support[receipt_support.index("--") + 1:],
                         tuple(expected_receipt_filters))
        viewport = "editor::viewport::"
        timeline = ["editor::musical_time::", "editor::seek::", "editor::signed_frame_viewport::", viewport]
        self.assertEqual(plan.module_test_filters(
            root, ["crates/hex-ui/src/editor/viewport.rs"], inventory, [owner]),
            {owner: sorted([*timeline, *GRID_CONSUMERS, *ARRANGEMENT_CONSUMERS])})
        self.assertEqual(plan.module_test_filters(
            root, ["crates/hex-ui/src/editor/mod.rs"], inventory, [owner]),
            {owner: ["editor::", "output_meter::logic::", "output_meter::tests::",
                     "section_surface::focus::geometry::",
                     "section_surface::focus::tests::theme_hatch_style_produces_bounded_visible_segments",
                     "section_surface::geometry::"]})
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
            {owner: METER_BOUNDARY})
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
                              play_intent, recovery, recovery_integration, *METER_BOUNDARY, *timeline])
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
                    if (name != recovery_integration and name not in timeline[:3]
                            and name not in receipt_consumers
                            and (name not in METER_BOUNDARY or name == scale)):
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
        self.assertIn("--lib", feature.argv)
        self.assertNotIn("--test", feature.argv)
        self.assertTrue(any(step.step_id == "ui-piano-unit-tests" for step in steps))
        self.assertTrue(any(step.step_id == "ui-timeline-unit-tests" for step in steps))
        self.assertTrue(any(step.step_id == "ui-automation-unit-tests" for step in steps))
        self.assertTrue(any(step.step_id == "ui-device-unit-tests" for step in steps))
        self.assertTrue(any(step.step_id == "ui-section-unit-tests" for step in steps))
        self.assertTrue(any(step.step_id == "ui-editor-logic-unit-tests" for step in steps))

    def test_editor_error_unit_target_follows_its_owner(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        names = ["editor::user_errors::"]
        self.assertEqual(plan.module_test_filters(root, ["crates/hex-ui/src/editor/user_errors.rs"], inventory, [owner]),
                         {owner: names})
        self.assertEqual(plan.module_test_filters(root, ["crates/hex-ui/tests/unit_editor_logic.rs"], inventory, [owner]),
                         {owner: ["editor::action_outcome_ui_tests::editor_reports_each_rejection_and_keeps_reader_through_retirement",
                                  "editor::arrangement_focus::visibility::",
                                  "editor::arrangement_main_track_ui_test::clips_and_main_sections_share_grid_endpoints_through_fractional_pan_and_resize",
                                  "editor::arrangement_map::model::",
                                  "editor::arrangement_map::tests::main_lane_paints_canonical_intervals_at_the_ruler_scale",
                                  "editor::arrangement_sections::tests::launch_action_geometry_matches_raw_exclusion_without_enlarging_tiny_sections",
                                  "editor::arrangement_sections::tests::section_autoplay_click_dispatches_stateless_loop_and_toggle_off_cancels",
                                  "editor::arrangement_sections::tests::section_has_opaque_chamfered_fill_and_compact_header_across_palettes_and_layouts",
                                  "editor::arrangement_sections::tests::section_lower_body_paints_all_tracks_without_section_edit_even_when_focused",
                                  "editor::audio_event_editor::free_placement_tests::finite_frame_drag_preserves_the_cycle_markers_and_root_waveform",
                                  "editor::audio_event_editor::paint::tests::replacement_accents_follow_occupied_victims_and_range_overwrite_semantics",
                                  "editor::audio_event_editor::tests::moved_and_pasted_waveform_positions_read_event_composed_pcm",
                                  "editor::audio_event_editor::workflow_tests::cut_publishes_clipboard_on_ack_and_paste_survives_save_reopen_with_original_media",
                                  "editor::audio_event_editor::workflow_tests::event_overlap_accent_covers_only_replaced_audio_and_survives_release",
                                  "editor::audio_import_settings::admission::",
                                  "editor::browser::audition_geometry::",
                                  "editor::browser::console_drop_ui_test::console_column_paged_narrow_target_uses_painted_anchor_and_rejects_stale_source",
                                  "editor::browser::folder_places_ui_test::moved_place_shows_missing_and_relocates_with_icon_order_and_samples",
                                  "editor::browser::geometry::",
                                  "editor::browser::random_rank::",
                                  "editor::browser::samples::state::",
                                  "editor::browser::scan::",
                                  "editor::browser::scrollbar_geometry::",
                                  "editor::browser::tree::state::",
                                  "editor::browser::ui_test::all_search_unions_samples_folder_and_devices_and_preserves_identity_on_refresh",
                                  "editor::browser::ui_test::browser_audio_drag_floats_outside_lanes_locks_preview_and_imports_exact_candidate",
                                  "editor::browser::ui_test::browser_audition_row_observes_pcm_completion_and_stop_without_project_transport",
                                  "editor::browser::ui_test::browser_folder_expansion_preserves_its_position_when_an_earlier_branch_closes",
                                  "editor::browser::ui_test::browser_history_ui_test::browser_history_missing_place_remains_recoverable_and_late_scan_cannot_retarget",
                                  "editor::browser::ui_test::browser_history_ui_test::review_history_waits_for_the_used_catalog_not_an_unrelated_scan",
                                  "editor::browser::ui_test::browser_history_ui_test::review_readiness_obsolete_tree_worker_does_not_hold_saved_root",
                                  "editor::browser::ui_test::browser_hover_and_selection_keep_selected_waveform_and_surroundings_stable",
                                  "editor::browser::ui_test::browser_inline_tree_nested_keyboard_refresh_and_hover_retain_identity",
                                  "editor::browser::ui_test::browser_right_edge_drag_resizes_and_release_stops_resizing",
                                  "editor::browser::ui_test::browser_samples_count_tracks_hierarchy_items_and_flat_sample_matches",
                                  "editor::browser::ui_test::browser_source_transition_discards_late_scan_and_keeps_query_and_keyboard_scope",
                                  "editor::browser::ui_test::browser_tree_refresh_updates_expanded_branch_hidden_by_indexed_query",
                                  "editor::browser::ui_test::browser_tree_same_name_files_keep_distinct_preview_paths_and_bounded_labels",
                                  "editor::browser::ui_test::browser_waveform_click_is_manual_positioned_audition_and_retains_focus",
                                  "editor::browser::ui_test::place_correctness_refresh_retains_identity_only_after_current_success",
                                  "editor::browser::ui_test::place_correctness_reselect_gates_cached_rows_and_old_audition_callbacks",
                                  "editor::browser::ui_test::random_ui_tests::random_button_and_r_reveal_offscreen_audio_and_share_auto_audition",
                                  "editor::browser::ui_test::random_ui_tests::random_pick_obeys_all_search_projection_and_empty_results_leave_playback_unchanged",
                                  "editor::browser::ui_test::samples_catalog_combines_roots_and_active_place_indexed_search",
                                  "editor::browser::ui_test::scrollbar_ui_tests::browser_thumb_recomputes_after_filter_and_resize_and_retires_capture",
                                  "editor::browser::ui_test::scrollbar_ui_tests::browser_thumb_tracks_wheel_and_drag_without_selection_or_audition",
                                  "editor::browser::ui_test::search_selection::toggle_fixture_same_root_retains_actual_catalog_job_after_view_and_caller_drop",
                                  "editor::clip_drag_ui_test::rendered_audio_trim_release_upper_rail_commits_captured_clip_and_actual_worker_ack",
                                  "editor::clip_drag_ui_test::rendered_audio_trim_updates_the_clip_waveform_geometry_and_commits_release_snap",
                                  "editor::clip_drag_ui_test::rendered_midi_trim_previews_note_geometry_and_commits_release_snap",
                                  "editor::clip_drag_ui_test::rendered_move_retains_destination_until_real_worker_ack",
                                  "editor::clip_drag_ui_test::shift_edge_stretch_previews_live_and_stationary_modifier_changes_dispatch_one_edit",
                                  "editor::computer_keyboard::state::", "editor::computer_keyboard::tests::",
                                  "editor::console_track_page::",
                                  "editor::context_menu::tests::global_launch_dropdown_renders_all_choices_and_restores_focus_without_project_edits",
                                  "editor::controller::audio_event_correlation::",
                                  "editor::controller::sampler_native_ui_protocol_tests::",
                                  "editor::controller::session_action_unit_tests::launch_and_transport_actions_have_explicit_play_intents",
                                  "editor::controller::session_action_unit_tests::project_edit_backpressure_preserves_transport_but_invalidates_session_actions",
                                  "editor::controller::session_feedback::",
                                  "editor::controller::source_binding_unit_tests::",
                                  "editor::controller::tests::earlier_session_rejection_is_not_reported_as_consumed",
                                  "editor::delete_ui_tests::",
                                  "editor::device_panel::eq_panel::axes::",
                                  "editor::device_panel::eq_panel::response::",
                                  "editor::device_panel::tests::device_comparison_tests::device_comparison_metadata_pending_callback_then_applied_authoring_is_not_a_new_ack",
                                  "editor::device_panel::tests::eq_ui_tests::graph_layout_tests::eq_axis_labels_are_logarithmic_nonoverlapping_and_keep_the_plot_hit_map",
                                  "editor::drag::geometry::",
                                  "editor::drag::group_drag_ui_test::group_overlap_is_passive_clipped_and_excludes_selected_originals_in_both_modes",
                                  "editor::drag::group_drag_ui_test::review_group_landing_paints_unselected_overwrite_victim",
                                  "editor::drag::group_drag_ui_test::stationary_group_snap_modifier_uses_one_anchor_delta_and_authoritative_release",
                                  "editor::drag::tests::",
                                  "editor::drag_autoscroll::resize_pan_tests::inward_pan_then_supported_stretch_release_commits_once_and_undoes",
                                  "editor::drag_autoscroll::resize_pan_tests::resize_pan_preserves_stationary_snap_and_shift_stretch_preview_release",
                                  "editor::edit_cursor_ui_test::audio_time_move_preview_masks_overlaps_and_restores_when_cancelled",
                                  "editor::edit_cursor_ui_test::midi_time_move_preview_translates_cached_notes_to_the_destination",
                                  "editor::edit_cursor_ui_test::pending_time_move_preview_survives_until_worker_failure_then_restores_source",
                                  "editor::fade_calculation::", "editor::fade_handle_geometry::",
                                  "editor::fade_handles::", "editor::fades::", "editor::group_overview::model::", "editor::group_overview::tests::",
                                  "editor::missing_media::ui_tests::sample_catalog_matches_are_offered_then_relinked_all_at_once",
                                  "editor::output_meter_ui_test::", "editor::overview_waveform::",
                                  "editor::recent_projects::labels::",
                                  "editor::refresh_state::",
                                  "editor::sampler_parameter::tests::source_hints_and_paint_require_same_view_source_facts_and_context",
                                  "editor::section_launch::geometry::",
                                  "editor::session_audio_panel::",
                                  "editor::session_grid::actions::", "editor::session_grid::tests::",
                                  "editor::session_navigation::", "editor::session_navigation_ui_test::",
                                  "editor::session_route_view::",
                                  "editor::session_track_reorder_ui_test::console_page_turn_rejects_fold_and_window_changes",
                                  "editor::session_track_reorder_ui_test::session_group_band_survives_paging_past_its_parent",
                                  "editor::tempo_input::keyboard_tests::focused_project_arrows_adjust_local_draft_then_commit_exact_micro_bpm",
                                  "editor::tempo_input::keyboard_tests::project_steps_preserve_bounds_precision_cancel_invalid_and_composition",
                                  "editor::tempo_input::tests::project_tempo_same_value_explicit_commit_collapses_later_points_and_cursor_is_display_only",
                                  "editor::tempo_input::value::",
                                  "editor::tempo_input_ui_test::project_tempo_blur_commits_valid_text_and_invalid_blur_releases_editor_shortcuts",
                                  "editor::tempo_input_ui_test::project_tempo_cursor_movement_cannot_retarget_an_active_draft",
                                  "editor::tempo_input_ui_test::project_tempo_direct_input_validation_cancel_focus_and_canonical_commit",
                                  "editor::tempo_input_ui_test::project_tempo_stale_revision_and_project_switch_retire_drafts",
                                  "editor::theme_settings::tests::",
                                  "editor::time_move_preview::intervals::tests::",
                                  "editor::time_move_preview::visual_tests::fragment_titles_cuts_crops_and_compact_body_preserve_source_mapping",
                                  "editor::track_clipboard_ui_test::console_fold_hides_cells_navigation_and_invalidates_column_frame",
                                  "editor::track_compact_ui_test::compact_audio_and_midi_trim_edges_keep_title_and_exact_preview_until_cancel",
                                  "editor::track_defaults::",
                                  "editor::track_latency::presentation::",
                                  "editor::track_latency::tests::view_action_mounts_the_indicator_in_actual_headers",
                                  "editor::track_layout_ui_test::empty_project_main_header_width_handle_uses_current_window_limit",
                                  "editor::track_meters::decay::", "editor::track_meters::follower::",
                                  "editor::track_meters::main_output::tests::", "editor::track_meters::scale::",
                                  "editor::track_meters::selection::", "editor::track_meters::tests::",
                                  "editor::track_performance::tests::view_actions_fit_independent_indicators_in_arrangement_and_console_headers",
                                  "editor::tracks::clone_source_window::",
                                  "editor::tracks::group_overlap::",
                                  "editor::tracks::overlap_tests::held_looped_warp_drag_keeps_last_ready_content_in_mixed_copy_group",
                                  "editor::tracks::overlap_tests::held_midi_drag_keeps_actual_note_paint_and_colored_name_below_header",
                                  "editor::tracks::overlap_tests::held_processed_audio_drag_reuses_ready_root_on_lane_and_floating",
                                  "editor::trim::admission::",
                                  "editor::trim::geometry::",
                                  "editor::trim::loop_ui_tests::clip_loop_outer_real_pointer_retains_released_paint_and_one_worker_history_edge",
                                  "editor::trim::midi_stretch_ui_tests::midi_shift_stretch_both_edges_projects_notes_through_release_ack_and_one_undo",
                                  "editor::trim::preview_state::",
                                  "editor::trim::resize_release_tests::midi_resize_keeps_released_geometry_until_actual_worker_ack_and_one_history_step",
                                  "editor::trim::resize_release_tests::normal_resize_preserves_visible_extent_and_source_through_release_and_ack",
                                  "editor::trim::resize_release_tests::resize_escape_stale_generation_pending_refusal_and_no_motion_do_not_retain_a_commit",
                                  "editor::trim::resize_release_tests::shift_resize_preserves_visible_extent_through_release_and_real_worker_ack",
                                  "editor::trim::resize_terminal_tests::restart_offline_pending_work_blocks_actual_resize_capture_until_completion",
                                  "editor::trim::stretch_limit_tests::",
                                  "editor::trim::stretch_preview::resolver::",
                                  *names,
                                  "editor::waveform::", "editor::waveform_columns::", "editor::waveform_geometry::", "editor::waveform_loop::",
                                  "editor::waveform_source::", "editor::waveform_stretch::",
                                  "log_viewer::copy_tests::copy_all_uses_frozen_filters_and_real_clipboard_and_read_failures_keep_previous_copy",
                                  "log_viewer::copy_tests::filtered_excerpt_reports_empty_limits_and_failed_clipboard_confirmation",
                                  "log_viewer::filter::",
                                  "log_viewer::presentation::",
                                  "log_viewer::scrollbar::geometry::",
                                  "log_viewer::scrollbar::tests::native_scrollbar_tracks_wheel_outside_release_escape_and_preserves_selection_copy",
                                  "log_viewer::scrollbar::tests::page_refresh_resize_and_deactivation_retire_old_scroll_captures",
                                  "log_viewer::tests::long_log_rows_stay_within_their_visual_bounds",
                                  "log_viewer::tests::rapid_filter_changes_install_only_the_latest_result",
                                  "log_viewer::tests::severity_badges_keep_rows_readable_and_copy_original_records",
                                  "output_meter::logic::", "output_meter::tests::", "section_surface::geometry::",
                                  "theme::"]})
        meter_consumer = "editor::track_meters::tests::steady_state_meter_ticks_reuse_the_staging_buffers"
        self.assertTrue(any(meter_consumer.startswith(selected) for selected in
                            plan.module_test_filters(root, ["crates/hex-ui/tests/unit_editor_logic.rs"],
                                                     inventory, [owner])[owner]))
        self.assertEqual(plan.module_test_filters(root, ["crates/hex-ui/tests/unit_session.rs"], inventory, [owner]), {})
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner], "lint_packages": [owner],
                    "single_run": {owner: "ui-test-support"}, "module_filters": {owner: names}}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": []})
        self.assertEqual(sum("unit_editor_logic" in step.argv for step in steps), 1)
        self.assertTrue(any(names[0] in step.argv for step in steps if "unit_editor_logic" in step.argv))
        self.assertTrue(any(step.step_id == "ui-editor-logic-unit-tests" for step in ci.planned_steps({"scope": "full"})))
        health = ci.planned_steps({"scope": "rust", "health": {"python": False, "workspace_inputs": True, "packages": []}})
        self.assertTrue(any(step.step_id == "health-ui-editor-logic-unit" for step in health))

    def test_group_overview_projection_keeps_native_and_project_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::group_overview::model::"
        native = "editor::group_overview::tests::"
        names = plan.module_test_filters(root, ["crates/hex-ui/src/editor/group_overview/model.rs"], inventory, [owner])
        self.assertEqual(names, {owner: sorted(ARRANGEMENT_CONSUMERS)})
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                    "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                    "module_filters": names}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": []})
        self.assertTrue(any(unit in step.argv and step.requires_tests
                            for step in steps if "unit_editor_logic" in step.argv))
        self.assertTrue(any(native in step.argv
                            for step in steps if step.step_id == "ui-feature-tests"))
        self.assertTrue(any("track_group_focus" in step.argv and step.requires_tests
                            for step in steps if step.step_id == "ui-consumer-track_group_focus"))
        for path in ("group_overview.rs", "arrangement_map/model.rs", "arrangement_focus/visibility.rs"):
            selected = plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + path], inventory, [owner])
            self.assertTrue(any(unit.startswith(name) for name in selected[owner]))

    def test_arrangement_map_projection_keeps_real_focus_and_render_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::arrangement_map::model::"
        required = [unit, "editor::arrangement_focus::visibility::", "editor::group_overview::tests::",
                    "editor::arrangement_map::tests::main_lane_paints_canonical_intervals_at_the_ruler_scale",
                    "editor::arrangement_sections::tests::section_lower_body_paints_all_tracks_without_section_edit_even_when_focused"]
        for path in ("arrangement_map/model.rs", "arrangement_focus/interval.rs", "arrangement_focus/visibility.rs"):
            names = plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + path], inventory, [owner])
            for name in required:
                self.assertTrue(any(name.startswith(selected) for selected in names[owner]))
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                        "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                        "module_filters": names}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                      "cargo_inventory": inventory, "paths": []})
            self.assertTrue(any(unit in step.argv and step.requires_tests
                                for step in steps if "unit_editor_logic" in step.argv))
            self.assertTrue(any(required[3] in step.argv and required[4] in step.argv
                                for step in steps if step.step_id == "ui-feature-tests"))
            self.assertTrue(any("track_group_focus" in step.argv and step.requires_tests
                                for step in steps if step.step_id == "ui-consumer-track_group_focus"))
        for path in ("arrangement_map.rs", "group_overview.rs", "arrangement_sections.rs", "viewport.rs"):
            names = plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + path], inventory, [owner])
            self.assertTrue(any(unit.startswith(name) for name in names[owner]))
        mixed = plan.module_test_filters(root, ["crates/hex-ui/src/editor/arrangement_focus/interval.rs",
                                                "crates/hex-ui/tests/track_group_focus.rs"], inventory, [owner])
        self.assertIn("editor::arrangement_focus::", mixed[owner])
        mixed_boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                          "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                          "module_filters": mixed}
        mixed_steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                        "boundary": mixed_boundary, "cargo_inventory": inventory, "paths": []})
        consumers = [step for step in mixed_steps if step.step_id == "ui-consumer-track_group_focus"]
        self.assertEqual(len(consumers), 1)
        self.assertEqual(consumers[0].argv, ("cargo", "test", "-p", "hex-ui", "--test", "track_group_focus", "--locked"))
        self.assertTrue(consumers[0].requires_tests)
        unrelated = {"kind": "packages", "tests": [], "test_packages": [owner],
                     "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                     "module_filters": {owner: ["editor::user_errors::"]}}
        unrelated_steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                            "boundary": unrelated, "cargo_inventory": inventory, "paths": []})
        self.assertFalse(any(step.step_id == "ui-consumer-track_group_focus" for step in unrelated_steps))

    def test_track_tag_palette_unit_keeps_rendered_tag_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::track_tags::tests::normalized_tags_use_only_the_eight_musical_element_colors"
        consumers = (
            "editor::track_tags::tests::tag_dots_are_compact_equal_and_shared_by_arrangement_and_console",
            "editor::track_tags::tests::rendered_tag_selection_modifiers_overflow_and_filter_are_view_only",
        )
        for path in ("crates/hex-ui/src/editor/track_tags.rs", "crates/hex-ui/src/editor/track_tags/tests.rs"):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                default = steps["ui-default-lib-unit-tests"]
                self.assertTrue(default.requires_tests)
                self.assertIn(unit, default.argv)
                feature = steps["ui-feature-tests"].argv
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in feature))
                self.assertNotIn("editor::", feature)

    def test_track_group_focus_source_consumers_and_full_run_deduplication(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        rendered_consumers = {
            "arrangement_focus/visibility.rs": (
                "editor::arrangement_focus::visibility::",
                "editor::arrangement_focus::tests::section_focus_renders_visible_child_with_nested_group_headers",
                "editor::track_tags::tests::rendered_tag_selection_modifiers_overflow_and_filter_are_view_only",
                "editor::track_tags::tests::folded_matches_stay_selected_and_filter_applies_to_console_headers",
                "editor::arrangement_sections::tests::section_lower_body_paints_all_tracks_without_section_edit_even_when_focused",
            ),
            "track_hierarchy_indent.rs": (
                "editor::track_hierarchy_indent::",
                "editor::track_clipboard_ui_test::nested_group_plates_and_poles_join_follow_selection_and_close_at_visible_ends",
                "editor::track_clipboard_ui_test::group_track_header_menu_arrangement",
                "editor::track_clipboard_ui_test::group_track_header_menu_session",
            ),
        }
        for path in (*rendered_consumers, "track_reorder/subtree.rs"):
            names = plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + path], inventory, [owner])
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                        "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                        "module_filters": names}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                      "boundary": boundary, "cargo_inventory": inventory, "paths": []})
            self.assertEqual(sum(step.step_id == "ui-consumer-track_group_focus" for step in steps), 1)
            if path in rendered_consumers:
                feature = next(step for step in steps if step.step_id == "ui-feature-tests")
                self.assertNotIn("editor::", feature.argv)
                for name in rendered_consumers[path]:
                    self.assertIn(name, feature.argv)
        harness = "crates/hex-ui/tests/track_group_focus.rs"
        target_only = plan.module_test_filters(root, [harness], inventory, [owner])
        self.assertIn("editor::arrangement_focus::", target_only[owner])
        target_boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                           "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                           "module_filters": target_only}
        target_steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                        "boundary": target_boundary, "cargo_inventory": inventory, "paths": []})
        self.assertEqual(sum(step.step_id == "ui-consumer-track_group_focus" for step in target_steps), 1)
        unknown = plan.module_test_filters(root, [harness, "crates/hex-ui/unknown-input.txt"], inventory, [owner])
        self.assertNotIn(owner, unknown)
        full = {"kind": "packages", "tests": [], "test_packages": [owner],
                "lint_packages": [owner], "single_run": {owner: "ui-test-support"}, "module_filters": {}}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                  "boundary": full, "cargo_inventory": inventory, "paths": []})
        self.assertFalse(any(step.step_id == "ui-consumer-track_group_focus" for step in steps))
        other = {"name": "hex-session", "manifest": "crates/hex-session/Cargo.toml",
                 "features": {}, "dependencies": [], "targets": [{"kind": ["lib"]}]}
        consumer_only = {"kind": "packages", "tests": [], "test_packages": ["hex-session"],
                         "lint_packages": [owner, "hex-session"], "single_run": {},
                         "module_filters": {owner: ["editor::arrangement_focus::"]}}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner, "hex-session"],
                                  "boundary": consumer_only, "cargo_inventory": [*inventory, other], "paths": []})
        self.assertFalse(any(step.step_id == "ui-consumer-track_group_focus" for step in steps))

    def test_midi_pressure_boundary_source_target_feature_and_full_run_selection(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        target = "midi_pressure_boundary"
        module = "editor::midi_recording::"
        fixture = "editor::midi_recording::pressure_fixture"
        paths = [
            ("crates/hex-ui/src/editor/midi_recording_pressure_fixture.rs", fixture),
            ("crates/hex-ui/src/editor/midi_recording.rs", "editor::midi_recording"),
        ]
        modules = plan.module_paths(root / "crates/hex-ui/src")
        for path, expected in paths:
            self.assertEqual(modules[(root / path).resolve()], expected)
            names = plan.module_test_filters(root, [path], inventory, [owner])[owner]
            self.assertTrue(any(module.startswith(selected) for selected in names), names)
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                        "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                        "module_filters": {owner: names}}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                      "boundary": boundary, "cargo_inventory": inventory,
                                      "paths": [path]})
            feature_suite = next(step for step in steps if step.step_id == "ui-feature-tests")
            self.assertIn("--lib", feature_suite.argv)
            self.assertTrue(any(module.startswith(selected) for selected in feature_suite.argv))
            consumer = [step for step in steps if step.step_id == "ui-consumer-" + target]
            self.assertEqual(len(consumer), 1)
            self.assertEqual(sum(target in step.argv for step in steps), 1)
            self.assertTrue(consumer[0].requires_tests)
            self.assertEqual(consumer[0].argv,
                             ("cargo", "test", "-p", owner, "--features", "ui-test-support",
                              "--test", target, "--locked"))

        harness = "crates/hex-ui/tests/midi_pressure_boundary.rs"
        target_change = plan.module_test_filters(root, [harness], inventory, [owner])
        self.assertEqual(target_change, {owner: [module]})
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                    "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                    "module_filters": target_change}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                  "boundary": boundary, "cargo_inventory": inventory,
                                  "paths": [harness]})
        self.assertEqual(sum(step.step_id == "ui-consumer-" + target for step in steps), 1)

        unrelated = plan.module_test_filters(
            root, ["crates/hex-ui/src/editor/user_errors.rs"], inventory, [owner])
        boundary["module_filters"] = unrelated
        unrelated_steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                            "boundary": boundary, "cargo_inventory": inventory,
                                            "paths": ["crates/hex-ui/src/editor/user_errors.rs"]})
        self.assertFalse(any(target in step.argv for step in unrelated_steps))

        full = {"kind": "packages", "tests": [], "test_packages": [owner],
                "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                "module_filters": {}}
        full_steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                      "boundary": full, "cargo_inventory": inventory,
                                      "paths": []})
        self.assertFalse(any(step.step_id == "ui-consumer-" + target for step in full_steps))
        full_suite = next(step for step in full_steps if step.step_id == "ui-feature-tests")
        self.assertIn("--lib", full_suite.argv)
        self.assertNotIn("--test", full_suite.argv)
        self.assertEqual(sum(target in step.argv for step in full_steps), 1)
        standalone = next(step for step in full_steps if step.step_id == "ui-default-standalone-tests")
        self.assertEqual({standalone.argv[i + 1] for i, arg in enumerate(standalone.argv) if arg == "--test"},
                         {"unit_editor_state", "track_group_focus", "browser_disclosure_assets",
                          "browser_place_assets", "cursor_assets", "midi_audition_icon", "transport_icon_assets"})
        self.assertNotIn("--features", standalone.argv)
        self.assertTrue(any(step.step_id == "single-run-doctests" for step in full_steps))
        self.assertTrue(any(step.step_id == "ui-feature-doctests" for step in full_steps))

        full["single_run"] = {}
        steps = {step.step_id: step for step in ci.planned_steps({
            "scope": "rust", "affected_packages": [owner], "boundary": full,
            "cargo_inventory": inventory, "paths": []})}
        self.assertEqual(steps["affected-tests"].argv, ("cargo", "test", "-p", owner, "--locked"))
        self.assertNotIn("ui-default-standalone-tests", steps)
        self.assertEqual(sum(target in step.argv for step in steps.values()), 1)

    def test_browser_history_units_keep_source_and_navigation_witnesses(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        consumers = (
            "editor::browser::ui_test::browser_history_ui_test::browser_history_typing_coalesces_until_navigation_and_branch_clears_forward",
            "editor::browser::ui_test::browser_history_ui_test::browser_history_toolbar_restores_source_without_preview_or_import",
            "editor::browser::ui_test::browser_history_ui_test::browser_history_removed_or_hidden_source_fallback_does_not_push_or_clear_forward",
        )
        for path in (
            "crates/hex-ui/src/editor/browser/history.rs",
            "crates/hex-ui/src/editor/browser_history_ui_test.rs",
        ):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                unit = steps["ui-default-lib-unit-tests"]
                self.assertIn("editor::browser::history::tests::", unit.argv)
                self.assertTrue(unit.requires_tests)
                feature = steps["ui-feature-tests"].argv
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in feature), (consumer, feature))
                self.assertNotIn("editor::browser::", feature)

    def test_navigation_metrics_unit_keeps_profile_and_rendered_resize_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        source = "crates/hex-ui/src/editor/navigation_metrics.rs"
        unit = "editor::navigation_metrics::tests::"
        profile = "editor::clip_content_cache::profile::navigation::headless_navigation_framework_profile"
        resize = "editor::waveform::resize_tests::musical_left_prefix_keeps_waveform_and_geometry_through_real_worker_ack"
        boundary = plan.boundary_selection(root, [source], inventory, [owner])
        self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
        filters = boundary["module_filters"]
        self.assertEqual(filters, {owner: sorted(["editor::navigation_metrics::", profile, resize])})
        steps = {step.step_id: step for step in ci.planned_steps({
            "scope": "rust", "affected_packages": [owner], "boundary": boundary,
            "cargo_inventory": inventory, "paths": [source]})}
        unit_step = steps["ui-default-lib-unit-tests"]
        self.assertTrue(unit_step.requires_tests)
        self.assertEqual(unit_step.argv,
                         ("cargo", "test", "-p", owner, "--lib", "--locked", "--", unit))
        self.assertNotIn("--features", unit_step.argv)
        feature = steps["ui-feature-tests"]
        self.assertFalse(feature.requires_tests)  # Required default units paired with support selection.
        self.assertIn("ui-test-support", feature.argv)
        for consumer in (profile, resize):
            self.assertIn(consumer, feature.argv)
        self.assertNotIn("--ignored", feature.argv)
        self.assertNotIn("module-tests-hex-ui", steps)

    def test_audio_unavailable_unit_route_keeps_rendered_status_consumer(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        source = "crates/hex-ui/src/editor/audio_unavailable.rs"
        unit = "editor::audio_unavailable::tests::"
        consumer = ("editor::audio_unavailable::ui_tests::"
                    "rate_mismatch_event_shows_the_recoverable_paused_status")
        boundary = plan.boundary_selection(root, [source], inventory, [owner])
        self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
        self.assertEqual(boundary["module_filters"], {owner: ["editor::audio_unavailable::"]})
        steps = {step.step_id: step for step in ci.planned_steps({
            "scope": "rust", "affected_packages": [owner], "boundary": boundary,
            "cargo_inventory": inventory, "paths": [source]})}
        default = steps["ui-default-lib-unit-tests"]
        self.assertTrue(default.requires_tests)
        self.assertIn(unit, default.argv)
        self.assertNotIn("--features", default.argv)
        feature = steps["ui-feature-tests"].argv
        self.assertTrue(any(consumer.startswith(prefix) for prefix in feature))
        self.assertNotIn("editor::", feature)

    def test_display_error_unit_route_keeps_batch_failure_window_consumer(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        source = "crates/hex-ui/src/editor/display_error.rs"
        unit = "editor::display_error::tests::"
        consumer = (
            "editor::controller::project_replacement_tests::"
            "failed_batch_worker_event_displays_original_path_in_status_and_tooltip"
        )
        boundary = plan.boundary_selection(root, [source], inventory, [owner])
        self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
        self.assertEqual(boundary["module_filters"],
                         {owner: sorted(["editor::display_error::", consumer])})
        steps = {step.step_id: step for step in ci.planned_steps({
            "scope": "rust", "affected_packages": [owner], "boundary": boundary,
            "cargo_inventory": inventory, "paths": [source]})}
        default = steps["ui-default-lib-unit-tests"]
        self.assertTrue(default.requires_tests)
        self.assertIn(unit, default.argv)
        self.assertNotIn("--features", default.argv)
        feature = steps["ui-feature-tests"]
        self.assertIn("ui-test-support", feature.argv)
        self.assertIn(consumer, feature.argv)

    def test_popup_clamping_unit_keeps_rendered_context_menu_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "context_menu::tests::popup_stays_inside_window_edges"
        consumers = (
            "editor::context_menu::tests::global_launch_dropdown_renders_all_choices_and_restores_focus_without_project_edits",
            "editor::context_menu::tests::menu_escape_outside_click_and_enter_do_not_leak_to_editor",
        )
        for source in ("crates/hex-ui/src/context_menu.rs",
                       "crates/hex-ui/src/editor/context_menu.rs"):
            with self.subTest(source=source):
                boundary = plan.boundary_selection(root, [source], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                filters = boundary["module_filters"][owner]
                self.assertTrue(any(unit.startswith(prefix) for prefix in filters), filters)
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [source]})}
                default = steps["ui-default-lib-unit-tests"]
                self.assertTrue(default.requires_tests)
                self.assertTrue(any(unit.startswith(selected) for selected in default.argv))
                self.assertNotIn("--features", default.argv)
                feature = steps["ui-feature-tests"].argv
                self.assertIn("ui-test-support", feature)
                support = feature[feature.index("--") + 1:]
                self.assertNotIn("editor::", support)
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in support), (consumer, support))

    def test_browser_physical_wheel_delta_keeps_arrangement_and_scrollbar_witnesses(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::browser::unit_tests::"
        consumers = (
            "editor::browser::ui_test::arrangement_wheel_zoom_is_pointer_anchored_and_does_not_escape_its_timeline",
            "editor::browser::ui_test::scrollbar_ui_tests::browser_thumb_tracks_wheel_and_drag_without_selection_or_audition",
        )
        for path in (
            "crates/hex-ui/src/editor/browser.rs",
            "crates/hex-ui/src/editor/browser_ui_test.rs",
            "crates/hex-ui/src/editor/browser_scrollbar_ui_test.rs",
        ):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                default = steps["ui-default-lib-unit-tests"]
                self.assertTrue(default.requires_tests)
                self.assertIn(unit, default.argv)
                feature = steps["ui-feature-tests"].argv
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in feature))
                self.assertNotIn("editor::", feature)

    def test_zoom_fit_interval_unit_keeps_rendered_fit_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::zoom_fit::unit_tests::"
        consumers = (
            "editor::zoom_fit::tests::arrange_keys_and_buttons_fit_reversed_range_clip_content_and_restore_exact_prior",
            "editor::zoom_fit::tests::midi_ruler_selection_fit_back_geometry_ack_and_document_reset_are_view_only",
        )
        for path in (
            "crates/hex-ui/src/editor/zoom_fit.rs",
            "crates/hex-ui/src/editor/zoom_fit_ui_tests.rs",
        ):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                default = steps["ui-default-lib-unit-tests"]
                self.assertTrue(default.requires_tests)
                self.assertIn(unit, default.argv)
                feature = steps["ui-feature-tests"].argv
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in feature))
                self.assertNotIn("editor::", feature)

    def test_clip_content_element_identity_keeps_rendered_focus_and_repaint(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::clip_content_cache::unit_tests::"
        witness = "editor::clip_content_cache::tests::deep_element_id_paths_keep_distinct_click_state_and_focus_across_repaints"
        for name in ("clip_content_cache.rs", "clip_content_cache_ui_test.rs"):
            with self.subTest(name=name):
                path = "crates/hex-ui/src/editor/" + name
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                default = steps["ui-default-lib-unit-tests"]
                self.assertTrue(default.requires_tests)
                self.assertIn(unit, default.argv)
                feature = steps["ui-feature-tests"].argv
                self.assertTrue(any(witness.startswith(prefix) for prefix in feature))
                self.assertNotIn("editor::", feature)

    def test_browser_sample_state_keeps_catalog_worker_and_window_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::browser::samples::state::"
        consumers = ["editor::browser::ui_test::samples_catalog_combines_roots_and_active_place_indexed_search",
                     "editor::browser::ui_test::all_search_unions_samples_folder_and_devices_and_preserves_identity_on_refresh",
                     "editor::browser::ui_test::browser_samples_count_tracks_hierarchy_items_and_flat_sample_matches",
                     "editor::browser::ui_test::browser_history_ui_test::review_history_waits_for_the_used_catalog_not_an_unrelated_scan",
                     "editor::browser::folder_places_ui_test::moved_place_shows_missing_and_relocates_with_icon_order_and_samples",
                     "editor::missing_media::ui_tests::sample_catalog_matches_are_offered_then_relinked_all_at_once",
                     "editor::browser::ui_test::search_selection::toggle_fixture_same_root_retains_actual_catalog_job_after_view_and_caller_drop"]
        names = plan.module_test_filters(root, ["crates/hex-ui/src/editor/browser/samples/state.rs"], inventory, [owner])
        self.assertEqual(names, {owner: sorted([unit, *consumers])})
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                    "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                    "module_filters": names}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": []})
        self.assertTrue(any(unit in step.argv and step.requires_tests
                            for step in steps if "unit_editor_logic" in step.argv))
        self.assertTrue(any(all(name in step.argv for name in consumers)
                            for step in steps if step.step_id == "ui-feature-tests"))
        for path in ("browser/samples.rs", "browser_ui_test.rs", "browser.rs",
                     "browser_history_ui_test.rs", "browser/folder_places_ui_test.rs", "missing_media/ui_tests.rs"):
            selected = plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + path], inventory, [owner])
            self.assertTrue(any(unit.startswith(name) for name in selected[owner]))

    def test_browser_geometry_keeps_rendered_resize_and_drag_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        source = "crates/hex-ui/src/editor/browser/geometry.rs"
        unit = "editor::browser::geometry::"
        consumers = [
            "editor::browser::ui_test::browser_right_edge_drag_resizes_and_release_stops_resizing",
            "editor::browser::ui_test::browser_audio_drag_floats_outside_lanes_locks_preview_and_imports_exact_candidate",
            "editor::track_layout_ui_test::empty_project_main_header_width_handle_uses_current_window_limit",
        ]
        names = plan.module_test_filters(root, [source], inventory, [owner])
        self.assertEqual(names, {owner: sorted([unit, *consumers])})
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                    "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                    "module_filters": names}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": [source]})
        self.assertTrue(any(unit in step.argv and step.requires_tests
                            for step in steps if "unit_editor_logic" in step.argv))
        self.assertTrue(any(all(name in step.argv for name in consumers)
                            for step in steps if step.step_id == "ui-feature-tests"))
        for path in ("browser.rs", "browser_ui_test.rs", "track_layout_ui_test.rs"):
            selected = plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + path], inventory, [owner])
            self.assertTrue(any(unit.startswith(name) for name in selected[owner]))

    def test_browser_scan_keeps_supersession_and_path_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::browser::scan::"
        tree_unit = "editor::browser::tree::state::"
        consumers = [
            "editor::browser::ui_test::browser_source_transition_discards_late_scan_and_keeps_query_and_keyboard_scope",
            "editor::browser::ui_test::place_correctness_reselect_gates_cached_rows_and_old_audition_callbacks",
            "editor::browser::ui_test::place_correctness_refresh_retains_identity_only_after_current_success",
            "editor::browser::ui_test::browser_inline_tree_nested_keyboard_refresh_and_hover_retain_identity",
            "editor::browser::ui_test::browser_tree_refresh_updates_expanded_branch_hidden_by_indexed_query",
            "editor::browser::ui_test::browser_tree_same_name_files_keep_distinct_preview_paths_and_bounded_labels",
            "editor::browser::ui_test::browser_folder_expansion_preserves_its_position_when_an_earlier_branch_closes",
            "editor::browser::ui_test::browser_history_ui_test::browser_history_missing_place_remains_recoverable_and_late_scan_cannot_retarget",
            "editor::browser::ui_test::browser_history_ui_test::review_readiness_obsolete_tree_worker_does_not_hold_saved_root",
        ]
        paths = [
            ("crates/hex-ui/src/editor/browser/scan.rs", "editor::browser::scan"),
            ("crates/hex-ui/src/editor/browser.rs", "editor::browser"),
            ("crates/hex-ui/src/editor/browser_ui_test.rs", "editor::browser::ui_test"),
            ("crates/hex-ui/src/editor/browser_history_ui_test.rs",
             "editor::browser::ui_test::browser_history_ui_test"),
        ]
        modules = plan.module_paths(root / "crates/hex-ui/src")
        for path, module in paths:
            self.assertEqual(modules[(root / path).resolve()], module)
            names = plan.module_test_filters(root, [path], inventory, [owner])[owner]
            if path == paths[0][0]:
                self.assertEqual(names, sorted([unit, tree_unit, *consumers]))
            if path == paths[1][0]:
                self.assertIn("editor::browser::", names)
            self.assertTrue(any(unit.startswith(selected) for selected in names),
                            f"{path} does not select {unit}: {names}")
            for consumer in consumers:
                self.assertTrue(any(consumer.startswith(selected) for selected in names),
                                f"{path} does not select {consumer}: {names}")
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                        "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                        "module_filters": {owner: names}}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                      "boundary": boundary, "cargo_inventory": inventory,
                                      "paths": [path]})
            unit_step = next(step for step in steps if "unit_editor_logic" in step.argv)
            self.assertTrue(unit_step.requires_tests)
            self.assertTrue(any(unit.startswith(selected) for selected in unit_step.argv))
            self.assertTrue(any(tree_unit.startswith(selected) for selected in unit_step.argv))
            feature = next(step for step in steps if step.step_id == "ui-feature-tests")
            for consumer in consumers:
                self.assertTrue(any(consumer.startswith(selected) for selected in feature.argv),
                                f"{path} feature step does not select {consumer}")

    def test_browser_tree_state_keeps_real_worker_and_window_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::browser::tree::state::"
        scan_unit = "editor::browser::scan::"
        folder_position = (
            "editor::browser::ui_test::"
            "browser_folder_expansion_preserves_its_position_when_an_earlier_branch_closes"
        )
        consumers = ["editor::browser::ui_test::browser_inline_tree_nested_keyboard_refresh_and_hover_retain_identity",
                     "editor::browser::ui_test::browser_tree_same_name_files_keep_distinct_preview_paths_and_bounded_labels",
                     "editor::browser::ui_test::browser_tree_refresh_updates_expanded_branch_hidden_by_indexed_query",
                     folder_position,
                     "editor::browser::ui_test::browser_history_ui_test::review_readiness_obsolete_tree_worker_does_not_hold_saved_root",
                     "editor::browser::ui_test::browser_source_transition_discards_late_scan_and_keeps_query_and_keyboard_scope",
                     "editor::browser::ui_test::place_correctness_reselect_gates_cached_rows_and_old_audition_callbacks",
                     "editor::browser::ui_test::place_correctness_refresh_retains_identity_only_after_current_success",
                     "editor::browser::ui_test::browser_history_ui_test::browser_history_missing_place_remains_recoverable_and_late_scan_cannot_retarget"]
        names = plan.module_test_filters(root, ["crates/hex-ui/src/editor/browser/tree/state.rs"], inventory, [owner])
        self.assertEqual(names, {owner: sorted([unit, scan_unit, *consumers])})
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                    "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                    "module_filters": names}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": []})
        self.assertTrue(any(unit in step.argv and step.requires_tests
                            for step in steps if "unit_editor_logic" in step.argv))
        self.assertTrue(any(scan_unit in step.argv and step.requires_tests
                            for step in steps if "unit_editor_logic" in step.argv))
        self.assertTrue(any(all(name in step.argv for name in consumers)
                            for step in steps if step.step_id == "ui-feature-tests"))
        for path in ("browser/tree.rs", "browser_ui_test.rs", "browser.rs"):
            selected = plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + path], inventory, [owner])
            self.assertTrue(any(unit.startswith(name) for name in selected[owner]))
            self.assertTrue(any(scan_unit.startswith(name) for name in selected[owner]))
            self.assertTrue(any(folder_position.startswith(name) for name in selected[owner]))

    def test_project_tempo_value_keeps_unit_and_focused_commit_coverage(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::tempo_input::value::"
        consumers = [
            "editor::tempo_input::keyboard_tests::focused_project_arrows_adjust_local_draft_then_commit_exact_micro_bpm",
            "editor::tempo_input::keyboard_tests::project_steps_preserve_bounds_precision_cancel_invalid_and_composition",
            "editor::tempo_input::tests::project_tempo_same_value_explicit_commit_collapses_later_points_and_cursor_is_display_only",
            "editor::tempo_input_ui_test::project_tempo_direct_input_validation_cancel_focus_and_canonical_commit",
            "editor::tempo_input_ui_test::project_tempo_blur_commits_valid_text_and_invalid_blur_releases_editor_shortcuts",
            "editor::tempo_input_ui_test::project_tempo_stale_revision_and_project_switch_retire_drafts",
            "editor::tempo_input_ui_test::project_tempo_cursor_movement_cannot_retarget_an_active_draft",
        ]
        path = "crates/hex-ui/src/editor/tempo_input/value.rs"
        names = plan.module_test_filters(root, [path], inventory, [owner])
        self.assertEqual(names, {owner: sorted([unit, *consumers])})
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                    "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                    "module_filters": names}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": [path]})
        unit_step = next(step for step in steps if "unit_editor_logic" in step.argv)
        self.assertTrue(unit_step.requires_tests)
        self.assertIn(unit, unit_step.argv)
        integration = next(step for step in steps if step.step_id == "ui-feature-tests")
        self.assertTrue(all(case in integration.argv for case in consumers))
        for source in ("tempo_input.rs", "tempo_input/keyboard_tests.rs", "tempo_input_ui_test.rs"):
            with self.subTest(source=source):
                selected = plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + source], inventory, [owner])
                for required in (unit, *consumers):
                    self.assertTrue(any(required.startswith(name) for name in selected[owner]))

    def test_time_move_interval_split_keeps_unit_and_window_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []},
                      "dependencies": [], "targets": [{"kind": ["lib"]}]}]
        unit = "editor::time_move_preview::intervals::"
        visual = "editor::time_move_preview::visual_tests::fragment_titles_cuts_crops_and_compact_body_preserve_source_mapping"
        move = "editor::edit_cursor_ui_test::audio_time_move_preview_masks_overlaps_and_restores_when_cancelled"
        midi = "editor::edit_cursor_ui_test::midi_time_move_preview_translates_cached_notes_to_the_destination"
        acknowledgement = "editor::edit_cursor_ui_test::pending_time_move_preview_survives_until_worker_failure_then_restores_source"
        expected = sorted([unit, visual, move, midi, acknowledgement])
        names = plan.module_test_filters(
            root, ["crates/hex-ui/src/editor/time_move_preview/intervals.rs"], inventory, [owner])
        self.assertEqual(names, {owner: expected})
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                    "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                    "module_filters": names}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                  "boundary": boundary, "cargo_inventory": inventory, "paths": []})
        self.assertTrue(any(unit in step.argv and step.requires_tests
                            for step in steps if "unit_editor_logic" in step.argv))
        self.assertTrue(any(all(name in step.argv for name in (visual, move, midi, acknowledgement))
                            for step in steps if step.step_id == "ui-feature-tests"))
        # Editing the GPUI owner still retains its real visual test.
        owner_names = plan.module_test_filters(
            root, ["crates/hex-ui/src/editor/time_move_preview.rs"], inventory, [owner])
        self.assertTrue(any(visual.startswith(name) for name in owner_names[owner]))
        self.assertTrue(any(unit.startswith(name) for name in owner_names[owner]))

    def test_trim_preview_state_keeps_rendered_release_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        source = "crates/hex-ui/src/editor/trim/preview_state.rs"
        unit = "editor::trim::preview_state::"
        consumers = [
            "editor::trim::resize_release_tests::normal_resize_preserves_visible_extent_and_source_through_release_and_ack",
            "editor::trim::resize_release_tests::midi_resize_keeps_released_geometry_until_actual_worker_ack_and_one_history_step",
            "editor::trim::loop_ui_tests::clip_loop_outer_real_pointer_retains_released_paint_and_one_worker_history_edge",
            "editor::trim::midi_stretch_ui_tests::midi_shift_stretch_both_edges_projects_notes_through_release_ack_and_one_undo",
        ]
        names = plan.module_test_filters(root, [source], inventory, [owner])
        self.assertEqual(names, {owner: sorted([unit, *consumers])})
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                    "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                    "module_filters": names}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": [source]})
        self.assertTrue(any(unit in step.argv and step.requires_tests
                            for step in steps if "unit_editor_logic" in step.argv))
        self.assertTrue(any(all(name in step.argv for name in consumers)
                            for step in steps if step.step_id == "ui-feature-tests"))
        for path in ("trim.rs", "resize_release_tests.rs", "clip_loop_ui_tests.rs", "midi_stretch_ui_tests.rs"):
            selected = plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + path], inventory, [owner])
            self.assertTrue(any(unit.startswith(name) for name in selected[owner]))

    def test_trim_admission_keeps_rendered_refusal_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        source = "crates/hex-ui/src/editor/trim/admission.rs"
        unit = "editor::trim::admission::"
        consumers = [
            "editor::trim::resize_terminal_tests::restart_offline_pending_work_blocks_actual_resize_capture_until_completion",
            "editor::trim::resize_release_tests::resize_escape_stale_generation_pending_refusal_and_no_motion_do_not_retain_a_commit",
        ]
        names = plan.module_test_filters(root, [source], inventory, [owner])
        self.assertEqual(names, {owner: sorted([unit, *consumers])})
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                    "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                    "module_filters": names}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": [source]})
        self.assertTrue(any(unit in step.argv and step.requires_tests
                            for step in steps if "unit_editor_logic" in step.argv))
        self.assertTrue(any(all(name in step.argv for name in consumers)
                            for step in steps if step.step_id == "ui-feature-tests"))
        for path in ("trim.rs", "resize_terminal_tests.rs", "resize_release_tests.rs"):
            selected = plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + path], inventory, [owner])
            self.assertTrue(any(unit.startswith(name) for name in selected[owner]))

    def test_trim_payload_unit_tier_routes_owner_and_real_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        source = "crates/hex-ui/src/editor/trim.rs"
        consumers = (
            "editor::trim::resize_release_tests::normal_resize_preserves_visible_extent_and_source_through_release_and_ack",
            "editor::trim::loop_ui_tests::clip_loop_outer_real_pointer_retains_released_paint_and_one_worker_history_edge",
            "editor::trim::midi_stretch_ui_tests::midi_shift_stretch_pending_receipt_ignores_earlier_ack_and_retires_real_worker_refusal",
        )
        boundary = plan.boundary_selection(root, [source], inventory, [owner])
        self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
        steps = {step.step_id: step for step in ci.planned_steps({
            "scope": "rust", "affected_packages": [owner], "boundary": boundary,
            "cargo_inventory": inventory, "paths": [source]})}
        default = steps["ui-default-lib-unit-tests"]
        self.assertTrue(default.requires_tests)
        self.assertIn("editor::trim::tests::", default.argv)
        self.assertIn("editor::drag_autoscroll::tests::", default.argv)
        standalone = steps["ui-unit-unit_editor_logic"]
        self.assertTrue(standalone.requires_tests)
        self.assertTrue(any("editor::trim::geometry::".startswith(prefix) for prefix in standalone.argv))
        feature = steps["ui-feature-tests"].argv
        for consumer in consumers:
            self.assertTrue(any(consumer.startswith(prefix) for prefix in feature))
        self.assertNotIn("editor::", feature)

    def test_trim_geometry_keeps_rendered_audio_and_midi_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        source = "crates/hex-ui/src/editor/trim/geometry.rs"
        unit = "editor::trim::geometry::"
        consumers = [
            "editor::clip_drag_ui_test::rendered_audio_trim_updates_the_clip_waveform_geometry_and_commits_release_snap",
            "editor::clip_drag_ui_test::rendered_audio_trim_release_upper_rail_commits_captured_clip_and_actual_worker_ack",
            "editor::clip_drag_ui_test::rendered_midi_trim_previews_note_geometry_and_commits_release_snap",
            "editor::track_compact_ui_test::compact_audio_and_midi_trim_edges_keep_title_and_exact_preview_until_cancel",
        ]
        names = plan.module_test_filters(root, [source], inventory, [owner])
        self.assertEqual(names, {owner: sorted([unit, *consumers])})
        boundary = plan.boundary_selection(root, [source], inventory, [owner])
        self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
        self.assertEqual(boundary["module_filters"], names)
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": [source]})
        self.assertTrue(any(unit in step.argv and step.requires_tests
                            for step in steps if "unit_editor_logic" in step.argv))
        self.assertTrue(any(all(name in step.argv for name in consumers)
                            for step in steps if step.step_id == "ui-feature-tests"))
        for path in ("trim.rs", "clip_drag_ui_test.rs", "track_compact_ui_test.rs"):
            selected = plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + path], inventory, [owner])
            self.assertTrue(any(unit.startswith(name) for name in selected[owner]))

        geometry_path = "src/editor/trim/geometry.rs"
        fixture_gate = plan.UI_FEATURE_EQUIVALENT_TEST_CFGS[geometry_path]
        self.assertIn(f"{fixture_gate}\npub(super) fn clip_fixture(", (root / source).read_text())
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            geometry = root / source
            geometry.parent.mkdir(parents=True)
            geometry.write_text(f"{fixture_gate}\nfn clip_fixture() {{}}\n")
            self.assertEqual(plan.single_run_packages(root, [owner], inventory), {owner: "ui-test-support"})
            geometry.rename(geometry.with_name("unreviewed.rs"))
            self.assertEqual(plan.single_run_packages(root, [owner], inventory), {})

    def test_operator_panel_units_keep_their_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        source = "crates/hex-ui/src/editor/device_panel/"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        cases = (
            ("operator_panel.rs", "editor::device_panel::operator_panel::tests::", (
                "editor::device_panel::tests::operator_tab_tests::operator_tab_cycles_rendered_controls_in_both_directions_without_project_edits",
                "editor::device_panel::tests::operator_waveform_editor_tests::waveform_cycle_updates_from_real_partial_edits_and_undo",
            )),
            ("operator_panel/envelope.rs", "editor::device_panel::operator_panel::envelope::tests::", (
                "editor::device_panel::tests::operator_envelope_reference_ui_tests::other_envelopes_refresh_without_stealing_selection_and_reuse_repaint_geometry",
            )),
            ("operator_panel/filter_response.rs", "editor::device_panel::operator_panel::filter_response::tests::", (
                "editor::device_panel::tests::operator_filter_response_tests::native_filter_display_fits_reuses_cache_and_follows_real_control_history",
                "editor::device_panel::tests::operator_filter_response_tests::filter_curve_keeps_released_parameter_preview_across_completed_repaints",
            )),
            ("operator_panel/harmonics.rs", "editor::device_panel::operator_panel::harmonics::tests::", (
                "editor::device_panel::tests::operator_waveform_editor_tests::waveform_cycle_updates_from_real_partial_edits_and_undo",
            )),
        )
        for path, unit, consumers in cases:
            with self.subTest(path=path):
                names = plan.module_test_filters(root, [source + path], inventory, [owner])
                self.assertTrue(any(unit.startswith(prefix) for prefix in names[owner]))
                boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                            "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                            "module_filters": names}
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [source + path]})}
                self.assertIn(unit, steps["ui-default-lib-unit-tests"].argv)
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(name) for name in steps["ui-feature-tests"].argv))
                    module = consumer.split("::")[-2]
                    reverse = plan.module_test_filters(
                        root, [source + "tests/" + module + ".rs"], inventory, [owner])
                    self.assertTrue(any(unit.startswith(prefix) for prefix in reverse[owner]))

    def test_arrangement_seek_units_keep_callback_worker_and_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::arrangement_seek::unit_tests::"
        consumers = (
            "editor::arrangement_seek::tests::post_loop_cursor_survives_actual_seek_worker_and_fenced_callback_installation",
            "editor::arrangement_sections::tests::section_autoplay_click_dispatches_stateless_loop_and_toggle_off_cancels",
            "editor::edit_cursor_ui_test::stateful_space_queues_play_from_without_a_seek_worker",
        )
        for name in ("arrangement_seek.rs", "arrangement_seek_tests.rs",
                     "arrangement_seek_unit_tests.rs", "arrangement_sections.rs", "edit_cursor_ui_test.rs"):
            path = "crates/hex-ui/src/editor/" + name
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                unit_args = steps["ui-default-lib-unit-tests"].argv
                self.assertIn(unit, unit_args)
                self.assertNotIn("editor::arrangement_seek::tests::", unit_args)
                feature_args = steps["ui-feature-tests"].argv
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in feature_args),
                                    f"{consumer} is missing from {feature_args}")

    def test_controller_monitoring_units_keep_ingress_and_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::controller_input::monitoring::unit_tests::"
        consumers = (
            "editor::controller_input::monitoring::tests::actual_wire_ingress_reaches_host_pcm_and_stale_sink_cannot_cancel_reconnect",
            "editor::controller_input::hardware_chord_step_uses_native_decoder_velocities_channel_ack_and_history",
            "editor::controller_input::hardware_step_discontinuity_excludes_old_keys_and_cancel_refuses_false_rest",
            "editor::midi_canvas::audition_tests::configured_controller_keeps_its_root_during_note_and_marquee_audition",
        )
        for name in ("controller_input/monitoring.rs", "controller_input.rs", "midi_audition_ui_tests.rs"):
            with self.subTest(name=name):
                path = "crates/hex-ui/src/editor/" + name
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                self.assertIn(unit, steps["ui-default-lib-unit-tests"].argv)
                self.assertNotIn("--features", steps["ui-default-lib-unit-tests"].argv)
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in steps["ui-feature-tests"].argv))

    def test_device_meter_units_keep_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::device_panel::device_meter::tests::"
        consumers = (
            "editor::device_panel::tests::clicking_a_device_meter_clears_its_held_overload_line",
        )
        for name in (
            "crates/hex-ui/src/editor/device_panel/device_meter.rs",
            "crates/hex-ui/src/editor/device_panel.rs",
        ):
            with self.subTest(name=name):
                boundary = plan.boundary_selection(root, [name], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [name]})}
                self.assertIn(unit, steps["ui-default-lib-unit-tests"].argv)
                self.assertNotIn("--features", steps["ui-default-lib-unit-tests"].argv)
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in steps["ui-feature-tests"].argv))

    def test_changelog_units_keep_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::changelog::tests::"
        consumers = (
            "editor::changelog::ui_tests::changelog_empty_groups_unavailable_and_malformed_render_truthfully",
            "editor::changelog::ui_tests::changelog_grouped_headings_render_once_and_rows_hover_independently",
            "editor::changelog::ui_tests::shortcut_opens_actual_embedded_generated_notes_without_network_or_files",
        )
        for name in (
            "crates/hex-ui/src/editor/changelog.rs",
        ):
            with self.subTest(name=name):
                boundary = plan.boundary_selection(root, [name], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [name]})}
                self.assertIn(unit, steps["ui-default-lib-unit-tests"].argv)
                self.assertNotIn("--features", steps["ui-default-lib-unit-tests"].argv)
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in steps["ui-feature-tests"].argv))

    def test_drag_autoscroll_units_keep_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::drag_autoscroll::tests::"
        consumers = (
            "editor::clip_drag_ui_test::held_clip_drag_scrolls_preview_without_editing_and_rejects_stale_project",
            "editor::clip_drag_ui_test::vertical_drag_scroll_invalidates_old_rows_before_fast_release",
            "editor::clip_drag_ui_test::vertical_drag_scroll_retargets_stationary_clip_before_release",
            "editor::drag_autoscroll::resize_pan_tests::resize_pan_preserves_stationary_snap_and_shift_stretch_preview_release",
            "editor::drag::group_drag_ui_test::group_vertical_autoscroll_invalidates_all_previews_before_repaint",
            "editor::trim::midi_left_extension_ui_tests::fresh_midi_left_extension_edge_pan_stops_interior_and_releases_exact_geometry",
        )
        for name in (
            "crates/hex-ui/src/editor/drag_autoscroll.rs",
            "crates/hex-ui/src/editor/clip_drag_ui_test.rs",
            "crates/hex-ui/src/editor/resize_pan_tests.rs",
            "crates/hex-ui/src/editor/trim.rs",
            "crates/hex-ui/src/editor/drag.rs",
            "crates/hex-ui/src/editor/group_drag_ui_test.rs",
            "crates/hex-ui/src/editor/midi_left_extension_ui_tests.rs",
        ):
            with self.subTest(name=name):
                boundary = plan.boundary_selection(root, [name], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [name]})}
                self.assertIn(unit, steps["ui-default-lib-unit-tests"].argv)
                self.assertNotIn("--features", steps["ui-default-lib-unit-tests"].argv)
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in steps["ui-feature-tests"].argv))

    def test_take_recovery_keeps_ordered_query_and_real_custody_coverage(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        module = "editor::automation_take_recovery::"
        unit = module + "unit_tests::"
        support = module + "tests::"
        consumers = (
            support + "cancelled_picker_and_stale_responses_cannot_release_the_original",
            support + "rendered_copy_action_can_cancel_without_releasing_the_close_fence",
            support + "file_action_selects_a_finished_take_without_applying_it",
        )
        for path in ("automation_take_recovery.rs", "automation_take_recovery/tests.rs",
                     "automation_take_recovery/test_fixture.rs"):
            name = "crates/hex-ui/src/editor/" + path
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [name], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [name]})}
                self.assertIn(unit, steps["ui-default-lib-unit-tests"].argv)
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in steps["ui-feature-tests"].argv))

    def test_musical_audio_control_units_keep_their_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        all_consumers = (
            "editor::musical_audio_controls::sample_bpm::ui_tests::sample_bpm_actual_text_input_commits_once_and_escape_cancels",
            "editor::musical_audio_controls::ui_tests::algorithm_picker_dispatches_direct_choice_without_resetting_current_settings",
            "editor::musical_audio_controls::ui_tests::selected_clip_beats_controls_are_visible_and_dispatch_existing_loop_envelope_bindings",
            "editor::musical_audio_controls::ui_tests::selected_clip_warp_toggle_is_visible_and_dispatches_transient_beats",
            "editor::musical_audio_controls::ui_tests::shift_selected_warp_applies_to_every_clip_with_its_own_source_region",
        )
        parent = "crates/hex-ui/src/editor/musical_audio_controls.rs"
        bpm = "crates/hex-ui/src/editor/musical_audio_controls/sample_bpm.rs"
        fixture = "crates/hex-ui/src/editor/musical_audio_controls/ui_tests.rs"
        modules = plan.module_paths(root / "crates/hex-ui/src")
        self.assertEqual(modules[(root / fixture).resolve()], "editor::musical_audio_controls::ui_tests")
        self.assertEqual(plan.single_run_packages(root, [owner], inventory),
                         {owner: "ui-test-support"})
        cases = (
            (parent, ("editor::musical_audio_controls::tests::",
                      "editor::musical_audio_controls::sample_bpm::tests::"), all_consumers),
            (bpm, ("editor::musical_audio_controls::sample_bpm::tests::",), (all_consumers[0],)),
            ("crates/hex-ui/src/editor/musical_audio_controls/tests.rs",
             ("editor::musical_audio_controls::tests::",), all_consumers[1:]),
            (fixture, (), all_consumers[1:]),
        )
        for path, unit_prefixes, consumers in cases:
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                names = boundary["module_filters"][owner]
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                feature = steps["ui-feature-tests"].argv
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(name) for name in feature), consumer)
                defaults = [step for name, step in steps.items()
                            if name == "ui-default-lib-unit-tests"]
                self.assertEqual(len(defaults), int(bool(unit_prefixes)))
                if unit_prefixes:
                    self.assertNotIn("--features", defaults[0].argv)
                    for unit in unit_prefixes:
                        self.assertTrue(any(unit.startswith(name) or name.startswith(unit)
                                            for name in defaults[0].argv), unit)
                else:
                    self.assertTrue(any(name == "editor::musical_audio_controls::ui_tests::"
                                        for name in names))

    def test_arrangement_audio_controls_keep_unit_and_worker_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit_cases = (
            "editor::arrangement_audio_panel::tests::controls_map_to_typed_clip_edits_and_loop_toggle",
            "editor::arrangement_audio_panel::tests::gain_steps_use_db_units_and_refuse_the_bounds",
            "editor::arrangement_audio_panel::tests::invalid_edges_and_overflow_refuse_without_partial_command",
            "editor::arrangement_audio_panel::tests::review_play_start_uses_retained_source_looping_independent_of_placement_mode",
            "editor::arrangement_audio_panel::controls::tests::",
        )
        windows = (
            "editor::arrangement_audio_panel::controls::pitch_ui_tests::pitch_knob_real_worker_history_entry_reset_group_scope_and_save_reopen",
            "editor::arrangement_audio_panel::controls::pitch_ui_tests::pitch_release_keeps_its_draft_until_accepted_or_refused_ack",
            "editor::arrangement_audio_panel::controls::ui_tests::clip_slider_cancel_and_frozen_release_wait_for_own_terminal",
            "editor::arrangement_audio_panel::tests::length_worker_returns::placed_length_ordinary_trim_real_worker_control",
            "editor::arrangement_audio_panel::tests::review_placed_mode_only_change_invalidates_captured_control",
        )
        for path in ("arrangement_audio_panel.rs", "arrangement_audio_panel/controls.rs",
                     "arrangement_audio_panel/pitch_ui_tests.rs"):
            with self.subTest(path=path):
                source = "crates/hex-ui/src/editor/" + path
                names = plan.module_test_filters(root, [source], inventory, [owner])
                boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                            "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                            "module_filters": names}
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [source]})}
                for name in unit_cases:
                    self.assertTrue(any(name.startswith(prefix) for prefix in steps["ui-default-lib-unit-tests"].argv))
                for name in windows:
                    self.assertTrue(any(name.startswith(prefix) for prefix in steps["ui-feature-tests"].argv))

    def test_looped_clip_paint_units_keep_real_header_and_release_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit_filters = (
            "editor::clip_header_loop::tests::",
            "editor::trim::loop_handles::tests::",
        )
        rendered = (
            "editor::waveform::waveform_header_ui_test::looped_header_paints_canonical_visible_cycle_edges_at_two_zooms",
            "editor::trim::loop_ui_tests::clip_loop_outer_real_pointer_retains_released_paint_and_one_worker_history_edge",
        )
        for path in (
            "crates/hex-ui/src/editor/clip_header_loop.rs",
            "crates/hex-ui/src/editor/clip_loop_handles.rs",
        ):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                unit = steps["ui-default-lib-unit-tests"]
                self.assertTrue(unit.requires_tests)
                for name in unit_filters:
                    self.assertIn(name, unit.argv)
                feature = steps["ui-feature-tests"].argv
                for name in rendered:
                    self.assertIn(name, feature)
                self.assertNotIn("editor::", feature)

    def test_clip_border_corner_unit_keeps_rendered_clip_edges(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        consumers = (
            "editor::clip_boundary::tests::adjacent_clip_edges_share_one_grid_pixel",
            "editor::clip_boundary::tests::all_four_clip_edges_paint_above_actual_row_separator_without_geometry_changes",
            "editor::clip_boundary::tests::resolved_clip_canvas_can_paint_a_distinct_edge_in_the_parent_scene",
        )
        for path in (
            "crates/hex-ui/src/editor/clip_boundary.rs",
            "crates/hex-ui/src/editor/clip_border_ui_test.rs",
        ):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                unit = steps["ui-default-lib-unit-tests"]
                self.assertIn("editor::clip_boundary::tests::corner_geometry_is_bounded_and_resolves_again_after_resize", unit.argv)
                self.assertTrue(unit.requires_tests)
                feature = steps["ui-feature-tests"].argv
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in feature), (consumer, feature))
                self.assertNotIn("editor::", feature)

        boundary["module_filters"] = {owner: [consumers[0]]}
        rendered_only = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                         "boundary": boundary, "cargo_inventory": inventory, "paths": []})
        self.assertFalse(any(step.step_id == "ui-default-lib-unit-tests" for step in rendered_only))

    def test_clip_drag_geometry_keeps_unit_and_runtime_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::drag::geometry::"
        drag = "editor::drag::tests::"
        worker = "editor::clip_drag_ui_test::rendered_move_retains_destination_until_real_worker_ack"
        group = "editor::drag::group_drag_ui_test::stationary_group_snap_modifier_uses_one_anchor_delta_and_authoritative_release"
        names = plan.module_test_filters(root, ["crates/hex-ui/src/editor/drag/geometry.rs"], inventory, [owner])
        self.assertEqual(names, {owner: sorted([unit, drag, worker, group])})
        parent = plan.module_test_filters(root, ["crates/hex-ui/src/editor/drag.rs"], inventory, [owner])
        self.assertEqual(parent, {owner: sorted([
            "editor::drag::", worker, "editor::tracks::group_overlap::",
            "editor::drag_autoscroll::tests::",
            "editor::clip_drag_ui_test::held_clip_drag_scrolls_preview_without_editing_and_rejects_stale_project",
            "editor::clip_drag_ui_test::vertical_drag_scroll_invalidates_old_rows_before_fast_release",
            "editor::clip_drag_ui_test::vertical_drag_scroll_retargets_stationary_clip_before_release",
            "editor::drag_autoscroll::resize_pan_tests::resize_pan_preserves_stationary_snap_and_shift_stretch_preview_release",
            "editor::trim::midi_left_extension_ui_tests::fresh_midi_left_extension_edge_pan_stops_interior_and_releases_exact_geometry",
            "editor::audio_event_editor::paint::tests::replacement_accents_follow_occupied_victims_and_range_overwrite_semantics",
            "editor::audio_event_editor::workflow_tests::event_overlap_accent_covers_only_replaced_audio_and_survives_release",
        ])})
        for path in ("clip_drag_ui_test.rs", "group_drag_ui_test.rs"):
            selected = plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + path], inventory, [owner])
            self.assertIn(unit, selected[owner])
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner], "lint_packages": [owner],
                    "single_run": {owner: "ui-test-support"}, "module_filters": names}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": []})
        self.assertTrue(any(unit in step.argv and step.requires_tests
                            for step in steps if "unit_editor_logic" in step.argv))
        drag_state = next(step for step in steps if step.step_id == "ui-default-lib-unit-tests")
        self.assertIn(drag, drag_state.argv)
        self.assertTrue(drag_state.requires_tests)
        self.assertTrue(any(all(name in step.argv for name in (worker, group))
                            for step in steps if step.step_id == "ui-feature-tests"))

    def test_pending_clip_move_unit_keeps_real_move_and_group_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        source = "crates/hex-ui/src/editor/pending_clip_move.rs"
        unit = "editor::pending_clip_move::tests::"
        consumers = (
            "editor::clip_drag_ui_test::rendered_move_survives_same_revision_worker_work",
            "editor::clip_drag_ui_test::rendered_move_real_refusal_retires_only_its_matching_fifo_terminal",
            "editor::clip_drag_ui_test::rendered_move_retains_destination_until_real_worker_ack",
            "editor::drag::group_drag_ui_test::group_pending_or_actual_revision_change_refuses_all_members_without_losing_independent_edit",
        )
        boundary = plan.boundary_selection(root, [source], inventory, [owner])
        self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
        steps = {step.step_id: step for step in ci.planned_steps({
            "scope": "rust", "affected_packages": [owner], "boundary": boundary,
            "cargo_inventory": inventory, "paths": [source]})}
        default = steps["ui-default-lib-unit-tests"]
        self.assertTrue(default.requires_tests)
        self.assertIn(unit, default.argv)
        feature = steps["ui-feature-tests"].argv
        for consumer in consumers:
            self.assertIn(consumer, feature)
        self.assertNotIn("editor::", feature)

    def test_stretch_projection_retains_real_cache_consumer(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []},
                      "dependencies": [], "targets": [{"kind": ["lib"]}]}]
        unit = "editor::waveform_stretch::projection::"
        cache = "editor::waveform_stretch::tests::geometry_cache_replaces_preview_snapshot_and_distinguishes_source_and_map"
        names = plan.module_test_filters(root, ["crates/hex-ui/src/editor/waveform_stretch/projection.rs"], inventory, [owner])
        self.assertTrue(any(unit.startswith(name) for name in names[owner]))
        self.assertTrue(any(cache.startswith(name) for name in names[owner]))
        consumers = (
            "editor::waveform::resize_tests::silent_left_prefix_keeps_waveform_and_geometry_through_real_worker_ack",
            "editor::waveform_geometry::", "editor::waveform_loop::", "editor::waveform_source::",
            "editor::browser::ui_test::browser_hover_and_selection_keep_selected_waveform_and_surroundings_stable",
            "editor::sampler_parameter::tests::source_hints_and_paint_require_same_view_source_facts_and_context",
            "editor::audio_event_editor::tests::moved_and_pasted_waveform_positions_read_event_composed_pcm",
            "editor::audio_event_editor::free_placement_tests::finite_frame_drag_preserves_the_cycle_markers_and_root_waveform",
        )
        for consumer in consumers:
            self.assertTrue(any(consumer.startswith(name) for name in names[owner]))
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                    "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                    "module_filters": names}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                  "boundary": boundary, "cargo_inventory": inventory, "paths": []})
        self.assertTrue(any("unit_editor_logic" in step.argv and step.requires_tests
                            and any(unit.startswith(arg) for arg in step.argv) for step in steps))
        feature = next(step for step in steps if step.step_id == "ui-feature-tests")
        for consumer in consumers:
            self.assertTrue(any(consumer.startswith(arg) for arg in feature.argv))
        for source in ("waveform_geometry.rs", "waveform_loop.rs", "waveform.rs"):
            reverse = plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + source], inventory, [owner])
            self.assertTrue(any(unit.startswith(name) for name in reverse[owner]))

    def test_stretch_cache_unit_keeps_projection_and_rendered_stretch(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::waveform_stretch::tests::geometry_cache_replaces_preview_snapshot_and_distinguishes_source_and_map"
        projection = "editor::waveform_stretch::projection::"
        witness = "editor::waveform::resize_tests::shift_stretch_keeps_live_sample_varied_waveforms_visible"
        for name in ("waveform_stretch.rs", "waveform_stretch/projection.rs", "waveform_stretch/tests.rs"):
            with self.subTest(name=name):
                path = "crates/hex-ui/src/editor/" + name
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                default = steps["ui-default-lib-unit-tests"]
                self.assertTrue(default.requires_tests)
                self.assertTrue(any(unit.startswith(prefix) for prefix in default.argv))
                standalone = steps["ui-unit-unit_editor_logic"]
                self.assertTrue(standalone.requires_tests)
                self.assertTrue(any(projection.startswith(prefix) for prefix in standalone.argv))
                feature = steps["ui-feature-tests"].argv
                self.assertTrue(any(witness.startswith(prefix) for prefix in feature))
                self.assertNotIn("editor::", feature)

    def test_session_feedback_keeps_real_callback_and_window_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::controller::session_feedback::"
        callback = "editor::controller::tests::earlier_session_rejection_is_not_reported_as_consumed"
        window = "editor::action_outcome_ui_tests::editor_reports_each_rejection_and_keeps_reader_through_retirement"
        names = plan.module_test_filters(root, ["crates/hex-ui/src/editor/controller/session_feedback.rs"], inventory, [owner])
        self.assertEqual(names, {owner: sorted([unit, callback, window])})
        parent = plan.module_test_filters(root, ["crates/hex-ui/src/editor/controller.rs"], inventory, [owner])
        self.assertIn("editor::controller::", parent[owner])
        self.assertIn(window, parent[owner])
        reverse = plan.module_test_filters(root, ["crates/hex-ui/src/editor/action_outcome_ui_tests.rs"], inventory, [owner])
        self.assertIn(unit, reverse[owner])
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner], "lint_packages": [owner],
                    "single_run": {owner: "ui-test-support"}, "module_filters": names}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": []})
        self.assertTrue(any(unit in step.argv and step.requires_tests
                            for step in steps if "unit_editor_logic" in step.argv))
        self.assertTrue(any(callback in step.argv and window in step.argv
                            for step in steps if step.step_id == "ui-feature-tests"))

    def test_computer_keyboard_state_selects_unit_and_integration(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        path = "crates/hex-ui/src/editor/computer_keyboard/state.rs"
        names = ["editor::computer_keyboard::state::", "editor::computer_keyboard::tests::"]
        self.assertEqual(plan.module_test_filters(root, [path], inventory, [owner]), {owner: names})
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                    "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                    "module_filters": {owner: names}}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": [path]})
        unit = next(step for step in steps if "unit_editor_logic" in step.argv)
        integration = next(step for step in steps if step.step_id == "ui-feature-tests")
        self.assertIn(names[0], unit.argv)
        self.assertTrue(unit.requires_tests)
        self.assertIn(names[1], integration.argv)

    def test_waveform_paint_geometry_keeps_exact_render_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::waveform::paint_geometry::"
        event_detail = "editor::waveform_geometry::tests::event_detail_keeps_a_transient_in_its_visible_column"
        consumers = [
            *WAVEFORM_PAINT_GEOMETRY_CONSUMERS,
            "editor::waveform::tests::waveform_canvas_paints_full_clip_extent",
            "editor::waveform::waveform_header_ui_test::waveform_body_painter_stays_below_ordinary_selected_and_looped_headers",
        ]
        source = "crates/hex-ui/src/editor/"
        names = plan.module_test_filters(root, [source + "waveform/paint_geometry.rs"], inventory, [owner])
        self.assertEqual(names, {owner: sorted([unit, event_detail, *consumers])})
        for path in ("waveform.rs", "waveform_geometry.rs", "tracks.rs",
                     "audio_event_editor/waveform_preview.rs", "audio_event_editor/workflow_tests.rs",
                     "audio_event_editor/free_placement_tests.rs", "audio_event_editor/tests.rs",
                     "fades/musical_tests.rs", "fade_handles_ui_test.rs", "waveform_header_ui_test.rs"):
            with self.subTest(path=path):
                selected = plan.module_test_filters(root, [source + path], inventory, [owner])[owner]
                self.assertTrue(any(unit.startswith(name) for name in selected))
                for consumer in (*consumers, event_detail):
                    self.assertTrue(any(consumer.startswith(name) for name in selected))
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                    "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                    "module_filters": names}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                  "boundary": boundary, "cargo_inventory": inventory, "paths": []})
        unit_step = next(step for step in steps if "unit_editor_logic" in step.argv)
        self.assertTrue(unit_step.requires_tests)
        self.assertIn(unit, unit_step.argv)
        cache_step = next(step for step in steps if step.step_id == "ui-default-lib-unit-tests")
        self.assertEqual(cache_step.argv[-1], event_detail)
        self.assertTrue(cache_step.requires_tests)
        feature_step = next(step for step in steps if step.step_id == "ui-feature-tests")
        for consumer in consumers:
            self.assertIn(consumer, feature_step.argv)

    def test_waveform_source_selects_unit_and_projection_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        names = sorted(GRID_CONSUMERS)
        for path in ("crates/hex-ui/src/editor/waveform_source.rs", "crates/hex-ui/src/editor/waveform.rs"):
            self.assertEqual(plan.module_test_filters(root, [path], inventory, [owner]), {owner: names})
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                        "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                        "module_filters": {owner: names}}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                      "cargo_inventory": inventory, "paths": [path]})
            unit = next(step for step in steps if "unit_editor_logic" in step.argv)
            integration = next(step for step in steps if step.step_id == "ui-feature-tests")
            self.assertIn("editor::waveform_source::", unit.argv)
            self.assertTrue(unit.requires_tests)
            for name in names:
                self.assertIn(name, integration.argv)

    def test_clone_source_window_keeps_unit_harness_and_rendered_drag_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::tracks::clone_source_window::"
        consumers = [
            "editor::tracks::overlap_tests::held_midi_drag_keeps_actual_note_paint_and_colored_name_below_header",
            "editor::tracks::overlap_tests::held_processed_audio_drag_reuses_ready_root_on_lane_and_floating",
            "editor::tracks::overlap_tests::held_looped_warp_drag_keeps_last_ready_content_in_mixed_copy_group",
        ]
        source = "crates/hex-ui/src/editor/tracks/clone_source_window.rs"
        names = plan.module_test_filters(root, [source], inventory, [owner])[owner]
        self.assertEqual(names, sorted([unit, *consumers]))

        tracks_shim = "crates/hex-ui/tests/editor_logic_unit/tracks.rs"
        overlap_source = "crates/hex-ui/src/editor/tracks/group_overlap.rs"
        overlap_names = plan.module_test_filters(root, [overlap_source], inventory, [owner])[owner]
        self.assertEqual(plan.module_test_filters(root, [tracks_shim], inventory, [owner]),
                         {owner: sorted({unit, *consumers, *overlap_names})})
        editor_shim = "crates/hex-ui/tests/editor_logic_unit/editor.rs"
        combined_paths = [source, "crates/hex-ui/src/editor/tracks.rs", tracks_shim]
        # The shared shim can register tests outside the known unit prefixes.
        self.assertEqual(plan.module_test_filters(root, [editor_shim], inventory, [owner]), {})
        self.assertEqual(plan.module_test_filters(root, [*combined_paths, editor_shim], inventory, [owner]), {})
        combined = plan.module_test_filters(root, combined_paths, inventory, [owner])[owner]
        self.assertIn("editor::tracks::", combined)
        self.assertNotIn("editor::", combined)
        self.assertTrue(all(any(consumer.startswith(selected) for selected in combined)
                            for consumer in consumers))
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                    "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                    "module_filters": {owner: combined}}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                  "boundary": boundary, "cargo_inventory": inventory,
                                  "paths": combined_paths})
        unit_step = next(step for step in steps if step.step_id == "ui-unit-unit_editor_logic")
        self.assertTrue(any(unit.startswith(selected) for selected in unit_step.argv))
        self.assertTrue(unit_step.requires_tests)
        feature = next(step for step in steps if step.step_id == "ui-feature-tests")
        for consumer in consumers:
            self.assertTrue(any(consumer.startswith(selected) for selected in feature.argv))

    def test_midi_trim_retained_window_unit_keeps_worker_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []},
                      "dependencies": [], "targets": [{"kind": ["lib"]}]}]
        unit = "editor::tracks::midi_trim_unit_tests::"
        test = "editor::tracks::midi_trim_unit_tests::split_midi_trim_preview_keeps_the_retained_source_window"
        consumers = (
            "editor::clip_drag_ui_test::rendered_midi_trim_previews_note_geometry_and_commits_release_snap",
            "editor::clip_drag_ui_test::rendered_midi_left_trim_keeps_source_note_timeline_position",
        )
        unit_source = "crates/hex-ui/src/editor/tracks/midi_trim_unit_tests.rs"
        modules = plan.module_paths(root / "crates/hex-ui/src")
        self.assertEqual(modules[(root / unit_source).resolve()],
                         "editor::tracks::midi_trim_unit_tests")
        for source in ("crates/hex-ui/src/editor/tracks.rs", unit_source):
            boundary = plan.boundary_selection(root, [source], inventory, [owner])
            self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
            steps = {step.step_id: step for step in ci.planned_steps({
                "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                "cargo_inventory": inventory, "paths": [source]})}
            unit_step = steps["ui-default-lib-unit-tests"]
            self.assertTrue(any(test.startswith(selected) or selected.startswith(test)
                                or selected == unit for selected in unit_step.argv))
            feature = steps["ui-feature-tests"]
            for consumer in consumers:
                self.assertTrue(any(consumer.startswith(selected) for selected in feature.argv))

        activation_source = "crates/hex-ui/src/editor/activation_ui_test.rs"
        activation_boundary = plan.boundary_selection(root, [activation_source], inventory, [owner])
        activation_steps = {step.step_id: step for step in ci.planned_steps({
            "scope": "rust", "affected_packages": [owner], "boundary": activation_boundary,
            "cargo_inventory": inventory, "paths": [activation_source]})}
        self.assertIn("editor::activation_ui_test::", activation_steps["ui-feature-tests"].argv)
        self.assertFalse(any(step.step_id == "ui-default-lib-unit-tests"
                             for step in activation_steps.values()))

        consumer_source = "crates/hex-ui/src/editor/clip_drag_ui_test.rs"
        consumer_boundary = plan.boundary_selection(root, [consumer_source], inventory, [owner])
        consumer_steps = {step.step_id: step for step in ci.planned_steps({
            "scope": "rust", "affected_packages": [owner], "boundary": consumer_boundary,
            "cargo_inventory": inventory, "paths": [consumer_source]})}
        for consumer in consumers:
            self.assertTrue(any(consumer.startswith(selected)
                                for selected in consumer_steps["ui-feature-tests"].argv))

    def test_group_overlap_keeps_arrangement_and_audio_event_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        source = "crates/hex-ui/src/editor/tracks/group_overlap.rs"
        unit = "editor::tracks::group_overlap::"
        paint = "crates/hex-ui/src/editor/audio_event_editor/paint.rs"
        paint_unit = ("editor::audio_event_editor::paint::tests::"
                      "replacement_accents_follow_occupied_victims_and_range_overwrite_semantics")
        consumers = [
            "editor::drag::group_drag_ui_test::group_overlap_is_passive_clipped_and_excludes_selected_originals_in_both_modes",
            "editor::drag::group_drag_ui_test::review_group_landing_paints_unselected_overwrite_victim",
            "editor::audio_event_editor::workflow_tests::event_overlap_accent_covers_only_replaced_audio_and_survives_release",
        ]
        for path in (source, paint):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                if path == source:
                    self.assertEqual(boundary["module_filters"],
                                     {owner: sorted([unit, paint_unit, *consumers])})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                standalone = steps["ui-unit-unit_editor_logic"]
                self.assertIn(unit, standalone.argv)
                self.assertTrue(standalone.requires_tests)
                default = steps["ui-default-lib-unit-tests"]
                self.assertTrue(default.requires_tests)
                self.assertTrue(any(paint_unit.startswith(prefix) for prefix in default.argv))
                feature = steps["ui-feature-tests"].argv
                self.assertTrue(all(any(name.startswith(prefix) for prefix in feature) for name in consumers))
                self.assertNotIn("editor::", feature)
        for path in ("tracks.rs", "group_drag_ui_test.rs", "audio_event_editor/paint.rs",
                     "audio_event_editor/workflow_tests.rs"):
            selected = plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + path], inventory, [owner])
            self.assertTrue(any(unit.startswith(name) for name in selected[owner]))

    def test_arrangement_section_insertion_keeps_unit_and_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [
                          {"name": "hex_ui", "kind": ["lib"], "crate_types": ["lib"],
                           "src_path": str(root / "crates/hex-ui/src/lib.rs"), "edition": "2024",
                           "test": True, "doc": True, "doctest": True},
                          {"name": "unit_section", "kind": ["test"], "crate_types": ["bin"],
                           "src_path": "tests/unit_section.rs", "edition": "2024", "test": True,
                           "doc": False, "doctest": False, "required-features": ["ui-unit-tier"]},
                      ]}]
        unit = (
            "editor::arrangement_sections::tests::"
            "section_insertion_uses_edges_and_does_not_move_a_clicked_source"
        )
        consumers = (
            "editor::arrangement_sections::tests::first_section_can_be_inserted_after_last",
            "editor::arrangement_sections::tests::painted_section_release_uses_stamped_queue_and_keeps_success_preview",
            "editor::arrangement_sections::tests::stationary_non_grid_content_edge_click_does_not_change_music",
        )
        for source in (
            "crates/hex-ui/src/editor/arrangement_sections.rs",
            "crates/hex-ui/src/editor/arrangement_sections/tests.rs",
        ):
            with self.subTest(source=source):
                boundary = plan.boundary_selection(root, [source], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                filters = boundary["module_filters"][owner]
                self.assertTrue(any(unit.startswith(prefix) for prefix in filters), filters)
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [source]})}
                default = steps["ui-default-lib-unit-tests"]
                self.assertTrue(default.requires_tests)
                self.assertIn(unit, default.argv)
                self.assertNotIn("--features", default.argv)
                feature = steps["ui-feature-tests"]
                self.assertIn("ui-test-support", feature.argv)
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in feature.argv), consumer)

        # Both real helper source owners select the composed unit, rendered witnesses,
        # and their existing standalone projection targets.
        for path, prefix in (
            ("crates/hex-ui/src/section_appearance.rs", "section_appearance::"),
            ("crates/hex-ui/src/section_reorder.rs", "section_reorder::"),
        ):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                filters = boundary["module_filters"][owner]
                for selected in (prefix, unit, *consumers):
                    self.assertIn(selected, filters)
                default = steps["ui-default-lib-unit-tests"]
                self.assertTrue(default.requires_tests)
                self.assertIn(unit, default.argv)
                feature = steps["ui-feature-tests"].argv
                for consumer in consumers:
                    self.assertIn(consumer, feature)
                self.assertNotIn("editor::arrangement_sections::tests::", feature)
                standalone = steps["ui-unit-unit_section"]
                self.assertTrue(standalone.requires_tests)
                self.assertIn("section_appearance::", standalone.argv)
                self.assertIn("section_reorder::", standalone.argv)

        projection = "crates/hex-ui/tests/unit_section.rs"
        boundary = plan.boundary_selection(root, [projection], inventory, [owner])
        self.assertEqual(boundary["ui_standalone_unit_targets"], {"unit_section": []})
        self.assertEqual(boundary["module_filters"], {})
        steps = {step.step_id: step for step in ci.planned_steps({
            "scope": "rust", "affected_packages": [owner], "boundary": boundary,
            "cargo_inventory": inventory, "paths": [projection]})}
        self.assertIn("ui-unit-unit_section", steps)
        self.assertNotIn("ui-unit-unit_editor_logic", steps)
        self.assertEqual(steps["ui-unit-unit_section"].argv,
                         ("cargo", "test", "-p", "hex-ui", "--features", "ui-unit-tier",
                          "--test", "unit_section", "--locked"))

    def test_section_hit_unit_target_follows_its_owner(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        names = ["section_hit::"]
        self.assertEqual(plan.module_test_filters(root, ["crates/hex-ui/src/section_hit.rs"], inventory, [owner]),
                         {owner: names})
        reorder = sorted([
            "section_appearance::",
            "section_reorder::",
            "editor::arrangement_sections::tests::section_insertion_uses_edges_and_does_not_move_a_clicked_source",
            "editor::arrangement_sections::tests::first_section_can_be_inserted_after_last",
            "editor::arrangement_sections::tests::painted_section_release_uses_stamped_queue_and_keeps_success_preview",
            "editor::arrangement_sections::tests::stationary_non_grid_content_edge_click_does_not_change_music",
        ])
        for path in ("section_appearance.rs", "section_reorder.rs"):
            self.assertEqual(plan.module_test_filters(root, ["crates/hex-ui/src/" + path], inventory, [owner]),
                             {owner: reorder})
        for name in ("section_input", "section_resize", "section_selection"):
            self.assertEqual(plan.module_test_filters(root, ["crates/hex-ui/src/" + name + ".rs"], inventory, [owner]),
                             {owner: [name + "::"]})
        self.assertEqual(plan.module_test_filters(root, ["crates/hex-ui/tests/unit_section.rs"], inventory, [owner]),
                         {owner: ["editor::arrangement_focus::tests::only_selected_sections_in_q_focus_receive_the_pattern",
                                  "editor::arrangement_focus::visibility::",
                                  "editor::arrangement_main_track_ui_test::clips_and_main_sections_share_grid_endpoints_through_fractional_pan_and_resize",
                                  "editor::arrangement_map::model::",
                                  "editor::arrangement_map::tests::main_lane_paints_canonical_intervals_at_the_ruler_scale",
                                  "editor::arrangement_sections::tests::section_has_opaque_chamfered_fill_and_compact_header_across_palettes_and_layouts",
                                  "editor::arrangement_sections::tests::section_lower_body_paints_all_tracks_without_section_edit_even_when_focused",
                                  "editor::group_overview::model::", "editor::group_overview::tests::",
                                  "section_appearance::", "section_hit::", "section_input::", "section_reorder::",
                                  "section_resize::", "section_selection::", "section_surface::focus::geometry::",
                                  "section_surface::focus::tests::theme_hatch_style_produces_bounded_visible_segments",
                                  "section_surface::geometry::"]})
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
                             "unit_editor_logic" in step.argv or "unit_timeline" in step.argv or
                             "unit_piano_roll" in step.argv for step in steps))
        self.assertTrue(any(step.step_id == "ui-section-unit-tests" for step in ci.planned_steps({"scope": "full"})))
        health = ci.planned_steps({"scope": "rust", "health": {"python": False, "workspace_inputs": True, "packages": []}})
        self.assertTrue(any(step.step_id == "health-ui-section-unit" for step in health))

    def test_section_surface_geometry_keeps_actual_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "section_surface::geometry::"
        consumers = [
            "editor::arrangement_sections::tests::section_has_opaque_chamfered_fill_and_compact_header_across_palettes_and_layouts",
            "editor::arrangement_main_track_ui_test::clips_and_main_sections_share_grid_endpoints_through_fractional_pan_and_resize",
            "editor::group_overview::tests::native_group_preview_tracks_viewport_content_removal_and_undo",
            "editor::arrangement_map::tests::main_lane_paints_canonical_intervals_at_the_ruler_scale",
        ]
        paths = [
            ("crates/hex-ui/src/section_surface/geometry.rs", "section_surface::geometry"),
            ("crates/hex-ui/src/section_surface.rs", "section_surface"),
            ("crates/hex-ui/src/editor/arrangement_sections.rs", "editor::arrangement_sections"),
            ("crates/hex-ui/src/editor/arrangement_main_track_ui_test.rs",
             "editor::arrangement_main_track_ui_test"),
            ("crates/hex-ui/src/editor/group_overview.rs", "editor::group_overview"),
            ("crates/hex-ui/src/editor/arrangement_map.rs", "editor::arrangement_map"),
        ]
        modules = plan.module_paths(root / "crates/hex-ui/src")
        for path, module in paths:
            self.assertEqual(modules[(root / path).resolve()], module)
            names = plan.module_test_filters(root, [path], inventory, [owner])[owner]
            self.assertTrue(any(unit.startswith(selected) for selected in names), path)
            for consumer in consumers:
                self.assertTrue(any(consumer.startswith(selected) for selected in names),
                                f"{path} does not select {consumer}: {names}")
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                        "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                        "module_filters": {owner: names}}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                      "boundary": boundary, "cargo_inventory": inventory,
                                      "paths": [path]})
            unit_step = next(step for step in steps if "unit_section" in step.argv)
            self.assertTrue(unit_step.requires_tests)
            self.assertTrue(any(unit.startswith(selected) for selected in unit_step.argv))
            feature = next(step for step in steps if step.step_id == "ui-feature-tests")
            for consumer in consumers:
                self.assertTrue(any(consumer.startswith(selected) for selected in feature.argv),
                                f"{path} feature step does not select {consumer}")

    def test_section_focus_geometry_keeps_q_focus_rendered_consumer(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "section_surface::focus::geometry::"
        theme_unit = "section_surface::focus::tests::theme_hatch_style_produces_bounded_visible_segments"
        consumers = [
            "editor::arrangement_focus::tests::only_selected_sections_in_q_focus_receive_the_pattern",
        ]
        paths = [
            ("crates/hex-ui/src/section_surface/focus/geometry.rs", "section_surface::focus::geometry"),
            ("crates/hex-ui/src/section_surface/focus.rs", "section_surface::focus"),
            ("crates/hex-ui/src/section_surface.rs", "section_surface"),
            ("crates/hex-ui/src/editor/arrangement_focus/tests.rs", "editor::arrangement_focus::tests"),
            ("crates/hex-ui/src/editor/arrangement_focus.rs", "editor::arrangement_focus"),
        ]
        modules = plan.module_paths(root / "crates/hex-ui/src")
        for path, module in paths:
            self.assertEqual(modules[(root / path).resolve()], module)
            boundary = plan.boundary_selection(root, [path], inventory, [owner])
            self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
            names = boundary["module_filters"][owner]
            if path == paths[0][0]:
                self.assertEqual(names, sorted([unit, theme_unit, *consumers]))
            self.assertTrue(any(unit.startswith(selected) for selected in names),
                            f"{path} does not select {unit}: {names}")
            for consumer in consumers:
                self.assertTrue(any(consumer.startswith(selected) for selected in names),
                                f"{path} does not select {consumer}: {names}")
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                      "boundary": boundary, "cargo_inventory": inventory,
                                      "paths": [path]})
            unit_step = next(step for step in steps if "unit_section" in step.argv)
            self.assertTrue(unit_step.requires_tests)
            self.assertTrue(any(unit.startswith(selected) for selected in unit_step.argv))
            default = next(step for step in steps if step.step_id == "ui-default-lib-unit-tests")
            self.assertTrue(default.requires_tests)
            self.assertTrue(any(theme_unit.startswith(selected) for selected in default.argv))
            focus = next(step for step in steps if step.step_id == "ui-consumer-track_group_focus")
            self.assertTrue(focus.requires_tests)
            self.assertIn("track_group_focus", focus.argv)
            feature = next(step for step in steps if step.step_id == "ui-feature-tests")
            for consumer in consumers:
                self.assertTrue(any(consumer.startswith(selected) for selected in feature.argv))

    def test_section_launch_geometry_keeps_action_and_autoplay_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::section_launch::geometry::"
        consumers = [
            "editor::arrangement_sections::tests::launch_action_geometry_matches_raw_exclusion_without_enlarging_tiny_sections",
            "editor::arrangement_sections::tests::section_autoplay_click_dispatches_stateless_loop_and_toggle_off_cancels",
        ]
        parent = "editor::section_launch::"
        paths = [
            ("crates/hex-ui/src/editor/section_launch/geometry.rs", "editor::section_launch::geometry"),
            ("crates/hex-ui/src/editor/section_launch.rs", "editor::section_launch"),
            ("crates/hex-ui/src/editor/arrangement_sections.rs", "editor::arrangement_sections"),
        ]
        modules = plan.module_paths(root / "crates/hex-ui/src")
        for path, module in paths:
            self.assertEqual(modules[(root / path).resolve()], module)
            names = plan.module_test_filters(root, [path], inventory, [owner])[owner]
            if path == paths[0][0]:
                self.assertEqual(names, sorted([unit, *consumers]))
            self.assertTrue(any(unit.startswith(selected) for selected in names),
                            f"{path} does not select {unit}: {names}")
            for consumer in consumers:
                self.assertTrue(any(consumer.startswith(selected) for selected in names),
                                f"{path} does not select {consumer}: {names}")
            if path == paths[1][0]:
                self.assertIn(parent, names)
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                        "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                        "module_filters": {owner: names}}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                      "boundary": boundary, "cargo_inventory": inventory,
                                      "paths": [path]})
            unit_step = next(step for step in steps if "unit_editor_logic" in step.argv)
            self.assertTrue(unit_step.requires_tests)
            self.assertTrue(any(unit.startswith(selected) for selected in unit_step.argv))
            if path == paths[1][0]:
                self.assertIn(parent, unit_step.argv)
            feature = next(step for step in steps if step.step_id == "ui-feature-tests")
            for consumer in consumers:
                self.assertTrue(any(consumer.startswith(selected) for selected in feature.argv))

    def test_track_latency_presentation_keeps_header_render_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::track_latency::presentation::"
        consumers = [
            "editor::track_latency::tests::view_action_mounts_the_indicator_in_actual_headers",
            "editor::track_performance::tests::view_actions_fit_independent_indicators_in_arrangement_and_console_headers",
        ]
        paths = [
            ("crates/hex-ui/src/editor/track_latency/presentation.rs",
             "editor::track_latency::presentation"),
            ("crates/hex-ui/src/editor/track_latency.rs", "editor::track_latency"),
            ("crates/hex-ui/src/editor/track_performance/tests.rs",
             "editor::track_performance::tests"),
            ("crates/hex-ui/src/editor/track_performance.rs", "editor::track_performance"),
        ]
        modules = plan.module_paths(root / "crates/hex-ui/src")
        for path, module in paths:
            self.assertEqual(modules[(root / path).resolve()], module)
            names = plan.module_test_filters(root, [path], inventory, [owner])[owner]
            if path == paths[0][0]:
                self.assertEqual(names, sorted([unit, *consumers]))
            if path == paths[1][0]:
                self.assertIn("editor::track_latency::", names)
            if path == paths[3][0]:
                self.assertIn("editor::track_performance::", names)
            self.assertTrue(any(unit.startswith(selected) for selected in names),
                            f"{path} does not select {unit}: {names}")
            for consumer in consumers:
                self.assertTrue(any(consumer.startswith(selected) for selected in names),
                                f"{path} does not select {consumer}: {names}")
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                        "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                        "module_filters": {owner: names}}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                      "boundary": boundary, "cargo_inventory": inventory,
                                      "paths": [path]})
            unit_step = next(step for step in steps if "unit_editor_logic" in step.argv)
            self.assertTrue(unit_step.requires_tests)
            self.assertTrue(any(unit.startswith(selected) for selected in unit_step.argv))
            if path == paths[1][0]:
                self.assertIn("editor::track_latency::", unit_step.argv)
            feature = next(step for step in steps if step.step_id == "ui-feature-tests")
            for consumer in consumers:
                self.assertTrue(any(consumer.startswith(selected) for selected in feature.argv),
                                f"{path} feature step does not select {consumer}")

    def test_stretch_preview_resolver_keeps_resize_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::trim::stretch_preview::resolver::"
        consumers = [
            "editor::trim::stretch_limit_tests::",
            "editor::trim::resize_release_tests::shift_resize_preserves_visible_extent_through_release_and_real_worker_ack",
            "editor::clip_drag_ui_test::shift_edge_stretch_previews_live_and_stationary_modifier_changes_dispatch_one_edit",
            "editor::drag_autoscroll::resize_pan_tests::resize_pan_preserves_stationary_snap_and_shift_stretch_preview_release",
            "editor::drag_autoscroll::resize_pan_tests::inward_pan_then_supported_stretch_release_commits_once_and_undoes",
        ]
        paths = [
            ("crates/hex-ui/src/editor/stretch_preview/resolver.rs",
             "editor::trim::stretch_preview::resolver"),
            ("crates/hex-ui/src/editor/trim.rs", "editor::trim"),
            ("crates/hex-ui/src/editor/stretch_preview.rs", "editor::trim::stretch_preview"),
            ("crates/hex-ui/src/editor/stretch_limit_tests.rs", "editor::trim::stretch_limit_tests"),
            ("crates/hex-ui/src/editor/resize_release_tests.rs", "editor::trim::resize_release_tests"),
            ("crates/hex-ui/src/editor/clip_drag_ui_test.rs", "editor::clip_drag_ui_test"),
            ("crates/hex-ui/src/editor/resize_pan_tests.rs",
             "editor::drag_autoscroll::resize_pan_tests"),
            ("crates/hex-ui/src/editor/drag_autoscroll.rs", "editor::drag_autoscroll"),
        ]
        modules = plan.module_paths(root / "crates/hex-ui/src")
        for path, module in paths:
            self.assertEqual(modules[(root / path).resolve()], module)
            names = plan.module_test_filters(root, [path], inventory, [owner])[owner]
            if path == paths[0][0]:
                self.assertEqual(names, sorted([unit, *consumers]))
            if path == paths[1][0]:
                self.assertIn("editor::trim::", names)
            if path == paths[2][0]:
                self.assertIn("editor::trim::stretch_preview::", names)
            self.assertTrue(any(unit.startswith(selected) for selected in names),
                            f"{path} does not select {unit}: {names}")
            for consumer in consumers:
                self.assertTrue(any(consumer.startswith(selected) for selected in names),
                                f"{path} does not select {consumer}: {names}")
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                        "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                        "module_filters": {owner: names}}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                      "boundary": boundary, "cargo_inventory": inventory,
                                      "paths": [path]})
            unit_step = next(step for step in steps if "unit_editor_logic" in step.argv)
            self.assertTrue(unit_step.requires_tests)
            self.assertTrue(any(unit.startswith(selected) for selected in unit_step.argv))
            if path in (paths[1][0], paths[2][0]):
                self.assertTrue(any(selected.startswith("editor::trim::")
                                    for selected in unit_step.argv))
            feature = next(step for step in steps if step.step_id == "ui-feature-tests")
            for consumer in consumers:
                self.assertTrue(any(consumer.startswith(selected) for selected in feature.argv),
                                f"{path} feature step does not select {consumer}")

    def test_device_display_unit_target_follows_its_owner(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        names = ["editor::device_panel::numeric_display::"]
        self.assertEqual(plan.module_test_filters(root, ["crates/hex-ui/src/editor/device_panel/numeric_display.rs"], inventory, [owner]),
                         {owner: names})
        eq = "editor::device_panel::eq_panel::state_edit::"
        for path in ("crates/hex-ui/src/editor/device_panel/eq_panel/state_edit.rs",
                     "crates/hex-ui/src/editor/device_panel/eq_panel/state_edit/legacy.rs"):
            self.assertEqual(plan.module_test_filters(root, [path], inventory, [owner]), {owner: [eq]})
        selection = "editor::device_panel::eq_panel::selection::"
        slope = "editor::device_panel::eq_panel::slope::"
        group = "editor::device_panel::eq_panel::group_move::"
        gestures = "editor::device_panel::eq_panel::gestures::"
        rendered_group = "editor::device_panel::tests::eq_ui_tests::marquee_tests::eq_group_drag_moves_every_selected_node_and_undoes_in_one_step"
        for path in ("group_move.rs", "gestures.rs"):
            self.assertEqual(plan.module_test_filters(root, ["crates/hex-ui/src/editor/device_panel/eq_panel/" + path],
                                                      inventory, [owner]), {owner: sorted([gestures, group, rendered_group])})
        spectrum = "editor::device_panel::spectrum_display::"
        self.assertEqual(plan.module_test_filters(root, ["crates/hex-ui/src/editor/device_panel/spectrum_display.rs"],
                                                  inventory, [owner]), {owner: [spectrum]})
        for path, name in (("selection.rs", selection), ("slope.rs", slope)):
            self.assertEqual(plan.module_test_filters(root, ["crates/hex-ui/src/editor/device_panel/eq_panel/" + path],
                                                      inventory, [owner]), {owner: [name]})
        self.assertEqual(plan.module_test_filters(root, ["crates/hex-ui/tests/unit_device.rs"], inventory, [owner]),
                         {owner: [gestures, group, selection, slope, eq, *names, spectrum,
                                  "editor::device_panel::vst3_parameters::"]})
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner], "lint_packages": [owner],
                    "single_run": {owner: "ui-test-support"}, "module_filters": {owner: names}}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": []})
        self.assertEqual(sum("unit_device" in step.argv for step in steps), 1)
        boundary["module_filters"] = {owner: [eq]}
        eq_steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                     "cargo_inventory": inventory, "paths": []})
        self.assertEqual(sum("unit_device" in step.argv for step in eq_steps), 1)
        self.assertTrue(any(eq in step.argv for step in eq_steps if "unit_device" in step.argv))
        for name in (group, selection, slope, spectrum):
            boundary["module_filters"] = {owner: [name]}
            scoped = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                       "cargo_inventory": inventory, "paths": []})
            self.assertTrue(any(name in step.argv for step in scoped if "unit_device" in step.argv))
        self.assertFalse(any("unit_automation" in step.argv or "unit_editor_logic" in step.argv or
                             "unit_timeline" in step.argv or "unit_piano_roll" in step.argv for step in steps))
        self.assertTrue(any(step.step_id == "ui-device-unit-tests" for step in ci.planned_steps({"scope": "full"})))
        health = ci.planned_steps({"scope": "rust", "health": {"python": False, "workspace_inputs": True, "packages": []}})
        self.assertTrue(any(step.step_id == "health-ui-device-unit" for step in health))

    def test_eq_group_capacity_unit_keeps_rendered_undo_and_standalone_group_move(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::device_panel::eq_panel::gestures::tests::"
        witness = "editor::device_panel::tests::eq_ui_tests::marquee_tests::eq_group_drag_moves_every_selected_node_and_undoes_in_one_step"
        for path in ("crates/hex-ui/src/editor/device_panel/eq_panel/gestures.rs",
                     "crates/hex-ui/src/editor/device_panel/eq_panel/group_move.rs"):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                default = steps["ui-default-lib-unit-tests"]
                self.assertTrue(default.requires_tests)
                self.assertIn(unit, default.argv)
                self.assertIn("editor::device_panel::eq_panel::group_move::", steps["ui-unit-unit_device"].argv)
                self.assertIn(witness, steps["ui-feature-tests"].argv)

    def test_modulation_cycle_unit_keeps_rendered_audio_rate_mapping(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::device_panel::modulators::tests::"
        witness = "editor::device_panel::tests::modulation_panel_adds_and_maps_an_audio_rate_modulator"
        path = "crates/hex-ui/src/editor/device_panel/modulators.rs"
        boundary = plan.boundary_selection(root, [path], inventory, [owner])
        self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
        steps = {step.step_id: step for step in ci.planned_steps({
            "scope": "rust", "affected_packages": [owner], "boundary": boundary,
            "cargo_inventory": inventory, "paths": [path]})}
        default = steps["ui-default-lib-unit-tests"]
        self.assertTrue(default.requires_tests)
        self.assertIn(unit, default.argv)
        self.assertIn(witness, steps["ui-feature-tests"].argv)
        self.assertNotIn("editor::device_panel::", steps["ui-feature-tests"].argv)

    def test_rattle_diamond_unit_keeps_rendered_panel_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        path = "crates/hex-ui/src/editor/device_panel/rattle_panel.rs"
        unit = "editor::device_panel::rattle_panel::tests::"
        rendered = (
            "editor::device_panel::tests::rattle_ui_tests::rattle_display_reads_authored_automation_and_manual_draft_precedence",
            "editor::device_panel::tests::rattle_ui_tests::rattle_track_return_panel_fits_and_drive_draft_commits_one_undo",
        )
        boundary = plan.boundary_selection(root, [path], inventory, [owner])
        self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
        self.assertIn("editor::device_panel::rattle_panel::", boundary["module_filters"][owner])
        steps = {step.step_id: step for step in ci.planned_steps({
            "scope": "rust", "affected_packages": [owner], "boundary": boundary,
            "cargo_inventory": inventory, "paths": [path]})}
        default = steps["ui-default-lib-unit-tests"]
        self.assertTrue(default.requires_tests)
        self.assertIn(unit, default.argv)
        feature = steps["ui-feature-tests"]
        for name in rendered:
            self.assertIn(name, feature.argv)

    def test_sampler_cursor_mapping_unit_keeps_exact_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        path = "crates/hex-ui/src/editor/device_panel/sampler_cursor_tests.rs"
        unit = "editor::device_panel::sampler_overview::tests::sampler_cursor_original_frame_mapping_bounds_and_time"
        rendered = (
            "editor::device_panel::sampler_overview::tests::sampler_cursor_clicks_original_frames_without_project_or_audio_edits",
            "editor::device_panel::sampler_overview::tests::sampler_cursor_source_replacement_undo_and_stale_click",
        )
        boundary = plan.boundary_selection(root, [path], inventory, [owner])
        self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
        self.assertEqual(boundary["module_filters"][owner], ["editor::device_panel::sampler_overview::tests::"])
        steps = {step.step_id: step for step in ci.planned_steps({
            "scope": "rust", "affected_packages": [owner], "boundary": boundary,
            "cargo_inventory": inventory, "paths": [path]})}
        default = steps["ui-default-lib-unit-tests"]
        self.assertTrue(default.requires_tests)
        self.assertIn(unit, default.argv)
        self.assertNotIn("editor::device_panel::sampler_overview::tests::", default.argv)
        feature = steps["ui-feature-tests"]
        support_prefix = "editor::device_panel::sampler_overview::tests::"
        self.assertIn(support_prefix, feature.argv)
        for name in rendered:
            self.assertTrue(name.startswith(support_prefix))

    def test_vst3_filesystem_cache_selects_support_without_default_or_projection_units(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        path = "crates/hex-ui/src/editor/device_panel/vst3_parameters/cache.rs"
        boundary = plan.boundary_selection(root, [path], inventory, [owner])
        self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
        prefix = "editor::device_panel::vst3_parameters::cache::"
        self.assertEqual(boundary["module_filters"], {owner: [prefix]})
        steps = {step.step_id: step for step in ci.planned_steps({
            "scope": "rust", "affected_packages": [owner], "boundary": boundary,
            "cargo_inventory": inventory, "paths": [path]})}
        self.assertNotIn("ui-default-lib-unit-tests", steps)
        self.assertNotIn("module-tests-hex-ui", steps)
        self.assertFalse(any("unit_device" in step.argv for step in steps.values()))
        self.assertIn(prefix, steps["ui-feature-tests"].argv)

    def test_vst3_scrollbar_thumb_unit_keeps_projection_and_rendered_panel(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        owner_path = "crates/hex-ui/src/editor/device_panel/vst3_parameters/scrollbar.rs"
        consumer_path = "crates/hex-ui/src/editor/device_panel/vst3_parameters.rs"
        projection_path = "crates/hex-ui/src/editor/device_panel/vst3_parameters/row_projection.rs"
        unit = "editor::device_panel::vst3_parameters::scrollbar::tests::thumb_clamps_extents_and_rejects_invalid_geometry"
        projection = "editor::device_panel::vst3_parameters::row_projection::"
        rendered = "editor::device_panel::vst3_parameters::tests::vst3_header_and_list_bounds_stay_fixed_across_touch_states"
        for path in (owner_path, consumer_path):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                default = steps["ui-default-lib-unit-tests"]
                self.assertTrue(default.requires_tests)
                self.assertIn(unit, default.argv)
                feature = steps["ui-feature-tests"].argv
                self.assertTrue(any(rendered.startswith(prefix) for prefix in feature))
                self.assertNotIn("editor::", feature)
                self.assertNotIn("editor::device_panel::", feature)

        boundary = plan.boundary_selection(root, [projection_path], inventory, [owner])
        self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
        steps = {step.step_id: step for step in ci.planned_steps({
            "scope": "rust", "affected_packages": [owner], "boundary": boundary,
            "cargo_inventory": inventory, "paths": [projection_path]})}
        standalone = steps["ui-unit-unit_device"].argv
        self.assertTrue(steps["ui-unit-unit_device"].requires_tests)
        self.assertTrue(any(projection.startswith(prefix) for prefix in standalone))
        self.assertIn("editor::device_panel::vst3_parameters::", steps["ui-feature-tests"].argv)

    def test_vst3_row_projection_selects_unit_and_panel_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        name = "editor::device_panel::vst3_parameters::"
        for path in ("crates/hex-ui/src/editor/device_panel/vst3_parameters.rs",
                     "crates/hex-ui/src/editor/device_panel/vst3_parameters/row_projection.rs"):
            self.assertEqual(plan.module_test_filters(root, [path], inventory, [owner]), {owner: [name]})
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                        "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                        "module_filters": {owner: [name]}}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                      "cargo_inventory": inventory, "paths": [path]})
            unit = next(step for step in steps if "unit_device" in step.argv)
            integration = next(step for step in steps if step.step_id == "ui-feature-tests")
            self.assertIn(name, unit.argv)
            self.assertTrue(unit.requires_tests)
            self.assertIn(name, integration.argv)

    def test_submission_observation_unit_keeps_refusal_consumer(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        path = "crates/hex-ui/src/editor/controller/submission.rs"
        unit = "editor::controller::submission::observation_tests::"
        consumer = "editor::controller::submission::tests::a_refused_request_leaves_identical_custody_on_both_paths"
        boundary = plan.boundary_selection(root, [path], inventory, [owner])
        self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
        self.assertIn("editor::controller::submission::", boundary["module_filters"][owner])
        steps = {step.step_id: step for step in ci.planned_steps({
            "scope": "rust", "affected_packages": [owner], "boundary": boundary,
            "cargo_inventory": inventory, "paths": [path]})}
        default = steps["ui-default-lib-unit-tests"]
        self.assertIn(unit, default.argv)
        self.assertTrue(default.requires_tests)
        feature = steps["ui-feature-tests"]
        self.assertIn("editor::controller::submission::", feature.argv)
        self.assertTrue(consumer.startswith("editor::controller::submission::"))

    def test_session_audio_playhead_selects_unit_and_panel_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        name = "editor::session_audio_panel::"
        for path in ("crates/hex-ui/src/editor/session_audio_panel.rs",
                     "crates/hex-ui/src/editor/session_audio_panel/playhead.rs"):
            self.assertEqual(plan.module_test_filters(root, [path], inventory, [owner]), {owner: [name]})
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                        "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                        "module_filters": {owner: [name]}}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                      "cargo_inventory": inventory, "paths": [path]})
            unit = next(step for step in steps if "unit_editor_logic" in step.argv)
            integration = next(step for step in steps if step.step_id == "ui-feature-tests")
            self.assertIn(name, unit.argv)
            self.assertTrue(unit.requires_tests)
            self.assertIn(name, integration.argv)

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
                             "unit_editor_logic" in step.argv for step in steps))
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
        self.assertEqual(plan.module_test_filters(root, ["crates/hex-ui/src/editor/session_route_view.rs"], inventory, [owner]),
                         {owner: names})
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner], "lint_packages": [owner],
                    "single_run": {owner: "ui-test-support"}, "module_filters": {owner: names}}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": []})
        self.assertEqual(sum("unit_editor_logic" in step.argv for step in steps), 1)
        self.assertTrue(any(names[0] in step.argv for step in steps if "unit_editor_logic" in step.argv))
        self.assertFalse(any("unit_timeline" in step.argv or "unit_piano_roll" in step.argv for step in steps))
        self.assertTrue(any(step.step_id == "ui-editor-logic-unit-tests" for step in ci.planned_steps({"scope": "full"})))
        health = ci.planned_steps({"scope": "rust", "health": {"python": False, "workspace_inputs": True, "packages": []}})
        self.assertTrue(any(step.step_id == "health-ui-editor-logic-unit" for step in health))

    def test_session_navigation_keeps_unit_and_window_coverage(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::session_navigation::"
        window = "editor::session_navigation_ui_test::"
        related = ["editor::controller::session_action_unit_tests::launch_and_transport_actions_have_explicit_play_intents",
                   "editor::controller::session_action_unit_tests::project_edit_backpressure_preserves_transport_but_invalidates_session_actions",
                   "editor::controller::session_action_unit_tests::close_prompt_allows_only_owned_duplicate_continuations",
                   "editor::controller::session_action_unit_tests::rejected_scene_duplicate_clears_only_its_completion_selection_intent",
                   "editor::delete_ui_tests::",
                   "editor::device_panel::tests::device_comparison_tests::device_comparison_metadata_pending_callback_then_applied_authoring_is_not_a_new_ack",
                   "editor::context_menu::tests::global_launch_dropdown_renders_all_choices_and_restores_focus_without_project_edits",
                   "editor::session_grid::actions::", "editor::session_grid::tests::", unit, window]
        for path in ("session_navigation.rs", "session_navigation_ui_test.rs"):
            names = plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + path], inventory, [owner])
            self.assertEqual(names, {owner: sorted(related)})
        self.assertEqual(plan.module_test_filters(root, ["crates/hex-ui/src/editor/mode.rs"], inventory, [owner]),
                         {owner: ["editor::", "output_meter::logic::", "output_meter::tests::",
                                  "section_surface::focus::geometry::",
                                  "section_surface::focus::tests::theme_hatch_style_produces_bounded_visible_segments",
                                  "section_surface::geometry::"]})
        action_cases = (
            "editor::controller::session_action_unit_tests::close_prompt_allows_only_owned_duplicate_continuations",
            "editor::controller::session_action_unit_tests::launch_and_transport_actions_have_explicit_play_intents",
            "editor::controller::session_action_unit_tests::project_edit_backpressure_preserves_transport_but_invalidates_session_actions",
            "editor::controller::session_action_unit_tests::rejected_scene_duplicate_clears_only_its_completion_selection_intent",
        )
        path = "crates/hex-ui/src/editor/session_navigation_ui_test.rs"
        boundary = plan.boundary_selection(root, [path], inventory, [owner])
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": []})
        self.assertTrue(any(unit in step.argv for step in steps if "unit_editor_logic" in step.argv))
        feature = next(step for step in steps if step.step_id == "ui-feature-tests")
        self.assertIn(window, feature.argv)
        self.assertIn("editor::session_grid::tests::", feature.argv)
        self.assertIn(
            "editor::context_menu::tests::global_launch_dropdown_renders_all_choices_and_restores_focus_without_project_edits",
            feature.argv,
        )
        for consumer in (
            "editor::session_navigation_ui_test::scene_launch_leads_the_caption_and_uses_the_guarded_offline_action",
            "editor::session_navigation_ui_test::session_scene_duplicate_and_reorder_keys_use_worker_edits",
            "editor::session_navigation_ui_test::failed_scene_duplicate_clears_its_pending_selection_intent",
        ):
            self.assertTrue(any(consumer.startswith(prefix) for prefix in feature.argv))

    def test_controller_correlation_keeps_unit_and_window_coverage(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::controller::audio_event_correlation::"
        window = "editor::controller::sampler_native_ui_protocol_tests::"
        names = plan.module_test_filters(root, ["crates/hex-ui/src/editor/controller/audio_event_correlation.rs"],
                                         inventory, [owner])
        source_unit = "editor::controller::source_binding_unit_tests::"
        self.assertEqual(names, {owner: [unit, window, source_unit]})
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner], "lint_packages": [owner],
                    "single_run": {owner: "ui-test-support"}, "module_filters": names}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": []})
        self.assertTrue(any(unit in step.argv for step in steps if "unit_editor_logic" in step.argv))
        self.assertTrue(any(window in step.argv for step in steps if step.step_id == "ui-feature-tests"))

    def test_import_admission_keeps_unit_and_window_coverage(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::audio_import_settings::admission::"
        parent = "editor::audio_import_settings::"
        window = "editor::track_defaults::"
        for path, expected in (("audio_import_settings.rs", [parent, window]),
                               ("audio_import_settings/admission.rs", [unit, window])):
            names = plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + path], inventory, [owner])
            self.assertEqual(names, {owner: expected})
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner], "lint_packages": [owner],
                        "single_run": {owner: "ui-test-support"}, "module_filters": names}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                      "cargo_inventory": inventory, "paths": []})
            self.assertTrue(any(expected[0] in step.argv for step in steps if "unit_editor_logic" in step.argv))
            self.assertTrue(any(window in step.argv for step in steps if step.step_id == "ui-feature-tests"))

    def test_device_panel_header_style_uses_default_lib_and_keeps_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::device_panel::header_style_unit_tests::"
        consumers = (
            "editor::device_panel::tests::header_style_tests::retained_source_path_indicator_is_rendered_with_the_existing_chain",
            "editor::device_panel::tests::header_style_tests::device_shell_outline_is_visible_rounded_and_tracks_selection_across_owners",
            "editor::device_panel::tests::sine_and_effect_place_their_shared_header_to_the_left_of_the_body",
        )
        for source in (
            "crates/hex-ui/src/editor/device_panel.rs",
            "crates/hex-ui/src/editor/device_panel/tests/header_style_tests.rs",
            "crates/hex-ui/src/editor/device_panel/return_view.rs",
        ):
            with self.subTest(source=source):
                boundary = plan.boundary_selection(root, [source], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [source]})}
                self.assertIn(unit, steps["ui-default-lib-unit-tests"].argv)
                chain = "editor::device_panel::chain_layout_unit_tests::"
                if source.endswith("/device_panel.rs"):
                    self.assertIn(chain, steps["ui-default-lib-unit-tests"].argv)
                if source.endswith("/header_style_tests.rs"):
                    self.assertNotIn(chain, steps["ui-default-lib-unit-tests"].argv)
                self.assertNotIn("--features", steps["ui-default-lib-unit-tests"].argv)
                feature_args = steps["ui-feature-tests"].argv
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in feature_args),
                                    (consumer, feature_args))

    def test_output_meter_contrast_uses_default_lib_and_keeps_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "output_meter::tests::unit_tests::"
        consumers = (
            "output_meter::tests::rendered::output_meter_uses_shared_level_colors_and_click_clear_clipping_caps",
            "output_meter::tests::rendered::fixed_meter_slot_at_minimum_window_preserves_neighbors_and_actual_fill_geometry",
            "editor::output_meter_ui_test::output_meter_status_snapshot_reaches_actual_editor_at_minimum_size",
        )
        for source in (
            "crates/hex-ui/src/output_meter.rs",
            "crates/hex-ui/src/output_meter/tests.rs",
            "crates/hex-ui/src/output_meter/tests/rendered.rs",
            "crates/hex-ui/src/output_meter/logic.rs",
        ):
            with self.subTest(source=source):
                boundary = plan.boundary_selection(root, [source], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [source]})}
                self.assertIn(unit, steps["ui-default-lib-unit-tests"].argv)
                self.assertNotIn("--features", steps["ui-default-lib-unit-tests"].argv)
                feature_args = steps["ui-feature-tests"].argv
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in feature_args),
                                    (consumer, feature_args))

    def test_clip_range_fit_units_keep_both_rendered_drag_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        for module in ("clip_range_fit", "midi_cue", "time_selection"):
            with self.subTest(module=module):
                source = f"crates/hex-ui/src/editor/{module}.rs"
                boundary = plan.boundary_selection(root, [source], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [source]})}
                self.assertIn("editor::clip_range_fit::tests::", steps["ui-default-lib-unit-tests"].argv)
                feature_args = steps["ui-feature-tests"].argv
                for consumer in (
                    "editor::clip_range_fit::ui_tests::dragged_selection_fits_the_audio_editor_to_the_selected_root_range",
                    "editor::clip_range_fit::ui_tests::dragged_selection_fits_the_piano_roll_to_the_selected_source_beats",
                ):
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in feature_args))

    def test_output_meter_logic_keeps_rendered_and_editor_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        for source in ("crates/hex-ui/src/output_meter/logic.rs",
                       "crates/hex-ui/src/output_meter/logic_tests.rs",
                       "crates/hex-ui/src/output_meter/tests.rs"):
            names = plan.module_test_filters(root, [source], inventory, [owner])
            self.assertEqual(names, {owner: METER_BOUNDARY})
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                        "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                        "module_filters": names}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                      "cargo_inventory": inventory, "paths": []})
            self.assertTrue(any("output_meter::logic::" in step.argv and step.requires_tests
                                for step in steps if "unit_editor_logic" in step.argv))
            library = next(step for step in steps if step.step_id == "ui-feature-tests")
            self.assertIn("output_meter::tests::", library.argv)
            self.assertIn("editor::output_meter_ui_test::", library.argv)
            self.assertIn("editor::refresh_state::", names[owner])
        parent = plan.module_test_filters(root, ["crates/hex-ui/src/output_meter.rs"], inventory, [owner])
        self.assertIn("output_meter::", parent[owner])
        self.assertIn("editor::output_meter_ui_test::", parent[owner])

    def test_waveform_cache_identity_keeps_profile_and_render_witnesses(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        rendered_consumers = (
            "editor::waveform::resize_tests::silent_left_prefix_keeps_waveform_and_geometry_through_real_worker_ack",
            "editor::browser::ui_test::browser_hover_and_selection_keep_selected_waveform_and_surroundings_stable",
        )
        profiler = "editor::clip_content_cache::profile::navigation::headless_navigation_framework_profile"
        cases = (
            ("crates/hex-ui/src/editor/waveform_geometry.rs", True,
             (*rendered_consumers, profiler)),
            ("crates/hex-ui/src/editor/clip_content_profile.rs", False, (profiler,)),
        )
        for path, runs_unit, consumers in cases:
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                if runs_unit:
                    default = steps["ui-default-lib-unit-tests"]
                    self.assertIn("editor::waveform_geometry::tests::", default.argv)
                    self.assertTrue(default.requires_tests)
                else:
                    self.assertNotIn("ui-default-lib-unit-tests", steps)
                feature_args = steps["ui-feature-tests"].argv
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in feature_args),
                                    (consumer, feature_args))

    def test_waveform_grid_selects_unit_geometry_and_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        grid = "editor::waveform_geometry::grid::"
        geometry = "editor::waveform_geometry::tests::"
        rendered = "editor::waveform::resize_tests::silent_left_prefix_keeps_waveform_and_geometry_through_real_worker_ack"
        for source in ("waveform_geometry/grid.rs", "viewport.rs"):
            names = plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + source], inventory, [owner])
            for required in (grid, geometry, rendered, *GRID_CONSUMERS):
                self.assertTrue(any(required.startswith(selected) for selected in names[owner]))
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                        "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                        "module_filters": names}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                      "cargo_inventory": inventory, "paths": []})
            self.assertTrue(any(any(grid.startswith(arg) for arg in step.argv) and step.requires_tests
                                for step in steps if "unit_editor_logic" in step.argv))
            cache_step = next(step for step in steps if step.step_id == "ui-default-lib-unit-tests")
            self.assertTrue(cache_step.requires_tests)
            self.assertIn(geometry, cache_step.argv)
            feature = next(step for step in steps if step.step_id == "ui-feature-tests")
            for required in (geometry, rendered, *GRID_CONSUMERS):
                self.assertTrue(any(required.startswith(arg) for arg in feature.argv))
        # A rendered consumer edit must select the new unit owner in reverse.
        rendered_source = plan.module_test_filters(
            root, ["crates/hex-ui/src/editor/waveform_resize_ui_test.rs"], inventory, [owner])
        self.assertTrue(any(grid.startswith(selected) for selected in rendered_source[owner]))
        self.assertTrue(any(geometry.startswith(selected) for selected in rendered_source[owner]))
        # A column-budget edit must still select the grid even when the broader
        # waveform geometry prefix subsumes its child in the final filter set.
        columns = plan.module_test_filters(root, ["crates/hex-ui/src/editor/waveform_columns.rs"], inventory, [owner])
        self.assertIn("editor::waveform_geometry::", columns[owner])
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                    "lint_packages": [owner], "single_run": {owner: "ui-test-support"}, "module_filters": columns}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": []})
        self.assertTrue(any("editor::waveform_geometry::" in step.argv
                            for step in steps if "unit_editor_logic" in step.argv))

    def test_loop_projection_keeps_real_waveform_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        for source in ("waveform_loop/projection.rs", "waveform_loop.rs",
                       "waveform_stretch/accepted_root.rs", "waveform_stretch.rs", "waveform_source.rs"):
            names = plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + source], inventory, [owner])
            for required in ("editor::waveform_loop::", "editor::waveform_stretch::",
                             "editor::waveform_source::", "editor::waveform_geometry::"):
                self.assertIn(required, names[owner])
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                        "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                        "module_filters": names}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                      "cargo_inventory": inventory, "paths": []})
            self.assertTrue(any("editor::waveform_loop::" in step.argv and step.requires_tests
                                for step in steps if "unit_editor_logic" in step.argv))
            feature = next(step for step in steps if step.step_id == "ui-feature-tests")
            self.assertIn("editor::waveform_loop::", feature.argv)
            self.assertIn("editor::waveform_geometry::", feature.argv)
            for consumer in (
                "editor::browser::ui_test::browser_hover_and_selection_keep_selected_waveform_and_surroundings_stable",
                "editor::sampler_parameter::tests::source_hints_and_paint_require_same_view_source_facts_and_context",
                "editor::audio_event_editor::tests::moved_and_pasted_waveform_positions_read_event_composed_pcm",
                "editor::audio_event_editor::free_placement_tests::finite_frame_drag_preserves_the_cycle_markers_and_root_waveform",
            ):
                self.assertIn(consumer, feature.argv)

    def test_accepted_loop_root_keeps_waveform_resize_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::waveform_loop::accepted_root::"
        parent = "editor::waveform_loop::"
        consumers = [
            "editor::waveform::resize_tests::shift_stretch_keeps_live_sample_varied_waveforms_visible",
            "editor::waveform::resize_tests::trim_keeps_sample_varied_waveforms_visible_with_warp_on_and_off",
        ]
        source_path = "crates/hex-ui/src/editor/waveform_loop/accepted_root.rs"
        parent_path = "crates/hex-ui/src/editor/waveform_loop.rs"
        consumer_path = "crates/hex-ui/src/editor/waveform_resize_ui_test.rs"
        modules = plan.module_paths(root / "crates/hex-ui/src")
        self.assertEqual(modules[(root / source_path).resolve()], "editor::waveform_loop::accepted_root")
        self.assertEqual(modules[(root / parent_path).resolve()], "editor::waveform_loop")
        self.assertEqual(modules[(root / consumer_path).resolve()], "editor::waveform::resize_tests")
        for path in (source_path, parent_path, consumer_path):
            names = plan.module_test_filters(root, [path], inventory, [owner])[owner]
            self.assertTrue(any(unit.startswith(selected) for selected in names))
            if path == parent_path:
                self.assertIn(parent, names)
            for consumer in consumers:
                self.assertTrue(any(consumer.startswith(selected) for selected in names))
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                        "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                        "module_filters": {owner: names}}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                      "boundary": boundary, "cargo_inventory": inventory,
                                      "paths": [path]})
            unit_step = next(step for step in steps if "unit_editor_logic" in step.argv)
            self.assertTrue(any(unit.startswith(selected) for selected in unit_step.argv))
            self.assertTrue(unit_step.requires_tests)
            if path == parent_path:
                self.assertIn(parent, unit_step.argv)
            feature = next(step for step in steps if step.step_id == "ui-feature-tests")
            for consumer in consumers:
                self.assertTrue(any(consumer.startswith(selected) for selected in feature.argv))

    def test_refresh_state_keeps_unit_and_window_coverage(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::refresh_state::"
        window = "editor::output_meter_ui_test::"
        names = plan.module_test_filters(root, ["crates/hex-ui/src/editor/refresh_state.rs"], inventory, [owner])
        self.assertEqual(names, {owner: METER_BOUNDARY})
        self.assertEqual(plan.module_test_filters(root, ["crates/hex-ui/src/editor/refresh.rs"], inventory, [owner]),
                         {owner: ["editor::", "output_meter::logic::", "output_meter::tests::",
                                  "section_surface::focus::geometry::",
                                  "section_surface::focus::tests::theme_hatch_style_produces_bounded_visible_segments",
                                  "section_surface::geometry::"]})
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner], "lint_packages": [owner],
                    "single_run": {owner: "ui-test-support"}, "module_filters": names}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": []})
        self.assertTrue(any(unit in step.argv for step in steps if "unit_editor_logic" in step.argv))
        self.assertTrue(any(window in step.argv for step in steps if step.step_id == "ui-feature-tests"))

    def test_timeline_target_follows_its_viewport_dependencies(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        timeline = ["editor::musical_time::", "editor::seek::", "editor::signed_frame_viewport::", "editor::viewport::"]
        for path in ("musical_time.rs", "seek.rs", "signed_frame_viewport.rs", "viewport.rs"):
            expected = sorted([*timeline, *GRID_CONSUMERS, *ARRANGEMENT_CONSUMERS]) if path == "viewport.rs" else timeline
            self.assertEqual(plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + path], inventory, [owner]),
                             {owner: expected})
        self.assertEqual(plan.module_test_filters(root, ["crates/hex-ui/tests/unit_timeline.rs"], inventory, [owner]),
                         {owner: ["editor::clip_range_fit::", "editor::midi_cue::", *timeline]})
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner], "lint_packages": [owner],
                    "single_run": {owner: "ui-test-support"}, "module_filters": {owner: timeline}}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": []})
        self.assertEqual(sum("unit_timeline" in step.argv for step in steps), 1)
        self.assertEqual(sum("unit_editor_state" in step.argv for step in steps), 1)
        self.assertTrue(any(step.step_id == "ui-timeline-unit-tests" for step in ci.planned_steps({"scope": "full"})))
        health = ci.planned_steps({"scope": "rust", "health": {"python": False, "workspace_inputs": True, "packages": []}})
        self.assertTrue(any(step.step_id == "health-ui-timeline-unit" for step in health))

    def test_device_chain_layout_units_keep_rendered_pointer_and_keyboard_cases(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit_prefixes = (
            "editor::device_panel::chain_layout_unit_tests::",
            "editor::device_panel::chain_window::tests::",
            "editor::device_panel::parameter_rows::tests::",
        )
        consumers = (
            "editor::device_panel::tests::chain_window_tests::short_chain_window_does_not_add_scroll_extent",
            "editor::device_panel::tests::clipped_overflow_card_cannot_restore_a_drop_preview_outside_the_rail",
            "editor::device_panel::tests::keyboard_navigation_tests::arrows_step_devices_and_paste_lands_in_the_clicked_gap",
            "editor::device_panel::tests::parameter_rows_tests::generic_track_effect_rows_preserve_extent_identity_and_pins",
        )
        paths = (
            "device_panel.rs",
            "device_panel/chain_window.rs",
            "device_panel/parameter_rows.rs",
            "device_panel/tests.rs",
            "device_panel/tests/chain_window_tests.rs",
            "device_panel/tests/keyboard_navigation_tests.rs",
            "device_panel/tests/parameter_rows_tests.rs",
        )
        for module in paths:
            path = "crates/hex-ui/src/editor/" + module
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                unit_args = steps["ui-default-lib-unit-tests"].argv
                for prefix in unit_prefixes:
                    self.assertIn(prefix, unit_args)
                self.assertNotIn("editor::device_panel::tests::", unit_args)
                feature_args = steps["ui-feature-tests"].argv
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in feature_args),
                                    f"{consumer} missing from {feature_args}")

    def test_browser_scalar_controls_keep_unit_and_native_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        controls = (
            ("audition", "audition_geometry", (
                "editor::browser::ui_test::browser_waveform_click_is_manual_positioned_audition_and_retains_focus",
                "editor::browser::ui_test::browser_audition_row_observes_pcm_completion_and_stop_without_project_transport",
            )),
            ("random", "random_rank", (
                "editor::browser::ui_test::random_ui_tests::random_button_and_r_reveal_offscreen_audio_and_share_auto_audition",
                "editor::browser::ui_test::random_ui_tests::random_pick_obeys_all_search_projection_and_empty_results_leave_playback_unchanged",
            )),
            ("scrollbar", "scrollbar_geometry", (
                "editor::browser::ui_test::scrollbar_ui_tests::browser_thumb_tracks_wheel_and_drag_without_selection_or_audition",
                "editor::browser::ui_test::scrollbar_ui_tests::browser_thumb_recomputes_after_filter_and_resize_and_retires_capture",
            )),
        )
        for runtime, helper, consumers in controls:
            unit = f"editor::browser::{helper}::"
            for module in (runtime, helper):
                path = f"crates/hex-ui/src/editor/browser/{module}.rs"
                with self.subTest(path=path):
                    boundary = plan.boundary_selection(root, [path], inventory, [owner])
                    self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                    self.assertNotIn("editor::browser::", boundary["module_filters"][owner])
                    steps = {step.step_id: step for step in ci.planned_steps({
                        "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                        "cargo_inventory": inventory, "paths": [path]})}
                    unit_step = steps["ui-unit-unit_editor_logic"]
                    self.assertIn(unit, unit_step.argv)
                    self.assertTrue(unit_step.requires_tests)
                    for consumer in consumers:
                        self.assertIn(consumer, steps["ui-feature-tests"].argv)

        consumer_paths = ("browser_ui_test.rs", "browser_random_ui_test.rs",
                          "browser_scrollbar_ui_test.rs")
        for path, (_, helper, _) in zip(consumer_paths, controls):
            path = "crates/hex-ui/src/editor/" + path
            with self.subTest(path=path):
                names = plan.module_test_filters(root, [path], inventory, [owner])[owner]
                self.assertIn(f"editor::browser::{helper}::", names)

    def test_close_policy_units_keep_real_prompt_and_window_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        path = "crates/hex-ui/src/editor/close.rs"
        boundary = plan.boundary_selection(root, [path], inventory, [owner])
        self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
        steps = {step.step_id: step for step in ci.planned_steps({
            "scope": "rust", "affected_packages": [owner], "boundary": boundary,
            "cargo_inventory": inventory, "paths": [path]})}
        self.assertNotIn("module-tests-hex-ui", steps)
        self.assertIn("editor::close::tests::", steps["ui-default-lib-unit-tests"].argv)
        self.assertNotIn("editor::close::", steps["ui-default-lib-unit-tests"].argv)
        feature_args = steps["ui-feature-tests"].argv
        for consumer in (
            "editor::close::window_tests::automation_close_freezes_admission_until_prompt_cancellation",
            "editor::close::window_tests::custom_close_button_keeps_cancelled_dirty_window_and_discards_on_second_close",
            "editor::close::window_tests::clean_base_device_draft_is_committed_before_cancel_and_discard_prompt",
        ):
            self.assertTrue(any(consumer.startswith(prefix) for prefix in feature_args))

    def test_controller_session_action_owner_keeps_real_worker_and_callback_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::controller::session_action_unit_tests::"
        consumers = (
            "editor::controller::tests::earlier_session_rejection_is_not_reported_as_consumed",
            "editor::session_navigation_ui_test::scene_launch_leads_the_caption_and_uses_the_guarded_offline_action",
            "editor::session_navigation_ui_test::session_scene_duplicate_and_reorder_keys_use_worker_edits",
            "editor::session_navigation_ui_test::failed_scene_duplicate_clears_its_pending_selection_intent",
        )
        for path in (
            "crates/hex-ui/src/editor/controller.rs",
            "crates/hex-ui/src/editor/controller/session_action_unit_tests.rs",
        ):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                self.assertIn(unit, steps["ui-default-lib-unit-tests"].argv)
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in steps["ui-feature-tests"].argv))

    def test_controller_source_binding_unit_keeps_preview_protocol_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::controller::source_binding_unit_tests::"
        publication = "editor::controller::publication_unit_tests::"
        session_action = "editor::controller::session_action_unit_tests::"
        preview = ("editor::controller::sampler_native_ui_protocol_tests::"
                   "worker_preview_ack_waits_for_exact_submission_metadata")
        native = ("editor::controller::sampler_native_ui_protocol_tests::"
                  "native_sampler_applied_source_recovers_lost_submitted_metadata_without_legacy_promotion")
        comparison = ("editor::controller::comparison_source_binding_tests::"
                     "comparison_exact_source_maps_verified_observations_without_status_oscillation_or_old_ack_promotion")
        for path in ("crates/hex-ui/src/editor/controller.rs",
                     "crates/hex-ui/src/editor/controller/audio_event_correlation.rs"):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                unit_args = steps["ui-default-lib-unit-tests"].argv
                self.assertIn(unit, unit_args)
                if path.endswith("controller.rs"):
                    self.assertIn(publication, unit_args)
                    self.assertIn(session_action, unit_args)
                feature_args = steps["ui-feature-tests"].argv
                consumers = (preview, native, comparison) if path.endswith("controller.rs") else (preview, native)
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in feature_args),
                                    f"{consumer} missing from {feature_args}")

    def test_device_automation_entry_units_keep_rendered_targets(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit_prefixes = (
            "editor::device_automation::unit_tests::",
        )
        consumers = (
            "editor::device_automation::host_tests::host_header_picker_assigns_real_lanes_for_utility_and_other_native_owners",
            "editor::device_automation::host_tests::utility_toggle_and_choice_offer_their_exact_recording_targets",
        )
        for path in (
            "crates/hex-ui/src/editor/device_automation.rs",
            "crates/hex-ui/src/editor/device_automation/tests.rs",
            "crates/hex-ui/src/editor/device_automation/unit_tests.rs",
            "crates/hex-ui/src/editor/device_automation/host_tests.rs",
        ):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                unit = steps["ui-default-lib-unit-tests"]
                for prefix in unit_prefixes:
                    self.assertIn(prefix, unit.argv)
                self.assertTrue(unit.requires_tests)
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in steps["ui-feature-tests"].argv))

    def test_utility_readout_unit_keeps_rendered_controls(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::device_panel::utility_panel::tests::"
        consumers = (
            "editor::device_panel::tests::utility_gain_is_presented_in_db_and_typed_values_remain_linear_canonical",
            "editor::device_panel::tests::utility_default_width_and_audio_readouts_use_reference_units",
        )
        for path in (
            "crates/hex-ui/src/editor/device_panel/utility_panel.rs",
            "crates/hex-ui/src/editor/device_panel/tests.rs",
        ):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                default = steps["ui-default-lib-unit-tests"].argv
                self.assertIn(unit, default)
                support = steps["ui-feature-tests"].argv
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in support))
                    if path.endswith("/utility_panel.rs"):
                        self.assertIn(consumer, support)
                if path.endswith("/utility_panel.rs"):
                    self.assertNotIn("editor::device_panel::tests::", support)

    def test_vst3_entry_unit_keeps_raw_zero_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        path = "crates/hex-ui/src/editor/device_automation/vst3.rs"
        unit = "editor::device_automation::vst3::tests::"
        consumers = (
            "editor::device_automation::host_tests::vst3_dotted_guide_tracks_raw_zero_through_delayed_commit_ack",
            "editor::device_automation::host_tests::vst3_first_lane_menu_arms_and_disarms_the_raw_zero_knob",
        )
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        boundary = plan.boundary_selection(root, [path], inventory, [owner])
        self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
        steps = {step.step_id: step for step in ci.planned_steps({
            "scope": "rust", "affected_packages": [owner], "boundary": boundary,
            "cargo_inventory": inventory, "paths": [path]})}
        default_args = steps["ui-default-lib-unit-tests"].argv
        self.assertIn(unit, default_args)
        feature_args = steps["ui-feature-tests"].argv
        for consumer in consumers:
            self.assertTrue(any(consumer == prefix or consumer.startswith(prefix)
                                for prefix in feature_args),
                            f"{consumer} missing from {feature_args}")

    def test_native_automation_recording_units_keep_receipt_and_vst3_witnesses(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit_prefixes = (
            "editor::native_automation_recording::unit_tests::",
            "editor::native_automation_recording::editor_input::tests::",
        )
        consumers = (
            "editor::native_automation_recording::tests::refused_and_lost_submissions_keep_delivered_receipts_until_recovery",
            "editor::device_automation::host_tests::vst3_first_lane_menu_arms_and_disarms_the_raw_zero_knob",
            "editor::device_automation::host_tests::vst3_dotted_guide_tracks_raw_zero_through_delayed_commit_ack",
        )
        routes = (
            "crates/hex-ui/src/editor/native_automation_recording.rs",
            "crates/hex-ui/src/editor/native_automation_recording/editor_input.rs",
            "crates/hex-ui/src/editor/native_automation_recording/unit_tests.rs",
            "crates/hex-ui/src/editor/device_automation/host_tests.rs",
        )
        for path in routes:
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                unit = steps["ui-default-lib-unit-tests"]
                self.assertTrue(unit.requires_tests)
                feature = steps["ui-feature-tests"]
                feature_filters = feature.argv[feature.argv.index("--") + 1:]
                for consumer in consumers:
                    self.assertTrue(any(consumer == prefix or consumer.startswith(prefix)
                                        for prefix in feature_filters),
                                    f"{consumer} missing from {feature_filters}")
                if "native_automation_recording" in path:
                    self.assertTrue(any(any(prefix in arg for arg in unit.argv)
                                        for prefix in unit_prefixes))
                    self.assertFalse(any("unit_editor_logic" in step.argv for step in steps.values()))
                    self.assertFalse(any("editor::save::tests::" in arg for arg in unit.argv))
                else:
                    self.assertIn("editor::device_automation::unit_tests::", unit.argv)
                    for prefix in unit_prefixes:
                        self.assertIn(prefix, unit.argv)

    def test_selection_units_keep_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::selection::tests::"
        consumers = (
            "editor::controller::project_replacement_tests::worker_snapshots_and_undo_keep_the_last_user_detail_focus",
        )
        for path in (
            "crates/hex-ui/src/editor/selection.rs",
            "crates/hex-ui/src/editor/controller/project_replacement_tests.rs",
        ):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                default = steps["ui-default-lib-unit-tests"]
                self.assertTrue(default.requires_tests)
                self.assertIn(unit, default.argv)
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in steps["ui-feature-tests"].argv))

    def test_session_audio_source_window_units_keep_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::session_audio_panel::unit_tests::"
        consumers = (
            "editor::session_audio_panel::tests::source_panel_keeps_its_compact_row_while_an_edit_is_pending",
            "editor::session_audio_panel::tests::console_audio_loop_controls_edit_real_cell_history_and_saved_state",
        )
        for path in (
            "crates/hex-ui/src/editor/session_audio_panel.rs",
            "crates/hex-ui/src/editor/session_audio_panel/unit_tests.rs",
            "crates/hex-ui/src/editor/session_audio_panel/playhead.rs",
        ):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                default = steps["ui-default-lib-unit-tests"]
                self.assertTrue(default.requires_tests)
                self.assertIn(unit, default.argv)
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in steps["ui-feature-tests"].argv))
                if path.endswith(("/session_audio_panel.rs", "/playhead.rs")):
                    standalone = steps["ui-unit-unit_editor_logic"].argv
                    self.assertTrue(any("editor::session_audio_panel::playhead::".startswith(name) for name in standalone))

    def test_edit_cursor_pulse_unit_keeps_rendered_cursor_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::edit_cursor::tests::"
        consumers = (
            "editor::edit_cursor_ui_test::clicked_audio_cursor_is_visible_and_e_uses_it_offline",
            "editor::edit_cursor_ui_test::completed_time_selection_survives_idle_ticks_and_remains_rendered",
            "editor::edit_cursor_ui_test::space_restarts_from_selection_then_edit_cursor_and_respects_focus",
        )
        for path in (
            "crates/hex-ui/src/editor/edit_cursor.rs",
            "crates/hex-ui/src/editor/edit_cursor_ui_test.rs",
        ):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                default = steps["ui-default-lib-unit-tests"].argv
                self.assertIn(unit, default)
                feature = steps["ui-feature-tests"].argv
                self.assertNotIn("editor::", feature)
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in feature))
                    if path.endswith("/edit_cursor.rs"):
                        self.assertIn(consumer, feature)

    def test_console_return_paging_units_keep_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::console_returns::tests::"
        consumers = (
            "editor::console_returns::ui_tests::return_columns_align_and_page_through_all_sixteen_buses",
            "editor::console_returns::ui_tests::return_draft_blocks_paging_and_stays_visible_after_a_narrow_resize",
            "editor::console_returns::ui_tests::console_return_controls_use_the_existing_actor_and_history",
        )
        for path in (
            "crates/hex-ui/src/editor/console_returns.rs",
        ):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                default = steps["ui-default-lib-unit-tests"]
                self.assertTrue(default.requires_tests)
                self.assertIn(unit, default.argv)
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in steps["ui-feature-tests"].argv))

    def test_midi_draw_units_keep_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::midi_draw::tests::"
        consumers = (
            "editor::midi_canvas::pending_drag_tests::stroke_preview_tests::capture_tests::draw_and_erase_own_input_then_handoff_preview_without_holding_capture",
            "editor::midi_canvas::created_selection_tests::committed_draw_selects_actual_authored_ids_with_last_primary_after_later_frames",
        )
        for path in (
            "crates/hex-ui/src/editor/midi_draw.rs",
            "crates/hex-ui/src/editor/midi_draw_capture_ui_tests.rs",
            "crates/hex-ui/src/editor/midi_created_selection_ui_tests.rs",
        ):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                default = steps["ui-default-lib-unit-tests"]
                self.assertIn(unit, default.argv)
                self.assertTrue(default.requires_tests)
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in steps["ui-feature-tests"].argv))

    def test_midi_pan_units_keep_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::midi_pan::tests::"
        consumers = (
            "editor::midi_canvas::pan_tests::middle_drag_moves_both_axes_without_editing_and_ack_keeps_blank_space",
            "editor::midi_pan::capture_tests::piano_pan_owns_both_buttons_and_finishes_outside_without_edit",
        )
        for path in (
            "crates/hex-ui/src/editor/midi_pan.rs",
            "crates/hex-ui/src/editor/midi_pan_ui_tests.rs",
            "crates/hex-ui/src/editor/midi_pan_capture_tests.rs",
        ):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                default = steps["ui-default-lib-unit-tests"]
                self.assertIn(unit, default.argv)
                self.assertTrue(default.requires_tests)
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in steps["ui-feature-tests"].argv))

    def test_poll_cadence_units_keep_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::poll_cadence::tests::"
        consumers = (
            "editor::poll_cadence::ui_tests::key_and_render_wake_a_sleeping_poll",
            "editor::poll_cadence::ui_tests::pending_edit_wakes_a_sleeping_poll_and_is_acknowledged",
            "editor::poll_cadence::ui_tests::play_command_wakes_a_sleeping_poll_and_keeps_sixteen_ms",
            "editor::poll_cadence::ui_tests::stopped_editor_idles_at_ten_wakeups_per_second",
        )
        for path in (
            "crates/hex-ui/src/editor/poll_cadence.rs",
            "crates/hex-ui/src/editor/poll_cadence_ui_tests.rs",
        ):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                default = steps["ui-default-lib-unit-tests"]
                self.assertIn(unit, default.argv)
                self.assertTrue(default.requires_tests)
                feature_args = steps["ui-feature-tests"].argv
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in feature_args),
                                    (consumer, feature_args))

    def test_eq_creation_spectrum_units_keep_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::device_panel::eq_panel::tests::"
        consumers = (
            "editor::device_panel::tests::eq_ui_tests::eq_notch_hover_matches_creation_and_node_priority_at_narrow_and_wide_sizes",
            "editor::device_panel::tests::eq_ui_tests::graph_layout_tests::eq_default_spectrum_intent_and_explicit_toggle_do_not_edit_or_fake_audio",
            "editor::device_panel::tests::eq_ui_tests::graph_layout_tests::both_visible_eq_curves_advance_from_real_fft_results_without_focus_stealing",
        )
        for path in (
            "crates/hex-ui/src/editor/device_panel/eq_panel.rs",
            "crates/hex-ui/src/editor/device_panel/tests/eq_ui_tests.rs",
            "crates/hex-ui/src/editor/device_panel/tests/eq_graph_layout_tests.rs",
        ):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                default = steps["ui-default-lib-unit-tests"]
                self.assertIn(unit, default.argv)
                self.assertTrue(default.requires_tests)
                feature_args = steps["ui-feature-tests"].argv
                expected = consumers[1:] if path.endswith("/eq_graph_layout_tests.rs") else consumers
                for consumer in expected:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in feature_args),
                                    (consumer, feature_args))

    def test_track_performance_units_keep_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::track_performance::tests::"
        consumers = (
            "editor::track_performance::tests::rendered::six_segments_fit_compact_width_and_fill_only_measured_work",
            "editor::track_performance::tests::view_actions_fit_independent_indicators_in_arrangement_and_console_headers",
            "editor::track_performance::tests::missing_output_invalidates_cached_work_inside_the_sample_interval",
        )
        for path in (
            "crates/hex-ui/src/editor/track_performance/tests.rs",
            "crates/hex-ui/src/editor/track_performance.rs",
        ):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                default = steps["ui-default-lib-unit-tests"]
                self.assertIn(unit, default.argv)
                self.assertTrue(default.requires_tests)
                feature_args = steps["ui-feature-tests"].argv
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in feature_args),
                                    (consumer, feature_args))

    def test_waveform_canonical_trim_unit_route_preserves_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::waveform_geometry::tests::"
        consumers = (
            "editor::clip_drag_ui_test::rendered_audio_trim_updates_the_clip_waveform_geometry_and_commits_release_snap",
            "editor::waveform::resize_tests::plain_lower_fade_prefix_uses_the_same_raw_source_mapping",
            "editor::trim::loop_ui_tests::clip_loop_audio_preview_keeps_nonzero_waveform_across_the_extended_body",
        )
        for path in (
            "crates/hex-ui/src/editor/waveform_geometry.rs",
            "crates/hex-ui/src/editor/waveform_geometry/grid.rs",
            "crates/hex-ui/src/editor/waveform_resize_ui_test.rs",
            "crates/hex-ui/src/editor/clip_drag_ui_test.rs",
            "crates/hex-ui/src/editor/clip_loop_ui_tests.rs",
        ):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                unit_step = steps["ui-default-lib-unit-tests"]
                self.assertIn(unit, unit_step.argv)
                self.assertTrue(unit_step.requires_tests)
                feature_args = steps["ui-feature-tests"].argv
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in feature_args),
                                    (consumer, feature_args))

    def test_missing_media_relink_unit_route_preserves_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::missing_media::tests::"
        consumers = (
            "editor::missing_media::ui_tests::missing_media_can_open_offline_and_shows_the_missing_count",
            "editor::missing_media::ui_tests::moved_media_shows_panel_instead_of_failing_and_opens_with_relink",
            "editor::missing_media::ui_tests::offline_media_relinks_in_place_from_the_indicator_and_undoes",
            "editor::missing_media::ui_tests::sample_catalog_matches_are_offered_then_relinked_all_at_once",
        )
        for path in (
            "crates/hex-ui/src/editor/missing_media/tests.rs",
            "crates/hex-ui/src/editor/missing_media.rs",
            "crates/hex-ui/src/editor/missing_media/ui_tests.rs",
        ):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                unit_step = steps["ui-default-lib-unit-tests"]
                self.assertIn(unit, unit_step.argv)
                self.assertTrue(unit_step.requires_tests)
                feature_args = steps["ui-feature-tests"].argv
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in feature_args),
                                    (consumer, feature_args))

    def test_export_status_units_keep_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::export::status_tests::"
        consumers = (
            "editor::export::tests::export_dialog_cancel_reopen_starts_in_user_folder_and_native_save_needs_no_second_consent",
            "editor::export::tests::export_button_cancels_pending_control_without_opening_another_dialog",
        )
        path = "crates/hex-ui/src/editor/export.rs"
        boundary = plan.boundary_selection(root, [path], inventory, [owner])
        self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
        steps = {step.step_id: step for step in ci.planned_steps({
            "scope": "rust", "affected_packages": [owner], "boundary": boundary,
            "cargo_inventory": inventory, "paths": [path]})}
        default = steps["ui-default-lib-unit-tests"]
        self.assertIn(unit, default.argv)
        self.assertTrue(default.requires_tests)
        feature_args = steps["ui-feature-tests"].argv
        self.assertIn("editor::export::", feature_args)
        for consumer in consumers:
            self.assertTrue(any(consumer.startswith(prefix) for prefix in feature_args),
                            (consumer, feature_args))

        logic_path = "crates/hex-ui/tests/unit_editor_logic.rs"
        logic_boundary = plan.boundary_selection(root, [logic_path], inventory, [owner])
        logic_names = logic_boundary["module_filters"][owner]
        self.assertFalse(any(name.startswith(unit) or unit.startswith(name) for name in logic_names))
        logic_steps = {step.step_id: step for step in ci.planned_steps({
            "scope": "rust", "affected_packages": [owner], "boundary": logic_boundary,
            "cargo_inventory": inventory, "paths": [logic_path]})}
        logic_feature = logic_steps["ui-feature-tests"].argv
        self.assertNotIn("editor::export::", logic_feature)
        for consumer in consumers:
            self.assertFalse(any(consumer.startswith(prefix) for prefix in logic_feature),
                             (consumer, logic_feature))

    def test_dialog_pending_import_generation_keeps_open_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::dialogs::tests::pending_import_rejects_same_numeric_track_id_from_a_new_project"
        consumers = (
            "editor::dialogs::ui_tests::one_open_stale_picker_or_confirmation_cannot_replace_a_new_project",
            "editor::dialogs::ui_tests::native_open_action_survives_editor_focus_loss",
        )
        for path in (
            "crates/hex-ui/src/editor/dialogs.rs",
            "crates/hex-ui/src/editor/dialogs/ui_tests.rs",
        ):
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                default = steps["ui-default-lib-unit-tests"]
                self.assertTrue(any(unit.startswith(arg) or arg.startswith(unit) for arg in default.argv),
                                default.argv)
                self.assertTrue(default.requires_tests)
                feature_args = steps["ui-feature-tests"].argv
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in feature_args),
                                    (consumer, feature_args))

    def test_project_title_units_keep_rendered_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::header::tests::"
        consumers = (
            "editor::header::ui_tests::compact_top_toolbar_and_bottom_project_title_preserve_chrome_and_controls",
            "editor::header::ui_tests::title_double_click_never_toggles_fullscreen",
        )
        path = "crates/hex-ui/src/editor/header.rs"
        boundary = plan.boundary_selection(root, [path], inventory, [owner])
        self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
        steps = {step.step_id: step for step in ci.planned_steps({
            "scope": "rust", "affected_packages": [owner], "boundary": boundary,
            "cargo_inventory": inventory, "paths": [path]})}
        default = steps["ui-default-lib-unit-tests"]
        self.assertIn(unit, default.argv)
        self.assertTrue(default.requires_tests)
        feature_args = steps["ui-feature-tests"].argv
        for consumer in consumers:
            self.assertTrue(any(consumer.startswith(prefix) for prefix in feature_args),
                            (consumer, feature_args))

    def test_controller_publication_units_keep_worker_and_recovery_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::controller::publication_unit_tests::"
        session_action_unit = "editor::controller::session_action_unit_tests::"
        consumers = (
            "editor::controller::tests::earlier_session_rejection_is_not_reported_as_consumed",
            "editor::controller::comparison_source_binding_tests::comparison_exact_source_maps_verified_observations_without_status_oscillation_or_old_ack_promotion",
            "editor::controller::project_replacement_tests::receive_preserves_fresh_installation_through_edits_and_failed_open",
            "editor::controller::project_replacement_tests::stopped_restore_installs_fresh_without_replacing_project_or_losing_policy_to_later_edits",
            "editor::controller::sampler_native_ui_protocol_tests::worker_preview_ack_waits_for_exact_submission_metadata",
            "editor::controller::verified_gain_tests::verified_preview_submission_waits_for_exact_view_without_canonical_promotion",
        )
        for module in ("controller.rs", "controller/publication_unit_tests.rs",
                       "controller/project_replacement_tests.rs", "controller/verified_gain_tests.rs"):
            path = "crates/hex-ui/src/editor/" + module
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                unit_args = steps["ui-default-lib-unit-tests"].argv
                self.assertIn(unit, unit_args)
                if module == "controller.rs":
                    self.assertIn(session_action_unit, unit_args)
                else:
                    self.assertNotIn(session_action_unit, unit_args)
                self.assertNotIn("editor::controller::tests::", unit_args)
                feature_args = steps["ui-feature-tests"].argv
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in feature_args),
                                    f"{consumer} missing from {feature_args}")

    def test_arrangement_loop_rail_units_keep_render_and_pending_ack(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::loop_rail::tests::"
        consumers = (
            "editor::loop_rail::loop_bar_ui_tests::review_ui_tests::minimum_frame_bar_body_keeps_move_even_when_pointer_rounds_to_end",
            "editor::loop_rail::loop_bar_ui_tests::review_ui_tests::narrow_loop_center_drag_preserves_whole_range_at_two_zoom_levels",
            "editor::loop_rail::loop_bar_ui_tests::review_ui_tests::viewport_edge_caps_resize_the_existing_loop_instead_of_painting_a_new_range",
            "editor::loop_rail::pending_ui_tests::replacement_loop_stays_painted_until_its_real_response_installs",
        )
        for file in ("loop_rail.rs", "loop_rail/review_ui_tests.rs", "loop_rail/pending_ui_tests.rs"):
            with self.subTest(file=file):
                path = "crates/hex-ui/src/editor/" + file
                names = plan.module_test_filters(root, [path], inventory, [owner])
                boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                            "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                            "module_filters": names}
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                self.assertIn(unit, steps["ui-default-lib-unit-tests"].argv)
                self.assertTrue(steps["ui-default-lib-unit-tests"].requires_tests)
                for name in consumers:
                    self.assertTrue(any(name.startswith(prefix) for prefix in steps["ui-feature-tests"].argv))

    def test_eq8_units_keep_rendered_marquee_and_group_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        units = (
            "editor::device_panel::eq8_panel::graph::tests::",
            "editor::device_panel::eq8_panel::group::tests::",
            "editor::device_panel::eq8_panel::selection::tests::",
        )
        consumers = (
            "editor::device_panel::tests::eq8_marquee_tests::eq8_group_drag_cancel_and_blank_selection_never_remove_fixed_device",
            "editor::device_panel::tests::eq8_marquee_tests::eq8_marquee_group_move_and_disable_each_commit_one_undo",
        )
        paths = ("eq8_panel/graph.rs", "eq8_panel/group.rs", "eq8_panel/selection.rs",
                 "tests/eq8_marquee_tests.rs")
        for file in paths:
            with self.subTest(file=file):
                path = "crates/hex-ui/src/editor/device_panel/" + file
                names = plan.module_test_filters(root, [path], inventory, [owner])
                boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                            "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                            "module_filters": names}
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                self.assertEqual(steps["ui-default-lib-unit-tests"].argv[-3:], units)
                self.assertTrue(steps["ui-default-lib-unit-tests"].requires_tests)
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(name) for name in steps["ui-feature-tests"].argv))

    def test_save_policy_units_keep_rendered_open_and_save_as(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::save::tests::"
        consumers = (
            "editor::dialogs::ui_tests::one_open_save_current_completes_original_selection_after_success",
            "editor::save::ui_tests::older_format_explicit_overwrite_warns_once_and_save_as_keeps_original",
        )
        for module in ("save.rs", "save/ui_tests.rs", "dialogs/ui_tests.rs"):
            path = "crates/hex-ui/src/editor/" + module
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                unit_args = steps["ui-default-lib-unit-tests"].argv
                self.assertIn(unit, unit_args)
                self.assertNotIn("editor::save::", unit_args)
                feature_args = steps["ui-feature-tests"].argv
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in feature_args),
                                    f"{consumer} missing from {feature_args}")

    def test_gain_lane_units_keep_rendered_worker_and_refusal_coverage(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::gain_lane::tests::unit_tests::"
        consumers = (
            "editor::automation_view::tests::foreground_curve_owns_pointer_and_released_marker_until_exact_worker_terminal",
            "editor::automation_view::tests::earlier_completion_cannot_retire_released_gain_preview_and_own_refusal_does",
            "editor::gain_lane::tests::native::occupied_pointer_move_refuses_before_actor_and_keeps_visible_draft",
        )
        for module in ("gain_lane/mod.rs", "gain_lane/tests.rs", "automation_view/tests.rs"):
            path = "crates/hex-ui/src/editor/" + module
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                unit_args = steps["ui-default-lib-unit-tests"].argv
                self.assertIn(unit, unit_args)
                self.assertNotIn("editor::gain_lane::tests::", unit_args)
                feature_args = steps["ui-feature-tests"].argv
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in feature_args),
                                    f"{consumer} missing from {feature_args}")

    def test_midi_loop_display_cohort_routes_source_and_consumer_changes(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit_roots = ["editor::midi_loop_notes::tests::",
                      "editor::midi_ruler::geometry_tests::",
                      "editor::midi_ruler::loop_brace::tests::"]
        consumers = [
            "editor::midi_loop_notes::tests::rendered_long_loop_paints_every_repeat_without_a_sample_badge",
            "editor::midi_loop_notes::tests::rendered_tiled_loop_paints_one_sprite_per_visible_tile",
            "editor::midi_loop_notes::tests::rendered_long_loop_trim_preview_paints_every_visible_repeat",
            "editor::midi_ruler::tests::marker_drag_at_nonzero_arrangement_placement_acks_once_and_preserves_other_state",
            "editor::midi_ruler::tests::marker_cancels_on_escape_focus_viewport_competing_ack_and_clip_retirement",
            "editor::midi_ruler::loop_tests::loop_capture_cancel_and_stale_frame_preserve_source_and_transport",
            "editor::midi_ruler::loop_tests::console_loop_resize_enable_and_roundtrip_leave_arrangement_source_unchanged",
        ]
        routes = {
            "crates/hex-ui/src/editor/midi_loop_notes.rs": "editor::midi_loop_notes",
            "crates/hex-ui/src/editor/midi_ruler.rs": "editor::midi_ruler",
            "crates/hex-ui/src/editor/midi_loop.rs": "editor::midi_ruler::loop_brace",
            "crates/hex-ui/src/editor/midi_ruler_ui_tests.rs": "editor::midi_ruler::tests",
            "crates/hex-ui/src/editor/midi_loop_ui_tests.rs": "editor::midi_ruler::loop_tests",
        }
        module_paths = plan.module_paths(root / "crates/hex-ui/src")
        for path, expected_module in routes.items():
            with self.subTest(path=path):
                self.assertEqual(module_paths[(root / path).resolve()], expected_module)
                names = plan.module_test_filters(root, [path], inventory, [owner])
                selected = names[owner]
                self.assertTrue(all(any(root_name.startswith(name) or name.startswith(root_name)
                                         for name in selected) for root_name in unit_roots), selected)
                self.assertTrue(all(any(consumer.startswith(name) for name in selected) for consumer in consumers), selected)
                boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                            "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                            "module_filters": names}
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                unit = steps["ui-default-lib-unit-tests"]
                self.assertEqual(unit.argv, ("cargo", "test", "-p", "hex-ui", "--lib", "--locked",
                                             "--", *unit_roots))
                self.assertTrue(unit.requires_tests)
                feature = steps["ui-feature-tests"].argv
                self.assertIn("--lib", feature)
                for consumer in consumers:
                    self.assertTrue(any(consumer.startswith(name) for name in feature))

        unrelated = plan.module_test_filters(
            root, ["crates/hex-ui/src/editor/user_errors.rs"], inventory, [owner])
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                    "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                    "module_filters": unrelated}
        unrelated_steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                            "boundary": boundary, "cargo_inventory": inventory,
                                            "paths": ["crates/hex-ui/src/editor/user_errors.rs"]})
        self.assertFalse(any(step.step_id == "ui-default-lib-unit-tests" for step in unrelated_steps))
        self.assertFalse(any(any(consumer.startswith(arg) for arg in step.argv) for step in unrelated_steps
                             for consumer in consumers))

    def test_midi_cue_timing_selects_unit_and_real_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        names = ["editor::clip_range_fit::", "editor::midi_cue::"]
        for path in ("crates/hex-ui/src/editor/midi_cue/timing.rs",
                     "crates/hex-ui/src/editor/midi_cue.rs", "crates/hex-ui/src/editor/clip_range_fit.rs"):
            self.assertEqual(plan.module_test_filters(root, [path], inventory, [owner]), {owner: names})
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                        "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                        "module_filters": {owner: names}}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                      "cargo_inventory": inventory, "paths": [path]})
            unit = next(step for step in steps if "unit_timeline" in step.argv)
            integration = next(step for step in steps if step.step_id == "ui-feature-tests")
            self.assertIn("editor::midi_cue::", unit.argv)
            self.assertTrue(unit.requires_tests)
            for name in names:
                self.assertIn(name, integration.argv)

    def test_piano_unit_target_follows_owners_and_runs_once(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        piano = ["editor::midi_canvas::geometry::", "editor::midi_drag::", "editor::midi_group_drag::", "editor::midi_scene::", "editor::midi_selection::", "editor::midi_stroke::", "editor::midi_velocity::", "editor::midi_viewport_grid::"]
        creation = "editor::midi_canvas::creation_tests::held_creation_clamps_to_clip_end_and_positive_minimum"
        for path in ("midi_drag.rs", "midi_group_drag.rs", "midi_scene.rs", "midi_selection.rs", "midi_stroke.rs", "midi_velocity.rs", "midi_viewport_grid.rs", "midi_canvas/geometry.rs"):
            self.assertEqual(plan.module_test_filters(
                root, ["crates/hex-ui/src/editor/" + path], inventory, [owner]),
                {owner: sorted([*piano, creation])})
        piano_harness = plan.module_test_filters(
            root, ["crates/hex-ui/tests/unit_piano_roll.rs"], inventory, [owner])
        self.assertEqual(piano_harness,
                         {owner: sorted(piano + ["editor::midi_source_controls::", "editor::midi_wheel::",
                                                 "editor::midi_wheel_mapping::", creation])})
        shim_paths = ["crates/hex-ui/tests/piano_roll_unit/editor.rs",
                      "crates/hex-ui/tests/piano_roll_unit/midi_canvas.rs"]
        for path in shim_paths:
            self.assertEqual(plan.module_test_filters(root, [path], inventory, [owner]), piano_harness)
        combined_shims = plan.module_test_filters(root, shim_paths, inventory, [owner])
        self.assertEqual(combined_shims, piano_harness)
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                    "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                    "module_filters": combined_shims}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                  "boundary": boundary, "cargo_inventory": inventory,
                                  "paths": shim_paths})
        units = [step for step in steps if "unit_piano_roll" in step.argv]
        self.assertEqual(len(units), 1)
        self.assertTrue(units[0].requires_tests)
        for name in [*piano, "editor::midi_source_controls::", "editor::midi_wheel_mapping::"]:
            self.assertIn(name, units[0].argv)
        self.assertNotIn("editor::midi_wheel::", units[0].argv)
        feature = next(step for step in steps if step.step_id == "ui-feature-tests")
        self.assertIn(creation, feature.argv)
        self.assertIn("editor::midi_wheel::", feature.argv)
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

    def test_midi_canvas_geometry_keeps_unit_dependencies_and_creation_consumer(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::midi_canvas::geometry::"
        collaborators = [
            "editor::midi_drag::",
            "editor::midi_group_drag::",
            "editor::midi_scene::",
            "editor::midi_selection::",
            "editor::midi_stroke::",
            "editor::midi_velocity::",
            "editor::midi_viewport_grid::",
        ]
        consumer = "editor::midi_canvas::creation_tests::held_creation_clamps_to_clip_end_and_positive_minimum"
        paths = [
            ("crates/hex-ui/src/editor/midi_canvas/geometry.rs", "editor::midi_canvas::geometry"),
            ("crates/hex-ui/src/editor/midi_canvas.rs", "editor::midi_canvas"),
            ("crates/hex-ui/src/editor/midi_creation_ui_tests.rs", "editor::midi_canvas::creation_tests"),
            ("crates/hex-ui/src/editor/midi_drag.rs", "editor::midi_drag"),
            ("crates/hex-ui/src/editor/midi_scene.rs", "editor::midi_scene"),
        ]
        modules = plan.module_paths(root / "crates/hex-ui/src")
        for path, module in paths:
            self.assertEqual(modules[(root / path).resolve()], module)
            names = plan.module_test_filters(root, [path], inventory, [owner])[owner]
            if path == paths[0][0]:
                self.assertEqual(names, sorted([unit, *collaborators, consumer]))
            self.assertTrue(any(unit.startswith(selected) for selected in names),
                            f"{path} does not select {unit}: {names}")
            for collaborator in collaborators:
                self.assertTrue(any(collaborator.startswith(selected) for selected in names),
                                f"{path} does not select {collaborator}: {names}")
            self.assertTrue(any(consumer.startswith(selected) for selected in names),
                            f"{path} does not select {consumer}: {names}")
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                        "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                        "module_filters": {owner: names}}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                      "boundary": boundary, "cargo_inventory": inventory,
                                      "paths": [path]})
            unit_step = next(step for step in steps if "unit_piano_roll" in step.argv)
            self.assertTrue(unit_step.requires_tests)
            for expected in [unit, *collaborators]:
                self.assertTrue(any(expected.startswith(selected) for selected in unit_step.argv),
                                f"{path} unit step misses {expected}")
            feature = next(step for step in steps if step.step_id == "ui-feature-tests")
            self.assertTrue(any(consumer.startswith(selected) for selected in feature.argv))

    def test_eq_response_keeps_axes_unit_and_window_coverage(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        axes = "editor::device_panel::eq_panel::axes::"
        response = "editor::device_panel::eq_panel::response::"
        window = ("editor::device_panel::tests::eq_ui_tests::graph_layout_tests::"
                  "eq_axis_labels_are_logarithmic_nonoverlapping_and_keep_the_plot_hit_map")
        expected = sorted([axes, response, window])
        for path in ("axes.rs", "response.rs"):
            names = plan.module_test_filters(
                root, ["crates/hex-ui/src/editor/device_panel/eq_panel/" + path], inventory, [owner])
            self.assertEqual(names, {owner: expected})
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner], "lint_packages": [owner],
                        "single_run": {owner: "ui-test-support"}, "module_filters": names}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                      "cargo_inventory": inventory, "paths": []})
            self.assertTrue(any(axes in step.argv and response in step.argv and step.requires_tests
                                for step in steps if "unit_editor_logic" in step.argv))
            self.assertTrue(any(window in step.argv
                                for step in steps if step.step_id == "ui-feature-tests"))
        sampling = plan.module_test_filters(
            root, ["crates/hex-ui/src/editor/device_panel/eq_panel/sampling.rs"], inventory, [owner])
        self.assertIn("editor::device_panel::eq_panel::", sampling[owner])
        self.assertIn(window, sampling[owner])
        boundary["module_filters"] = sampling
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": []})
        self.assertTrue(any("editor::device_panel::eq_panel::" in step.argv
                            for step in steps if "unit_editor_logic" in step.argv))
        self.assertTrue(any(window in step.argv for step in steps if step.step_id == "ui-feature-tests"))
        graph = "editor::device_panel::tests::eq_ui_tests::graph_layout_tests::"
        names = plan.module_test_filters(
            root, ["crates/hex-ui/src/editor/device_panel/tests/eq_graph_layout_tests.rs"], inventory, [owner])
        creation_spectrum = "editor::device_panel::eq_panel::tests::"
        self.assertEqual(names, {owner: sorted([axes, response, graph, creation_spectrum])})
        boundary["module_filters"] = names
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": []})
        self.assertTrue(any(axes in step.argv and response in step.argv
                            for step in steps if "unit_editor_logic" in step.argv))
        self.assertTrue(any(creation_spectrum in step.argv and step.requires_tests
                            for step in steps if step.step_id == "ui-default-lib-unit-tests"))
        self.assertTrue(any(graph in step.argv for step in steps if step.step_id == "ui-feature-tests"))

    def test_overview_waveform_unit_cohort_keeps_real_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::overview_waveform::"
        columns = "editor::waveform_columns::"
        browser = ("editor::browser::ui_test::"
                   "browser_hover_and_selection_keep_selected_waveform_and_surroundings_stable")
        sampler = ("editor::sampler_parameter::tests::"
                   "source_hints_and_paint_require_same_view_source_facts_and_context")
        for source in ("overview_waveform.rs", "waveform_columns.rs", "waveform.rs"):
            names = plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + source], inventory, [owner])
            for name in (unit, columns, browser, sampler, "editor::waveform::"):
                self.assertIn(name, names[owner])
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                        "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                        "module_filters": names}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                      "cargo_inventory": inventory, "paths": []})
            self.assertTrue(any(unit in step.argv and columns in step.argv and step.requires_tests
                                for step in steps if "unit_editor_logic" in step.argv))
            self.assertTrue(any(browser in step.argv and sampler in step.argv
                                for step in steps if step.step_id == "ui-feature-tests"))
        for source in ("browser_ui_test.rs", "sampler_parameter/tests.rs"):
            names = plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + source], inventory, [owner])
            self.assertIn(unit, names[owner])
            self.assertIn(columns, names[owner])

    def test_midi_wheel_mapping_keeps_unit_and_window_coverage(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::midi_wheel_mapping::"
        window = "editor::midi_wheel::"
        for path in ("midi_wheel_mapping.rs", "midi_wheel.rs"):
            names = plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + path], inventory, [owner])
            self.assertEqual(names, {owner: [window, unit]})
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner], "lint_packages": [owner],
                        "single_run": {owner: "ui-test-support"}, "module_filters": names}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                      "cargo_inventory": inventory, "paths": []})
            self.assertTrue(any(unit in step.argv for step in steps if "unit_piano_roll" in step.argv))
            self.assertTrue(any(window in step.argv for step in steps if step.step_id == "ui-feature-tests"))

    def test_fade_calculation_keeps_unit_and_musical_coverage(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::fade_calculation::"
        integration = "editor::fades::"
        for path in ("fade_calculation.rs", "fades.rs"):
            names = plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + path], inventory, [owner])
            self.assertEqual(names, {owner: [unit, integration]})
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner], "lint_packages": [owner],
                        "single_run": {owner: "ui-test-support"}, "module_filters": names}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                      "cargo_inventory": inventory, "paths": []})
            self.assertTrue(any(unit in step.argv for step in steps if "unit_editor_logic" in step.argv))
            self.assertTrue(any(integration in step.argv for step in steps if step.step_id == "ui-feature-tests"))

    def test_fade_handle_geometry_keeps_unit_and_capture_coverage(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::fade_handle_geometry::"
        integration = "editor::fade_handles::"
        for path in ("fade_handle_geometry.rs", "fade_handles.rs", "fade_handles_ui_test.rs"):
            source = "crates/hex-ui/src/editor/" + path
            boundary = plan.boundary_selection(root, [source], inventory, [owner])
            self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
            names = boundary["module_filters"]
            expected = [unit, integration]
            if path == "fade_handles_ui_test.rs":
                expected += [
                    "editor::audio_event_editor::free_placement_tests::finite_frame_drag_preserves_the_cycle_markers_and_root_waveform",
                    "editor::audio_event_editor::tests::moved_and_pasted_waveform_positions_read_event_composed_pcm",
                    "editor::audio_event_editor::workflow_tests::cut_publishes_clipboard_on_ack_and_paste_survives_save_reopen_with_original_media",
                    "editor::fades::musical_tests::transformed_cropped_musical_fades_keep_readouts_reset_adjust_and_projected_curve",
                    "editor::waveform::paint_geometry::",
                    "editor::waveform::tests::waveform_canvas_paints_full_clip_extent",
                    "editor::waveform::waveform_header_ui_test::waveform_body_painter_stays_below_ordinary_selected_and_looped_headers",
                    "editor::waveform_geometry::tests::event_detail_keeps_a_transient_in_its_visible_column",
                ]
            self.assertEqual(names, {owner: sorted(expected)})
            steps = {step.step_id: step for step in ci.planned_steps({
                "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                "cargo_inventory": inventory, "paths": [source]})}
            standalone = steps["ui-unit-unit_editor_logic"]
            self.assertIn(unit, standalone.argv)
            self.assertTrue(standalone.requires_tests)
            default = steps["ui-default-lib-unit-tests"]
            self.assertIn("editor::fade_handles::tests::", default.argv)
            self.assertTrue(default.requires_tests)
            feature = steps["ui-feature-tests"].argv
            for consumer in (
                "editor::fade_handles::ui_tests::authored_fades_reveal_on_hover_and_each_endpoint_and_curve_uses_its_real_handle",
                "editor::fade_handles::ui_tests::narrow_clip_hides_hover_and_f_targets_until_zoomed_in",
                "editor::fade_handles::ui_tests::released_two_axis_fade_stays_painted_until_real_worker_ack",
            ):
                self.assertTrue(any(consumer.startswith(prefix) for prefix in feature), consumer)

    def test_console_meter_selection_keeps_unit_and_integration_coverage(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::track_meters::selection::"
        integration = "editor::track_meters::tests::"
        for path in ("track_meters/selection.rs", "track_meters/decay.rs",
                     "track_meters/follower.rs", "track_meters/scale.rs", "refresh_state.rs"):
            names = plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + path], inventory, [owner])
            self.assertEqual(names, {owner: METER_BOUNDARY})
        meter_path = "crates/hex-ui/src/editor/track_meters.rs"
        compact_header = "editor::track_layout_ui_test::track_meter_uses_own_peak_and_stays_inside_compact_header"
        parent = plan.module_test_filters(root, [meter_path], inventory, [owner])
        self.assertEqual(parent, {owner: [
            "editor::browser::console_drop_ui_test::console_column_paged_narrow_target_uses_painted_anchor_and_rejects_stale_source",
            "editor::console_track_page::", "editor::output_meter_ui_test::", "editor::refresh_state::",
            "editor::session_track_reorder_ui_test::console_page_turn_rejects_fold_and_window_changes",
            "editor::session_track_reorder_ui_test::session_group_band_survives_paging_past_its_parent",
            "editor::track_clipboard_ui_test::console_fold_hides_cells_navigation_and_invalidates_column_frame",
            compact_header,
            "editor::track_meters::", "output_meter::logic::", "output_meter::tests::"]})
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner], "lint_packages": [owner],
                    "single_run": {owner: "ui-test-support"}, "module_filters": names}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": []})
        self.assertTrue(any(unit in step.argv for step in steps if "unit_editor_logic" in step.argv))
        self.assertTrue(any(integration in step.argv for step in steps if step.step_id == "ui-feature-tests"))

        parent_boundary = plan.boundary_selection(root, [meter_path], inventory, [owner])
        parent_steps = {step.step_id: step for step in ci.planned_steps({
            "scope": "rust", "affected_packages": [owner], "boundary": parent_boundary,
            "cargo_inventory": inventory, "paths": [meter_path]})}
        pure = parent_steps["ui-default-lib-unit-tests"]
        self.assertIn("editor::track_meters::tests::", pure.argv)
        self.assertTrue(pure.requires_tests)
        feature = parent_steps["ui-feature-tests"].argv
        for consumer in (
            "editor::track_meters::tests::compact_and_expanded_meter_geometry_has_headroom_and_a_unity_mark",
            "editor::track_meters::tests::revision_handoff_keeps_meter_visible_across_repaints",
            "editor::track_meters::tests::steady_state_meter_ticks_reuse_the_staging_buffers",
            compact_header,
        ):
            self.assertTrue(any(consumer.startswith(prefix) for prefix in feature), (consumer, feature))

    def test_console_track_page_keeps_unit_and_paging_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::console_track_page::"
        meter_unit = "editor::track_meters::selection::"
        consumers = [
            "editor::browser::console_drop_ui_test::console_column_paged_narrow_target_uses_painted_anchor_and_rejects_stale_source",
            "editor::session_track_reorder_ui_test::session_group_band_survives_paging_past_its_parent",
            "editor::session_track_reorder_ui_test::console_page_turn_rejects_fold_and_window_changes",
            "editor::track_clipboard_ui_test::console_fold_hides_cells_navigation_and_invalidates_column_frame",
            "editor::track_meters::tests::steady_state_meter_ticks_reuse_the_staging_buffers",
        ]
        paths = ["crates/hex-ui/src/editor/console_track_page.rs",
                 "crates/hex-ui/src/editor/console_drop_ui_test.rs",
                 "crates/hex-ui/src/editor/session_track_reorder_ui_test.rs",
                 "crates/hex-ui/src/editor/track_clipboard_ui_test.rs",
                 "crates/hex-ui/src/editor/track_meters/selection.rs",
                 "crates/hex-ui/src/editor/track_meters.rs"]
        for path in paths:
            names = plan.module_test_filters(root, [path], inventory, [owner])[owner]
            self.assertIn(unit, names)
            self.assertTrue(any(meter_unit.startswith(selected) for selected in names))
            self.assertNotIn("editor::", names)
            for consumer in consumers:
                self.assertTrue(any(consumer.startswith(selected) for selected in names))
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                        "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                        "module_filters": {owner: names}}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                      "boundary": boundary, "cargo_inventory": inventory, "paths": [path]})
            unit_step = next(step for step in steps if "unit_editor_logic" in step.argv)
            self.assertIn(unit, unit_step.argv)
            self.assertTrue(any(meter_unit.startswith(selected) for selected in unit_step.argv))
            self.assertTrue(unit_step.requires_tests)
            feature = next(step for step in steps if step.step_id == "ui-feature-tests")
            for consumer in consumers:
                self.assertTrue(any(consumer.startswith(selected) for selected in feature.argv))

        combined_paths = [
            "crates/hex-ui/src/editor/session_grid.rs",
            "crates/hex-ui/src/editor/console_track_page.rs",
        ]
        combined = plan.module_test_filters(root, combined_paths, inventory, [owner])[owner]
        self.assertIn(unit, combined)
        self.assertTrue(any(meter_unit.startswith(selected) for selected in combined))
        self.assertTrue(any("editor::session_grid::" == selected for selected in combined))
        self.assertTrue(all(any(consumer.startswith(selected) for selected in combined)
                            for consumer in consumers))
        action_consumers = [
            "editor::delete_ui_tests::",
            "editor::controller::session_action_unit_tests::project_edit_backpressure_preserves_transport_but_invalidates_session_actions",
            "editor::session_navigation_ui_test::scene_launch_leads_the_caption_and_uses_the_guarded_offline_action",
            "editor::context_menu::tests::global_launch_dropdown_renders_all_choices_and_restores_focus_without_project_edits",
            "editor::device_panel::tests::device_comparison_tests::device_comparison_metadata_pending_callback_then_applied_authoring_is_not_a_new_ack",
        ]
        self.assertTrue(all(any(consumer.startswith(selected) for selected in combined)
                            for consumer in action_consumers))
        combined_boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                             "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                             "module_filters": {owner: combined}}
        combined_steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                           "boundary": combined_boundary,
                                           "cargo_inventory": inventory, "paths": combined_paths})
        combined_unit = next(step for step in combined_steps
                             if step.step_id == "ui-unit-unit_editor_logic")
        self.assertIn(unit, combined_unit.argv)
        self.assertTrue(any(meter_unit.startswith(selected) for selected in combined_unit.argv))
        self.assertIn("editor::session_grid::", combined_unit.argv)
        self.assertTrue(combined_unit.requires_tests)
        combined_feature = next(step for step in combined_steps if step.step_id == "ui-feature-tests")
        self.assertIn("editor::session_grid::", combined_feature.argv)
        for consumer in [*consumers, *action_consumers]:
            self.assertTrue(any(consumer.startswith(selected) for selected in combined_feature.argv))

    def test_recent_project_labels_keep_menu_and_mru_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::recent_projects::labels::"
        menu_unit = "editor::menus::tests::"
        menu_test = "editor::menus::tests::project_menu_keeps_open_actions_and_recent_project_paths"
        mru = "editor::recent_projects::tests::recent_menu_action_keeps_its_path_after_mru_reordering_and_obeys_open_guard"
        closed_menu = "editor::dialogs::ui_tests::project_menu_without_active_or_after_closed_window_cannot_touch_another_editor"
        paths = {
            "crates/hex-ui/src/editor/recent_projects/labels.rs": [unit, menu_unit, mru, closed_menu],
            "crates/hex-ui/tests/editor_logic_unit/recent_projects.rs": [unit],
            "crates/hex-ui/src/editor/recent_projects.rs": [
                "editor::recent_projects::", menu_unit, closed_menu],
            "crates/hex-ui/src/editor/menus.rs": [
                "editor::menus::", unit, mru, closed_menu],
        }
        for path, expected in paths.items():
            with self.subTest(path=path):
                names = plan.module_test_filters(root, [path], inventory, [owner])
                self.assertEqual(names, {owner: sorted(expected)})
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                logic = steps["ui-unit-unit_editor_logic"]
                self.assertTrue(logic.requires_tests)
                self.assertTrue(any(unit.startswith(arg) or arg.startswith(unit) for arg in logic.argv),
                                logic.argv)
                if any(menu_unit.startswith(arg) or arg.startswith(menu_unit) for arg in names[owner]):
                    default_lib = steps["ui-default-lib-unit-tests"]
                    self.assertIn(menu_unit, default_lib.argv)
                feature = steps["ui-feature-tests"].argv
                self.assertNotIn(menu_test, feature)
                self.assertEqual(any(mru.startswith(prefix) for prefix in feature), path != "crates/hex-ui/tests/editor_logic_unit/recent_projects.rs")
                self.assertEqual(any(closed_menu.startswith(prefix) for prefix in feature), path != "crates/hex-ui/tests/editor_logic_unit/recent_projects.rs")

        self.assertEqual(plan.module_test_filters(
            root, ["crates/hex-ui/src/editor/recent_projects/labels.rs",
                  "crates/hex-ui/tests/editor_logic_unit/editor.rs"], inventory, [owner]), {})
        logic_names = plan.module_test_filters(
            root, ["crates/hex-ui/tests/unit_editor_logic.rs"], inventory, [owner])[owner]
        self.assertNotIn(menu_test, logic_names)
        self.assertFalse(any(mru.startswith(name) or closed_menu.startswith(name) for name in logic_names))

    def test_tuning_pure_units_keep_exact_rendered_tuning_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit_prefixes = (
            "editor::tuning::snap_tests::",
            "editor::tuning::plots::tests::",
            "editor::tuning::audition_level_unit_tests::",
        )
        consumers = (
            "editor::tuning::ui_tests::review_followup_dice_changes_the_actual_palette_without_editing_the_project",
            "editor::tuning::ui_tests::review_first_imported_degree_can_drag_without_changing_reference_or_count",
            "editor::tuning::ui_tests::audition_keys_are_owned_by_tuning_focus_and_release_after_modifier_changes",
        )
        cases = (
            ("crates/hex-ui/src/editor/tuning/mod.rs", unit_prefixes, consumers),
            ("crates/hex-ui/src/editor/tuning/plots.rs", (unit_prefixes[1],), (consumers[0],)),
            ("crates/hex-ui/src/editor/tuning/ui_tests.rs", (), consumers),
        )
        for path, units, rendered in cases:
            with self.subTest(path=path):
                boundary = plan.boundary_selection(root, [path], inventory, [owner])
                self.assertEqual(boundary["single_run"], {owner: "ui-test-support"})
                steps = {step.step_id: step for step in ci.planned_steps({
                    "scope": "rust", "affected_packages": [owner], "boundary": boundary,
                    "cargo_inventory": inventory, "paths": [path]})}
                if units:
                    unit = steps["ui-default-lib-unit-tests"]
                    self.assertTrue(unit.requires_tests)
                    for name in units:
                        self.assertIn(name, unit.argv)
                else:
                    self.assertNotIn("ui-default-lib-unit-tests", steps)
                feature = steps["ui-feature-tests"].argv
                for consumer in rendered:
                    self.assertTrue(any(consumer.startswith(prefix) for prefix in feature), (consumer, feature))
                self.assertNotIn("editor::", feature)

    def test_guarded_session_launch_keeps_real_action_and_intent_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        source = "crates/hex-ui/src/editor/session_grid/actions.rs"
        shim = "crates/hex-ui/tests/editor_logic_unit/session_grid.rs"
        names = plan.module_test_filters(root, [source], inventory, [owner])
        self.assertEqual(plan.module_test_filters(root, [shim], inventory, [owner]), names)
        unit = "editor::session_grid::actions::"
        intent = "editor::controller::session_action_unit_tests::launch_and_transport_actions_have_explicit_play_intents"
        self.assertIn(unit, names[owner])
        self.assertIn(intent, names[owner])
        # The shared editor shim may register unknown modules: keep its package fallback.
        self.assertEqual(plan.module_test_filters(
            root, [source, "crates/hex-ui/tests/editor_logic_unit/editor.rs"], inventory, [owner]), {})
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                    "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                    "module_filters": names}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                  "boundary": boundary, "cargo_inventory": inventory, "paths": [source]})
        unit_step = next(step for step in steps if step.step_id == "ui-unit-unit_editor_logic")
        self.assertIn(unit, unit_step.argv)
        self.assertTrue(unit_step.requires_tests)
        feature = next(step for step in steps if step.step_id == "ui-feature-tests")
        default_units = next(step for step in steps if step.step_id == "ui-default-lib-unit-tests")
        self.assertIn(intent, default_units.argv)
        launch = "editor::session_navigation_ui_test::scene_launch_leads_the_caption_and_uses_the_guarded_offline_action"
        self.assertTrue(any(launch.startswith(prefix) for prefix in feature.argv))
        self.assertIn("editor::session_grid::tests::", feature.argv)

    def test_session_grid_quantization_keeps_unit_parent_and_ui_coverage(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []}, "dependencies": [],
                      "targets": [{"kind": ["lib"]}]}]
        unit = "editor::session_grid::actions::"
        wrapper = "editor::session_grid::tests::"
        delete = "editor::delete_ui_tests::"
        launch = ("editor::device_panel::tests::device_comparison_tests::"
                  "device_comparison_metadata_pending_callback_then_applied_authoring_is_not_a_new_ack")
        queued = "editor::controller::session_action_unit_tests::project_edit_backpressure_preserves_transport_but_invalidates_session_actions"
        intent = "editor::controller::session_action_unit_tests::launch_and_transport_actions_have_explicit_play_intents"
        navigation = "editor::session_navigation_ui_test::"
        navigation_unit = "editor::session_navigation::"
        menu = ("editor::context_menu::tests::"
                "global_launch_dropdown_renders_all_choices_and_restores_focus_without_project_edits")
        names = plan.module_test_filters(root, ["crates/hex-ui/src/editor/session_grid/actions.rs"], inventory, [owner])
        self.assertEqual(names, {owner: sorted([unit, wrapper, delete, launch, queued, intent, navigation,
                                                navigation_unit, menu])})
        parent = plan.module_test_filters(root, ["crates/hex-ui/src/editor/session_grid.rs"], inventory, [owner])
        self.assertEqual(parent, {owner: sorted(["editor::session_grid::", delete, launch, queued, intent, navigation,
                                                 navigation_unit, menu])})
        combined = plan.module_test_filters(
            root,
            ["crates/hex-ui/src/editor/session_grid/actions.rs",
             "crates/hex-ui/src/editor/session_grid.rs"],
            inventory,
            [owner],
        )
        self.assertEqual(combined, {owner: sorted(["editor::session_grid::", delete, launch, queued, intent,
                                                   navigation, navigation_unit, menu])})
        self.assertNotIn(unit, combined[owner])
        boundary = {"kind": "packages", "tests": [], "test_packages": [owner], "lint_packages": [owner],
                    "single_run": {owner: "ui-test-support"}, "module_filters": combined}
        steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner], "boundary": boundary,
                                  "cargo_inventory": inventory, "paths": []})
        unit_step = next(step for step in steps if step.step_id == "ui-unit-unit_editor_logic")
        self.assertIn("editor::session_grid::", unit_step.argv)
        feature = next(step for step in steps if step.step_id == "ui-feature-tests")
        self.assertTrue(any("editor::session_grid::" in step.argv for step in steps
                            if step.step_id == "ui-feature-tests"))
        default_units = next(step for step in steps if step.step_id == "ui-default-lib-unit-tests")
        self.assertIn(queued, default_units.argv)
        self.assertIn(intent, default_units.argv)
        self.assertIn(wrapper, default_units.argv)
        self.assertTrue(all(test in feature.argv for test in (delete, launch, navigation, menu)))
        for path in ("delete_ui_tests.rs", "controller.rs", "session_navigation_ui_test.rs",
                     "context_menu.rs",
                     "device_panel/tests/device_comparison_tests.rs"):
            consumers = plan.module_test_filters(root, ["crates/hex-ui/src/editor/" + path], inventory, [owner])
            self.assertIn(unit, consumers[owner])

        action_path = "crates/hex-ui/src/editor/session_grid/actions.rs"
        action_boundary = plan.boundary_selection(root, [action_path], inventory, [owner])
        self.assertEqual(action_boundary["single_run"], {owner: "ui-test-support"})
        action_steps = {step.step_id: step for step in ci.planned_steps({
            "scope": "rust", "affected_packages": [owner], "boundary": action_boundary,
            "cargo_inventory": inventory, "paths": [action_path]})}
        self.assertIn(wrapper, action_steps["ui-default-lib-unit-tests"].argv)
        self.assertIn(unit, action_steps["ui-unit-unit_editor_logic"].argv)
        self.assertIn(navigation, action_steps["ui-feature-tests"].argv)
        self.assertIn(menu, action_steps["ui-feature-tests"].argv)

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

    def test_log_viewer_scrollbar_geometry_selects_nested_unit_and_real_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []},
                      "dependencies": [], "targets": [{"kind": ["lib"]}]}]
        unit = "log_viewer::scrollbar::geometry::"
        consumers = [
            "log_viewer::scrollbar::tests::native_scrollbar_tracks_wheel_outside_release_escape_and_preserves_selection_copy",
            "log_viewer::scrollbar::tests::page_refresh_resize_and_deactivation_retire_old_scroll_captures",
        ]
        geometry_path = "crates/hex-ui/src/log_viewer/scrollbar/geometry.rs"
        parent_path = "crates/hex-ui/src/log_viewer/scrollbar.rs"
        consumer_path = "crates/hex-ui/src/log_viewer/scrollbar_tests.rs"
        modules = plan.module_paths(root / "crates/hex-ui/src")
        self.assertEqual(modules[(root / geometry_path).resolve()], "log_viewer::scrollbar::geometry")
        self.assertEqual(modules[(root / parent_path).resolve()], "log_viewer::scrollbar")
        self.assertEqual(modules[(root / consumer_path).resolve()], "log_viewer::scrollbar::tests")
        for path in (geometry_path, parent_path, consumer_path):
            names = plan.module_test_filters(root, [path], inventory, [owner])[owner]
            self.assertTrue(any(unit.startswith(selected) for selected in names))
            if path == parent_path:
                self.assertIn("log_viewer::scrollbar::", names)
            self.assertNotIn("log_viewer::", names)
            for consumer in consumers:
                self.assertTrue(any(consumer.startswith(selected) for selected in names))
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                        "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                        "module_filters": {owner: names}}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                      "boundary": boundary, "cargo_inventory": inventory,
                                      "paths": [path]})
            unit_step = next(step for step in steps if "unit_editor_logic" in step.argv)
            self.assertTrue(any(unit.startswith(selected) for selected in unit_step.argv))
            if path == parent_path:
                self.assertIn("log_viewer::scrollbar::", unit_step.argv)
            self.assertTrue(unit_step.requires_tests)
            feature = next(step for step in steps if step.step_id == "ui-feature-tests")
            for consumer in consumers:
                self.assertTrue(any(consumer.startswith(selected) for selected in feature.argv))

    def test_log_viewer_filter_selects_unit_and_copy_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []},
                      "dependencies": [], "targets": [{"kind": ["lib"]}]}]
        unit = "log_viewer::filter::"
        consumers = [
            "log_viewer::tests::rapid_filter_changes_install_only_the_latest_result",
            "log_viewer::copy_tests::copy_all_uses_frozen_filters_and_real_clipboard_and_read_failures_keep_previous_copy",
            "log_viewer::copy_tests::filtered_excerpt_reports_empty_limits_and_failed_clipboard_confirmation",
        ]
        filter_path = "crates/hex-ui/src/log_viewer/filter.rs"
        parent_path = "crates/hex-ui/src/log_viewer.rs"
        copy_path = "crates/hex-ui/src/log_viewer/copy_tests.rs"
        modules = plan.module_paths(root / "crates/hex-ui/src")
        self.assertEqual(modules[(root / filter_path).resolve()], "log_viewer::filter")
        self.assertEqual(modules[(root / parent_path).resolve()], "log_viewer")
        self.assertEqual(modules[(root / copy_path).resolve()], "log_viewer::copy_tests")
        for path in (filter_path, parent_path, copy_path):
            names = plan.module_test_filters(root, [path], inventory, [owner])[owner]
            self.assertTrue(any(unit.startswith(selected) for selected in names))
            if path == parent_path:
                self.assertIn("log_viewer::", names)
            else:
                self.assertNotIn("log_viewer::", names)
            for consumer in consumers:
                self.assertTrue(any(consumer.startswith(selected) for selected in names))
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                        "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                        "module_filters": {owner: names}}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                      "boundary": boundary, "cargo_inventory": inventory,
                                      "paths": [path]})
            unit_step = next(step for step in steps if "unit_editor_logic" in step.argv)
            self.assertTrue(any(unit.startswith(selected) for selected in unit_step.argv))
            self.assertTrue(unit_step.requires_tests)
            feature = next(step for step in steps if step.step_id == "ui-feature-tests")
            for consumer in consumers:
                self.assertTrue(any(consumer.startswith(selected) for selected in feature.argv))

    def test_log_viewer_presentation_selects_unit_and_rendered_copy_consumers(self):
        root = SCRIPTS.parent
        owner = "hex-ui"
        inventory = [{"name": owner, "manifest": "crates/hex-ui/Cargo.toml",
                      "features": {"ui-test-support": [], "ui-unit-tier": []},
                      "dependencies": [], "targets": [{"kind": ["lib"]}]}]
        unit = "log_viewer::presentation::"
        consumers = [
            "log_viewer::tests::severity_badges_keep_rows_readable_and_copy_original_records",
            "log_viewer::tests::long_log_rows_stay_within_their_visual_bounds",
            "log_viewer::copy_tests::copy_all_uses_frozen_filters_and_real_clipboard_and_read_failures_keep_previous_copy",
            "log_viewer::copy_tests::filtered_excerpt_reports_empty_limits_and_failed_clipboard_confirmation",
        ]
        presentation_path = "crates/hex-ui/src/log_viewer/presentation.rs"
        parent_path = "crates/hex-ui/src/log_viewer.rs"
        copy_path = "crates/hex-ui/src/log_viewer/copy_tests.rs"
        modules = plan.module_paths(root / "crates/hex-ui/src")
        self.assertEqual(modules[(root / presentation_path).resolve()], "log_viewer::presentation")
        self.assertEqual(modules[(root / parent_path).resolve()], "log_viewer")
        self.assertEqual(modules[(root / copy_path).resolve()], "log_viewer::copy_tests")
        for path in (presentation_path, parent_path, copy_path):
            names = plan.module_test_filters(root, [path], inventory, [owner])[owner]
            self.assertTrue(any(unit.startswith(selected) for selected in names))
            if path == parent_path:
                self.assertEqual(names, ["log_viewer::"])
            else:
                self.assertNotIn("log_viewer::", names)
            for consumer in consumers:
                self.assertTrue(any(consumer.startswith(selected) for selected in names))
            boundary = {"kind": "packages", "tests": [], "test_packages": [owner],
                        "lint_packages": [owner], "single_run": {owner: "ui-test-support"},
                        "module_filters": {owner: names}}
            steps = ci.planned_steps({"scope": "rust", "affected_packages": [owner],
                                      "boundary": boundary, "cargo_inventory": inventory,
                                      "paths": [path]})
            unit_step = next(step for step in steps if "unit_editor_logic" in step.argv)
            self.assertTrue(any(unit.startswith(selected) for selected in unit_step.argv))
            self.assertTrue(unit_step.requires_tests)
            if path == parent_path:
                self.assertIn("log_viewer::", unit_step.argv)
            feature = next(step for step in steps if step.step_id == "ui-feature-tests")
            for consumer in consumers:
                self.assertTrue(any(consumer.startswith(selected) for selected in feature.argv))

    def test_changed_module_roots_run_the_complete_test_subtree(self):
        root = SCRIPTS.parent
        for owner, path, prefix, child, consumer in (
            ("hex-session", "crates/hex-session/src/service.rs", "service::", "format_upgrade_tests", None),
            ("hex-ui", "crates/hex-ui/src/editor/save.rs", "editor::save::", "ui_tests",
             "editor::dialogs::ui_tests::one_open_save_current_completes_original_selection_after_success"),
        ):
            with self.subTest(owner=owner):
                inventory = [{"name": owner, "manifest": f"crates/{owner}/Cargo.toml",
                              "features": {}, "dependencies": [], "targets": [{"kind": ["lib"]}]}]
                filters = plan.module_test_filters(root, [path], inventory, [owner])
                expected = [prefix] if consumer is None else sorted([prefix, consumer])
                self.assertEqual(filters, {owner: expected})
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
                if consumer is not None:
                    self.assertTrue(any(consumer in step.argv for step in selected))

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
