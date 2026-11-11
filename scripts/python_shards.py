"""Run selected Python self-tests in isolated concurrent processes.

Usage: python3 scripts/python_shards.py [--workers N] [--discover | -- SELECTOR ...]

Selectors are unittest names (modules, classes or test IDs) under scripts/tests.
Owner selections and discovery batch small sets of ordinary cases to keep
process startup from dominating. Every case runs
once and every process reports a unittest result. Tests cannot rely on sharing
state with other cases in the same process.

Methods marked `__hex_shard_bounded__ = True` use at most two workers. Both
owner selections and discovery queue those lanes after the first ordinary wave
and share the same worker limit.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager, nullcontext
from itertools import groupby
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from concurrent.futures import ThreadPoolExecutor

TESTS = Path(__file__).resolve().parent / "tests"
BOOTSTRAP = ("import sys, unittest; sys.path.insert(0, 'scripts/tests'); "
             "unittest.main(module=None)")


def test_cases(selectors: list[str] | None) -> list[tuple[str, bool, bool]]:
    """Expand selectors to unique IDs with bounded and early fixture markers."""
    sys.path.insert(0, str(TESTS))
    found: dict[str, bool] = {}
    early: set[str] = set()

    def walk(suite) -> None:
        for item in suite:
            if isinstance(item, unittest.TestSuite):
                walk(item)
            else:
                method = getattr(type(item), item._testMethodName, None)
                bounded = getattr(method, "__hex_shard_bounded__", False) is True
                name = item.id()
                found[name] = found.get(name, False) or bounded
                if selectors is None and getattr(item, "__hex_shard_early__", False) is True:
                    early.add(name)

    if selectors is None:
        loader = unittest.TestLoader()
        suite = loader.discover(str(TESTS), pattern="test_*.py")
        if loader.errors:
            raise ValueError("\n".join(loader.errors))
    else:
        suite = unittest.defaultTestLoader.loadTestsFromNames(selectors)
    walk(suite)
    return [(name, bounded, name in early) for name, bounded in sorted(found.items())]


def run_cases(cases, workers, run, *, overlap_bounded=False):
    """Give marked cases at most two lanes within the overall worker limit."""
    ordinary = [name for name, marked in cases if not marked]
    bounded = [name for name, marked in cases if marked]
    lanes = min(2, workers, len(bounded))
    split = workers if overlap_bounded and workers > 1 else len(ordinary)
    with ThreadPoolExecutor(max_workers=workers) as pool:
        first = pool.map(run, ordinary[:split])
        if not overlap_bounded:
            yield from first
        # Each lane runs one fixture at a time. In discovery these jobs enter
        # the same queue between the first wave and the remaining batches.
        bounded_jobs = [pool.submit(lambda names: list(map(run, names)), bounded[index::lanes])
                        for index in range(lanes)]
        remaining = pool.map(run, ordinary[split:])
        if overlap_bounded:
            yield from first
        yield from remaining
        results = [job.result() for job in bounded_jobs]
        for index in range(len(bounded)):
            yield results[index % lanes][index // lanes]


def batches(cases: list[tuple[str, bool, bool]], size: int) -> list[tuple[tuple[str, ...], bool]]:
    """Reuse module imports and class setup while isolating marked fixtures."""
    jobs = [((name,), False) for name, bounded, early in cases if early and not bounded]
    ordinary = (name for name, bounded, early in cases if not bounded and not early)
    for _, group in groupby(ordinary, key=lambda name: name.split(".", 1)[0]):
        names = list(group)
        jobs.extend((tuple(names[index:index + size]), False) for index in range(0, len(names), size))
    jobs.extend(((name,), True) for name, bounded, _ in cases if bounded)
    return jobs


@contextmanager
def discovery_environment():
    """Resolve Apple's Git launcher once for subprocess-heavy discovery fixtures."""
    if sys.platform != "darwin" or shutil.which("git") != "/usr/bin/git":
        yield None
        return
    try:
        selected = Path(subprocess.check_output(
            ["/usr/bin/xcrun", "--find", "git"], text=True, timeout=5,
            stderr=subprocess.DEVNULL).strip())
    except (OSError, subprocess.SubprocessError):
        yield None
        return
    if (not selected.is_absolute() or selected == Path("/usr/bin/git")
            or not selected.is_file() or not os.access(selected, os.X_OK)):
        yield None
        return
    with tempfile.TemporaryDirectory(prefix="hex-discovery-tools-") as directory:
        (Path(directory) / "git").symlink_to(selected)
        yield {**os.environ, "PATH": directory + os.pathsep + os.environ.get("PATH", os.defpath)}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--discover", action="store_true", help="discover every test_*.py suite")
    parser.add_argument("--workers", type=int)
    parser.add_argument("selectors", nargs=argparse.REMAINDER)
    arguments = parser.parse_args(argv)
    selectors = arguments.selectors[1:] if arguments.selectors[:1] == ["--"] else arguments.selectors
    workers = arguments.workers
    if workers is None:
        workers = min(8 if arguments.discover else 12, os.cpu_count() or 1)
    if arguments.discover == bool(selectors) or not 1 <= workers <= 64:
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
        flags = ["-v"] if arguments.discover or len(tests) > 1 else []
        result = subprocess.run([sys.executable, "-c", BOOTSTRAP, *flags, *tests], cwd=root,
                                env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        label = tests[0] if len(tests) == 1 else f"{tests[0]} and {len(tests) - 1} more"
        return label, result.returncode, result.stdout

    failed = False
    jobs = batches(cases, size=8)
    environment = discovery_environment() if arguments.discover else nullcontext(None)
    with environment as env:
        for test, code, output in run_cases(jobs, workers, run, overlap_bounded=True):
            # unittest reports on stderr; keep that stream for callers and logs.
            print(f"===== {test} (exit {code}) =====\n{output}", file=sys.stderr, flush=True)
            failed |= code != 0
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
