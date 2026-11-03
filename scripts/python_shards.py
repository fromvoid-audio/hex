"""Run selected Python self-tests as concurrent single-test processes.

Usage: python3 scripts/python_shards.py [--workers N] -- SELECTOR [SELECTOR ...]

Selectors are unittest names (modules, classes or test IDs) under scripts/tests.
Each test runs once in its own `unittest` process, pulled from one work queue,
so long cases (real Cargo fixtures) spread across workers instead of one serial
chain. Every process prints its own `Ran N tests` / `OK` lines, which the local
runner counts. A test needing state shared across cases in one process cannot
rely on it here; per-class fixtures are rebuilt in each process.

Methods marked `__hex_shard_bounded__ = True` run after ordinary tests with at
most two workers. This keeps subprocess-heavy isolated fixtures from competing
with the ordinary queue; it does not skip cases or alter their reported times.
"""
from __future__ import annotations

import argparse
import os
from pathlib import Path
import subprocess
import sys
import unittest
from concurrent.futures import ThreadPoolExecutor

TESTS = Path(__file__).resolve().parent / "tests"
BOOTSTRAP = ("import sys, unittest; sys.path.insert(0, 'scripts/tests'); "
             "unittest.main(module=None)")


def test_cases(selectors: list[str]) -> list[tuple[str, bool]]:
    """Expand selectors to unique test IDs and their bounded-fixture marker."""
    sys.path.insert(0, str(TESTS))
    found: dict[str, bool] = {}

    def walk(suite) -> None:
        for item in suite:
            if isinstance(item, unittest.TestSuite):
                walk(item)
            else:
                method = getattr(type(item), item._testMethodName, None)
                bounded = getattr(method, "__hex_shard_bounded__", False) is True
                found[item.id()] = found.get(item.id(), False) or bounded

    walk(unittest.defaultTestLoader.loadTestsFromNames(selectors))
    return sorted(found.items())


def run_cases(cases, workers, run):
    """Run ordinary cases first, then bound marked fixture concurrency to two."""
    for bounded in (False, True):
        selected = [name for name, marked in cases if marked == bounded]
        if not selected:
            continue
        with ThreadPoolExecutor(max_workers=min(workers, 2) if bounded else workers) as pool:
            yield from pool.map(run, selected)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--workers", type=int, default=min(12, os.cpu_count() or 1))
    parser.add_argument("selectors", nargs=argparse.REMAINDER)
    arguments = parser.parse_args(argv)
    selectors = arguments.selectors[1:] if arguments.selectors[:1] == ["--"] else arguments.selectors
    if not selectors or not 1 <= arguments.workers <= 64:
        parser.error("expected --workers 1..64 -- SELECTOR [SELECTOR ...]")
    cases = test_cases(selectors)
    if not cases:
        print("no tests matched the selectors", file=sys.stderr)
        return 1
    root = TESTS.parent.parent

    def run(test: str) -> tuple[str, int, str]:
        result = subprocess.run([sys.executable, "-c", BOOTSTRAP, test], cwd=root,
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        return test, result.returncode, result.stdout

    failed = False
    for test, code, output in run_cases(cases, arguments.workers, run):
        # unittest reports on stderr; keep that stream for callers and logs.
        print(f"===== {test} (exit {code}) =====\n{output}", file=sys.stderr, flush=True)
        failed |= code != 0
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
