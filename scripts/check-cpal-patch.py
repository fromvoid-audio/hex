#!/usr/bin/env python3
"""Verify the pinned external CPAL source and its recorded narrow delta."""

import hashlib
import json
import sys
from pathlib import Path


def main() -> int:
    root = Path(__file__).resolve().parent.parent / "third_party" / "cpal"
    manifest = json.loads((root / "hex-provenance.json").read_text())
    expected = manifest["files"]
    metadata = {"HEX-PATCHES.md", "hex-patches.diff", "hex-provenance.json"}
    actual = {
        str(path.relative_to(root))
        for path in root.rglob("*")
        if path.is_file() and "target" not in path.relative_to(root).parts
    }
    if actual != set(expected) | metadata:
        print(f"cpal file inventory differs: {sorted(actual ^ (set(expected) | metadata))}", file=sys.stderr)
        return 1
    errors = []
    for name, digests in expected.items():
        digest = hashlib.sha256((root / name).read_bytes()).hexdigest()
        if digest != digests["current"]:
            errors.append(name)
        if name != "src/host/coreaudio/macos/device.rs" and digests["original"] != digests["current"]:
            errors.append(f"unexpected declared delta: {name}")
    patch = hashlib.sha256((root / "hex-patches.diff").read_bytes()).hexdigest()
    if patch != manifest["patch_sha256"]:
        errors.append("hex-patches.diff")
    if errors:
        print(f"cpal provenance mismatch: {errors}", file=sys.stderr)
        return 1
    print(f"cpal 0.17.3 provenance verified: {len(expected)} files, one source delta")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
