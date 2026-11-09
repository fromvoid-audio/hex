#!/usr/bin/env python3
"""Verify the two pinned external future-compatibility patches and provenance."""
import difflib
import hashlib
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
PATCHES = {
    "block": (
        "0.1.6",
        "0d8c1fef690941d3e7788d328517591fecc684c084084702d6ff1641e993699a",
        "enum Class { }",
        "#[repr(C)]\nstruct Class {\n    _private: [u8; 0],\n}",
    ),
    "proc-macro-error2": (
        "2.0.1",
        "11ec05c52be0a07b08061f7dd003e7d7092e0472bc731b4af7bb1ef876109802",
        "extern crate proc_macro;",
        "pub extern crate proc_macro;",
    ),
}


def sha(data):
    return hashlib.sha256(data).hexdigest()


def section(source, name):
    match = re.search(r"(?ms)^\[" + re.escape(name) + r"\]\s*\n(.*?)(?=^\[|\Z)", source)
    return match.group(1) if match else ""


def string_value(source, key):
    match = re.search(r'(?m)^\s*' + re.escape(key) + r'\s*=\s*"([^"\n]*)"', source)
    return match.group(1) if match else None


def check():
    workspace = (ROOT / "Cargo.toml").read_text()
    lock = re.split(r"(?m)^\[\[package\]\]\s*\n", (ROOT / "Cargo.lock").read_text())
    assert string_value(section(workspace, "workspace.lints.rust"), "unsafe_code") == "forbid"
    for name, (version, checksum, old, new) in PATCHES.items():
        relative = f"third_party/{name}"
        vendor = ROOT / relative
        manifest = json.loads((vendor / "hex-provenance.json").read_text())
        assert manifest["package"] == name and manifest["version"] == version
        assert manifest["archive_sha256"] == checksum
        assert manifest["archive_url"] == f"https://static.crates.io/crates/{name}/{name}-{version}.crate"
        assert re.search(
            r'(?m)^' + re.escape(name) + r'\s*=\s*\{\s*path\s*=\s*"'
            + re.escape(relative) + r'"\s*\}\s*$', section(workspace, "patch.crates-io"),
        )
        excluded = re.search(r"(?s)exclude\s*=\s*(\[.*?\])", section(workspace, "workspace"))
        assert excluded and relative in json.loads(excluded.group(1))
        package = section((vendor / "Cargo.toml").read_text(), "package")
        assert string_value(package, "name") == name and string_value(package, "version") == version
        entries = [entry for entry in lock if string_value(entry, "name") == name]
        assert len(entries) == 1 and string_value(entries[0], "version") == version
        assert string_value(entries[0], "source") is None and string_value(entries[0], "checksum") is None
        assert set(manifest["patched_files"]) == {"src/lib.rs"}
        metadata = {"HEX-PATCHES.md", "hex-provenance.json", "hex-patches.diff"}
        # Finder metadata is ignored by Git and is not a dependency build input.
        # Keep symlinks and every other unexpected file visible to this check.
        actual = {
            str(path.relative_to(vendor)) for path in vendor.rglob("*")
            if path.is_symlink() or (path.is_file() and path.name != ".DS_Store")
        }
        assert actual == set(manifest["original_files"]) | metadata, (
            f"unexpected files in {relative}: {sorted(actual - set(manifest['original_files']) - metadata)}"
        )
        for path, original in manifest["original_files"].items():
            assert sha((vendor / path).read_bytes()) == manifest["patched_files"].get(path, original), path
        patched = (vendor / "src/lib.rs").read_text()
        assert patched.count(new) == 1
        # Explicit C is the previous implicit ABI, including macro-expanded callbacks.
        # Reverse this exact finite delta before checking the published archive hash.
        if name == "block":
            assert patched.count('extern "C" ') == 10
            assert not re.search(r'extern\s+(?:fn|\{)', patched)
            original = patched.replace('extern "C" ', 'extern ')
        else:
            original = patched
        original = original.replace(new, old, 1)
        assert sha(original.encode()) == manifest["original_files"]["src/lib.rs"]
        diff = "".join(difflib.unified_diff(
            original.splitlines(keepends=True), patched.splitlines(keepends=True),
            fromfile="a/src/lib.rs", tofile="b/src/lib.rs", n=0 if name == "block" else 3,
        ))
        assert (vendor / "hex-patches.diff").read_text() == diff
    print("dependency patches passed: pinned baselines, exact opaque-marker/C-ABI/visibility deltas, excluded native dependencies")


if __name__ == "__main__":
    try:
        check()
    except (AssertionError, KeyError, ValueError, OSError) as error:
        raise SystemExit(f"dependency patch/provenance mismatch: {error}") from error
