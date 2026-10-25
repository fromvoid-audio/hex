#!/usr/bin/env python3
"""Verify the external xattr patch and compile its real public API regression.

Requires the requested Rust target to be installed. Native runs execute the
filesystem regression; cross runs compile it without claiming runtime evidence.
Use `cargo +1.94.0` through RUSTUP_TOOLCHAIN=1.94.0 for the cloud toolchain.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import re

ROOT = Path(__file__).resolve().parents[1]
VENDOR = ROOT / "third_party/xattr"
ARCHIVE_SHA256 = "6d1526bbe5aaeb5eb06885f4d987bcdfa5e23187055de9b83fe00156a821fabc"


def verify():
    provenance = json.loads((VENDOR / "hex-provenance.json").read_text())
    assert (provenance["package"], provenance["version"], provenance["archive_sha256"]) == (
        "xattr", "0.2.3", ARCHIVE_SHA256)
    assert set(provenance["patched_files"]) == {"src/sys/mod.rs", "src/sys/linux_macos/macos.rs"}
    expected = provenance["original_files"] | provenance["patched_files"]
    extras = {str(p.relative_to(VENDOR)) for p in VENDOR.rglob("*") if p.is_file()} - set(expected)
    assert extras == {"hex-provenance.json", "hex-patches.diff", "HEX-PATCHES.md"}, extras
    for name, digest in expected.items():
        assert hashlib.sha256((VENDOR / name).read_bytes()).hexdigest() == digest, name
    workspace = (ROOT / "Cargo.toml").read_text()
    assert re.search(r'(?m)^exclude = \[.*"third_party/xattr".*\]$', workspace)
    assert re.search(r'(?m)^xattr = \{ path = "third_party/xattr" \}$', workspace)
    assert '[workspace.lints.rust]\nunsafe_code = "forbid"' in workspace
    packages = re.split(r"(?m)^\[\[package\]\]\s*\n", (ROOT / "Cargo.lock").read_text())
    xattr = [p.strip() for p in packages if re.search(r'(?m)^name = "xattr"$', p)]
    assert xattr == ['name = "xattr"\nversion = "0.2.3"\ndependencies = [\n "libc",\n]'], xattr
    print("xattr provenance and dependency boundary passed", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", help="installed target; defaults to rustc host")
    args = parser.parse_args()
    verify()
    host = next(line.split(": ", 1)[1] for line in subprocess.check_output(
        ["rustc", "-vV"], text=True).splitlines() if line.startswith("host: "))
    target = args.target or host
    if "linux" not in target and "apple-darwin" not in target:
        parser.error("regression supports Linux and macOS targets")
    # Apply the warning regression to xattr itself, beyond dependency cap-lints.
    command = ["cargo", "rustc", "--locked", "-p", "xattr", "--lib", "--target", target,
               "--message-format=json", "--", "-D", "deprecated"]
    result = subprocess.run(command, cwd=ROOT, text=True, stdout=subprocess.PIPE, check=True)
    artifacts = {}
    for line in result.stdout.splitlines():
        message = json.loads(line)
        if message.get("reason") == "compiler-artifact":
            for filename in message["filenames"]:
                if filename.endswith(".rlib"):
                    artifacts[message["target"]["name"]] = Path(filename)
    with tempfile.TemporaryDirectory(prefix="hex-xattr-regression-") as tmp:
        binary = Path(tmp) / "missing-attribute"
        command = ["rustc", "--edition=2024", "--test", "--target", target,
                   str(ROOT / "scripts/tests/xattr_missing_attribute.rs"),
                   "--extern", f"xattr={artifacts['xattr']}",
                   "-L", f"dependency={artifacts['libc'].parent}", "-o", str(binary)]
        if target != host:
            command += ["--emit=metadata"]
        subprocess.run(command, cwd=ROOT, check=True)
        if target == host:
            subprocess.run([str(binary)], check=True)
        else:
            print(f"public API regression compiled for {target}; cross run, no execution")


if __name__ == "__main__":
    main()
