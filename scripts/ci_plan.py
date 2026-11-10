"""Deterministic local check selection; this module never builds or mutates Git."""
from __future__ import annotations

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
from urllib.parse import unquote

_PYTHON_SPEC = importlib.util.spec_from_file_location(
    "hex_python_test_plan", Path(__file__).with_name("python_test_plan.py"))
if _PYTHON_SPEC is None or _PYTHON_SPEC.loader is None:
    raise RuntimeError("Python self-test impact policy unavailable")
python_tests = importlib.util.module_from_spec(_PYTHON_SPEC)
_PYTHON_SPEC.loader.exec_module(python_tests)

POLICY_VERSION = 51
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
        manifest = Path(package["manifest_path"]).resolve().relative_to(root.resolve()).as_posix()
        inventory.append({
            "name": package["name"], "manifest": manifest,
            "features": package["features"],
            "targets": [{k: t[k] for k in ("name", "kind", "required-features") if k in t}
                        for t in package["targets"]],
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
#   debug selectors only) and hex-session's overview hooks; no cfg(not) gates.
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


def single_run_packages(root: Path, affected: list[str], inventory: list[dict]) -> dict[str, str]:
    """Affected packages whose non-companion feature is reviewed additive.

    Any line combining `not(` with that feature in the package's Rust sources is a
    tripwire: the package keeps both default and feature test runs.
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
        gated = any("not(" in line and needle in line
                    for source in directory.rglob("*.rs") if source.is_file()
                    for line in source.read_text(errors="replace").splitlines())
        if package["name"] == "hex-ui" and "ui-unit-tier" in package["features"]:
            # Fail closed if the reviewed test-only companion ever gates
            # production code or gains an unreviewed expression.
            companion = 'feature = "ui-unit-tier"'
            gated |= any(companion in line and line.strip() != '#[cfg(all(test, feature = "ui-unit-tier"))]'
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
UI_UNIT_TARGETS = {"editor::waveform_source::": "unit_editor_logic",
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
                   "editor::recovery_state::": "unit_editor_state",
                   "editor::refresh_state::": "unit_editor_logic",
                   "editor::seek::": "unit_timeline",
                   "editor::session_navigation::": "unit_editor_logic",
                   "editor::session_route_view::": "unit_editor_logic",
                   "editor::signed_frame_viewport::": "unit_timeline",
                   "editor::track_meters::decay::": "unit_editor_logic",
                   "editor::track_meters::follower::": "unit_editor_logic",
                   "editor::track_meters::scale::": "unit_editor_state",
                   "editor::track_meters::selection::": "unit_editor_logic",
                   "editor::user_errors::": "unit_editor_logic",
                   "editor::viewport::": "unit_editor_state",
                   "section_appearance::": "unit_section",
                   "section_hit::": "unit_section",
                   "section_input::": "unit_section",
                   "section_reorder::": "unit_section",
                   "section_resize::": "unit_section",
                   "section_selection::": "unit_section",
                   "theme::": "unit_editor_logic"}
UI_UNIT_FEATURES = {"unit_piano_roll": "ui-unit-tier", "unit_timeline": "ui-unit-tier", "unit_automation": "ui-unit-tier", "unit_device": "ui-unit-tier", "unit_section": "ui-unit-tier", "unit_editor_logic": "ui-unit-tier"}
# These geometry modules call one another; a change to one must exercise the
# complete pure boundary rather than only its own test-name prefix.
UI_UNIT_COHORTS = [{"editor::midi_drag::", "editor::midi_group_drag::", "editor::midi_scene::", "editor::midi_selection::", "editor::midi_stroke::", "editor::midi_velocity::", "editor::midi_viewport_grid::"},
                   {"editor::device_panel::eq_panel::group_move::", "editor::device_panel::eq_panel::gestures::"},
                   {"editor::musical_time::", "editor::seek::", "editor::signed_frame_viewport::", "editor::viewport::"},
                   {"editor::refresh_state::", "editor::track_meters::decay::", "editor::track_meters::follower::", "editor::track_meters::scale::"},
                   {"section_appearance::", "section_reorder::"},
                   {"editor::waveform_source::", "editor::waveform::", "editor::waveform_geometry::",
                    "editor::waveform_loop::", "editor::waveform_stretch::",
                    "editor::audio_event_editor::tests::moved_and_pasted_waveform_positions_read_event_composed_pcm",
                    "editor::audio_event_editor::free_placement_tests::finite_frame_drag_preserves_the_cycle_markers_and_root_waveform"}]
# Recovery's real prepared-audio test is a sibling so importing its state into
# the unit binary cannot also compile and run the integration fixture there.
UI_INTEGRATION_PAIRS = {"editor::computer_keyboard::state::": "editor::computer_keyboard::tests::",
                        "editor::recovery_state::": "editor::recovery_state_integration_tests::",
                        "editor::audio_import_settings::admission::": "editor::track_defaults::",
                        "editor::controller::audio_event_correlation::": "editor::controller::sampler_native_ui_protocol_tests::",
                        "editor::fade_calculation::": "editor::fades::",
                        "editor::fade_handle_geometry::": "editor::fade_handles::",
                        "editor::midi_wheel_mapping::": "editor::midi_wheel::",
                        "editor::refresh_state::": "editor::output_meter_ui_test::",
                        "editor::session_navigation::": "editor::session_navigation_ui_test::",
                        "editor::track_meters::selection::": "editor::track_meters::tests::",
                        "editor::track_meters::decay::": "editor::track_meters::tests::",
                        "editor::track_meters::follower::": "editor::track_meters::main_output::tests::",
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
            unit_modules = {module for module, target in UI_UNIT_TARGETS.items()
                            if file == directory / "tests" / f"{target}.rs"}
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
            if owner == "hex-ui" and any(name.startswith(candidate + "::")
                                         for name in UI_UNIT_TARGETS):
                return True
            return any(TEST_ATTRIBUTE.search(f.read_text(errors="replace"))
                       for f, name in modules.items()
                       if name == candidate or name.startswith(candidate + "::"))
        while module and not has_tests(module):
            module = module.rpartition("::")[0]
        if not module:
            whole.add(owner)
        else:
            filters.setdefault(owner, set()).add(module + "::")
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
                if len(names) == before:
                    break
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
    module_filters = {} if narrow else module_test_filters(root, paths, inventory, tested)
    return {"kind": "compressor" if narrow else "packages", "tests": tests,
            "module_filters": module_filters,
            "doc_example_owners": doc_example_owners(root, paths, inventory),
            "test_packages": [] if narrow else tested,
            "single_run": {} if narrow else single_run_packages(root, tested, inventory),
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
