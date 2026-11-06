#!/usr/bin/env python3
"""Verify the narrow external GPUI baseline/patch and workspace exclusion."""
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile


def section(source, name):
    match = re.search(r"(?ms)^\[" + re.escape(name) + r"\]\s*\n(.*?)(?=^\[|\Z)", source)
    return match.group(1) if match else ""


def string_value(source, key):
    match = re.search(r'(?m)^\s*' + re.escape(key) + r'\s*=\s*"([^"]*)"', source)
    return match.group(1) if match else None

def verify_cumulative_patch(vendor, manifest):
    """Restore published bytes from private copies; never mutate the dependency.

    Source hashes are checked by the caller. Reverse application also verifies
    additions and catches omitted hunks even when the current source hashes match.
    Existing cumulative zero-context hunks require --unidiff-zero.
    """
    errors = []
    with tempfile.TemporaryDirectory(prefix="hex-gpui-patch-") as directory:
        restored = Path(directory)
        for name in {*manifest["patched_files"], *manifest.get("added_files", {})}:
            relative = Path(name)
            if relative.is_absolute() or ".." in relative.parts:
                return ["invalid GPUI patch source path"]
            destination = restored / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(vendor / relative, destination)
        try:
            result = subprocess.run(
                ["git", "apply", "--reverse", "--unidiff-zero",
                 str((vendor / "hex-patches.diff").resolve())],
                cwd=restored, capture_output=True, text=True, timeout=30,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            return [f"could not verify GPUI cumulative patch: {error}"]
        if result.returncode:
            return ["GPUI cumulative patch cannot be reverse-applied: "
                    + result.stderr.strip()[:2000]]
        for name in manifest["patched_files"]:
            path = restored / name
            if (path.is_symlink() or not path.is_file()
                    or hashlib.sha256(path.read_bytes()).hexdigest()
                    != manifest["original_files"][name]):
                errors.append(f"GPUI cumulative patch does not restore original source: {name}")
        actual = {str(path.relative_to(restored)) for path in restored.rglob("*")
                  if path.is_file() or path.is_symlink()}
        unexpected = actual - set(manifest["patched_files"])
        if unexpected:
            errors.append(f"GPUI cumulative patch restores undeclared sources: {sorted(unexpected)}")
        for name in manifest.get("added_files", {}):
            if (restored / name).exists():
                errors.append(f"GPUI cumulative patch does not remove added source: {name}")
    return errors


def main():
    root = Path(__file__).resolve().parents[1]
    vendor = root / "third_party/gpui"
    manifest = json.loads((vendor / "hex-provenance.json").read_text())
    workspace = (root / "Cargo.toml").read_text()
    package = section((vendor / "Cargo.toml").read_text(), "package")
    errors = []
    if not re.search(r'(?m)^gpui\s*=\s*\{\s*path\s*=\s*"third_party/gpui"\s*\}',
                     section(workspace, "patch.crates-io")):
        errors.append("GPUI must resolve through the documented external patch")
    excluded = re.search(r"(?s)exclude\s*=\s*\[(.*?)\]", section(workspace, "workspace"))
    if not excluded or '"third_party/gpui"' not in excluded.group(1):
        errors.append("external GPUI must not become a Hex workspace member")
    if string_value(section(workspace, "workspace.lints.rust"), "unsafe_code") != "forbid":
        errors.append("Hex workspace unsafe prohibition must remain intact")
    if string_value(package, "name") != manifest["package"] or string_value(package, "version") != manifest["version"]:
        errors.append("GPUI package identity differs from the recorded baseline")
    expected_changes = {
        "src/app.rs",
        "src/interactive.rs",
        "src/platform/mac/events.rs",
        "src/platform/mac/window.rs",
        "src/platform/blade/blade_renderer.rs",
        "src/platform/windows/events.rs",
        "src/platform/linux/wayland/client.rs",
        "src/platform/linux/x11/client.rs",
        "src/platform.rs",
        "src/platform/mac/platform.rs",
        "src/platform/linux/platform.rs",
        "src/platform/linux/wayland.rs",
        "src/platform/windows/util.rs",
        "src/app/test_context.rs",
        "src/platform/test/platform.rs",
        "src/window.rs",
        "src/view.rs",
        "src/scene.rs",
        "src/bounds_tree.rs",
        "src/element.rs",
        "src/taffy.rs",
        "src/gpui.rs",
        "src/elements/div.rs",
        "src/view.rs",
        "src/text_system.rs",
        "src/text_system/line.rs",
    }
    if set(manifest["patched_files"]) != expected_changes:
        errors.append("unexpected GPUI patch scope")
    expected_additions = {
        "src/accessibility.rs",
        "src/platform/accessibility_bridge.rs",
        "src/platform/mac/accessibility.rs",
    }
    if set(manifest.get("added_files", {})) != expected_additions:
        errors.append("unexpected GPUI added-file scope")
    for name, expected in manifest.get("added_files", {}).items():
        path = vendor / name
        if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != expected:
            errors.append(f"GPUI added-source/provenance mismatch: {name}")
    for name, original in manifest["original_files"].items():
        path = vendor / name
        expected = manifest["patched_files"].get(name, original)
        if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != expected:
            errors.append(f"GPUI source/provenance mismatch: {name}")
    local_metadata = {"HEX-PATCHES.md", "hex-provenance.json", "hex-patches.diff"}
    extra = {
        # Finder metadata is ignored by Git and is not a dependency build input.
        # Keep symlinks and every other unexpected file visible to this check.
        str(path.relative_to(vendor)) for path in vendor.rglob("*")
        if path.is_symlink() or (path.is_file() and path.name != ".DS_Store")
    } - set(manifest["original_files"]) - expected_additions - local_metadata
    if extra:
        errors.append(f"unexpected files in external GPUI: {sorted(extra)}")
    lock = re.split(r"(?m)^\[\[package\]\]\s*\n", (root / "Cargo.lock").read_text())
    gpui = [item for item in lock if string_value(item, "name") == "gpui"]
    if len(gpui) != 1 or string_value(gpui[0], "source") is not None or string_value(gpui[0], "version") != "0.2.2":
        errors.append("lockfile must resolve exactly the local GPUI 0.2.2 patch")
    if not errors:
        errors.extend(verify_cumulative_patch(vendor, manifest))
    if errors:
        raise SystemExit("\n".join(errors))
    print("GPUI patch passed: verified baseline, input/cursor/titlebar, cache epochs/replay, test support and bounded accessibility, excluded native dependency")


if __name__ == "__main__":
    main()
