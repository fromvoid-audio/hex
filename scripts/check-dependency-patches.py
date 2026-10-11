#!/usr/bin/env python3
"""Verify the two pinned external future-compatibility patches and provenance."""
import difflib
import hashlib
import json
from pathlib import Path
import tomllib

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


def check():
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text())
    assert workspace["workspace"]["lints"]["rust"]["unsafe_code"] == "forbid"
    for name, (version, checksum, old, new) in PATCHES.items():
        relative = f"third_party/{name}"
        vendor = ROOT / relative
        manifest = json.loads((vendor / "hex-provenance.json").read_text())
        assert manifest["package"] == name and manifest["version"] == version
        assert manifest["archive_sha256"] == checksum
        assert manifest["archive_url"] == f"https://static.crates.io/crates/{name}/{name}-{version}.crate"
        assert workspace["patch"]["crates-io"][name] == {"path": relative}
        assert relative in workspace["workspace"]["exclude"]
        package = tomllib.loads((vendor / "Cargo.toml").read_text())["package"]
        assert package["name"] == name and package["version"] == version
        entries = [entry for entry in lock["package"] if entry["name"] == name]
        assert len(entries) == 1 and entries[0]["version"] == version
        assert "source" not in entries[0] and "checksum" not in entries[0]
        assert set(manifest["patched_files"]) == {"src/lib.rs"}
        metadata = {"HEX-PATCHES.md", "hex-provenance.json", "hex-patches.diff"}
        actual = {str(path.relative_to(vendor)) for path in vendor.rglob("*") if path.is_file()}
        assert actual == set(manifest["original_files"]) | metadata
        for path, original in manifest["original_files"].items():
            assert sha((vendor / path).read_bytes()) == manifest["patched_files"].get(path, original), path
        patched = (vendor / "src/lib.rs").read_text()
        assert patched.count(new) == 1
        original = patched.replace(new, old, 1)
        assert sha(original.encode()) == manifest["original_files"]["src/lib.rs"]
        diff = "".join(difflib.unified_diff(
            original.splitlines(keepends=True), patched.splitlines(keepends=True),
            fromfile="a/src/lib.rs", tofile="b/src/lib.rs",
        ))
        assert (vendor / "hex-patches.diff").read_text() == diff
    print("dependency patches passed: pinned baselines, exact opaque-marker/visibility deltas, excluded native dependencies")


if __name__ == "__main__":
    try:
        check()
    except (AssertionError, KeyError, ValueError, OSError) as error:
        raise SystemExit(f"dependency patch/provenance mismatch: {error}") from error
