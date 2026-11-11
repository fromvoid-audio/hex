"""Deterministic local check selection; this module never builds or mutates Git."""
from __future__ import annotations

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import stat
import subprocess
from urllib.parse import unquote

_PYTHON_SPEC = importlib.util.spec_from_file_location(
    "hex_python_test_plan", Path(__file__).with_name("python_test_plan.py"))
if _PYTHON_SPEC is None or _PYTHON_SPEC.loader is None:
    raise RuntimeError("Python self-test impact policy unavailable")
python_tests = importlib.util.module_from_spec(_PYTHON_SPEC)
# Use the exact source rather than a timestamp-valid stale bytecode cache.
_PYTHON_SOURCE_PATH = Path(_PYTHON_SPEC.origin)
_PYTHON_SOURCE_DIR = os.open(_PYTHON_SOURCE_PATH.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
try:
    _PYTHON_SOURCE_FD = os.open(_PYTHON_SOURCE_PATH.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,
                                dir_fd=_PYTHON_SOURCE_DIR)
    try:
        if not stat.S_ISREG(os.fstat(_PYTHON_SOURCE_FD).st_mode):
            raise RuntimeError("Python test policy source is not a regular file")
        with os.fdopen(_PYTHON_SOURCE_FD, "rb", closefd=False) as _python_source:
            _PYTHON_SOURCE_BYTES = _python_source.read()
    finally:
        os.close(_PYTHON_SOURCE_FD)
finally:
    os.close(_PYTHON_SOURCE_DIR)
_PYTHON_SOURCE_SHA256 = hashlib.sha256(_PYTHON_SOURCE_BYTES).hexdigest()
exec(compile(_PYTHON_SOURCE_BYTES, _PYTHON_SPEC.origin, "exec", dont_inherit=True), python_tests.__dict__)

POLICY_VERSION = 54
UI_PACKAGES = {"hex-ui", "hex-widgets"}
# Opaque state reaches storage through runtime type IDs, not a Cargo dependency.
# Changes here must be reviewed with the real registration/codec consumers.
RUNTIME_CONSUMERS = {"hex-devices": {"hex-storage"}}
# Both consumers compile this exact storage source through theme.rs by path.
PATH_RUNTIME_CONSUMERS = {"crates/hex-storage/src/text.rs": {"hex-ui", "hex-sandbox"},
                          "crates/hex-ui/src/theme.rs": {"hex-sandbox"},
                          "crates/hex-ui/src/theme/device.rs": {"hex-sandbox"}}
RUNTIME_CONSUMER_NAMES = {name for names in RUNTIME_CONSUMERS.values() for name in names}
TOOLING = {
    "launch", "scripts/launch-restart.py", "scripts/hex-test-build.py",
    "scripts/hex-test-build.json", "scripts/bundle-macos.sh",
    "scripts/ci_plan.py", "scripts/local-ci.py", "scripts/python_test_plan.py", "scripts/ci_checkpoint.py", "scripts/ci_target_storage.py",
    "scripts/test_shards.py", "scripts/python_shards.py",
}


def git(root: Path, *args: str, env=None) -> bytes:
    return subprocess.check_output(["git", *args], cwd=root, env=env)


def metadata_path(path: str) -> bool:
    """Only contributor prose is metadata; executable policy and datasets are inputs."""
    return (path in {"README.md", "AGENTS.md"}
            or path.startswith("docs/") and path.endswith(".md"))


def runtime_inputs(root: Path, env=None) -> dict:
    digest = hashlib.sha256()
    paths = sorted(set(git(root, "ls-files", "-z", "--cached", "--others",
                           "--exclude-standard", env=env).decode().rstrip("\0").split("\0")) - {""})
    symlinks = [name for name in paths if (root / name).is_symlink()]
    for name in paths:
        if metadata_path(name):
            continue
        path = root / name
        mode = (0o120000 if path.is_symlink() else 0o100755 if path.is_file() and path.stat().st_mode & 0o111
                else 0o100644 if path.is_file() else 0)
        content = os.readlink(path).encode() if path.is_symlink() else (
            path.read_bytes() if path.is_file() else b"<missing>")
        for value in (name.encode(), str(mode).encode(), content):
            digest.update(len(value).to_bytes(8, "big"))
            digest.update(value)
    return {"version": POLICY_VERSION, "sha256": digest.hexdigest(),
            "symlinks": symlinks,
            "metadata_exclusions": [p for p in paths if metadata_path(p)]}


def changed_paths(root: Path, base: str, source: str) -> list[str]:
    """Both sides of renames, deletions, and untracked inputs participate."""
    paths = set()
    for arguments in ((base, source), (source,)):
        fields = git(root, "diff", "--name-status", "-z", "--find-renames", *arguments).decode().split("\0")
        index = 0
        while index < len(fields) and fields[index]:
            status = fields[index]
            count = 2 if status[0] in "RC" else 1
            paths.update(fields[index + 1:index + 1 + count])
            index += count + 1
    paths.update(git(root, "ls-files", "--others", "--exclude-standard", "-z").decode().split("\0"))
    return sorted(paths - {""})


def require_qualifying_inputs(inputs: dict) -> None:
    if any(not metadata_path(name) for name in inputs.get("symlinks", [])):
        raise ValueError("qualifying executable source inputs cannot be symlinks")


def cargo_inventory(metadata: dict, root: Path) -> list[dict]:
    members = set(metadata["workspace_members"])
    inventory = []
    for package in metadata["packages"]:
        if package["id"] not in members:
            continue
        manifest_path = Path(package["manifest_path"]).resolve()
        manifest = manifest_path.relative_to(root.resolve()).as_posix()
        targets = []
        for target in package["targets"]:
            entry = {key: target[key] for key in ("name", "kind", "required-features") if key in target}
            if package["name"] == "hex-ui" and target.get("kind") == ["test"] and target.get("src_path"):
                source = Path(target["src_path"]).resolve()
                if source.is_relative_to(manifest_path.parent):
                    entry["src_path"] = source.relative_to(manifest_path.parent).as_posix()
            targets.append(entry)
        inventory.append({
            "name": package["name"], "manifest": manifest,
            "features": package["features"],
            "targets": targets,
            # Include dev, build, optional and target-dependent declarations:
            # default resolve.nodes alone omits some required configurations.
            "dependencies": [{k: d.get(k) for k in
                              ("name", "rename", "kind", "optional", "target", "features", "uses_default_features", "path")}
                             for d in package["dependencies"]],
        })
    if len(inventory) != len(members) or not inventory:
        raise ValueError("incomplete workspace Cargo inventory")
    return sorted(inventory, key=lambda item: item["name"])


def classify(paths: list[str], inventory: list[dict]) -> tuple[str, list[str], list[str]]:
    changed = set()
    scopes = set()
    reasons = []
    for path in paths:
        if metadata_path(path):
            continue
        if (path in TOOLING or path.startswith("scripts/tests/test_") and path.endswith(".py")):
            scopes.add("tooling")
            continue
        owners = [p for p in inventory if path.startswith(str(Path(p["manifest"]).parent) + "/")]
        owner = max(owners, key=lambda p: len(p["manifest"]), default=None)
        if (owner and path.endswith(".rs") and not path.endswith("/build.rs")
                and Path(path).relative_to(Path(owner["manifest"]).parent).parts[0]
                in {"src", "tests", "examples", "benches"}):
            scopes.add("ui" if owner["name"] in UI_PACKAGES else "rust")
            changed.add(owner["name"])
        elif owner and (path == owner["manifest"] or path.endswith("/build.rs")):
            scopes.add("rust")
            changed.add(owner["name"])
            reasons.append("package build/config input: include owner and declared consumers: " + path)
        else:
            scopes.add("full")
            reasons.append("high-risk or unknown input: " + path)
    path_consumers = set().union(*(PATH_RUNTIME_CONSUMERS.get(path, set()) for path in paths))
    missing = path_consumers - {p["name"] for p in inventory}
    if missing:
        raise ValueError("reviewed path consumers missing from Cargo inventory: " + ", ".join(sorted(missing)))
    if path_consumers:
        reasons.append("path-included source: include reviewed theme consumers")
    affected = set(changed) | path_consumers
    while True:
        next_affected = affected | {p["name"] for p in inventory
                                   if any(d["name"] in affected for d in p["dependencies"])}
        next_affected |= {consumer for owner in affected for consumer in RUNTIME_CONSUMERS.get(owner, set())
                          if any(p["name"] == consumer for p in inventory)}
        if next_affected == affected:
            break
        affected = next_affected
    if scopes == {"ui"} and affected - UI_PACKAGES - {"hex-desktop"}:
        scopes.add("rust")
        reasons.append("UI has additional consumers: include their package configurations")
    if scopes == {"ui"} and any(set(p["features"]) - ({"ui-test-support", "ui-unit-tier"} if p["name"] == "hex-ui" else set())
                                for p in inventory if p["name"] in UI_PACKAGES):
        scopes.add("rust")
        reasons.append("additional UI features require affected-package configurations")
    scope = ("full" if "full" in scopes else "rust" if "rust" in scopes
             or "ui" in scopes and "tooling" in scopes else
             "metadata" if not scopes else next(iter(scopes)))
    if not reasons:
        reasons.append("known " + scope + " inputs; prose does not widen executable scope")
    return scope, sorted(affected), reasons


# Reviewed test-only features that add APIs/tests without changing default behavior,
# so a package's default `cargo test` is a subset of its feature run (doctests
# excepted; --all-targets omits them, so plans keep a --doc run). Default-feature
# compilation stays proven by affected clippy. Reviewed 31 October 2026:
# - hex-ui ui-test-support: enables gpui/test-support (test executor/platform and
#   debug selectors only) and hex-session's overview hooks. Complementary
#   library unit tests are explicitly scheduled by their module prefixes.
#   Its separately selected ui-unit-tier companion gates source-local tests only.
# - hex-session source-overview-test-support: an optional observer hook, None by default.
# - hex-engine/hex-export instrument-test-support: probe/fixture modules only.
# allocation-probe installs a counting global allocator and has cfg(not) gates;
# it is deliberately absent and keeps both runs.
ADDITIVE_TEST_FEATURES = {
    "hex-ui": "ui-test-support",
    "hex-session": "source-overview-test-support",
    "hex-engine": "instrument-test-support",
    "hex-export": "instrument-test-support",
}


# Test-only cfg exclusions whose default lib module runs explicitly in merge plans.
CONTROLLER_PUBLICATION_UNIT = "editor::controller::publication_unit_tests::"
CONTROLLER_SESSION_ACTION_UNIT = "editor::controller::session_action_unit_tests::"
CONTROLLER_SOURCE_BINDING_UNIT = "editor::controller::source_binding_unit_tests::"
UI_DEFAULT_LIB_UNIT_MODULES: dict[str, tuple[str, ...]] = {
    "src/context_menu.rs": ("context_menu::tests::",),
    "src/editor/export.rs": ("editor::export::status_tests::",),
    "src/editor/header.rs": ("editor::header::tests::",),
    "src/editor/automation_take_recovery.rs": ("editor::automation_take_recovery::unit_tests::",),
    "src/editor/device_automation.rs": ("editor::device_automation::unit_tests::",),
    "src/editor/device_automation/vst3.rs": ("editor::device_automation::vst3::tests::",),
    "src/editor/edit_cursor.rs": ("editor::edit_cursor::tests::",),
    "src/editor/native_automation_recording.rs": ("editor::native_automation_recording::unit_tests::",),
    "src/editor/native_automation_recording/editor_input.rs": ("editor::native_automation_recording::editor_input::tests::",),
    "src/editor/drag_autoscroll.rs": ("editor::drag_autoscroll::tests::",),
    "src/editor/clip_selection.rs": ("editor::clip_selection::unit_tests::",),
    "src/editor/trim.rs": ("editor::trim::tests::",),
    "src/editor/changelog.rs": ("editor::changelog::tests::",),
    "src/editor/device_panel/device_meter.rs": ("editor::device_panel::device_meter::tests::",),
    "src/editor/device_panel/utility_panel.rs": ("editor::device_panel::utility_panel::tests::",),
    "src/editor/controller_input/monitoring.rs": ("editor::controller_input::monitoring::unit_tests::",),
    "src/editor/clip_range_fit.rs": ("editor::clip_range_fit::tests::",),
    "src/section_surface/focus.rs": ("section_surface::focus::tests::",),
    "src/output_meter/tests.rs": ("output_meter::tests::unit_tests::",),
    "src/editor/selection.rs": ("editor::selection::tests::",),
    "src/editor/clip_header_loop.rs": ("editor::clip_header_loop::tests::",),
    "src/editor/clip_loop_handles.rs": ("editor::trim::loop_handles::tests::",),
    "src/editor/browser/history.rs": ("editor::browser::history::tests::",),
    "src/editor/musical_audio_controls.rs": ("editor::musical_audio_controls::tests::",),
    "src/editor/musical_audio_controls/sample_bpm.rs": ("editor::musical_audio_controls::sample_bpm::tests::",),
    "src/editor/tuning/mod.rs": ("editor::tuning::snap_tests::", "editor::tuning::audition_level_unit_tests::"),
    "src/editor/tuning/plots.rs": ("editor::tuning::plots::tests::",),
    "src/editor/session_grid.rs": ("editor::session_grid::tests::",),
    "src/editor/clip_boundary.rs": ("editor::clip_boundary::tests::corner_geometry_is_bounded_and_resolves_again_after_resize",),
    "src/editor/midi_draw.rs": ("editor::midi_draw::tests::",),
    "src/editor/midi_pan.rs": ("editor::midi_pan::tests::",),
    "src/editor/track_performance/tests.rs": ("editor::track_performance::tests::",),
    "src/editor/clip_content_cache.rs": ("editor::clip_content_cache::unit_tests::",),
    "src/editor/track_meters.rs": ("editor::track_meters::tests::",),
    "src/editor/missing_media.rs": ("editor::missing_media::tests::",),
    "src/editor/display_error.rs": ("editor::display_error::tests::",),
    "src/editor/audio_unavailable.rs": ("editor::audio_unavailable::tests::",),
    "src/editor/audio_event_editor/paint.rs": ("editor::audio_event_editor::paint::tests::",),
    "src/editor/navigation_metrics.rs": ("editor::navigation_metrics::tests::",),
    "src/editor/poll_cadence.rs": ("editor::poll_cadence::tests::",),
    "src/editor/gain_lane/tests.rs": ("editor::gain_lane::tests::unit_tests::",),
    "src/editor/close.rs": ("editor::close::tests::",),
    "src/editor/dialogs.rs": ("editor::dialogs::tests::",),
    "src/editor/console_returns.rs": ("editor::console_returns::tests::",),
    "src/editor/session_audio_panel.rs": ("editor::session_audio_panel::unit_tests::",),
    "src/editor/save.rs": ("editor::save::tests::",),
    "src/editor/zoom_fit.rs": ("editor::zoom_fit::unit_tests::",),
    "src/editor/browser.rs": ("editor::browser::unit_tests::",),
    "src/editor/track_tags.rs": ("editor::track_tags::tests::normalized_tags_use_only_the_eight_musical_element_colors",),
    "src/editor/menus.rs": ("editor::menus::tests::",),
    "src/editor/device_panel.rs": ("editor::device_panel::chain_layout_unit_tests::", "editor::device_panel::header_style_unit_tests::",),
    "src/editor/device_panel/chain_window.rs": ("editor::device_panel::chain_window::tests::",),
    "src/editor/device_panel/sampler_cursor_tests.rs": ("editor::device_panel::sampler_overview::tests::sampler_cursor_original_frame_mapping_bounds_and_time",),
    "src/editor/device_panel/rattle_panel.rs": ("editor::device_panel::rattle_panel::tests::",),
    "src/editor/device_panel/modulators.rs": ("editor::device_panel::modulators::tests::",),
    "src/editor/device_panel/eq_panel/gestures.rs": ("editor::device_panel::eq_panel::gestures::tests::",),
    "src/editor/device_panel/parameter_rows.rs": ("editor::device_panel::parameter_rows::tests::",),
    "src/editor/arrangement_seek.rs": ("editor::arrangement_seek::unit_tests::",),
    "src/editor/arrangement_sections.rs": (
        "editor::arrangement_sections::tests::section_insertion_uses_edges_and_does_not_move_a_clicked_source",
    ),
    "src/editor/device_panel/eq8_panel/graph.rs": ("editor::device_panel::eq8_panel::graph::tests::",),
    "src/editor/device_panel/eq8_panel/group.rs": ("editor::device_panel::eq8_panel::group::tests::",),
    "src/editor/device_panel/eq8_panel/selection.rs": ("editor::device_panel::eq8_panel::selection::tests::",),
    "src/editor/device_panel/eq_panel.rs": ("editor::device_panel::eq_panel::tests::",),
    "src/editor/loop_rail.rs": ("editor::loop_rail::tests::",),
    "src/editor/fade_handles.rs": ("editor::fade_handles::tests::",),
    "src/editor/controller.rs": (CONTROLLER_PUBLICATION_UNIT, CONTROLLER_SESSION_ACTION_UNIT,
                                   CONTROLLER_SOURCE_BINDING_UNIT),
    "src/editor/arrangement_audio_panel.rs": ("editor::arrangement_audio_panel::tests::",),
    "src/editor/arrangement_audio_panel/controls.rs": ("editor::arrangement_audio_panel::controls::tests::",),
    "src/editor/device_gesture.rs": ("editor::device_gesture::unit_tests::",),
    "src/editor/device_panel/operator_panel.rs": ("editor::device_panel::operator_panel::tests::",),
    "src/editor/device_panel/operator_panel/envelope.rs": ("editor::device_panel::operator_panel::envelope::tests::",),
    "src/editor/device_panel/operator_panel/filter_response.rs": ("editor::device_panel::operator_panel::filter_response::tests::",),
    "src/editor/device_panel/operator_panel/harmonics.rs": ("editor::device_panel::operator_panel::harmonics::tests::",),
    "src/editor/drag.rs": ("editor::drag::tests::",),
    "src/editor/tracks.rs": ("editor::tracks::midi_trim_unit_tests::",),
    "src/editor/pending_clip_move.rs": ("editor::pending_clip_move::tests::",),
    "src/editor/midi_loop.rs": ("editor::midi_ruler::loop_brace::tests::",),
    "src/editor/midi_loop_notes.rs": ("editor::midi_loop_notes::tests::",),
    "src/editor/midi_ruler.rs": ("editor::midi_ruler::geometry_tests::",),
    "src/editor/waveform_geometry.rs": ("editor::waveform_geometry::tests::",),
    "src/editor/waveform_stretch.rs": ("editor::waveform_stretch::tests::",),
}
UI_DEFAULT_LIB_UNIT_CFG = '#[cfg(all(test, not(feature = "ui-test-support")))]'

# These test-only gates preserve the reviewed test registrations.
UI_FEATURE_EQUIVALENT_TEST_CFGS = {
    "src/editor/clip_boundary.rs": '#[cfg(not(any(test, feature = "ui-test-support")))]',
    "src/editor/trim/geometry.rs": '#[cfg(all(test, any(feature = "ui-unit-tier", not(feature = "ui-test-support"))))]',
}
UI_UNIT_TIER_TEST_CFGS = frozenset({
    '#[cfg(all(test, feature = "ui-unit-tier"))]',
    '#[cfg(all(test, any(feature = "ui-test-support", feature = "ui-unit-tier")))]',
})


def single_run_packages(root: Path, affected: list[str], inventory: list[dict]) -> dict[str, str]:
    """Affected packages with a reviewed feature test configuration.

    A negative gate in Rust source keeps both default and feature runs. The
    exact test-only cfg in known hex-ui modules has its own default lib step.
    """
    selected = {}
    for package in inventory:
        feature = ADDITIVE_TEST_FEATURES.get(package["name"])
        if package["name"] not in affected or not feature:
            continue
        companions = {"ui-unit-tier"} if package["name"] == "hex-ui" else set()
        if set(package["features"]) - {"default"} - companions != {feature}:
            continue
        directory = root / Path(package["manifest"]).parent
        needle = f'feature = "{feature}"'
        unit_sources = UI_DEFAULT_LIB_UNIT_MODULES if package["name"] == "hex-ui" else {}
        equivalent_tests = UI_FEATURE_EQUIVALENT_TEST_CFGS if package["name"] == "hex-ui" else {}
        gated = any("not(" in line and needle in line
                    and (line.strip() != UI_DEFAULT_LIB_UNIT_CFG
                         or source.relative_to(directory).as_posix() not in unit_sources)
                    and line.strip() != equivalent_tests.get(source.relative_to(directory).as_posix())
                    for source in directory.rglob("*.rs") if source.is_file()
                    for line in source.read_text(errors="replace").splitlines())
        if package["name"] == "hex-ui" and "ui-unit-tier" in package["features"]:
            # Fail closed if the reviewed test-only companion ever gates
            # production code or gains an unreviewed expression.
            companion = 'feature = "ui-unit-tier"'
            gated |= any(companion in line and line.strip() not in UI_UNIT_TIER_TEST_CFGS
                         and line.strip() != equivalent_tests.get(source.relative_to(directory).as_posix())
                         for source in directory.joinpath("src").rglob("*.rs") if source.is_file()
                         for line in source.read_text(errors="replace").splitlines())
        if not gated:
            selected[package["name"]] = feature
    return selected


TEST_CFG = re.compile(r"#\[cfg\((test|all\(test\b[^()]*(\([^()]*\)[^()]*)*\))\)\]")


def module_declarations(source_root: Path) -> dict[Path, list[tuple[Path, list[str]]]]:
    """Map each declared module file to (declaring file, attributes) pairs.

    Only top-level `mod NAME;` lines (no indentation) are indexed; `#[path]` is
    resolved relative to the declaring file's directory, as rustc does outside
    inline modules. Declarations nested in inline modules are not indexed, so
    their files stay unproven rather than misattributed.
    """
    declaration = re.compile(r"(pub(\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;")
    path_attribute = re.compile(r'#\[path\s*=\s*"([^"]+)"\]')
    found: dict[Path, list[tuple[Path, list[str]]]] = {}
    for parent in sorted(f for f in source_root.rglob("*.rs") if f.is_file()):
        lines = parent.read_text(errors="replace").splitlines()
        for index, line in enumerate(lines):
            match = declaration.fullmatch(line.rstrip())
            if not match:
                continue
            attributes = []
            cursor = index - 1
            while cursor >= 0 and lines[cursor].startswith(("#[", "//")):
                attributes.append(lines[cursor].strip())
                cursor -= 1
            name = match.group(3)
            explicit = next((path_attribute.fullmatch(a).group(1) for a in attributes
                             if path_attribute.fullmatch(a)), None)
            if explicit:
                targets = [parent.parent / explicit]
            else:
                base = parent.parent if parent.name in {"mod.rs", "lib.rs", "main.rs"} else parent.with_suffix("")
                targets = [base / f"{name}.rs", base / name / "mod.rs"]
            for target in targets:
                found.setdefault(target.resolve(), []).append((parent.resolve(), attributes))
    return found


TEST_ATTRIBUTE = re.compile(r"#\[(?:gpui::)?test\b")
# Unit test targets which compile the owning source directly. Keep their libtest
# names under the original module path so scoped coverage remains observable.
UI_UNIT_TARGETS = {"editor::tempo_input::value::": "unit_editor_logic",
                   "log_viewer::presentation::": "unit_editor_logic",
                   "log_viewer::filter::": "unit_editor_logic",
                   "log_viewer::scrollbar::geometry::": "unit_editor_logic",
                   "editor::console_track_page::": "unit_editor_logic",
                   "editor::time_move_preview::intervals::tests::": "unit_editor_logic",
                   "editor::browser::geometry::": "unit_editor_logic",
                   "editor::browser::audition_geometry::": "unit_editor_logic",
                   "editor::browser::random_rank::": "unit_editor_logic",
                   "editor::browser::scrollbar_geometry::": "unit_editor_logic",
                   "editor::browser::samples::state::": "unit_editor_logic",
                   "editor::browser::scan::": "unit_editor_logic",
                   "editor::group_overview::model::": "unit_editor_logic",
                   "editor::browser::tree::state::": "unit_editor_logic",
                   "editor::arrangement_map::model::": "unit_editor_logic",
                   "editor::controller::session_feedback::": "unit_editor_logic",
                   "editor::waveform::paint_geometry::": "unit_editor_logic",
                   "editor::waveform_geometry::grid::": "unit_editor_logic",
                   "editor::waveform_loop::projection::": "unit_editor_logic",
                   "editor::waveform_loop::accepted_root::": "unit_editor_logic",
                   "editor::waveform_stretch::accepted_root::": "unit_editor_logic",
                   "editor::waveform_stretch::projection::": "unit_editor_logic",
                   "editor::drag::geometry::": "unit_editor_logic",
                   "output_meter::logic::": "unit_editor_logic",
                   "editor::overview_waveform::": "unit_editor_logic",
                   "editor::waveform_columns::": "unit_editor_logic",
                   "editor::device_panel::eq_panel::axes::": "unit_editor_logic",
                   "editor::device_panel::eq_panel::response::": "unit_editor_logic",
                   "editor::session_audio_panel::playhead::": "unit_editor_logic",
                   "editor::device_panel::vst3_parameters::row_projection::": "unit_device",
                   "editor::midi_cue::timing::": "unit_timeline",
                   "editor::waveform_source::": "unit_editor_logic",
                   "editor::computer_keyboard::state::": "unit_editor_logic",
                   "editor::audio_admission::": "unit_editor_state",
                   "editor::audio_import_settings::admission::": "unit_editor_logic",
                   "editor::clip_content_cache::profile::navigation::selection::": "unit_editor_state",
                   "editor::controller::audio_event_correlation::": "unit_editor_logic",
                   "editor::cpu_meter::": "unit_editor_state",
                   "editor::device_automation::plot_geometry::": "unit_automation",
                   "editor::device_panel::eq_panel::group_move::": "unit_device",
                   "editor::device_panel::eq_panel::selection::": "unit_device",
                   "editor::device_panel::eq_panel::slope::": "unit_device",
                   "editor::device_panel::eq_panel::state_edit::": "unit_device",
                   "editor::device_panel::numeric_display::": "unit_device",
                   "editor::device_panel::spectrum_display::": "unit_device",
                   "editor::disk_overload::": "unit_editor_state",
                   "editor::fade_calculation::": "unit_editor_logic",
                   "editor::fade_handle_geometry::": "unit_editor_logic",
                   "editor::midi_drag::": "unit_piano_roll",
                   "editor::midi_group_drag::": "unit_piano_roll",
                   "editor::midi_canvas::geometry::": "unit_piano_roll",
                   "editor::midi_scene::": "unit_piano_roll",
                   "editor::midi_selection::": "unit_piano_roll",
                   "editor::midi_source_controls::": "unit_piano_roll",
                   "editor::midi_stroke::": "unit_piano_roll",
                   "editor::midi_thumbnail::": "unit_editor_state",
                   "editor::midi_velocity::": "unit_piano_roll",
                   "editor::midi_viewport_grid::": "unit_piano_roll",
                   "editor::midi_wheel_mapping::": "unit_piano_roll",
                   "editor::musical_time::": "unit_timeline",
                   "editor::pending_edit::": "unit_editor_state",
                   "editor::play_intent::": "unit_editor_state",
                   "editor::recent_projects::labels::": "unit_editor_logic",
                   "editor::recovery_state::": "unit_editor_state",
                   "editor::refresh_state::": "unit_editor_logic",
                   "editor::seek::": "unit_timeline",
                   "editor::session_grid::actions::": "unit_editor_logic",
                   "editor::session_navigation::": "unit_editor_logic",
                   "editor::session_route_view::": "unit_editor_logic",
                   "editor::section_launch::geometry::": "unit_editor_logic",
                   "editor::track_latency::presentation::": "unit_editor_logic",
                   "editor::trim::admission::": "unit_editor_logic",
                   "editor::trim::geometry::": "unit_editor_logic",
                   "editor::trim::preview_state::": "unit_editor_logic",
                   "editor::trim::stretch_preview::resolver::": "unit_editor_logic",
                   "editor::signed_frame_viewport::": "unit_timeline",
                   "editor::track_meters::decay::": "unit_editor_logic",
                   "editor::track_meters::follower::": "unit_editor_logic",
                   "editor::track_meters::scale::": "unit_editor_state",
                   "editor::track_meters::selection::": "unit_editor_logic",
                   "editor::tracks::clone_source_window::": "unit_editor_logic",
                   "editor::tracks::group_overlap::": "unit_editor_logic",
                   "editor::user_errors::": "unit_editor_logic",
                   "editor::viewport::": "unit_editor_state",
                   "section_appearance::": "unit_section",
                   "section_hit::": "unit_section",
                   "section_input::": "unit_section",
                   "section_reorder::": "unit_section",
                   "section_surface::focus::geometry::": "unit_section",
                   "section_surface::geometry::": "unit_section",
                   "section_resize::": "unit_section",
                   "section_selection::": "unit_section",
                   "theme::": "unit_editor_logic"}
UI_UNIT_HARNESS_FILES = {"tests/piano_roll_unit/editor.rs": "unit_piano_roll",
                         "tests/piano_roll_unit/midi_canvas.rs": "unit_piano_roll"}
UI_UNIT_HARNESS_MODULES = {
    "tests/editor_logic_unit/recent_projects.rs": {"editor::recent_projects::labels::"},
    "tests/editor_logic_unit/session_grid.rs": {"editor::session_grid::actions::"},
    "tests/editor_logic_unit/tracks.rs": {
        "editor::tracks::clone_source_window::", "editor::tracks::group_overlap::",
    },
}
UI_UNIT_FEATURES = {"unit_piano_roll": "ui-unit-tier", "unit_timeline": "ui-unit-tier", "unit_automation": "ui-unit-tier", "unit_device": "ui-unit-tier", "unit_section": "ui-unit-tier", "unit_editor_logic": "ui-unit-tier"}
UI_DEFAULT_STANDALONE_TESTS = (
    "unit_editor_state", "track_group_focus", "browser_disclosure_assets", "browser_place_assets",
    "cursor_assets", "midi_audition_icon", "transport_icon_assets",
)
# These standalone targets run separately from a module-scoped library run.
UI_EXTRA_TEST_TARGETS = {"editor::arrangement_focus::": "track_group_focus",
                         "editor::track_hierarchy_indent::": "track_group_focus",
                         "editor::track_reorder::subtree::": "track_group_focus",
                         "editor::midi_recording::pressure_fixture::": "midi_pressure_boundary"}
UI_EXTRA_TEST_FEATURES = {"midi_pressure_boundary": "ui-test-support"}
# Reviewed tests for owners whose direct coverage lives in sibling modules.
UI_MODULE_TEST_DEPENDENCIES = {
    "editor::zoom_fit::tests::": ("editor::zoom_fit::unit_tests::",),
    "editor::browser::ui_test::": ("editor::browser::unit_tests::",),
    "editor::browser::ui_test::scrollbar_ui_tests::": (
        "editor::browser::unit_tests::",
        "editor::browser::ui_test::arrangement_wheel_zoom_is_pointer_anchored_and_does_not_escape_its_timeline",
    ),
    "context_menu::": (
        "editor::context_menu::tests::global_launch_dropdown_renders_all_choices_and_restores_focus_without_project_edits",
        "editor::context_menu::tests::menu_escape_outside_click_and_enter_do_not_leak_to_editor",
    ),
    "editor::context_menu::": ("context_menu::tests::",),
    "editor::device_panel::rattle_panel::": (
        "editor::device_panel::tests::rattle_ui_tests::rattle_display_reads_authored_automation_and_manual_draft_precedence",
        "editor::device_panel::tests::rattle_ui_tests::rattle_track_return_panel_fits_and_drive_draft_commits_one_undo",
    ),
    "editor::display_error::": (
        "editor::controller::project_replacement_tests::failed_batch_worker_event_displays_original_path_in_status_and_tooltip",
    ),
    "editor::menus::": (
        "editor::recent_projects::labels::",
        "editor::recent_projects::tests::recent_menu_action_keeps_its_path_after_mru_reordering_and_obeys_open_guard",
        "editor::dialogs::ui_tests::project_menu_without_active_or_after_closed_window_cannot_touch_another_editor",
    ),
    "editor::recent_projects::labels::": (
        "editor::menus::tests::",
        "editor::recent_projects::tests::recent_menu_action_keeps_its_path_after_mru_reordering_and_obeys_open_guard",
        "editor::dialogs::ui_tests::project_menu_without_active_or_after_closed_window_cannot_touch_another_editor",
    ),
    "editor::recent_projects::": (
        "editor::menus::tests::",
        "editor::dialogs::ui_tests::project_menu_without_active_or_after_closed_window_cannot_touch_another_editor",
    ),
    "editor::midi_canvas::created_selection_tests::": ("editor::midi_draw::tests::",),
    "editor::midi_canvas::pending_drag_tests::stroke_preview_tests::capture_tests::": ("editor::midi_draw::tests::",),
    "editor::session_audio_panel::playhead::": ("editor::session_audio_panel::unit_tests::",),
    "editor::device_automation::host_tests::": ("editor::device_automation::unit_tests::",
        "editor::native_automation_recording::unit_tests::",
        "editor::native_automation_recording::editor_input::tests::"),
    "editor::device_automation::tests::": ("editor::device_automation::unit_tests::",),
    "editor::device_automation::vst3::": (
        "editor::device_automation::host_tests::vst3_dotted_guide_tracks_raw_zero_through_delayed_commit_ack",
        "editor::device_automation::host_tests::vst3_first_lane_menu_arms_and_disarms_the_raw_zero_knob",
    ),
    "editor::pending_clip_move::": (
        "editor::clip_drag_ui_test::rendered_move_survives_same_revision_worker_work",
        "editor::clip_drag_ui_test::rendered_move_real_refusal_retires_only_its_matching_fifo_terminal",
        "editor::clip_drag_ui_test::rendered_move_retains_destination_until_real_worker_ack",
        "editor::drag::group_drag_ui_test::group_pending_or_actual_revision_change_refuses_all_members_without_losing_independent_edit",
    ),
    "editor::clip_content_cache::tests::": ("editor::clip_content_cache::unit_tests::",),
    "editor::device_panel::modulators::": (
        "editor::device_panel::tests::modulation_panel_adds_and_maps_an_audio_rate_modulator",
    ),
    "editor::clip_drag_ui_test::": ("editor::drag_autoscroll::tests::",),
    "editor::drag_autoscroll::resize_pan_tests::": ("editor::drag_autoscroll::tests::",),
    "editor::drag::": ("editor::drag_autoscroll::tests::",),
    "editor::drag::group_drag_ui_test::": ("editor::drag_autoscroll::tests::",),
    "editor::trim::midi_left_extension_ui_tests::": ("editor::drag_autoscroll::tests::",),
    "editor::trim::": ("editor::drag_autoscroll::tests::",),
    "editor::tracks::": (
        "editor::clip_drag_ui_test::rendered_midi_trim_previews_note_geometry_and_commits_release_snap",
        "editor::clip_drag_ui_test::rendered_midi_left_trim_keeps_source_note_timeline_position",
    ),
    "editor::tracks::midi_trim_unit_tests::": (
        "editor::clip_drag_ui_test::rendered_midi_trim_previews_note_geometry_and_commits_release_snap",
        "editor::clip_drag_ui_test::rendered_midi_left_trim_keeps_source_note_timeline_position",
    ),
    "editor::controller_input::": ("editor::controller_input::monitoring::unit_tests::",),
    "editor::midi_canvas::audition_tests::": ("editor::controller_input::monitoring::unit_tests::",),
    "editor::device_panel::tests::eq_ui_tests::graph_layout_tests::": ("editor::device_panel::eq_panel::tests::",),
    "editor::device_panel::tests::eq_ui_tests::": ("editor::device_panel::eq_panel::tests::",),
    "editor::device_panel::tests::header_style_tests::": ("editor::device_panel::header_style_unit_tests::",),
    "editor::arrangement_seek::tests::": ("editor::arrangement_seek::unit_tests::",),
    "editor::arrangement_sections::": ("editor::arrangement_seek::unit_tests::",),
    "section_appearance::": (
        "editor::arrangement_sections::tests::section_insertion_uses_edges_and_does_not_move_a_clicked_source",
        "editor::arrangement_sections::tests::first_section_can_be_inserted_after_last",
        "editor::arrangement_sections::tests::painted_section_release_uses_stamped_queue_and_keeps_success_preview",
        "editor::arrangement_sections::tests::stationary_non_grid_content_edge_click_does_not_change_music",
    ),
    "section_reorder::": (
        "editor::arrangement_sections::tests::section_insertion_uses_edges_and_does_not_move_a_clicked_source",
        "editor::arrangement_sections::tests::first_section_can_be_inserted_after_last",
        "editor::arrangement_sections::tests::painted_section_release_uses_stamped_queue_and_keeps_success_preview",
        "editor::arrangement_sections::tests::stationary_non_grid_content_edge_click_does_not_change_music",
    ),
    "editor::edit_cursor::": (
        "editor::edit_cursor_ui_test::clicked_audio_cursor_is_visible_and_e_uses_it_offline",
        "editor::edit_cursor_ui_test::completed_time_selection_survives_idle_ticks_and_remains_rendered",
        "editor::edit_cursor_ui_test::space_restarts_from_selection_then_edit_cursor_and_respects_focus",
    ),
    "editor::edit_cursor_ui_test::": (
        "editor::arrangement_seek::unit_tests::",
        "editor::edit_cursor::tests::",
    ),
    "editor::browser::audition::": ("editor::browser::audition_geometry::",),
    "editor::browser::random::": ("editor::browser::random_rank::",),
    "editor::browser::scrollbar::": ("editor::browser::scrollbar_geometry::",),
    "editor::controller::project_replacement_tests::": (CONTROLLER_PUBLICATION_UNIT,),
    "editor::controller::verified_gain_tests::": (CONTROLLER_PUBLICATION_UNIT,),
    "editor::arrangement_focus::visibility::": (
        "editor::arrangement_focus::tests::section_focus_renders_visible_child_with_nested_group_headers",
        "editor::track_tags::tests::rendered_tag_selection_modifiers_overflow_and_filter_are_view_only",
        "editor::track_tags::tests::folded_matches_stay_selected_and_filter_applies_to_console_headers",
    ),
    "editor::track_hierarchy_indent::": (
        "editor::track_clipboard_ui_test::nested_group_plates_and_poles_join_follow_selection_and_close_at_visible_ends",
        "editor::track_clipboard_ui_test::group_track_header_menu_arrangement",
        "editor::track_clipboard_ui_test::group_track_header_menu_session",
    ),
}
# These geometry modules call one another; a change to one must exercise the
# complete pure boundary rather than only its own test-name prefix.
# These consumers live in larger test modules; a selected ancestor must retain
# the unit owner as well as the specific consumer checks.
UI_PREFIX_COHORTS = [{"editor::device_panel::utility_panel::tests::",
                      "editor::device_panel::tests::utility_gain_is_presented_in_db_and_typed_values_remain_linear_canonical",
                      "editor::device_panel::tests::utility_default_width_and_audio_readouts_use_reference_units"},
                    {"editor::tempo_input::value::",
                      "editor::tempo_input::keyboard_tests::focused_project_arrows_adjust_local_draft_then_commit_exact_micro_bpm",
                      "editor::tempo_input::keyboard_tests::project_steps_preserve_bounds_precision_cancel_invalid_and_composition",
                      "editor::tempo_input::tests::project_tempo_same_value_explicit_commit_collapses_later_points_and_cursor_is_display_only",
                      "editor::tempo_input_ui_test::project_tempo_direct_input_validation_cancel_focus_and_canonical_commit",
                      "editor::tempo_input_ui_test::project_tempo_blur_commits_valid_text_and_invalid_blur_releases_editor_shortcuts",
                      "editor::tempo_input_ui_test::project_tempo_stale_revision_and_project_switch_retire_drafts",
                      "editor::tempo_input_ui_test::project_tempo_cursor_movement_cannot_retarget_an_active_draft"},
                    {"editor::midi_canvas::geometry::",
                     "editor::midi_canvas::creation_tests::held_creation_clamps_to_clip_end_and_positive_minimum"},
                    {"editor::midi_pan::tests::",
                     "editor::midi_canvas::pan_tests::middle_drag_moves_both_axes_without_editing_and_ack_keeps_blank_space",
                     "editor::midi_pan::capture_tests::piano_pan_owns_both_buttons_and_finishes_outside_without_edit"},
                    {"editor::dialogs::tests::pending_import_rejects_same_numeric_track_id_from_a_new_project",
                     "editor::dialogs::ui_tests::one_open_stale_picker_or_confirmation_cannot_replace_a_new_project",
                     "editor::dialogs::ui_tests::native_open_action_survives_editor_focus_loss"},
                    {"editor::save::tests::",
                     "editor::dialogs::ui_tests::one_open_save_current_completes_original_selection_after_success",
                     "editor::save::ui_tests::older_format_explicit_overwrite_warns_once_and_save_as_keeps_original"},
                    {"editor::gain_lane::tests::unit_tests::",
                     "editor::automation_view::tests::foreground_curve_owns_pointer_and_released_marker_until_exact_worker_terminal",
                     "editor::automation_view::tests::earlier_completion_cannot_retire_released_gain_preview_and_own_refusal_does",
                     "editor::gain_lane::tests::native::occupied_pointer_move_refuses_before_actor_and_keeps_visible_draft"},
                    {"editor::midi_loop_notes::tests::",
                     "editor::midi_ruler::geometry_tests::",
                     "editor::midi_ruler::loop_brace::tests::",
                     "editor::midi_loop_notes::tests::rendered_long_loop_paints_every_repeat_without_a_sample_badge",
                     "editor::midi_loop_notes::tests::rendered_tiled_loop_paints_one_sprite_per_visible_tile",
                     "editor::midi_loop_notes::tests::rendered_long_loop_trim_preview_paints_every_visible_repeat",
                     "editor::midi_ruler::tests::marker_drag_at_nonzero_arrangement_placement_acks_once_and_preserves_other_state",
                     "editor::midi_ruler::tests::marker_cancels_on_escape_focus_viewport_competing_ack_and_clip_retirement",
                     "editor::midi_ruler::loop_tests::loop_capture_cancel_and_stale_frame_preserve_source_and_transport",
                     "editor::midi_ruler::loop_tests::console_loop_resize_enable_and_roundtrip_leave_arrangement_source_unchanged"},
                     {"editor::time_move_preview::intervals::tests::",
                      "editor::time_move_preview::visual_tests::fragment_titles_cuts_crops_and_compact_body_preserve_source_mapping",
                      "editor::edit_cursor_ui_test::audio_time_move_preview_masks_overlaps_and_restores_when_cancelled",
                      "editor::edit_cursor_ui_test::midi_time_move_preview_translates_cached_notes_to_the_destination",
                      "editor::edit_cursor_ui_test::pending_time_move_preview_survives_until_worker_failure_then_restores_source"},
                    {"editor::browser::samples::state::",
                      "editor::browser::ui_test::samples_catalog_combines_roots_and_active_place_indexed_search",
                      "editor::browser::ui_test::all_search_unions_samples_folder_and_devices_and_preserves_identity_on_refresh",
                      "editor::browser::ui_test::browser_samples_count_tracks_hierarchy_items_and_flat_sample_matches",
                      "editor::browser::ui_test::browser_history_ui_test::review_history_waits_for_the_used_catalog_not_an_unrelated_scan",
                      "editor::browser::folder_places_ui_test::moved_place_shows_missing_and_relocates_with_icon_order_and_samples",
                      "editor::missing_media::ui_tests::sample_catalog_matches_are_offered_then_relinked_all_at_once",
                      "editor::browser::ui_test::search_selection::toggle_fixture_same_root_retains_actual_catalog_job_after_view_and_caller_drop"},
                     {"editor::browser::tree::state::",
                      "editor::browser::ui_test::browser_inline_tree_nested_keyboard_refresh_and_hover_retain_identity",
                      "editor::browser::ui_test::browser_tree_same_name_files_keep_distinct_preview_paths_and_bounded_labels",
                      "editor::browser::ui_test::browser_tree_refresh_updates_expanded_branch_hidden_by_indexed_query",
                      "editor::browser::ui_test::browser_folder_expansion_preserves_its_position_when_an_earlier_branch_closes",
                      "editor::browser::ui_test::browser_history_ui_test::review_readiness_obsolete_tree_worker_does_not_hold_saved_root"},
                    {"editor::browser::audition_geometry::",
                     "editor::browser::ui_test::browser_waveform_click_is_manual_positioned_audition_and_retains_focus",
                     "editor::browser::ui_test::browser_audition_row_observes_pcm_completion_and_stop_without_project_transport"},
                    {"editor::browser::random_rank::",
                     "editor::browser::ui_test::random_ui_tests::random_button_and_r_reveal_offscreen_audio_and_share_auto_audition",
                     "editor::browser::ui_test::random_ui_tests::random_pick_obeys_all_search_projection_and_empty_results_leave_playback_unchanged"},
                    {"editor::browser::scrollbar_geometry::",
                     "editor::browser::ui_test::scrollbar_ui_tests::browser_thumb_tracks_wheel_and_drag_without_selection_or_audition",
                     "editor::browser::ui_test::scrollbar_ui_tests::browser_thumb_recomputes_after_filter_and_resize_and_retires_capture"},
                    {"editor::browser::geometry::",
                     "editor::browser::ui_test::browser_right_edge_drag_resizes_and_release_stops_resizing",
                     "editor::browser::ui_test::browser_audio_drag_floats_outside_lanes_locks_preview_and_imports_exact_candidate",
                     "editor::track_layout_ui_test::empty_project_main_header_width_handle_uses_current_window_limit"},
                    {"editor::browser::scan::",
                     "editor::browser::ui_test::browser_source_transition_discards_late_scan_and_keeps_query_and_keyboard_scope",
                     "editor::browser::ui_test::place_correctness_reselect_gates_cached_rows_and_old_audition_callbacks",
                     "editor::browser::ui_test::place_correctness_refresh_retains_identity_only_after_current_success",
                     "editor::browser::ui_test::browser_inline_tree_nested_keyboard_refresh_and_hover_retain_identity",
                     "editor::browser::ui_test::browser_history_ui_test::browser_history_missing_place_remains_recoverable_and_late_scan_cannot_retarget"},
                    {"editor::loop_rail::tests::",
                     "editor::loop_rail::loop_bar_ui_tests::review_ui_tests::minimum_frame_bar_body_keeps_move_even_when_pointer_rounds_to_end",
                     "editor::loop_rail::loop_bar_ui_tests::review_ui_tests::narrow_loop_center_drag_preserves_whole_range_at_two_zoom_levels",
                     "editor::loop_rail::loop_bar_ui_tests::review_ui_tests::viewport_edge_caps_resize_the_existing_loop_instead_of_painting_a_new_range",
                     "editor::loop_rail::pending_ui_tests::replacement_loop_stays_painted_until_its_real_response_installs"},
                    {"editor::device_panel::eq8_panel::graph::tests::",
                     "editor::device_panel::eq8_panel::group::tests::",
                     "editor::device_panel::eq8_panel::selection::tests::",
                     "editor::device_panel::tests::eq8_marquee_tests::eq8_group_drag_cancel_and_blank_selection_never_remove_fixed_device",
                     "editor::device_panel::tests::eq8_marquee_tests::eq8_marquee_group_move_and_disable_each_commit_one_undo"},
                    {"editor::device_panel::chain_layout_unit_tests::",
                     "editor::device_panel::chain_window::tests::",
                     "editor::device_panel::parameter_rows::tests::",
                     "editor::device_panel::tests::chain_window_tests::short_chain_window_does_not_add_scroll_extent",
                     "editor::device_panel::tests::clipped_overflow_card_cannot_restore_a_drop_preview_outside_the_rail",
                     "editor::device_panel::tests::keyboard_navigation_tests::arrows_step_devices_and_paste_lands_in_the_clicked_gap",
                     "editor::device_panel::tests::parameter_rows_tests::generic_track_effect_rows_preserve_extent_identity_and_pins"},
                    {"editor::device_panel::operator_panel::tests::",
                     "editor::device_panel::tests::operator_tab_tests::operator_tab_cycles_rendered_controls_in_both_directions_without_project_edits",
                     "editor::device_panel::tests::operator_waveform_editor_tests::waveform_cycle_updates_from_real_partial_edits_and_undo"},
                    {"editor::device_panel::operator_panel::envelope::tests::",
                     "editor::device_panel::tests::operator_envelope_reference_ui_tests::other_envelopes_refresh_without_stealing_selection_and_reuse_repaint_geometry"},
                    {"editor::device_panel::operator_panel::filter_response::tests::",
                     "editor::device_panel::tests::operator_filter_response_tests::native_filter_display_fits_reuses_cache_and_follows_real_control_history",
                     "editor::device_panel::tests::operator_filter_response_tests::filter_curve_keeps_released_parameter_preview_across_completed_repaints"},
                    {"editor::device_panel::operator_panel::harmonics::tests::",
                     "editor::device_panel::tests::operator_waveform_editor_tests::waveform_cycle_updates_from_real_partial_edits_and_undo"},
                    {"editor::arrangement_audio_panel::tests::controls_map_to_typed_clip_edits_and_loop_toggle",
                     "editor::arrangement_audio_panel::tests::gain_steps_use_db_units_and_refuse_the_bounds",
                     "editor::arrangement_audio_panel::tests::invalid_edges_and_overflow_refuse_without_partial_command",
                     "editor::arrangement_audio_panel::tests::review_play_start_uses_retained_source_looping_independent_of_placement_mode",
                     "editor::arrangement_audio_panel::controls::tests::",
                     "editor::arrangement_audio_panel::controls::pitch_ui_tests::pitch_knob_real_worker_history_entry_reset_group_scope_and_save_reopen",
                     "editor::arrangement_audio_panel::controls::pitch_ui_tests::pitch_release_keeps_its_draft_until_accepted_or_refused_ack",
                     "editor::arrangement_audio_panel::controls::ui_tests::clip_slider_cancel_and_frozen_release_wait_for_own_terminal",
                     "editor::arrangement_audio_panel::tests::length_worker_returns::placed_length_ordinary_trim_real_worker_control",
                     "editor::arrangement_audio_panel::tests::review_placed_mode_only_change_invalidates_captured_control"},
                    {"editor::arrangement_map::model::",
                      "editor::arrangement_map::tests::main_lane_paints_canonical_intervals_at_the_ruler_scale",
                      "editor::arrangement_focus::visibility::",
                      "editor::group_overview::model::",
                      "editor::group_overview::tests::",
                      "editor::arrangement_sections::tests::section_lower_body_paints_all_tracks_without_section_edit_even_when_focused"},
                    {"section_surface::geometry::",
                     "editor::arrangement_sections::tests::section_has_opaque_chamfered_fill_and_compact_header_across_palettes_and_layouts",
                     "editor::arrangement_main_track_ui_test::clips_and_main_sections_share_grid_endpoints_through_fractional_pan_and_resize",
                     "editor::group_overview::tests::native_group_preview_tracks_viewport_content_removal_and_undo",
                     "editor::arrangement_map::tests::main_lane_paints_canonical_intervals_at_the_ruler_scale"},
                    {"section_surface::focus::geometry::",
                     "section_surface::focus::tests::theme_hatch_style_produces_bounded_visible_segments",
                     "editor::arrangement_focus::tests::only_selected_sections_in_q_focus_receive_the_pattern"},
                    {"editor::section_launch::geometry::",
                     "editor::arrangement_sections::tests::launch_action_geometry_matches_raw_exclusion_without_enlarging_tiny_sections",
                     "editor::arrangement_sections::tests::section_autoplay_click_dispatches_stateless_loop_and_toggle_off_cancels"},
                    {"editor::track_latency::presentation::",
                     "editor::track_latency::tests::view_action_mounts_the_indicator_in_actual_headers",
                     "editor::track_performance::tests::view_actions_fit_independent_indicators_in_arrangement_and_console_headers"},
                    {"editor::trim::admission::",
                     "editor::trim::resize_terminal_tests::restart_offline_pending_work_blocks_actual_resize_capture_until_completion",
                     "editor::trim::resize_release_tests::resize_escape_stale_generation_pending_refusal_and_no_motion_do_not_retain_a_commit"},
                    {"editor::trim::geometry::",
                     "editor::clip_drag_ui_test::rendered_audio_trim_updates_the_clip_waveform_geometry_and_commits_release_snap",
                     "editor::clip_drag_ui_test::rendered_audio_trim_release_upper_rail_commits_captured_clip_and_actual_worker_ack",
                     "editor::clip_drag_ui_test::rendered_midi_trim_previews_note_geometry_and_commits_release_snap",
                     "editor::track_compact_ui_test::compact_audio_and_midi_trim_edges_keep_title_and_exact_preview_until_cancel"},
                    {"editor::trim::preview_state::",
                     "editor::trim::resize_release_tests::normal_resize_preserves_visible_extent_and_source_through_release_and_ack",
                     "editor::trim::resize_release_tests::midi_resize_keeps_released_geometry_until_actual_worker_ack_and_one_history_step",
                     "editor::trim::loop_ui_tests::clip_loop_outer_real_pointer_retains_released_paint_and_one_worker_history_edge",
                     "editor::trim::midi_stretch_ui_tests::midi_shift_stretch_both_edges_projects_notes_through_release_ack_and_one_undo"},
                    {"editor::trim::stretch_preview::resolver::",
                     "editor::trim::stretch_limit_tests::",
                     "editor::trim::resize_release_tests::shift_resize_preserves_visible_extent_through_release_and_real_worker_ack",
                     "editor::clip_drag_ui_test::shift_edge_stretch_previews_live_and_stationary_modifier_changes_dispatch_one_edit",
                     "editor::drag_autoscroll::resize_pan_tests::resize_pan_preserves_stationary_snap_and_shift_stretch_preview_release",
                     "editor::drag_autoscroll::resize_pan_tests::inward_pan_then_supported_stretch_release_commits_once_and_undoes"},
                     {"editor::controller::session_feedback::",
                      "editor::controller::tests::earlier_session_rejection_is_not_reported_as_consumed",
                      "editor::action_outcome_ui_tests::editor_reports_each_rejection_and_keeps_reader_through_retirement"},
                     {"editor::waveform_stretch::projection::",
                      "editor::waveform_stretch::tests::geometry_cache_replaces_preview_snapshot_and_distinguishes_source_and_map"},
                     {"editor::waveform_loop::accepted_root::",
                      "editor::waveform::resize_tests::shift_stretch_keeps_live_sample_varied_waveforms_visible",
                      "editor::waveform::resize_tests::trim_keeps_sample_varied_waveforms_visible_with_warp_on_and_off"},
                     {"editor::waveform_geometry::grid::",
                      "editor::waveform::resize_tests::silent_left_prefix_keeps_waveform_and_geometry_through_real_worker_ack"},
                     {"editor::drag::geometry::", "editor::drag::tests::",
                      "editor::clip_drag_ui_test::rendered_move_retains_destination_until_real_worker_ack",
                      "editor::drag::group_drag_ui_test::stationary_group_snap_modifier_uses_one_anchor_delta_and_authoritative_release"},
                     {"editor::session_grid::actions::", "editor::delete_ui_tests::",
                      "editor::controller::session_action_unit_tests::launch_and_transport_actions_have_explicit_play_intents",
                      "editor::controller::session_action_unit_tests::project_edit_backpressure_preserves_transport_but_invalidates_session_actions",
                      "editor::session_navigation_ui_test::scene_launch_leads_the_caption_and_uses_the_guarded_offline_action",
                      "editor::context_menu::tests::global_launch_dropdown_renders_all_choices_and_restores_focus_without_project_edits",
                      "editor::device_panel::tests::device_comparison_tests::device_comparison_metadata_pending_callback_then_applied_authoring_is_not_a_new_ack"},
                    {"editor::tracks::clone_source_window::",
                     "editor::tracks::overlap_tests::held_midi_drag_keeps_actual_note_paint_and_colored_name_below_header",
                     "editor::tracks::overlap_tests::held_processed_audio_drag_reuses_ready_root_on_lane_and_floating",
                     "editor::tracks::overlap_tests::held_looped_warp_drag_keeps_last_ready_content_in_mixed_copy_group"},
                    {"editor::tracks::group_overlap::",
                     "editor::drag::group_drag_ui_test::group_overlap_is_passive_clipped_and_excludes_selected_originals_in_both_modes",
                     "editor::drag::group_drag_ui_test::review_group_landing_paints_unselected_overwrite_victim",
                     "editor::audio_event_editor::paint::tests::replacement_accents_follow_occupied_victims_and_range_overwrite_semantics",
                     "editor::audio_event_editor::workflow_tests::event_overlap_accent_covers_only_replaced_audio_and_survives_release"},
                    {"editor::console_track_page::",
                     "editor::browser::console_drop_ui_test::console_column_paged_narrow_target_uses_painted_anchor_and_rejects_stale_source",
                     "editor::session_track_reorder_ui_test::session_group_band_survives_paging_past_its_parent",
                     "editor::session_track_reorder_ui_test::console_page_turn_rejects_fold_and_window_changes",
                     "editor::track_clipboard_ui_test::console_fold_hides_cells_navigation_and_invalidates_column_frame",
                     "editor::track_meters::selection::",
                     "editor::track_meters::tests::steady_state_meter_ticks_reuse_the_staging_buffers"},
                    {"log_viewer::scrollbar::geometry::",
                     "log_viewer::scrollbar::tests::native_scrollbar_tracks_wheel_outside_release_escape_and_preserves_selection_copy",
                     "log_viewer::scrollbar::tests::page_refresh_resize_and_deactivation_retire_old_scroll_captures"},
                    {"log_viewer::filter::",
                     "log_viewer::tests::rapid_filter_changes_install_only_the_latest_result",
                     "log_viewer::copy_tests::copy_all_uses_frozen_filters_and_real_clipboard_and_read_failures_keep_previous_copy",
                     "log_viewer::copy_tests::filtered_excerpt_reports_empty_limits_and_failed_clipboard_confirmation"},
                    {"log_viewer::presentation::",
                     "log_viewer::tests::severity_badges_keep_rows_readable_and_copy_original_records",
                     "log_viewer::tests::long_log_rows_stay_within_their_visual_bounds",
                     "log_viewer::copy_tests::copy_all_uses_frozen_filters_and_real_clipboard_and_read_failures_keep_previous_copy",
                     "log_viewer::copy_tests::filtered_excerpt_reports_empty_limits_and_failed_clipboard_confirmation"}]
UI_UNIT_COHORTS = [{"editor::device_panel::eq_panel::axes::", "editor::device_panel::eq_panel::response::"},
                   {"editor::midi_canvas::geometry::", "editor::midi_drag::", "editor::midi_group_drag::", "editor::midi_scene::", "editor::midi_selection::", "editor::midi_stroke::", "editor::midi_velocity::", "editor::midi_viewport_grid::"},
                   {"editor::device_panel::eq_panel::group_move::", "editor::device_panel::eq_panel::gestures::"},
                   {"editor::musical_time::", "editor::seek::", "editor::signed_frame_viewport::", "editor::viewport::"},
                   {"output_meter::logic::", "editor::refresh_state::", "editor::track_meters::decay::", "editor::track_meters::follower::", "editor::track_meters::scale::"},
                   {"section_appearance::", "section_reorder::"},
                   {"editor::midi_cue::", "editor::midi_cue::timing::", "editor::clip_range_fit::"},
                   {"editor::device_panel::vst3_parameters::", "editor::device_panel::vst3_parameters::row_projection::"},
                   {"editor::session_audio_panel::", "editor::session_audio_panel::playhead::"},
                   {"editor::overview_waveform::", "editor::waveform_columns::",
                    "editor::browser::ui_test::browser_hover_and_selection_keep_selected_waveform_and_surroundings_stable",
                    "editor::sampler_parameter::tests::source_hints_and_paint_require_same_view_source_facts_and_context",
                    "editor::waveform_source::", "editor::waveform::", "editor::waveform_geometry::",
                    "editor::waveform_geometry::grid::",
                    "editor::waveform_loop::", "editor::waveform_loop::projection::",
                    "editor::waveform_stretch::", "editor::waveform_stretch::accepted_root::",
                    "editor::waveform_stretch::projection::",
                    "editor::audio_event_editor::tests::moved_and_pasted_waveform_positions_read_event_composed_pcm",
                    "editor::audio_event_editor::free_placement_tests::finite_frame_drag_preserves_the_cycle_markers_and_root_waveform"}]
# Recovery's real prepared-audio test is a sibling so importing its state into
# the unit binary cannot also compile and run the integration fixture there.
UI_INTEGRATION_PAIRS = {"editor::automation_take_recovery::unit_tests::": "editor::automation_take_recovery::tests::",
                        "editor::selection::tests::": "editor::controller::project_replacement_tests::worker_snapshots_and_undo_keep_the_last_user_detail_focus",
                        "editor::session_audio_panel::unit_tests::": "editor::session_audio_panel::tests::",
                        "editor::waveform_geometry::grid::": "editor::waveform_geometry::tests::",
                        "output_meter::logic::": "output_meter::tests::",
                        "editor::overview_waveform::": "editor::browser::ui_test::browser_hover_and_selection_keep_selected_waveform_and_surroundings_stable",
                        "editor::waveform_columns::": "editor::sampler_parameter::tests::source_hints_and_paint_require_same_view_source_facts_and_context",
                        "editor::device_panel::eq_panel::response::": "editor::device_panel::tests::eq_ui_tests::graph_layout_tests::eq_axis_labels_are_logarithmic_nonoverlapping_and_keep_the_plot_hit_map",
                        "editor::computer_keyboard::state::": "editor::computer_keyboard::tests::",
                        "editor::recovery_state::": "editor::recovery_state_integration_tests::",
                        "editor::audio_import_settings::admission::": "editor::track_defaults::",
                        "editor::controller::audio_event_correlation::": "editor::controller::sampler_native_ui_protocol_tests::",
                        "editor::controller::source_binding_unit_tests::": "editor::controller::sampler_native_ui_protocol_tests::worker_preview_ack_waits_for_exact_submission_metadata",
                        "editor::fade_calculation::": "editor::fades::",
                        "editor::fade_handle_geometry::": "editor::fade_handles::",
                        "editor::midi_wheel_mapping::": "editor::midi_wheel::",
                        "editor::refresh_state::": "editor::output_meter_ui_test::",
                        "editor::session_grid::actions::": "editor::session_grid::tests::",
                        "editor::session_navigation::": "editor::session_navigation_ui_test::",
                        "editor::track_meters::selection::": "editor::track_meters::tests::",
                        "editor::track_meters::decay::": "editor::track_meters::tests::",
                        "editor::track_meters::follower::": "editor::track_meters::main_output::tests::",
                        "editor::poll_cadence::tests::": "editor::poll_cadence::ui_tests::",
                        "theme::": "editor::theme_settings::tests::"}
MOD_LINE = re.compile(r"(pub(\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;")
PATH_ATTRIBUTE = re.compile(r'#\[path\s*=\s*"([^"]+)"\]')


def module_paths(source_root: Path) -> dict[Path, str] | None:
    """Rust module path of every file reachable from the crate root.

    Walks top-level `mod NAME;` declarations from lib.rs (or main.rs), resolving
    `#[path]` as rustc does. Returns None when the crate includes source text
    (`include!`), so a module filter can never miss included tests.
    """
    root_file = next((source_root / name for name in ("lib.rs", "main.rs")
                      if (source_root / name).is_file()), None)
    if root_file is None:
        return None
    paths: dict[Path, str] = {root_file.resolve(): ""}
    pending = [root_file.resolve()]
    while pending:
        parent = pending.pop()
        text = parent.read_text(errors="replace")
        if "include!(" in text:
            return None
        lines = text.splitlines()
        for index, line in enumerate(lines):
            match = MOD_LINE.fullmatch(line.rstrip())
            if not match:
                continue
            attributes, cursor = [], index - 1
            while cursor >= 0 and lines[cursor].startswith(("#[", "//")):
                attributes.append(lines[cursor].strip())
                cursor -= 1
            name = match.group(3)
            explicit = next((PATH_ATTRIBUTE.fullmatch(a).group(1) for a in attributes
                             if PATH_ATTRIBUTE.fullmatch(a)), None)
            base = parent.parent if parent.name in {"mod.rs", "lib.rs", "main.rs"} else parent.with_suffix("")
            targets = [parent.parent / explicit] if explicit else [base / f"{name}.rs", base / name / "mod.rs"]
            target = next((t.resolve() for t in targets if t.is_file()), None)
            if target is None or target in paths:
                continue
            prefix = paths[parent]
            paths[target] = f"{prefix}::{name}" if prefix else name
            pending.append(target)
    return paths


def doc_example_owners(root: Path, paths: list[str], inventory: list[dict]) -> list[str]:
    """Owners whose changed Rust files carry doc examples (fenced code in doc comments)."""
    owners = set()
    for path in paths:
        file = root / path
        if not path.endswith(".rs") or not file.is_file():
            continue
        if any(line.lstrip().startswith(("///", "//!")) and "```" in line
               for line in file.read_text(errors="replace").splitlines()):
            owner = owner_package(path, inventory)
            if owner:
                owners.add(owner)
    return sorted(owners)


WAVEFORM_PAINT_GEOMETRY_UNIT = "editor::waveform::paint_geometry::"
WAVEFORM_PAINT_GEOMETRY_TESTS = {
    "editor::audio_event_editor::free_placement_tests::finite_frame_drag_preserves_the_cycle_markers_and_root_waveform",
    "editor::audio_event_editor::tests::moved_and_pasted_waveform_positions_read_event_composed_pcm",
    "editor::audio_event_editor::workflow_tests::cut_publishes_clipboard_on_ack_and_paste_survives_save_reopen_with_original_media",
    "editor::fade_handles::ui_tests::released_two_axis_fade_stays_painted_until_real_worker_ack",
    "editor::fades::musical_tests::transformed_cropped_musical_fades_keep_readouts_reset_adjust_and_projected_curve",
    "editor::waveform::tests::waveform_canvas_paints_full_clip_extent",
    "editor::waveform::waveform_header_ui_test::waveform_body_painter_stays_below_ordinary_selected_and_looped_headers",
    "editor::waveform_geometry::tests::event_detail_keeps_a_transient_in_its_visible_column",
}
CONTROLLER_PUBLICATION_TESTS = {
    "editor::controller::tests::earlier_session_rejection_is_not_reported_as_consumed",
    "editor::controller::comparison_source_binding_tests::comparison_exact_source_maps_verified_observations_without_status_oscillation_or_old_ack_promotion",
    "editor::controller::project_replacement_tests::receive_preserves_fresh_installation_through_edits_and_failed_open",
    "editor::controller::project_replacement_tests::stopped_restore_installs_fresh_without_replacing_project_or_losing_policy_to_later_edits",
    "editor::controller::sampler_native_ui_protocol_tests::worker_preview_ack_waits_for_exact_submission_metadata",
    "editor::controller::verified_gain_tests::verified_preview_submission_waits_for_exact_view_without_canonical_promotion",
}
WAVEFORM_PAINT_GEOMETRY_CONSUMERS = {
    "crates/hex-ui/src/editor/audio_event_editor/waveform_preview.rs",
    "crates/hex-ui/src/editor/audio_event_editor/free_placement_tests.rs",
    "crates/hex-ui/src/editor/audio_event_editor/tests.rs",
    "crates/hex-ui/src/editor/audio_event_editor/workflow_tests.rs",
    "crates/hex-ui/src/editor/fade_handles_ui_test.rs",
    "crates/hex-ui/src/editor/fades/musical_tests.rs",
    "crates/hex-ui/src/editor/tracks.rs",
    "crates/hex-ui/src/editor/waveform_geometry.rs",
    "crates/hex-ui/src/editor/waveform_header_ui_test.rs",
}

def module_test_filters(root: Path, paths: list[str], inventory: list[dict],
                        tested: list[str]) -> dict[str, list[str]]:
    """Per-owner libtest filters for module-scoped merges (user policy, 1 November 2026).

    A changed `src/` file selects the tests of its own module subtree, named
    `module::path::`. When that subtree declares no tests, the nearest ancestor
    module with tests is selected instead, so a change never merges untested.
    A crate root, an unresolved file, an `include!`, a non-`src/` input or a
    reviewed runtime consumer keeps the whole package (absent from the result).
    """
    filters: dict[str, set[str]] = {}
    whole: set[str] = set()
    resolved: dict[str, dict[Path, str] | None] = {}
    executable = [p for p in paths if not metadata_path(p)]
    for path in executable:
        owner = owner_package(path, inventory)
        if owner not in tested:
            continue
        package = next(p for p in inventory if p["name"] == owner)
        directory = (root / Path(package["manifest"]).parent).resolve()
        file = (root / path).resolve()
        if owner == "hex-storage" and path == "crates/hex-storage/src/text.rs":
            if file.is_file() and (directory / "src" / "text_tests.rs").is_file():
                filters.setdefault(owner, set()).add("text_tests::")
            else:
                whole.add(owner)
            continue
        if owner == "hex-ui":
            # This target compiles the real focus source by path. Keep mixed
            # owner-and-harness changes scoped while selecting its full binary.
            if file == directory / "tests" / "track_group_focus.rs" and file.is_file():
                filters.setdefault(owner, set()).add("editor::arrangement_focus::")
                continue
            if file == directory / "tests" / "midi_pressure_boundary.rs" and file.is_file():
                filters.setdefault(owner, set()).add("editor::midi_recording::")
                continue
            relative_path = file.relative_to(directory).as_posix()
            harness_target = UI_UNIT_HARNESS_FILES.get(relative_path)
            unit_modules = {module for module, target in UI_UNIT_TARGETS.items()
                            if (file == directory / "tests" / f"{target}.rs"
                                or target == harness_target)}
            unit_modules.update(UI_UNIT_HARNESS_MODULES.get(relative_path, set()))
            if unit_modules and file.is_file():
                filters.setdefault(owner, set()).update(unit_modules)
                continue
        if not path.endswith(".rs") or not file.is_file() or file.relative_to(directory).parts[0] != "src":
            whole.add(owner)
            continue
        if owner not in resolved:
            resolved[owner] = module_paths(directory / "src")
        modules = resolved[owner]
        module = None if modules is None else modules.get(file)
        if not module:
            whole.add(owner)
            continue
        def has_tests(candidate: str) -> bool:
            prefix = candidate + "::"
            if owner == "hex-ui" and (prefix in UI_MODULE_TEST_DEPENDENCIES
                                      or any(name.startswith(prefix) for name in UI_UNIT_TARGETS)):
                return True
            return any(TEST_ATTRIBUTE.search(f.read_text(errors="replace"))
                       for f, name in modules.items()
                       if name == candidate or name.startswith(candidate + "::"))
        while module and not has_tests(module):
            module = module.rpartition("::")[0]
        if not module:
            whole.add(owner)
        else:
            prefix = module + "::"
            owner_filters = filters.setdefault(owner, set())
            owner_filters.add(prefix)
            if owner == "hex-ui":
                owner_filters.update(UI_MODULE_TEST_DEPENDENCIES.get(prefix, ()))
    # The real viewport is an input to the waveform grid. Select its owner
    # when that source changes, including the grid's geometry consumers.
    if "crates/hex-ui/src/editor/viewport.rs" in executable and "hex-ui" in tested:
        filters.setdefault("hex-ui", set()).update({"editor::waveform_geometry::grid::", "editor::arrangement_map::model::"})
    if "crates/hex-ui/src/editor/navigation_metrics.rs" in executable and "hex-ui" in tested:
        # Keep the ignored profiler and a real rendered waveform ACK consumer.
        filters.setdefault("hex-ui", set()).update((
            "editor::clip_content_cache::profile::navigation::headless_navigation_framework_profile",
            "editor::waveform::resize_tests::musical_left_prefix_keeps_waveform_and_geometry_through_real_worker_ack",
        ))
    if "hex-ui" in tested and WAVEFORM_PAINT_GEOMETRY_CONSUMERS.intersection(executable):
        filters.setdefault("hex-ui", set()).add(WAVEFORM_PAINT_GEOMETRY_UNIT)
    # The theme source is compiled by path in each consumer. Run its own text
    # contract alongside storage's test, without widening other storage edits.
    if "crates/hex-storage/src/text.rs" in executable:
        for consumer in PATH_RUNTIME_CONSUMERS["crates/hex-storage/src/text.rs"] & set(tested):
            filters.setdefault(consumer, set()).add("theme::tests::")
    if "hex-sandbox" in tested:
        if "crates/hex-ui/src/theme.rs" in executable:
            filters.setdefault("hex-sandbox", set()).add("theme::")
        if "crates/hex-ui/src/theme/device.rs" in executable:
            filters.setdefault("hex-sandbox", set()).add("theme::device::")
    selected = {}
    for owner, names in filters.items():
        if owner in whole:
            continue
        if owner == "hex-ui":
            # Pairs and shared unit dependencies can introduce one another;
            # close over both before dropping paths covered by an ancestor.
            while True:
                before = len(names)
                for module, integration in UI_INTEGRATION_PAIRS.items():
                    if any(module.startswith(name) or name.startswith(module)
                           or (integration.startswith(name) and
                               (module.startswith(name) or name.count("::") > 1))
                           or name.startswith(integration)
                           for name in names):
                        names.update((module, integration))
                for cohort in UI_UNIT_COHORTS:
                    if names & cohort:
                        names.update(cohort)
                for cohort in UI_PREFIX_COHORTS:
                    if any(member.startswith(name) or name.startswith(member)
                           for name in names for member in cohort):
                        names.update(cohort)
                if len(names) == before:
                    break
            # These exact consumers cover their owners without expanding shared cohorts.
            for unit, consumers in (
                (WAVEFORM_PAINT_GEOMETRY_UNIT, WAVEFORM_PAINT_GEOMETRY_TESTS),
                (CONTROLLER_PUBLICATION_UNIT, CONTROLLER_PUBLICATION_TESTS),
                ("editor::device_panel::eq_panel::", (
                    "editor::device_panel::tests::eq_ui_tests::eq_notch_hover_matches_creation_and_node_priority_at_narrow_and_wide_sizes",
                    "editor::device_panel::tests::eq_ui_tests::graph_layout_tests::eq_default_spectrum_intent_and_explicit_toggle_do_not_edit_or_fake_audio",
                    "editor::device_panel::tests::eq_ui_tests::graph_layout_tests::both_visible_eq_curves_advance_from_real_fft_results_without_focus_stealing",
                )),
                ("editor::midi_draw::tests::", (
                    "editor::midi_canvas::pending_drag_tests::stroke_preview_tests::capture_tests::draw_and_erase_own_input_then_handoff_preview_without_holding_capture",
                    "editor::midi_canvas::created_selection_tests::committed_draw_selects_actual_authored_ids_with_last_primary_after_later_frames",
                )),
            ):
                if any(unit.startswith(name) for name in names):
                    names.update(consumers)
            if any("editor::arrangement_seek::unit_tests::".startswith(name) for name in names):
                names.update((
                    "editor::arrangement_seek::tests::post_loop_cursor_survives_actual_seek_worker_and_fenced_callback_installation",
                    "editor::arrangement_sections::tests::section_autoplay_click_dispatches_stateless_loop_and_toggle_off_cancels",
                    "editor::edit_cursor_ui_test::stateful_space_queues_play_from_without_a_seek_worker",
                ))
            if any("editor::device_panel::header_style_unit_tests::".startswith(name) for name in names):
                names.update((
                    "editor::device_panel::tests::header_style_tests::retained_source_path_indicator_is_rendered_with_the_existing_chain",
                    "editor::device_panel::tests::header_style_tests::device_shell_outline_is_visible_rounded_and_tracks_selection_across_owners",
                    "editor::device_panel::tests::sine_and_effect_place_their_shared_header_to_the_left_of_the_body",
                ))
            if "crates/hex-ui/src/theme.rs" in executable:
                names.update(("output_meter::tests::unit_tests::", "output_meter::tests::rendered::output_meter_uses_shared_level_colors_and_click_clear_clipping_caps"))
            if "crates/hex-ui/src/editor/time_selection.rs" in executable:
                names.update(("editor::clip_range_fit::tests::", "editor::clip_range_fit::ui_tests::"))
            if any("editor::controller_input::monitoring::unit_tests::".startswith(name) for name in names):
                names.update((
                    "editor::controller_input::monitoring::tests::actual_wire_ingress_reaches_host_pcm_and_stale_sink_cannot_cancel_reconnect",
                    "editor::controller_input::hardware_chord_step_uses_native_decoder_velocities_channel_ack_and_history",
                    "editor::controller_input::hardware_step_discontinuity_excludes_old_keys_and_cancel_refuses_false_rest",
                    "editor::midi_canvas::audition_tests::configured_controller_keeps_its_root_during_note_and_marquee_audition",
                ))
            if any("editor::device_panel::device_meter::tests::".startswith(name) for name in names):
                names.update((
                    "editor::device_panel::tests::clicking_a_device_meter_clears_its_held_overload_line",
                ))
            if {
                "crates/hex-ui/src/editor/session_navigation.rs",
                "crates/hex-ui/src/editor/session_navigation_ui_test.rs",
            }.intersection(executable):
                names.update((
                    "editor::controller::session_action_unit_tests::close_prompt_allows_only_owned_duplicate_continuations",
                    "editor::controller::session_action_unit_tests::rejected_scene_duplicate_clears_only_its_completion_selection_intent",
                ))
            if any(CONTROLLER_SESSION_ACTION_UNIT.startswith(name) for name in names):
                names.update((
                    "editor::controller::tests::earlier_session_rejection_is_not_reported_as_consumed",
                    "editor::session_navigation_ui_test::scene_launch_leads_the_caption_and_uses_the_guarded_offline_action",
                    "editor::session_navigation_ui_test::session_scene_duplicate_and_reorder_keys_use_worker_edits",
                    "editor::session_navigation_ui_test::failed_scene_duplicate_clears_its_pending_selection_intent",
                ))
            if any("editor::drag_autoscroll::tests::".startswith(name) for name in names):
                names.update((
                    "editor::clip_drag_ui_test::held_clip_drag_scrolls_preview_without_editing_and_rejects_stale_project",
                    "editor::clip_drag_ui_test::vertical_drag_scroll_invalidates_old_rows_before_fast_release",
                    "editor::clip_drag_ui_test::vertical_drag_scroll_retargets_stationary_clip_before_release",
                    "editor::drag_autoscroll::resize_pan_tests::resize_pan_preserves_stationary_snap_and_shift_stretch_preview_release",
                    "editor::drag::group_drag_ui_test::group_vertical_autoscroll_invalidates_all_previews_before_repaint",
                    "editor::trim::midi_left_extension_ui_tests::fresh_midi_left_extension_edge_pan_stops_interior_and_releases_exact_geometry",
                ))
            if any(unit.startswith(name) for unit in ('editor::device_automation::unit_tests::',)
                   for name in names):
                names.update((
                    "editor::device_automation::host_tests::host_header_picker_assigns_real_lanes_for_utility_and_other_native_owners",
                    "editor::device_automation::host_tests::utility_toggle_and_choice_offer_their_exact_recording_targets",
                ))
            native_recording_units = (
                "editor::native_automation_recording::unit_tests::",
                "editor::native_automation_recording::editor_input::tests::",
            )
            if any(unit.startswith(name) for unit in native_recording_units for name in names):
                names.update((
                    *native_recording_units,
                    "editor::native_automation_recording::tests::refused_and_lost_submissions_keep_delivered_receipts_until_recovery",
                    "editor::device_automation::host_tests::vst3_first_lane_menu_arms_and_disarms_the_raw_zero_knob",
                    "editor::device_automation::host_tests::vst3_dotted_guide_tracks_raw_zero_through_delayed_commit_ack",
                ))
            if {
                "crates/hex-ui/src/editor/missing_media.rs",
                "crates/hex-ui/src/editor/missing_media/tests.rs",
                "crates/hex-ui/src/editor/missing_media/ui_tests.rs",
            }.intersection(executable):
                names.update(("editor::missing_media::tests::", "editor::missing_media::ui_tests::"))
            if {
                "crates/hex-ui/src/editor/waveform_geometry.rs",
                "crates/hex-ui/src/editor/waveform_geometry/grid.rs",
                "crates/hex-ui/src/editor/waveform_resize_ui_test.rs",
                "crates/hex-ui/src/editor/clip_drag_ui_test.rs",
                "crates/hex-ui/src/editor/clip_loop_ui_tests.rs",
            }.intersection(executable):
                names.update((
                    "editor::waveform_geometry::tests::",
                    "editor::clip_drag_ui_test::rendered_audio_trim_updates_the_clip_waveform_geometry_and_commits_release_snap",
                    "editor::waveform::resize_tests::plain_lower_fade_prefix_uses_the_same_raw_source_mapping",
                    "editor::trim::loop_ui_tests::clip_loop_audio_preview_keeps_nonzero_waveform_across_the_extended_body",
                    "editor::clip_content_cache::profile::navigation::headless_navigation_framework_profile",
                ))
            if {
                "crates/hex-ui/src/editor/clip_header_loop.rs",
                "crates/hex-ui/src/editor/clip_loop_handles.rs",
            }.intersection(executable):
                names.update((
                    "editor::clip_header_loop::tests::",
                    "editor::trim::loop_handles::tests::",
                    "editor::waveform::waveform_header_ui_test::looped_header_paints_canonical_visible_cycle_edges_at_two_zooms",
                    "editor::trim::loop_ui_tests::clip_loop_outer_real_pointer_retains_released_paint_and_one_worker_history_edge",
                ))
            if "crates/hex-ui/src/editor/track_meters.rs" in executable:
                names.add("editor::track_layout_ui_test::track_meter_uses_own_peak_and_stays_inside_compact_header")
            if {
                "crates/hex-ui/src/editor/browser/history.rs",
                "crates/hex-ui/src/editor/browser_history_ui_test.rs",
            }.intersection(executable):
                names.update((
                    "editor::browser::history::tests::",
                    "editor::browser::ui_test::browser_history_ui_test::browser_history_typing_coalesces_until_navigation_and_branch_clears_forward",
                    "editor::browser::ui_test::browser_history_ui_test::browser_history_toolbar_restores_source_without_preview_or_import",
                    "editor::browser::ui_test::browser_history_ui_test::browser_history_removed_or_hidden_source_fallback_does_not_push_or_clear_forward",
                ))
            if "crates/hex-ui/src/editor/musical_audio_controls/tests.rs" in executable:
                names.update((
                    "editor::musical_audio_controls::tests::",
                    "editor::musical_audio_controls::ui_tests::algorithm_picker_dispatches_direct_choice_without_resetting_current_settings",
                    "editor::musical_audio_controls::ui_tests::selected_clip_beats_controls_are_visible_and_dispatch_existing_loop_envelope_bindings",
                    "editor::musical_audio_controls::ui_tests::selected_clip_warp_toggle_is_visible_and_dispatches_transient_beats",
                    "editor::musical_audio_controls::ui_tests::shift_selected_warp_applies_to_every_clip_with_its_own_source_region",
                ))
            if "crates/hex-ui/src/editor/tuning/plots.rs" in executable:
                names.add("editor::tuning::ui_tests::review_followup_dice_changes_the_actual_palette_without_editing_the_project")
            if {
                "crates/hex-ui/src/editor/waveform_stretch.rs",
                "crates/hex-ui/src/editor/waveform_stretch/projection.rs",
                "crates/hex-ui/src/editor/waveform_stretch/tests.rs",
            }.intersection(executable):
                names.add("editor::waveform::resize_tests::shift_stretch_keeps_live_sample_varied_waveforms_visible")
            if {
                "crates/hex-ui/src/editor/device_panel/eq_panel/gestures.rs",
                "crates/hex-ui/src/editor/device_panel/eq_panel/group_move.rs",
            }.intersection(executable):
                names.add("editor::device_panel::tests::eq_ui_tests::marquee_tests::eq_group_drag_moves_every_selected_node_and_undoes_in_one_step")
            if "crates/hex-ui/src/editor/pending_edit.rs" in executable:
                names.update((
                    "editor::clip_loop::audio::capture_tests::audio_loop_start_edge_keeps_input_and_pending_receipt_separate",
                    "editor::fade_handles::ui_tests::real_refusal_retires_frozen_curve_with_its_canonical_state",
                ))
        # A filter already covered by a broader selected module is redundant.
        selected[owner] = sorted(n for n in names if not any(n != o and n.startswith(o) for o in names))
    return selected


def test_only_owner(root: Path, path: str, inventory: list[dict]) -> str | None:
    """Owner of a Rust file no other package can observe, or None when unproven.

    Integration tests, benches and examples are owner-only targets. A `src/`
    file qualifies only when every declaration of it is directly preceded by
    `#[cfg(test)]` or `#[cfg(all(test, …))]`, or lives in a file that itself
    qualifies. `any(`/`not(` conditions, `include!(` in the package, missing,
    undeclared or inline-nested files are not proven and keep consumers.
    """
    owner = owner_package(path, inventory)
    package = next((p for p in inventory if p["name"] == owner), None)
    if package is None or not path.endswith(".rs") or path.endswith("/build.rs"):
        return None
    directory = root / Path(package["manifest"]).parent
    file = (root / path).resolve()
    if not file.is_file():
        return None
    relative = file.relative_to(directory.resolve())
    if relative.parts[0] in {"tests", "benches", "examples"}:
        return owner
    if relative.parts[0] != "src":
        return None
    source_root = directory / "src"
    if any("include!(" in f.read_text(errors="replace") for f in source_root.rglob("*.rs") if f.is_file()):
        return None
    declarations = module_declarations(source_root)

    def gated(module: Path, seen: frozenset) -> bool:
        uses = declarations.get(module, [])
        if not uses or module in seen:
            return False
        return all(any(TEST_CFG.fullmatch(a) and "any(" not in a and "not(" not in a for a in attributes)
                   or gated(parent, seen | {module}) for parent, attributes in uses)

    return owner if gated(file, frozenset()) else None


def ui_standalone_unit_targets(root: Path, paths: list[str], package: dict) -> dict[str, list[str]]:
    """Route only reviewed standalone test inputs without expanding source-owner cohorts."""
    directory = Path(package["manifest"]).parent
    roots = {f"tests/{target}.rs": target for target in (*UI_DEFAULT_STANDALONE_TESTS, *UI_UNIT_FEATURES)}
    selected: dict[str, set[str]] = {}
    whole_targets = set()
    for path in paths:
        relative_path = Path(path)
        file = root / relative_path
        if not relative_path.is_relative_to(directory) or file.is_symlink() or not file.is_file():
            return {}
        relative = relative_path.relative_to(directory).as_posix()
        target = roots.get(relative) or UI_UNIT_HARNESS_FILES.get(relative)
        if target is not None:
            whole_targets.add(target)
            continue
        filters = UI_UNIT_HARNESS_MODULES.get(relative)
        if filters is None:
            return {}
        for name in filters:
            selected.setdefault(UI_UNIT_TARGETS[name], set()).add(name)
    for target in whole_targets:
        selected[target] = set()
    for target in selected:
        matches = [entry for entry in package["targets"] if entry.get("name") == target and entry["kind"] == ["test"]]
        feature = UI_UNIT_FEATURES.get(target)
        expected_features = {feature} if feature else set()
        if len(matches) != 1 or matches[0].get("src_path") != f"tests/{target}.rs":
            return {}
        if (set(matches[0].get("required-features", ())) != expected_features
                or expected_features - package["features"].keys()):
            return {}
    return {target: sorted(filters) for target, filters in sorted(selected.items())}


def boundary_selection(root: Path, paths: list[str], inventory: list[dict], affected: list[str]) -> dict:
    """Narrow only reviewed runtime relationships; Cargo closure remains the lint boundary.

    Compressor state/DSP uses the unchanged Lattice host interfaces. Its real
    registry, session revision, callback/allocation, persistence and panel tests
    cover the dynamic path that Cargo edges cannot name. Other device modules,
    registration, host APIs, macros and build inputs retain package coverage.
    This is a reviewed ownership rule, not inference from diff size or signatures.
    """
    executable = [p for p in paths if not metadata_path(p)]
    names = {p["name"] for p in inventory}
    missing = {consumer for owner in affected for consumer in RUNTIME_CONSUMERS.get(owner, set())} - names
    if missing:
        raise ValueError("reviewed runtime consumers missing from Cargo inventory: " + ", ".join(sorted(missing)))
    path_consumers = set().union(*(PATH_RUNTIME_CONSUMERS.get(path, set()) for path in executable))
    missing = path_consumers - (names & set(affected))
    if missing:
        raise ValueError("reviewed path consumers missing from affected packages: " + ", ".join(sorted(missing)))
    compressor = {"crates/hex-devices/src/compressor.rs", "crates/hex-devices/src/compressor/tests.rs"}
    # hex-sandbox (design tool over hex-engine/hex-widgets) has no compressor
    # code, fixtures or tests; it is linted with every other Cargo consumer.
    known_consumers = {"hex-devices", "hex-engine", "hex-audio", "hex-ui", "hex-storage",
                       "hex-desktop", "hex-cli", "hex-export", "hex-session", "hex-media", "hex-stream",
                       "hex-sandbox"}
    reviewed_features = {"hex-engine": {"instrument-test-support"}, "hex-export": {"instrument-test-support"},
                         # Stream test hooks gate no compressor code or tests; compressor
                         # plans still lint those configurations (local-ci boundary_steps).
                         "hex-audio": {"allocation-probe", "stream-test-support"}, "hex-desktop": {"allocation-probe"},
                         "hex-ui": {"ui-test-support", "ui-unit-tier"}, "hex-session": {"source-overview-test-support"},
                         "hex-stream": {"allocation-probe", "test-support"},
                         # One-shot verified-WAV read fault injection; it does not
                         # change compressor processing or its host contract.
                         "hex-media": {"test-support"},
                         "hex-sandbox": {"ui-test-support", "ui-unit-tier"}}
    narrow = bool(executable) and set(executable) <= compressor and set(affected) <= known_consumers
    narrow = narrow and all(set(p["features"]) <= reviewed_features.get(p["name"], set())
                            for p in inventory if p["name"] in affected)
    if narrow:
        # Macro exports/includes can introduce consumers not represented by the
        # reviewed runtime relationship. Never try to parse Rust with a regex.
        narrow = all((root / p).is_file() and not any(token in (root / p).read_text()
                     for token in ("macro_rules!", "macro_export", "include!", "include_str!", "include_bytes!", "#[path"))
                     for p in compressor)
    tests = []
    if narrow:
        tests = [
            {"package": "hex-devices", "lib": True, "filter": "compressor::"},
            {"package": "hex-engine", "lib": True, "filter": "compressor"},
            {"package": "hex-engine", "test": "device_chain", "filter": ""},
            {"package": "hex-storage", "lib": True, "filter": "compressor"},
            {"package": "hex-audio", "lib": True, "filter": "compressor", "features": "allocation-probe"},
            {"package": "hex-audio", "test": "device_comparison", "filter": "", "features": "allocation-probe"},
            {"package": "hex-audio", "test": "allocation_probe_calibration", "filter": "", "features": "allocation-probe"},
            {"package": "hex-ui", "lib": True, "filter": "compressor", "features": "ui-test-support"},
        ]
    tested = owner_test_packages(paths, inventory, affected)
    # Only owners of changed files get module filters; a runtime consumer tested
    # on another owner's behalf has no changed file and keeps its whole suite.
    single_run = {} if narrow else single_run_packages(root, tested, inventory)
    standalone = {}
    if tested == ["hex-ui"] and single_run.get("hex-ui") == "ui-test-support":
        package = next(p for p in inventory if p["name"] == "hex-ui")
        standalone = ui_standalone_unit_targets(root, executable, package)
    module_filters = {} if narrow or standalone else module_test_filters(root, paths, inventory, tested)
    return {"kind": "compressor" if narrow else "packages", "tests": tests,
            "module_filters": module_filters,
            **({"ui_standalone_unit_targets": standalone} if standalone else {}),
            "doc_example_owners": doc_example_owners(root, paths, inventory),
            "test_packages": [] if narrow else tested,
            "single_run": single_run,
            "lint_packages": affected,
            "excluded_behavior_packages": sorted(set(affected) - {t["package"] for t in tests}) if narrow else [],
            "excluded_configurations": (["unrelated instrument/export and whole UI/audio suites: compressor-only implementation; unchanged shared host/registration/build inputs"] if narrow else []),
            "excluded_packages": sorted({p["name"] for p in inventory} - set(affected)),
            "rationale": ("unchanged host interfaces/registration; real compressor host, history, allocation, save/reopen and UI consumers; other device behavior omitted; all Cargo consumers linted"
                          if narrow else "owner packages and reviewed runtime consumers test in every configuration; every transitive Cargo consumer compiles and lints; consumer suites rely on the owner's boundary tests")}


def owner_test_packages(paths: list[str], inventory: list[dict], affected: list[str]) -> list[str]:
    """Packages whose test suites a merge runs: the boundary owners.

    The owning crate's tests guard its public contract. Cargo consumers still
    compile and lint against it, but their suites are not rerun: a consumer
    regression is a missing owner boundary test, fixed there (user policy,
    1 November 2026). Reviewed runtime consumers without a Cargo edge test too.
    A manifest or build-script change can alter how consumers build, so it keeps
    the whole affected closure.
    """
    executable = [p for p in paths if not metadata_path(p)]
    if any(p.endswith(("Cargo.toml", "/build.rs")) for p in executable):
        return sorted(affected)
    owners = {owner for p in executable if (owner := owner_package(p, inventory))}
    owners |= {consumer for owner in set(owners) for consumer in RUNTIME_CONSUMERS.get(owner, set())}
    owners |= set().union(*(PATH_RUNTIME_CONSUMERS.get(path, set()) for path in executable))
    tested = sorted(owners & set(affected))
    return tested or sorted(affected)


def health_selection(paths: list[str], inventory: list[dict]) -> dict:
    """Post-merge main health: the merged change's own packages, not every consumer.

    A short signal that main is not red right after a merge, never qualification.
    Tooling paths add the Python self-tests; unknown/high-risk inputs test the
    workspace; prose alone keeps the metadata check.
    """
    owners, python, workspace = set(), False, []
    for path in paths:
        if metadata_path(path):
            continue
        scope = classify([path], inventory)[0]
        if scope == "tooling":
            python = True
        elif scope in {"rust", "ui"} and owner_package(path, inventory):
            owners.add(owner_package(path, inventory))
            owners.update(PATH_RUNTIME_CONSUMERS.get(path, set()))
        else:
            workspace.append(path)
    return {"packages": sorted(owners), "python": python, "workspace_inputs": workspace}


# Build-speed and debug-info profile keys: they change codegen, never program
# semantics. debug-assertions, overflow-checks, panic, lto and rpath stay unreviewed.
PROFILE_SAFE_KEYS = {"opt-level", "debug", "codegen-units", "incremental", "split-debuginfo", "strip"}
PROFILE_PACKAGE = re.compile(r'profile\.[A-Za-z0-9_-]+\.package\."?([A-Za-z0-9_-]+)"?')


def strip_toml_comment(line: str) -> str:
    quote = None
    for index, char in enumerate(line):
        if quote:
            quote = None if char == quote else quote
        elif char in "\"'":
            quote = char
        elif char == "#":
            return line[:index]
    return line


def manifest_sections(text: str) -> dict[str, list[str]] | None:
    """Root-manifest tables to their meaningful lines; None when not plainly parseable."""
    sections: dict[str, list[str]] = {"": []}
    current = ""
    for raw in text.splitlines():
        line = strip_toml_comment(raw).strip()
        if not line:
            continue
        if line.startswith("["):
            if not line.endswith("]") or line.startswith("[["):
                return None
            current = line.strip("[] ")
            if current in sections:
                return None
            sections[current] = []
        else:
            sections[current].append(line)
    return sections


def profile_only_packages(root: Path, base: str, source: str, inventory: list[dict]) -> list[str] | None:
    """Workspace packages named by a root-manifest change that only retunes their profiles.

    Every changed table must be `[profile.NAME.package.MEMBER]` (never `"*"`) using
    only PROFILE_SAFE_KEYS. Such a change alters how those packages compile, not
    what they mean, so their own suites verify it (user policy, 1 November 2026).
    Anything else returns None and keeps the comprehensive audit.
    """
    def read(revision: str) -> str | None:
        result = subprocess.run(["git", "show", f"{revision}:Cargo.toml"], cwd=root, capture_output=True)
        return result.stdout.decode(errors="replace") if result.returncode == 0 else None

    old_text, new_text = read(base), read(source)
    old = None if old_text is None else manifest_sections(old_text)
    new = None if new_text is None else manifest_sections(new_text)
    if old is None or new is None:
        return None
    names = {package["name"] for package in inventory}
    packages = set()
    for header in sorted(set(old) | set(new)):
        if old.get(header) == new.get(header):
            continue
        match = PROFILE_PACKAGE.fullmatch(header)
        if not match or match.group(1) not in names:
            return None
        for line in [*old.get(header, []), *new.get(header, [])]:
            key = line.split("=", 1)[0].strip().strip('"') if "=" in line else None
            if key not in PROFILE_SAFE_KEYS:
                return None
        packages.add(match.group(1))
    return sorted(packages) or None


def crate_root_path(package: str, inventory: list[dict], root: Path) -> str | None:
    directory = Path(next(p for p in inventory if p["name"] == package)["manifest"]).parent
    return next((str(directory / "src" / name) for name in ("lib.rs", "main.rs")
                 if (root / directory / "src" / name).is_file()), None)


def resolve(root: Path, *, profile: str, base: str, source: str, metadata: dict,
            allow_same_tree_merge: bool = False) -> dict:
    if profile not in {"edit", "merge", "full", "release", "health"}:
        raise ValueError("unknown check profile")
    source_sha = git(root, "rev-parse", "--verify", source + "^{commit}").decode().strip()
    base_sha = git(root, "rev-parse", "--verify", base + "^{commit}").decode().strip()
    head = git(root, "rev-parse", "HEAD").decode().strip()
    if head != source_sha:
        if (not allow_same_tree_merge or git(root, "rev-parse", "HEAD^{tree}")
                != git(root, "rev-parse", source_sha + "^{tree}")):
            raise ValueError("requested source is not the current checkout")
        subprocess.run(["git", "merge-base", "--is-ancestor", source_sha, head], cwd=root, check=True)
    if subprocess.run(["git", "merge-base", "--is-ancestor", base_sha, source_sha],
                      cwd=root, capture_output=True).returncode != 0:
        raise ValueError(f"base {base_sha[:12]} is not an ancestor of source {source_sha[:12]}; "
                         "merge the current base into the candidate first, then rerun "
                         "(--carry-report <last passed summary.json> reuses unaffected steps)")
    paths = changed_paths(root, base_sha, source_sha)
    if not paths:
        raise ValueError("empty scope is not qualifying test evidence; use --profile full for an audit")
    inventory = cargo_inventory(metadata, root)
    inputs = runtime_inputs(root)
    if profile in {"merge", "full", "release"}:
        require_qualifying_inputs(inputs)
    if profile == "health" and git(root, "rev-parse", "--verify", source_sha + "^1").decode().strip() != base_sha:
        raise ValueError("health checks a merge commit against its first parent: use --base SOURCE^1")
    # A profile-only root-manifest change selects the retuned packages as if
    # their crate roots changed: their whole suites test, consumers lint.
    selection_paths = paths
    profile_packages = profile_only_packages(root, base_sha, source_sha, inventory) if "Cargo.toml" in paths else None
    profile_roots = [crate_root_path(p, inventory, root) for p in profile_packages or []]
    if profile_packages and all(profile_roots):
        selection_paths = [p for p in paths if p != "Cargo.toml"] + profile_roots
    scope, affected, reasons = classify(selection_paths, inventory)
    if selection_paths is not paths:
        reasons.append("profile-only root manifest change: owner suites of " + ", ".join(profile_packages)
                       + " (codegen keys only; semantics unchanged)")
    executable = [p for p in selection_paths if not metadata_path(p)]
    if profile == "merge" and scope in {"rust", "ui"} and executable:
        owners = [test_only_owner(root, p, inventory) for p in executable]
        if all(owners):
            excluded = sorted(set(affected) - set(owners))
            scope, affected = "rust", sorted(set(owners))
            reasons.append("test-only inputs (cfg(test) modules or owner test targets): owner packages "
                           "only; consumers cannot observe them: " + ", ".join(excluded))
    if profile in {"full", "release"}:
        scope = "full"
        reasons.append("explicit full audit/release request")
    result = {"policy_version": POLICY_VERSION, "profile": profile,
              "base": base_sha, "source": source_sha,
              "tree": git(root, "rev-parse", source_sha + "^{tree}").decode().strip(),
              "paths": paths, "scope": scope, "reasons": reasons,
              "affected_packages": affected, "cargo_inventory": inventory,
              "runtime_inputs": inputs,
              "python_tests": python_tests.select(
                  profile, [path for path in selection_paths if not metadata_path(path)],
                  inventory, UI_PACKAGES, python_tests.available_modules(root)),
              "qualifies_merge": profile in {"merge", "full", "release"}}
    if scope in {"rust", "ui"}:
        result["boundary"] = boundary_selection(root, selection_paths, inventory, affected)
    if profile == "health":
        result["health"] = health_selection(selection_paths, inventory)
    result["plan_sha256"] = hashlib.sha256(json.dumps(result, sort_keys=True).encode()).hexdigest()
    return result


def reuse_checks(root: Path, plan: dict, report: Path, env=None) -> dict:
    """Reuse runtime checks only for an ordered, prose-only continuation."""
    if plan["profile"] != "merge":
        raise ValueError("runtime reuse requires an explicit merge profile")
    if python_tests.native_acceptance_inputs(os.environ if env is None else env):
        raise ValueError("native/external acceptance inputs require fresh checks; runtime reuse refused")
    if plan["runtime_inputs"].get("symlinks"):
        raise ValueError("runtime continuation cannot certify external repository symlink targets")
    raw = report.read_bytes()
    evidence = json.loads(raw)
    if evidence.get("native_acceptance_opt_in") is not False:
        raise ValueError("native/external acceptance opt-in is present or unknown; runtime reuse refused")
    old_plan = evidence.get("plan")
    if (evidence.get("schema_version") != 2 or evidence.get("mode") not in {"full", "merge", "release"}
            or evidence.get("status") != "passed" or evidence.get("exit_code") != 0
            or evidence.get("stale") is not False or evidence.get("dirty") is not False
            or evidence.get("start_tree") != evidence.get("end_tree")
            or not evidence.get("steps") or any(s.get("status") != "passed" for s in evidence["steps"])
            or old_plan and not old_plan.get("qualifies_merge")
            or evidence.get("runtime_inputs", {}).get("sha256") != plan["runtime_inputs"]["sha256"]):
        raise ValueError("runtime report is incomplete/failed/stale/partial or executable inputs changed")
    previous = evidence["commit"]
    subprocess.run(["git", "merge-base", "--is-ancestor", previous, plan["source"]], cwd=root, check=True)
    delta = changed_paths(root, previous, plan["source"])
    if not delta or any(not metadata_path(p) for p in delta):
        raise ValueError("reused runtime evidence requires a nonempty prose-only continuation")
    dependency = evidence.get("target_cache", {}).get("dependency_inputs")
    if not isinstance(dependency, dict):
        raise ValueError("runtime report lacks guarded toolchain/dependency identity")
    result = dict(plan)
    result["reuse"] = {"report": str(report.resolve()), "sha256": hashlib.sha256(raw).hexdigest(),
                       "source": previous, "dependency_inputs": dependency,
                       "native_acceptance_opt_in": False}
    result["reasons"] = [*plan["reasons"], "runtime inputs identical; metadata continuation checked separately"]
    result.pop("plan_sha256")
    result["plan_sha256"] = hashlib.sha256(json.dumps(result, sort_keys=True).encode()).hexdigest()
    return result


# A base advance through these inputs can change every compiled configuration.
CARRY_BLOCKING_NAMES = {"Cargo.toml", "Cargo.lock", "build.rs", "rust-toolchain.toml", "rust-toolchain"}


def owner_package(path: str, inventory: list[dict]) -> str | None:
    owners = [p for p in inventory if path.startswith(str(Path(p["manifest"]).parent) + "/")]
    owner = max(owners, key=lambda p: len(p["manifest"]), default=None)
    return None if owner is None else owner["name"]


def dependency_closure(packages: list[str], inventory: list[dict]) -> set[str]:
    """Packages plus every workspace dependency of any kind, optional or target-specific."""
    by_name = {p["name"]: p for p in inventory}
    closure = set(packages)
    pending = list(packages)
    while pending:
        package = by_name.get(pending.pop())
        for dependency in package["dependencies"] if package else ():
            if dependency["name"] in by_name and dependency["name"] not in closure:
                closure.add(dependency["name"])
                pending.append(dependency["name"])
    return closure


def carry_basis(root: Path, plan: dict, evidence: dict, env=None) -> dict:
    """Prove a candidate differs from a passed one only by verbatim base-advance inputs.

    The earlier candidate (merge of the PR onto an older base) passed every step.
    The new candidate merged a newer base. Results may carry only when every
    executable path changed since the earlier candidate came unmodified from the
    base advance, touches none of the PR's own paths, and is ordinary package
    source (no manifest, lockfile, toolchain, build script, tooling or unknown
    input). Returns the directly changed owner packages and the Python selection
    that those paths require; callers decide per step and rerun on any doubt.
    """
    if plan["profile"] != "merge":
        raise ValueError("base-advance carry requires an explicit merge profile")
    if python_tests.native_acceptance_inputs(os.environ if env is None else env):
        raise ValueError("native/external acceptance inputs require fresh checks")
    old = evidence.get("plan") or {}
    steps = evidence.get("steps") or []
    tree = evidence.get("start_tree")
    if (evidence.get("schema_version") != 2 or evidence.get("mode") != "merge"
            or evidence.get("status") != "passed" or evidence.get("exit_code") != 0
            or evidence.get("stale") is not False or evidence.get("dirty") is not False
            or evidence.get("native_acceptance_opt_in") is not False
            or not tree or tree != evidence.get("end_tree")
            or evidence.get("resume") or old.get("reuse")
            or not old.get("qualifies_merge") or old.get("profile") != "merge"
            or not steps or any(s.get("status") not in {"passed", "carried"} for s in steps)):
        raise ValueError("carried report is not a complete passed merge-profile run")
    if old.get("cargo_inventory") != plan["cargo_inventory"]:
        raise ValueError("Cargo workspace inventory changed since the carried report")
    previous, previous_base = old["source"], old["base"]
    if evidence.get("commit") != previous or tree.get("commit") != previous:
        raise ValueError("carried report does not identify its own candidate")
    for ancestor, descendant, what in ((previous_base, plan["base"], "base"),
                                       (previous, plan["source"], "candidate")):
        if subprocess.run(["git", "merge-base", "--is-ancestor", ancestor, descendant],
                          cwd=root, capture_output=True).returncode != 0:
            raise ValueError(f"carried report's {what} is not an ancestor of the current {what}")
    delta = changed_paths(root, previous, plan["source"])
    executable = [p for p in delta if not metadata_path(p)]
    upstream = set(changed_paths(root, previous_base, plan["base"]))
    own = set(old.get("paths", []))

    def blob(revision: str, path: str) -> str | None:
        result = subprocess.run(["git", "rev-parse", "--verify", "--quiet", f"{revision}:{path}"],
                                cwd=root, capture_output=True, text=True)
        return result.stdout.strip() if result.returncode == 0 else None

    inventory = plan["cargo_inventory"]
    owners = set()
    for path in executable:
        if path in own:
            raise ValueError("candidate's own path changed since the carried report: " + path)
        if path not in upstream or blob(plan["source"], path) != blob(plan["base"], path):
            raise ValueError("change is not a verbatim base-advance input: " + path)
        if Path(path).name in CARRY_BLOCKING_NAMES or classify([path], inventory)[0] not in {"rust", "ui"}:
            raise ValueError("dependency, toolchain, build, tooling or unknown input changed: " + path)
        owner = owner_package(path, inventory)
        if owner is None:
            raise ValueError("changed input has no workspace owner: " + path)
        owners.add(owner)
    return {"report_commit": previous, "report_base": previous_base,
            "delta_paths": delta, "delta_owners": sorted(owners),
            "python_tests": python_tests.select(
                "merge", executable, inventory, UI_PACKAGES,
                python_tests.available_modules(root), env=env)}


def check_metadata(root: Path, base: str, source: str) -> list[str]:
    """Check changed prose without pretending to automate editorial claims."""
    checked = []
    for name in changed_paths(root, base, source):
        if not metadata_path(name):
            raise ValueError("metadata-only check received executable input: " + name)
        path = root / name
        if not path.exists():  # Deleted prose still participates in the plan.
            continue
        text = path.read_text(encoding="utf-8")
        if any(line.startswith(("<<<<<<< ", ">>>>>>> ")) for line in text.splitlines()):
            raise ValueError("unresolved prose conflict: " + name)
        for target in re.findall(r"\]\(<?([^\s)>]+)>?\)", text):
            if ":" in target or target.startswith(("/", "#")):
                continue
            relative = unquote(target.split("#", 1)[0])
            destination = (path.parent / relative).resolve()
            if not destination.is_relative_to(root.resolve()) or not destination.exists():
                raise ValueError(f"broken repository link in {name}: {target}")
        checked.append(name)
    subprocess.run(["git", "diff", "--check", base, source], cwd=root, check=True)
    return checked


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check-metadata", action="store_true", required=True)
    parser.add_argument("--base", required=True)
    parser.add_argument("--source", required=True)
    args = parser.parse_args()
    print(json.dumps({"metadata_checked": check_metadata(Path.cwd(), args.base, args.source),
                      "claim_review": "editorial claims require final diff review"}, indent=2))
