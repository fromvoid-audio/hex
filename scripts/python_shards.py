"""Run selected Python self-tests in isolated concurrent processes.

Usage: python3 scripts/python_shards.py [--workers N] [--discover | -- SELECTOR ...]

Selectors are unittest names (modules, classes or test IDs) under scripts/tests.
Owner selections run one case per process. Complete discovery batches small
sets of ordinary cases to keep process startup from dominating. Every case runs
once and every process reports a unittest result. Tests cannot rely on sharing
state with other cases in the same process.

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


def test_cases(selectors: list[str] | None) -> list[tuple[str, bool]]:
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

    if selectors is None:
        loader = unittest.TestLoader()
        suite = loader.discover(str(TESTS), pattern="test_*.py")
        if loader.errors:
            raise ValueError("\n".join(loader.errors))
    else:
        suite = unittest.defaultTestLoader.loadTestsFromNames(selectors)
    walk(suite)
    return sorted(found.items())


def run_cases(cases, workers, run):
    """Run ordinary cases first, then bound marked fixture concurrency to two."""
    for bounded in (False, True):
        selected = [name for name, marked in cases if marked == bounded]
        if not selected:
            continue
        with ThreadPoolExecutor(max_workers=min(workers, 2) if bounded else workers) as pool:
            yield from pool.map(run, selected)


def batches(cases: list[tuple[str, bool]], size: int) -> list[tuple[tuple[str, ...], bool]]:
    """Keep bounded fixtures isolated and distribute ordinary cases across batches."""
    ordinary = [name for name, bounded in cases if not bounded]
    count = (len(ordinary) + size - 1) // size
    return ([(tuple(ordinary[index::count]), False) for index in range(count)]
            + [((name,), True) for name, bounded in cases if bounded])


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--discover", action="store_true", help="discover every test_*.py suite")
    parser.add_argument("--workers", type=int, default=min(12, os.cpu_count() or 1))
    parser.add_argument("selectors", nargs=argparse.REMAINDER)
    arguments = parser.parse_args(argv)
    selectors = arguments.selectors[1:] if arguments.selectors[:1] == ["--"] else arguments.selectors
    if arguments.discover == bool(selectors) or not 1 <= arguments.workers <= 64:
        parser.error("expected --workers 1..64 and exactly one of --discover or -- SELECTOR ...")
    try:
        cases = test_cases(None if arguments.discover else selectors)
    except ValueError as error:
        print(f"test discovery failed: {error}", file=sys.stderr)
        return 1
    if not cases:
        print("no tests matched the selectors", file=sys.stderr)
        return 1
    root = TESTS.parent.parent

    def run(tests: tuple[str, ...]) -> tuple[str, int, str]:
        flags = ["-v"] if arguments.discover else []
        result = subprocess.run([sys.executable, "-c", BOOTSTRAP, *flags, *tests], cwd=root,
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        label = tests[0] if len(tests) == 1 else f"{tests[0]} and {len(tests) - 1} more"
        return label, result.returncode, result.stdout

    failed = False
    jobs = batches(cases, size=8 if arguments.discover else 1)
    for test, code, output in run_cases(jobs, arguments.workers, run):
        # unittest reports on stderr; keep that stream for callers and logs.
        print(f"===== {test} (exit {code}) =====\n{output}", file=sys.stderr, flush=True)
        failed |= code != 0
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
