#!/usr/bin/env python3
"""Verify the pinned external CPAL source and its recorded narrow delta."""

import hashlib
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def verify_cumulative_patch(root: Path, manifest: dict) -> list[str]:
    """Reverse-apply hex-patches.diff onto private copies and check original hashes."""
    expected = manifest["files"]
    changed = [name for name, digests in expected.items() if digests["original"] != digests["current"]]
    errors: list[str] = []
    with tempfile.TemporaryDirectory(prefix="hex-cpal-patch-") as directory:
        restored = Path(directory)
        for name in changed:
            relative = Path(name)
            if relative.is_absolute() or ".." in relative.parts:
                return ["invalid CPAL patch source path"]
            destination = restored / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(root / relative, destination)
        try:
            result = subprocess.run(
                ["git", "apply", "--reverse", str((root / "hex-patches.diff").resolve())],
                cwd=restored,
                capture_output=True,
                text=True,
                timeout=30,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            return [f"could not verify CPAL cumulative patch: {error}"]
        if result.returncode:
            return ["CPAL cumulative patch cannot be reverse-applied: " + result.stderr.strip()[:2000]]
        for name in changed:
            path = restored / name
            if (
                path.is_symlink()
                or not path.is_file()
                or hashlib.sha256(path.read_bytes()).hexdigest() != expected[name]["original"]
            ):
                errors.append(f"CPAL cumulative patch does not restore original source: {name}")
        actual = {
            str(path.relative_to(restored))
            for path in restored.rglob("*")
            if path.is_file() or path.is_symlink()
        }
        unexpected = actual - set(changed)
        if unexpected:
            errors.append(f"CPAL cumulative patch restores undeclared sources: {sorted(unexpected)}")
    return errors


def main() -> int:
    root = Path(__file__).resolve().parent.parent / "third_party" / "cpal"
    manifest = json.loads((root / "hex-provenance.json").read_text())
    expected = manifest["files"]
    metadata = {"HEX-PATCHES.md", "hex-patches.diff", "hex-provenance.json"}
    # Finder metadata is ignored by Git and is not a dependency build input.
    # Keep symlinks and every other unexpected file visible to this check.
    actual = {
        str(path.relative_to(root))
        for path in root.rglob("*")
        if "target" not in path.relative_to(root).parts
        and (path.is_symlink() or (path.is_file() and path.name != ".DS_Store"))
    }
    if actual != set(expected) | metadata:
        print(f"cpal file inventory differs: {sorted(actual ^ (set(expected) | metadata))}", file=sys.stderr)
        return 1
    errors = []
    allowed_delta = {"src/host/coreaudio/macos/device.rs", "src/host/coreaudio/macos/mod.rs",
                     "src/host/coreaudio/mod.rs", "src/platform/mod.rs"}
    for name, digests in expected.items():
        digest = hashlib.sha256((root / name).read_bytes()).hexdigest()
        if digest != digests["current"]:
            errors.append(name)
        if name not in allowed_delta and digests["original"] != digests["current"]:
            errors.append(f"unexpected declared delta: {name}")
    patch = hashlib.sha256((root / "hex-patches.diff").read_bytes()).hexdigest()
    if patch != manifest["patch_sha256"]:
        errors.append("hex-patches.diff")
    if not errors:
        errors.extend(verify_cumulative_patch(root, manifest))
    if errors:
        print(f"cpal provenance mismatch: {errors}", file=sys.stderr)
        return 1
    changed = sum(d["original"] != d["current"] for d in expected.values())
    print(f"cpal 0.17.3 provenance verified: {len(expected)} files, {changed} source deltas")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
