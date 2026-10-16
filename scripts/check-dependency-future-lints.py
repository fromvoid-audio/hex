#!/usr/bin/env python3
"""Compile the actual patched dependencies with their future lints denied."""
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix="hex-dependency-lints-") as scratch:
    subprocess.run([
        "rustc", "--crate-name", "block", "--crate-type", "lib", "--emit", "metadata",
        "-D", "uninhabited_static", str(ROOT / "third_party/block/src/lib.rs"),
        "-o", str(Path(scratch) / "block.rmeta"),
    ], check=True, cwd=ROOT)
env = os.environ.copy()
# Cargo ignores RUSTFLAGS when encoded flags exist. Preserve the caller's flags
# and append this denial in the representation Cargo actually uses.
if "CARGO_ENCODED_RUSTFLAGS" in env:
    env["CARGO_ENCODED_RUSTFLAGS"] += "\x1f-D\x1fpub_use_of_private_extern_crate"
else:
    env["RUSTFLAGS"] = env.get("RUSTFLAGS", "") + " -D pub_use_of_private_extern_crate"
manifest = ROOT / "scripts/tests/dependency-warnings/Cargo.toml"
for features in ([], ["--no-default-features"]):
    subprocess.run([
        "cargo", "run", "--locked", "--manifest-path", str(manifest),
        "-p", "hex-warning-consumer", *features,
    ], check=True, cwd=ROOT, env=env)
print("dependency future lints passed: block declaration and downstream proc-macro consumer, default and no-default features")
