#!/usr/bin/env python3
"""Compile the actual patched dependencies with their future lints denied."""
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
DENY_ENCODED = "-D\x1fpub_use_of_private_extern_crate"


def rustflags_env(base_env: dict[str, str], target_dir: str) -> dict[str, str]:
    env = base_env.copy()
    env.setdefault("CARGO_TARGET_DIR", target_dir)
    # Cargo ignores RUSTFLAGS when encoded flags exist. Preserve the caller's flags
    # and append this denial in the representation Cargo actually uses.
    if "CARGO_ENCODED_RUSTFLAGS" in env:
        existing = env["CARGO_ENCODED_RUSTFLAGS"]
        env["CARGO_ENCODED_RUSTFLAGS"] = (
            f"{existing}\x1f{DENY_ENCODED}" if existing else DENY_ENCODED
        )
    else:
        env["RUSTFLAGS"] = env.get("RUSTFLAGS", "") + " -D pub_use_of_private_extern_crate"
    return env


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="hex-dependency-lints-") as scratch:
        subprocess.run([
            "rustc", "--crate-name", "block", "--crate-type", "lib", "--emit", "metadata",
            "-D", "uninhabited_static", "-D", "missing_abi", str(ROOT / "third_party/block/src/lib.rs"),
            "-o", str(Path(scratch) / "block.rmeta"),
        ], check=True, cwd=ROOT)
        env = rustflags_env(os.environ, str(Path(scratch) / "target"))
        manifest = ROOT / "scripts/tests/dependency-warnings/Cargo.toml"
        for features in ([], ["--no-default-features"]):
            subprocess.run([
                "cargo", "run", "--locked", "--manifest-path", str(manifest),
                "-p", "hex-warning-consumer", *features,
            ], check=True, cwd=ROOT, env=env)
    print("dependency future lints passed: block declaration and downstream proc-macro consumer, default and no-default features")


if __name__ == "__main__":
    main()
