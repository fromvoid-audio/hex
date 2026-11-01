"""Run a package's Rust test binaries as disjoint concurrent processes.

Usage: python3 scripts/test_shards.py --shards N [--filter NAME ...] -- cargo test -p PACKAGE [--features F] --locked

Some fixtures serialize on process-wide resources (hex-desktop's native output
lease). Separate processes each own that resource, so their serialized chains
run concurrently. Every listed test runs exactly once: names are split
round-robin into `--exact` sets (optionally only names containing a
`--filter` substring, as libtest filters do), each binary runs with Cargo's package working
directory and environment, and each shard prints its own `test result` line so
the local runner keeps counting real passes. Doctests are not run; use only for
packages without them or keep a separate `cargo test --doc` step.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor


def test_binaries(cargo: list[str], env: dict[str, str]) -> list[dict]:
    """Build once and return each test executable with its package directory."""
    built = subprocess.run([*cargo, "--no-run", "--message-format=json-render-diagnostics"],
                           env=env, stdout=subprocess.PIPE, check=True, text=True)
    binaries = []
    for line in built.stdout.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if (message.get("reason") == "compiler-artifact" and message.get("executable")
                and message.get("profile", {}).get("test")):
            binaries.append({"executable": message["executable"],
                             "package_dir": str(Path(message["manifest_path"]).parent),
                             "package": message["target"].get("name", "")})
    if not binaries:
        raise SystemExit("no test executables were built")
    return binaries


def list_tests(executable: str, cwd: str, env: dict[str, str]) -> list[str]:
    listed = subprocess.run([executable, "--list", "--format", "terse"], cwd=cwd, env=env,
                            stdout=subprocess.PIPE, check=True, text=True)
    return sorted(line[:-len(": test")] for line in listed.stdout.splitlines() if line.endswith(": test"))


def partition(names: list[str], shards: int) -> list[list[str]]:
    """Disjoint, deterministic, covering split; empty shards are dropped."""
    groups = [names[index::shards] for index in range(shards)]
    return [group for group in groups if group]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--shards", type=int, required=True)
    parser.add_argument("--filter", action="append", default=[],
                        help="keep only tests whose name contains this substring (repeatable)")
    parser.add_argument("--skip", action="append", default=[],
                        help="drop tests whose name contains this substring (repeatable)")
    parser.add_argument("cargo", nargs=argparse.REMAINDER)
    arguments = parser.parse_args(argv)
    cargo = arguments.cargo[1:] if arguments.cargo[:1] == ["--"] else arguments.cargo
    if not 1 <= arguments.shards <= 32 or cargo[:2] != ["cargo", "test"] or "--" in cargo:
        parser.error("expected --shards 1..32 -- cargo test ... (no test-binary arguments)")
    env = dict(os.environ)
    threads = max(1, (os.cpu_count() or 1) // arguments.shards)
    jobs = []
    for binary in test_binaries(cargo, env):
        names = list_tests(binary["executable"], binary["package_dir"], env)
        if arguments.filter:
            names = [name for name in names if any(f in name for f in arguments.filter)]
        names = [name for name in names if not any(s in name for s in arguments.skip)]
        for index, group in enumerate(partition(names, arguments.shards)):
            jobs.append((f"{Path(binary['executable']).name} shard {index + 1}", binary, group))

    def run(job):
        label, binary, group = job
        result = subprocess.run([binary["executable"], "--exact", f"--test-threads={threads}", *group],
                                cwd=binary["package_dir"], env={**env, "CARGO_MANIFEST_DIR": binary["package_dir"]},
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        return label, result.returncode, result.stdout

    failed = False
    with ThreadPoolExecutor(max_workers=max(1, arguments.shards)) as pool:
        for label, code, output in pool.map(run, jobs):
            print(f"===== {label} (exit {code}) =====\n{output}", flush=True)
            failed |= code != 0
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
