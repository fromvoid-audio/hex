#!/usr/bin/env python3
"""Verify the narrow external GPUI baseline/patch and workspace exclusion."""
import hashlib
import json
from pathlib import Path
import re


def section(source, name):
    match = re.search(r"(?ms)^\[" + re.escape(name) + r"\]\s*\n(.*?)(?=^\[|\Z)", source)
    return match.group(1) if match else ""


def string_value(source, key):
    match = re.search(r'(?m)^\s*' + re.escape(key) + r'\s*=\s*"([^"]*)"', source)
    return match.group(1) if match else None

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
    "src/interactive.rs",
    "src/platform/mac/events.rs",
    "src/platform/mac/window.rs",
    "src/platform/windows/events.rs",
    "src/platform/linux/wayland/client.rs",
    "src/platform/linux/x11/client.rs",
    "src/platform.rs",
    "src/platform/mac/platform.rs",
    "src/platform/linux/platform.rs",
    "src/platform/linux/wayland.rs",
    "src/platform/windows/util.rs",
    "src/app/test_context.rs",
    "src/window.rs",
}
if set(manifest["patched_files"]) != expected_changes:
    errors.append("unexpected GPUI patch scope")
for name, original in manifest["original_files"].items():
    path = vendor / name
    expected = manifest["patched_files"].get(name, original)
    if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != expected:
        errors.append(f"GPUI source/provenance mismatch: {name}")
local_metadata = {"HEX-PATCHES.md", "hex-provenance.json", "hex-patches.diff"}
extra = {
    str(path.relative_to(vendor)) for path in vendor.rglob("*") if path.is_file()
} - set(manifest["original_files"]) - local_metadata
if extra:
    errors.append(f"unexpected files in external GPUI: {sorted(extra)}")
lock = re.split(r"(?m)^\[\[package\]\]\s*\n", (root / "Cargo.lock").read_text())
gpui = [item for item in lock if string_value(item, "name") == "gpui"]
if len(gpui) != 1 or string_value(gpui[0], "source") is not None or string_value(gpui[0], "version") != "0.2.2":
    errors.append("lockfile must resolve exactly the local GPUI 0.2.2 patch")
if errors:
    raise SystemExit("\n".join(errors))
print("GPUI patch passed: verified baseline, documented input, cursor, titlebar and test-support deltas, excluded native dependency")
