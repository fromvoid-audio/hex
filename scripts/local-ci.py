#!/usr/bin/env python3
"""Run Hex's local validation pipeline and retain a machine-readable report."""

from __future__ import annotations

import argparse
from contextlib import contextmanager
from dataclasses import dataclass
from datetime import datetime, timezone
import errno
import fcntl
import hashlib
import importlib.util
import json
import math
import os
import re
import shlex
import shutil
from pathlib import Path
import signal
import stat
import subprocess
import sys
import tempfile
import threading
import time
import uuid


ROOT = Path(__file__).resolve().parent.parent
_PLAN_SPEC = importlib.util.spec_from_file_location("hex_ci_plan", Path(__file__).with_name("ci_plan.py"))
if _PLAN_SPEC is None or _PLAN_SPEC.loader is None:
    raise RuntimeError("local CI planner unavailable")
ci_plan = importlib.util.module_from_spec(_PLAN_SPEC)
# Execute and retain the exact source witness; timestamp-valid .pyc files can
# otherwise contain different code than the source currently on disk.
_PLAN_SOURCE_BYTES = Path(_PLAN_SPEC.origin).read_bytes()
_PLAN_SOURCE_SHA256 = hashlib.sha256(_PLAN_SOURCE_BYTES).hexdigest()
exec(compile(_PLAN_SOURCE_BYTES, _PLAN_SPEC.origin, "exec", dont_inherit=True), ci_plan.__dict__)
_INPUT_SPEC = importlib.util.spec_from_file_location("hex_build_inputs", Path(__file__).with_name("build_inputs.py"))
if _INPUT_SPEC is None or _INPUT_SPEC.loader is None:
    raise RuntimeError("Cargo input identity helper unavailable")
build_inputs = importlib.util.module_from_spec(_INPUT_SPEC)
_INPUT_SPEC.loader.exec_module(build_inputs)
_CHECKPOINT_SPEC = importlib.util.spec_from_file_location("hex_ci_checkpoint", Path(__file__).with_name("ci_checkpoint.py"))
if _CHECKPOINT_SPEC is None or _CHECKPOINT_SPEC.loader is None:
    raise RuntimeError("CI checkpoint helper unavailable")
ci_checkpoint = importlib.util.module_from_spec(_CHECKPOINT_SPEC)
_CHECKPOINT_SPEC.loader.exec_module(ci_checkpoint)
_STORAGE_SPEC = importlib.util.spec_from_file_location("hex_ci_target_storage", Path(__file__).with_name("ci_target_storage.py"))
if _STORAGE_SPEC is None or _STORAGE_SPEC.loader is None:
    raise RuntimeError("Cargo target storage helper unavailable")
ci_target_storage = importlib.util.module_from_spec(_STORAGE_SPEC)
_STORAGE_SPEC.loader.exec_module(ci_target_storage)
GIB = 1024 ** 3
DEFAULT_FULL_MIN_FREE_BYTES = 2 * GIB


@contextmanager
def terminal_progress(label: str, *, announce: bool = True):
    """Show activity only around work whose child output is captured/logged.

    Callers must not print or inherit subprocess terminal output in this scope.
    Redirected stdout or stderr gets one plain stage line and no worker thread.
    """
    stream = sys.stderr
    if not (sys.stdout.isatty() and stream.isatty()):
        if announce:
            print(f"{label}...", file=stream, flush=True)
        yield
        return

    stopped = threading.Event()

    def draw(frame: str) -> None:
        # Leave a spare column so a narrow/resized terminal never wraps frames.
        width = max(1, shutil.get_terminal_size().columns - 1)
        stream.write(f"\r\033[2K{(frame + ' ' + label)[:width]}")
        stream.flush()

    def animate() -> None:
        frames = "/-\\|"
        index = 0
        while not stopped.wait(0.1):
            try:
                draw(frames[index % len(frames)])
            except (OSError, ValueError):
                # A display error must not replace the actual subprocess result.
                return
            index += 1

    worker = threading.Thread(target=animate, name="hex-terminal-progress")
    try:
        draw("|")
        worker.start()
        yield
    finally:
        stopped.set()
        try:
            if worker.ident is not None:
                worker.join()
        finally:
            try:
                stream.write("\r\033[2K")
                stream.flush()
            except (OSError, ValueError):
                pass


@dataclass(frozen=True)
class Step:
    """A check with a 1–100 character ASCII ID starting with a letter or digit.

    Remaining characters may be letters, digits, underscores or hyphens.
    The runner validates the entire plan before executing any command. IDs are
    used verbatim in numbered log basenames; the index keeps repeated IDs distinct.
    """

    step_id: str
    argv: tuple[str, ...]
    uses_cargo_target: bool = False
    requires_tests: bool = False


QUICK_STEPS = (
    Step("changelog", ("python3", "scripts/changelog.py", "validate", "--if-present")),
    Step("architecture", ("python3", "scripts/check-architecture.py")),
    Step("gpui-patch", ("python3", "scripts/check-gpui-patch.py")),
    Step("dependency-patches", ("python3", "scripts/check-dependency-patches.py")),
    Step("ui-style", ("python3", "scripts/check-ui-style.py")),
    Step("device-layout", ("python3", "scripts/check-device-layout.py")),
    Step("format", ("cargo", "fmt", "--all", "--check")),
)

# One source of truth schedules every feature-gated UI unit binary in full,
# health and whole-package plans. Preserve the established piano step ID.
def ui_unit_label(target: str) -> str:
    return "piano" if target == "unit_piano_roll" else target.removeprefix("unit_").replace("_", "-")


UI_UNIT_STEPS = tuple(
    Step(f"ui-{ui_unit_label(target)}-unit-tests",
         ("cargo", "test", "-p", "hex-ui", "--features", feature, "--test", target, "--locked"))
    for target, feature in ci_plan.UI_UNIT_FEATURES.items()
)
UI_UNIT_STEP_IDS = frozenset(step.step_id for step in UI_UNIT_STEPS)

# Cheap failures should stop before self-tests; keep every configuration once.
FULL_STEPS = (
    *QUICK_STEPS,
    Step(
        "runner-tests",
        ("python3", "-m", "unittest", "discover", "-s", "scripts/tests", "-p", "test_*.py"),
        uses_cargo_target=True,
    ),
    Step("dependency-future-lints", ("python3", "scripts/check-dependency-future-lints.py"), uses_cargo_target=True),
    Step("workspace-tests", ("cargo", "test", "--workspace", "--locked")),
    Step("prepared-export-instrument-tests", ("cargo", "test", "-p", "hex-export", "--features", "instrument-test-support", "--locked")),
    Step("ui-feature-tests", ("cargo", "test", "-p", "hex-ui", "--features", "ui-test-support", "--locked")),
    *UI_UNIT_STEPS,
    Step("sandbox-unit-tests", ("cargo", "test", "-p", "hex-sandbox", "--features", "ui-unit-tier", "--locked", "theme::"), requires_tests=True),
    Step("device-gallery-tests", ("cargo", "test", "-p", "hex-ui", "--example", "layout_review", "--features", "ui-test-support", "--locked")),
    Step("audio-allocation-probe", ("cargo", "test", "-p", "hex-audio", "-p", "hex-plugin-host", "--features", "allocation-probe", "--locked")),
    # Native fixtures serialize on hex-desktop's process-wide output lease;
    # disjoint test processes each own one, so their chains run concurrently.
    Step("desktop-allocation-probe", ("python3", "scripts/test_shards.py", "--shards", "4", "--",
                                      "cargo", "test", "-p", "hex-desktop", "--features", "allocation-probe", "--locked")),
    Step("stream-allocation-probe", ("cargo", "test", "-p", "hex-stream", "--features", "allocation-probe", "--locked")),
    Step("workspace-clippy", ("cargo", "clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings")),
    Step("prepared-export-instrument-clippy", ("cargo", "clippy", "-p", "hex-export", "--all-targets", "--features", "instrument-test-support", "--locked", "--", "-D", "warnings")),
    Step("ui-feature-clippy", ("cargo", "clippy", "-p", "hex-ui", "--all-targets", "--features", "ui-test-support", "--locked", "--", "-D", "warnings")),
    Step("ui-unit-clippy", ("cargo", "clippy", "-p", "hex-ui", "--all-targets", "--features", "ui-unit-tier", "--locked", "--", "-D", "warnings")),
    Step("sandbox-unit-clippy", ("cargo", "clippy", "-p", "hex-sandbox", "--all-targets", "--features", "ui-unit-tier", "--locked", "--", "-D", "warnings")),
    Step("audio-feature-clippy", ("cargo", "clippy", "-p", "hex-audio", "-p", "hex-desktop", "-p", "hex-plugin-host", "--all-targets", "--features", "allocation-probe", "--locked", "--", "-D", "warnings")),
    Step("stream-feature-clippy", ("cargo", "clippy", "-p", "hex-stream", "--all-targets", "--features", "allocation-probe", "--locked", "--", "-D", "warnings")),
    Step("desktop-build", ("cargo", "build", "-p", "hex-desktop", "--locked")),
)


def runs_rust_tests(argv: tuple[str, ...]) -> bool:
    """Plain `cargo test` or the same command run through the test sharder."""
    return argv[:2] == ("cargo", "test") or argv[:2] == ("python3", "scripts/test_shards.py")


def log_timing_estimates(text: str, duration: float) -> dict[str, float | None]:
    """Diagnostic wall estimates from Cargo/harness logs, never qualification evidence.

    Cargo's Finished duration includes its build phase, not just compiler CPU.
    Rounded or nested harness output can overlap; do not invent a negative residual.
    """
    build = [3600 * float(h or 0) + 60 * float(m or 0) + float(s)
             for h, m, s in re.findall(
                 r"^\s*Finished [^\n]* in (?:(\d+)h )?(?:(\d+)m )?(\d+(?:\.\d+)?)s", text, re.MULTILINE)]
    rust = [float(s) for s in re.findall(r"^test result: [^\n]*finished in (\d+(?:\.\d+)?)s", text, re.MULTILINE)]
    python = [float(s) for s in re.findall(r"^Ran \d+ tests? in (\d+(?:\.\d+)?)s", text, re.MULTILINE)]
    result = {name: sum(values) if values and math.isfinite(sum(values)) else None
              for name, values in (("cargo_build_phase_seconds", build),
                                   ("rust_harness_seconds", rust), ("python_harness_seconds", python))}
    known = sum(value for value in result.values() if value is not None)
    result["unclassified_seconds"] = round(duration - known, 6) if known <= duration else None
    return result


# Merge test-execution budget (user policy, 1 November 2026): owner-boundary
# merge checks aim to keep test setup and execution, excluding Cargo's build
# phase, under one minute. Exceeding it is a warning, not a merge failure (user
# decision, 1 November 2026; overruns are investigated in Pulse #4100). Carried
# steps cost nothing in this run and are excluded.
MERGE_TEST_BUDGET_SECONDS = 60.0


def merge_test_budget(entries: list, reason: str | None) -> dict:
    """Test-only seconds of executed test steps: wall time minus the build phase.

    Harness overhead and doctest compilation count as test time, so this is a
    conservative upper bound. Lint, format and build-only steps are excluded.
    """
    spent = []
    for entry in entries:
        if not isinstance(entry, dict) or entry.get("execution") == "carried":
            continue
        timing = entry.get("timing_estimates") or {}
        argv = tuple(entry.get("argv") or ())
        tests = (runs_rust_tests(argv) or timing.get("rust_harness_seconds") is not None
                 or timing.get("python_harness_seconds") is not None)
        if not tests or not isinstance(entry.get("duration_seconds"), (int, float)):
            continue
        seconds = max(0.0, entry["duration_seconds"] - (timing.get("cargo_build_phase_seconds") or 0.0))
        spent.append((round(seconds, 3), entry.get("step_id")))
    total = round(sum(seconds for seconds, _ in spent), 3)
    return {"limit_seconds": MERGE_TEST_BUDGET_SECONDS, "test_seconds": total,
            "exceeded": total > MERGE_TEST_BUDGET_SECONDS,
            "slowest": [{"step_id": step, "seconds": seconds}
                        for seconds, step in sorted(spent, reverse=True)[:3]],
            "over_budget_reason": reason}


def edit_selection(inventory: list[dict], *, package: str, test_filter: str,
                   features: str = "", lib: bool = False, test: str | None = None) -> dict:
    """Validate an explicit edit target; omissions keep Cargo's package-wide behavior."""
    packages = [item for item in inventory if item["name"] == package]
    if len(packages) != 1 or not test_filter.strip():
        raise ValueError("edit feedback requires one known workspace package and a nonempty test filter")
    owner = packages[0]
    requested_features = {name for name in features.split(",") if name}
    if requested_features - owner["features"].keys():
        raise ValueError("unknown workspace package feature")
    selection = {"package": package, "filter": test_filter, "features": features}
    if lib or test is not None:
        if lib and test is not None:
            raise ValueError("select either a library or a named integration test")
        library_kinds = {"lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"}
        targets = [target for target in owner["targets"]
                   if (bool(library_kinds.intersection(target["kind"])) if lib else
                       "test" in target["kind"] and target["name"] == test)]
        if len(targets) != 1:
            raise ValueError("unknown or ambiguous edit test target")
        enabled = requested_features | ({"default"} if "default" in owner["features"] else set())
        pending = list(enabled)
        while pending:
            for feature in owner["features"].get(pending.pop(), []):
                if feature not in enabled:
                    enabled.add(feature)
                    pending.append(feature)
        if set(targets[0].get("required-features", [])) - enabled:
            raise ValueError("edit test target requires features that are not enabled")
        selection["target"] = {"kind": "lib" if lib else "test", "name": targets[0]["name"]}
    return selection


def boundary_steps(plan: dict) -> tuple[Step, ...]:
    """Compile consumers even when reviewed device behavior tests are narrower."""
    boundary = plan["boundary"]
    inventory = plan["cargo_inventory"]
    names = {p["name"] for p in inventory}
    affected = boundary["lint_packages"]
    if not affected or len(set(affected)) != len(affected) or set(affected) - names:
        raise ValueError("boundary plan requires nonempty known affected packages")
    if sorted(affected) != sorted(plan["affected_packages"]):
        raise ValueError("boundary consumers do not match resolved Cargo closure")
    packages = tuple(arg for p in affected for arg in ("-p", p))
    # Main is formatted; only the changed owners' sources can need it.
    owners = boundary.get("test_packages") or affected
    steps = [*(Step("format", ("cargo", "fmt", *(a for p in owners for a in ("-p", p)), "--check"))
               if s.step_id == "format" else s for s in QUICK_STEPS),
             next(s for s in FULL_STEPS if s.step_id == "runner-tests")]
    if any(path.endswith(("Cargo.toml", "/build.rs")) for path in plan.get("paths", [])):
        steps.append(next(s for s in FULL_STEPS if s.step_id == "dependency-future-lints"))
    tests = boundary["tests"]
    # Owners test; every affected consumer still compiles and lints below.
    tested = boundary.get("test_packages", [])
    module_filters = boundary.get("module_filters", {})
    by_name = {p["name"]: p for p in inventory}
    doc_examples = set(boundary.get("doc_example_owners", []))

    def scoped(argv: tuple[str, ...]) -> tuple[str, ...]:
        """Restrict a test command to the changed modules when every owner it tests is module-scoped."""
        owners = [argv[i + 1] for i, arg in enumerate(argv) if arg == "-p"]
        if not owners or any(owner not in module_filters for owner in owners):
            return argv
        names = sorted({name for owner in owners for name in module_filters[owner]})
        # Module paths name only unit tests in the library or binary; integration
        # tests, examples and benches would build and link only to match nothing.
        if "--doc" not in argv and not {"--test", "--example", "--bench", "--lib", "--bins"} & set(argv):
            kinds = {kind for owner in owners for target in by_name[owner]["targets"] for kind in target["kind"]}
            selector = ("--lib",) if "lib" in kinds else ("--bins",)
            argv = (*(a for a in argv if a != "--all-targets"), *selector)
        if argv[:2] == ("python3", "scripts/test_shards.py"):
            split = argv.index("--")
            # Coverage is verified across module-scoped configurations together.
            return (*argv[:split], "--allow-empty",
                    *(a for name in names for a in ("--filter", name)), *argv[split:])
        return (*argv, "--", *names)
    if boundary["kind"] == "packages":
        if not tested or set(tested) - set(affected) or tests:
            raise ValueError("package boundary must test its owners within the affected closure")
        # Reviewed additive test features run once, with the feature (below);
        # their doctests stay here because --all-targets feature runs omit them.
        single = boundary.get("single_run", {})
        defaults = tuple(arg for p in tested if p not in single and p not in module_filters for arg in ("-p", p))
        if defaults:
            steps.append(Step("affected-tests", ("cargo", "test", *defaults, "--locked")))
        for package in tested:
            if package in module_filters and package not in single:
                steps.append(Step(f"module-tests-{package}", scoped(("cargo", "test", "-p", package, "--locked"))))
        if single:
            for package in (p for p in tested if p in single):
                # Module-scoped doctests run only when the changed files carry doc examples.
                if package in module_filters and package not in doc_examples:
                    continue
                steps.append(Step("single-run-doctests" if package not in module_filters else f"module-doctests-{package}",
                                  scoped(("cargo", "test", "--doc", "-p", package, "--locked"))))
    elif boundary["kind"] == "compressor":
        if not tests:
            raise ValueError("device boundary has no contract targets")
        for index, test in enumerate(tests):
            # Reuse the edit target validator, not a second Cargo selector.
            selected = edit_selection(inventory, package=test["package"],
                                      test_filter=test["filter"] or "target-validation",
                                      features=test.get("features", ""), lib=test.get("lib", False),
                                      test=test.get("test"))
            target = selected["target"]
            feature = ("--features", selected["features"]) if selected["features"] else ()
            selector = ("--lib",) if target["kind"] == "lib" else ("--test", target["name"])
            steps.append(Step(f"contract-{index}-{selected['package']}",
                              ("cargo", "test", "-p", selected["package"], *feature, *selector,
                               "--locked", *([test["filter"]] if test["filter"] else []))))
    else:
        raise ValueError("unknown ownership boundary")
    steps.append(Step("affected-clippy", ("cargo", "clippy", *packages, "--all-targets", "--locked", "--", "-D", "warnings")))
    # Keep separate default/feature configurations, including feature-gated
    # examples and allocation probes. Do not deduplicate by overlapping names.
    covered_features = {"hex-plugin-host": {"allocation-probe"}, "hex-ui": {"ui-test-support", "ui-unit-tier"}, "hex-export": {"instrument-test-support"},
                        "hex-sandbox": {"ui-unit-tier"},
                        "hex-audio": {"allocation-probe"}, "hex-stream": {"allocation-probe"},
                        "hex-desktop": {"allocation-probe"}}
    # Every consumer's reviewed feature configurations lint in ONE invocation:
    # separate invocations recompiled the shared dependency chain per feature
    # (55-112 s against 38-42 s measured, 1 November 2026). The default-feature
    # affected-clippy above keeps cfg(not(feature)) branches compiling.
    lint_features: set[str] = set()
    for step in FULL_STEPS:
        if "--features" not in step.argv:
            continue
        if step.step_id in UI_UNIT_STEP_IDS | {"ui-unit-clippy"} and (
                "hex-ui" not in by_name or "ui-unit-tier" not in by_name["hex-ui"]["features"]):
            continue
        owners = [step.argv[i + 1] for i, arg in enumerate(step.argv) if arg == "-p"]
        # Feature test suites run for tested owners; feature lints for every consumer.
        scope = tested if runs_rust_tests(step.argv) else affected
        selected = [p for p in owners if p in scope]
        if not selected or boundary["kind"] == "compressor" and runs_rust_tests(step.argv):
            continue
        # Scoped UI changes select mapped unit targets below; a whole-package
        # run retains this explicit feature-gated target from FULL_STEPS.
        if step.step_id in UI_UNIT_STEP_IDS and "hex-ui" in module_filters:
            continue
        # Example targets are separate crates: a src module filter cannot name
        # their tests. A module-scoped owner's own module tests cover the change.
        if "--example" in step.argv and runs_rust_tests(step.argv) and all(p in module_filters for p in selected):
            continue
        if not runs_rust_tests(step.argv):
            # Feature lints are folded into one invocation below.
            feature = step.argv[step.argv.index("--features") + 1]
            lint_features.update(f"{p}/{feature}" for p in selected)
            continue
        argv = list(step.argv)
        for owner in set(owners) - set(selected):
            index = argv.index(owner)
            del argv[index - 1:index + 1]
        if step.step_id == "sandbox-unit-tests" and "hex-sandbox" in module_filters:
            # Scoped changes use the owning module filters in place of the
            # full/health theme filter, not both filters together.
            argv.remove("theme::")
        sandbox_theme = (step.step_id == "sandbox-unit-tests" and
                         ("hex-sandbox" not in module_filters or
                          any(name.startswith("theme::") for name in module_filters["hex-sandbox"])))
        steps.append(Step(step.step_id, scoped(tuple(argv)), requires_tests=sandbox_theme))
    for package in inventory:
        if package["name"] not in affected:
            continue
        extra = set(package["features"]) - {"default"} - covered_features.get(package["name"], set())
        # Preserve each declared configuration; all-features alone can mask
        # mutually exclusive cfg branches. Feature interaction contracts still
        # need reviewed configuration rules rather than a guessed power set.
        # Compressor plans run reviewed contract tests, but still lint every
        # extra configuration so an unexercised cfg branch cannot stop compiling.
        lint_features.update(f"{package['name']}/{feature}" for feature in extra)
        if boundary["kind"] == "compressor" or package["name"] not in tested:
            continue
        for index, feature in enumerate(sorted(extra)):
            steps.append(Step(f"feature-test-{package['name']}-{index}",
                              scoped(("cargo", "test", "-p", package["name"], "--features", feature,
                                      "--all-targets", "--locked"))))
    # The normal whole-package run already includes these targets. A lib-only
    # module selection must explicitly retain tests moved to a small unit binary.
    unit_targets: dict[str, set[str]] = {}
    if "hex-ui" in tested:
        for module, target in ci_plan.UI_UNIT_TARGETS.items():
            names = {name for name in module_filters.get("hex-ui", [])
                     if module.startswith(name) or name.startswith(module)}
            if names:
                unit_targets.setdefault(target, set()).update(names)
    for target, names in sorted(unit_targets.items()):
        feature = ci_plan.UI_UNIT_FEATURES.get(target)
        feature_args = ("--features", feature) if feature else ()
        steps.append(Step(f"ui-unit-{target}", ("cargo", "test", "-p", "hex-ui", *feature_args,
                                                  "--test", target, "--locked", "--", *sorted(names)),
                          requires_tests=True))
    if lint_features:
        lint_packages = sorted({feature.split("/")[0] for feature in lint_features})
        steps.append(Step("affected-feature-clippy",
                          ("cargo", "clippy", *(a for p in lint_packages for a in ("-p", p)), "--all-targets",
                           "--locked", "--features", ",".join(sorted(lint_features)), "--", "-D", "warnings")))
    if "hex-desktop" in (tested if boundary["kind"] == "packages" else affected):
        steps.append(next(s for s in FULL_STEPS if s.step_id == "desktop-build"))
    return tuple(steps)


def planned_steps(plan: dict) -> tuple[Step, ...]:
    """Select complete configurations, never deduplicate by test-name overlap."""
    scope = plan["scope"]
    if plan.get("edit"):
        edit = plan["edit"]
        feature = ("--features", edit["features"]) if edit["features"] else ()
        selected = edit.get("target")
        target = () if selected is None else (("--lib",) if selected["kind"] == "lib" else ("--test", selected["name"]))
        return (Step("edit-tests", ("cargo", "test", "-p", edit["package"], *feature, *target, "--locked", edit["filter"], "--", "--nocapture"),
                     requires_tests=True),
                Step("edit-clippy", ("cargo", "clippy", "-p", edit["package"], *feature, "--all-targets", "--locked", "--", "-D", "warnings")),
                next(s for s in QUICK_STEPS if s.step_id == "format"))
    if plan.get("health"):
        health = plan["health"]
        steps = [next(s for s in QUICK_STEPS if s.step_id == "format")]
        if health["python"]:
            steps.append(Step("health-runner-tests", ci_plan.python_tests.argv(plan["python_tests"]), True, True))
        if health["workspace_inputs"]:
            steps.append(Step("health-tests", ("cargo", "test", "--workspace", "--locked"), requires_tests=True))
        elif health["packages"]:
            steps.append(Step("health-tests", ("cargo", "test", *(a for p in health["packages"] for a in ("-p", p)),
                                               "--locked"), requires_tests=True))
        if health["workspace_inputs"] or "hex-ui" in health["packages"]:
            steps.extend(Step("health-" + step.step_id.removesuffix("-tests"), step.argv, requires_tests=True)
                         for step in UI_UNIT_STEPS)
        if health["workspace_inputs"] or "hex-sandbox" in health["packages"]:
            step = next(s for s in FULL_STEPS if s.step_id == "sandbox-unit-tests")
            steps.append(Step("health-sandbox-unit", step.argv, requires_tests=True))
        if len(steps) == 1:
            steps.append(Step("metadata", ("python3", "scripts/ci_plan.py", "--check-metadata",
                                           "--base", plan["base"], "--source", plan["source"])))
        return tuple(steps)
    if plan.get("reuse"):
        return (Step("metadata-continuation", ("python3", "scripts/ci_plan.py", "--check-metadata",
                                             "--base", plan["reuse"]["source"], "--source", plan["source"]),
                     uses_cargo_target=True),)
    if scope == "full":
        steps = FULL_STEPS
    elif scope == "rust":
        steps = boundary_steps(plan)
    elif scope == "metadata":
        return (Step("metadata", ("python3", "scripts/ci_plan.py", "--check-metadata",
                                  "--base", plan["base"], "--source", plan["source"])),)
    elif scope == "tooling":
        steps = (*QUICK_STEPS, next(s for s in FULL_STEPS if s.step_id == "runner-tests"))
    elif scope == "ui" and plan.get("boundary"):
        # UI owners follow the same owner/module boundary as Rust packages.
        steps = boundary_steps(plan)
    elif scope == "ui":
        steps = (*QUICK_STEPS, next(s for s in FULL_STEPS if s.step_id == "runner-tests"),
                 Step("ui-default-tests", ("cargo", "test", "-p", "hex-ui", "-p", "hex-widgets", "--locked")),
                 next(s for s in FULL_STEPS if s.step_id == "ui-feature-tests"),
                 *UI_UNIT_STEPS,
                 next(s for s in FULL_STEPS if s.step_id == "device-gallery-tests"),
                 Step("ui-default-clippy", ("cargo", "clippy", "-p", "hex-ui", "-p", "hex-widgets", "-p", "hex-desktop", "--all-targets", "--locked", "--", "-D", "warnings")),
                 next(s for s in FULL_STEPS if s.step_id == "ui-feature-clippy"),
                 next(s for s in FULL_STEPS if s.step_id == "ui-unit-clippy"),
                 next(s for s in FULL_STEPS if s.step_id == "desktop-build"))
    else:
        raise ValueError("unsupported resolved scope")
    return tuple(Step(s.step_id,
                      ci_plan.python_tests.argv(plan["python_tests"])
                      if s.step_id == "runner-tests" and "python_tests" in plan else s.argv,
                      s.uses_cargo_target,
                      s.requires_tests or (runs_rust_tests(s.argv) and "--doc" not in s.argv and not module_scoped(s.argv))
                      or s.step_id == "runner-tests") for s in steps)


def module_scoped(argv: tuple[str, ...]) -> bool:
    """A test command restricted to changed modules by a libtest filter.

    One configuration may hold none of a module's tests (for example a
    feature-gated test module), so these steps are checked together instead:
    a merge whose module-scoped steps pass no test at all fails."""
    return "--filter" in argv or ("--" in argv and argv[:2] == ("cargo", "test")
                                   and any(a.endswith("::") for a in argv[argv.index("--") + 1:]))


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def reusable_plan(root: Path, plan: dict, report: Path) -> dict:
    evidence = json.loads(report.read_bytes())
    old_plan = evidence.get("plan")
    if old_plan and old_plan.get("reuse"):
        raise ValueError("reference the original runtime report, not a chain of metadata reports")
    expected = planned_steps(old_plan or {"scope": "full"})
    if [(s.get("step_id"), s.get("argv")) for s in evidence.get("steps", [])] != [
            (s.step_id, list(s.argv)) for s in expected]:
        raise ValueError("runtime report does not contain its complete required configurations")
    for required, actual in zip(expected, evidence["steps"]):
        results = actual.get("test_results", {})
        if required.requires_tests and sum(results.get("rust_passed_by_binary", [])) + sum(
                results.get("python_executions", [])) - sum(results.get("python_skipped", [])) <= 0:
            raise ValueError("runtime report has a zero-match test configuration")
    return ci_plan.reuse_checks(root, plan, report)


def carry_decisions(steps: tuple[Step, ...], evidence: dict, basis: dict,
                    inventory: list[dict]) -> list[dict]:
    """Per planned step: carry an identical passed result, narrow, or run.

    A Cargo step carries only when the dependency closure of every package it
    names excludes each package the base advance changed; workspace-wide steps
    carry only when no package changed. Cheap guards always rerun. The Python
    step reruns just the selection the changed paths require. Unknown shapes run.
    """
    previous = {}
    for entry in evidence.get("steps", []):
        planned = entry.get("carry", {}).get("planned_argv", entry.get("argv"))
        if entry.get("status") in {"passed", "carried"} and isinstance(planned, list):
            previous[(entry.get("step_id"), tuple(planned))] = entry
    owners = set(basis["delta_owners"])
    decisions = []
    for step in steps:
        entry = previous.get((step.step_id, step.argv))
        results = (entry or {}).get("test_results", {})
        counted = sum(results.get("rust_passed_by_binary", [])) + sum(
            results.get("python_executions", [])) - sum(results.get("python_skipped", []))
        if entry is None:
            decisions.append({"action": "run", "reason": "no identical passed step in the carried report"})
        elif step.requires_tests and counted <= 0:
            decisions.append({"action": "run", "reason": "carried step has no counted passing tests"})
        elif step.step_id == "runner-tests":
            selection = basis["python_tests"]
            if selection["mode"] == "none":
                decisions.append({"action": "carry", "entry": entry, "reason": "no executable change"})
            elif selection["mode"] == "shared":
                decisions.append({"action": "narrow", "entry": entry,
                                  "argv": ci_plan.python_tests.argv(selection),
                                  "reason": "; ".join(selection["reasons"])})
            else:
                decisions.append({"action": "run", "reason": "; ".join(selection["reasons"])})
        elif (step.argv[:1] != ("cargo",) or step.argv[1:2] == ("fmt",)) and not runs_rust_tests(step.argv):
            decisions.append({"action": "run", "reason": "guard steps always rerun"})
        else:
            packages = [step.argv[i + 1] for i, arg in enumerate(step.argv[:-1]) if arg == "-p"]
            if "--workspace" in step.argv or not packages:
                hit = owners
            else:
                hit = ci_plan.dependency_closure(packages, inventory) & owners
            if hit:
                decisions.append({"action": "run", "reason": "depends on changed " + ", ".join(sorted(hit))})
            else:
                decisions.append({"action": "carry", "entry": entry,
                                  "reason": "dependency closure excludes " + (", ".join(sorted(owners)) or "nothing")})
    return decisions


def command_output(argv: list[str], cwd: Path, env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(argv, cwd=cwd, env=env, capture_output=True, text=True, check=False)


class InterruptedRun(Exception):
    """Raised when a user or supervisor interrupts the pipeline."""


class ReportPublicationError(OSError):
    """The durable report is unavailable; never a successful qualification."""


class TargetCacheError(Exception):
    """Raised when Cargo target ownership is unknown or already in use."""


def target_state_paths(target_directory: Path, state_dir: Path | None = None) -> tuple[Path, Path]:
    """Return persistent cooperative-lock and owner-marker paths for a target."""
    identity = hashlib.sha256(os.fsencode(str(target_directory))).hexdigest()
    location = state_dir or (Path(tempfile.gettempdir()) / "hex-local-ci-targets")
    return location / f"{identity}.lock", location / f"{identity}.owner.json"


class CargoTargetLock:
    """Nonblocking cooperative lock; direct Cargo commands do not participate."""

    def __init__(self, path: Path, holder: dict[str, object]) -> None:
        self.path = path
        self.holder = holder
        self.file = None

    def acquire(self) -> None:
        try:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            if self.path.is_symlink():
                raise TargetCacheError(f"Cargo target lock path is a symlink: {self.path}")
            self.file = self.path.open("a+", encoding="utf-8")
            fcntl.flock(self.file.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            previous = ""
            detail = ""
            if self.file is not None:
                try:
                    self.file.seek(0)
                    previous = self.file.read().strip()
                except OSError as read_error:
                    detail = f"; could not read current holder: {read_error}"
                finally:
                    try:
                        self.file.close()
                    except OSError as close_error:
                        detail = f"{detail}; closing lock file failed: {close_error}"
                    self.file = None
            holder_detail = f"; current holder: {previous}" if previous else ""
            raise TargetCacheError(
                f"Cargo target is already in use by another local CI run{holder_detail}{detail}"
            ) from error
        except OSError as error:
            detail = str(error)
            if self.file is not None:
                try:
                    self.file.close()
                except OSError as close_error:
                    detail = f"{detail}; closing lock file also failed: {close_error}"
                self.file = None
            if error.errno in (errno.EACCES, errno.EAGAIN):
                raise TargetCacheError("Cargo target is already in use by another local CI run") from error
            raise TargetCacheError(f"could not open or lock Cargo target: {detail}") from error
        try:
            self._write({"active_holder": self.holder})
        except OSError as error:
            detail = str(error)
            if self.file is not None:
                try:
                    fcntl.flock(self.file.fileno(), fcntl.LOCK_UN)
                except OSError as unlock_error:
                    detail = f"{detail}; unlocking also failed: {unlock_error}"
                finally:
                    try:
                        self.file.close()
                    except OSError as close_error:
                        detail = f"{detail}; closing lock file also failed: {close_error}"
                    self.file = None
            raise TargetCacheError(f"could not record Cargo target lock holder: {detail}") from error

    def _write(self, value: dict[str, object]) -> None:
        if self.file is None:
            return
        self.file.seek(0)
        self.file.truncate()
        json.dump(value, self.file, sort_keys=True)
        self.file.write("\n")
        self.file.flush()

    def release(self) -> str | None:
        if self.file is None:
            return None
        failure = None
        try:
            try:
                self._write({"last_holder": self.holder, "released_at": utc_now()})
            except OSError as error:
                failure = f"could not record Cargo target lock release: {error}"
            try:
                fcntl.flock(self.file.fileno(), fcntl.LOCK_UN)
            except OSError as error:
                if failure is None:
                    failure = f"could not explicitly unlock Cargo target: {error}"
        finally:
            try:
                self.file.close()
            except OSError as error:
                if failure is None:
                    failure = f"could not close Cargo target lock file: {error}"
            self.file = None
        return failure


class LocalCiRunner:
    def __init__(
        self,
        root: Path = ROOT,
        *,
        mode: str = "full",
        run_id: str | None = None,
        env: dict[str, str] | None = None,
        steps: tuple[Step, ...] | None = None,
        min_free_bytes: int | None = None,
        adopt_target: bool = False,
        target_state_dir: Path | None = None,
        plan: dict | None = None,
        resume_report: Path | None = None,
        carry_report: Path | None = None,
        over_budget_reason: str | None = None,
    ) -> None:
        self.root = root.resolve()
        self.over_budget_reason = over_budget_reason
        default_floor = DEFAULT_FULL_MIN_FREE_BYTES if mode in {"full", "merge", "release"} else 0
        self.min_free_bytes = default_floor if min_free_bytes is None else min_free_bytes
        if (
            isinstance(self.min_free_bytes, bool)
            or not isinstance(self.min_free_bytes, int)
            or not 0 <= self.min_free_bytes <= sys.maxsize
        ):
            raise ValueError("minimum free space must be a bounded nonnegative integer byte count")
        self.mode = mode
        self.plan = plan
        self.resume_report = resume_report
        self.carry_report = carry_report
        self._carry: list[dict] | None = None
        self._carry_dependency_inputs: object = None
        self._resume_evidence = None
        self._checkpoint_inputs = None
        self._checkpoint_target = None
        self.adopt_target = adopt_target
        self.target_state_dir = target_state_dir
        self.run_id = run_id or f"{datetime.now(timezone.utc):%Y%m%dT%H%M%SZ}-{uuid.uuid4().hex[:8]}"
        self.env = dict(os.environ if env is None else env)
        self.steps = steps or (planned_steps(plan) if plan else QUICK_STEPS if mode == "quick"
                               else planned_steps({"scope": "full"}))
        self.output_dir = self.root / "artifacts" / "local-ci" / self.run_id
        self.summary_path = self.output_dir / "summary.json"
        self.started_monotonic = time.monotonic()
        self.summary: dict[str, object] = {
            "schema_version": 2,
            "run_id": self.run_id,
            "mode": self.mode,
            "platform": sys.platform,
            # Paths alone cannot bind mutable external native fixtures/receipts
            # or Keychain state. Historical absence is unknown for reuse.
            "native_acceptance_opt_in": bool(ci_plan.python_tests.native_acceptance_inputs(self.env)),
            "python_version": sys.version,
            "status": "incomplete",
            "repo_root": str(self.root),
            "commit": None,
            "dirty": None,
            "dirty_files": None,
            "started_at": utc_now(),
            "ended_at": None,
            "exit_code": None,
            "invocation_argv": list(sys.argv),
            "steps": [],
            "plan": plan,
            "minimum_free_bytes": self.min_free_bytes,
            "time_to_first_result_seconds": None,
        }
        self.git_identity: dict[str, object] | None = None
        self.cargo_identity: dict[str, object] | None = None
        self._lexical_cargo_target: Path | None = None
        self.target_lock: CargoTargetLock | None = None
        self.target_owner_path: Path | None = None
        self.target_owner_claimed = False
        self._release_metadata = None
        self._dependency_inputs = None
        self._compiler_inputs = None
        self._workspace_inputs = None
        self._path_inputs = []
        self._invalidate_dependency_profile = False

    def _write_summary(self) -> None:
        temporary = self.summary_path.with_suffix(".json.tmp")
        phase = "writing temporary report"
        try:
            temporary.write_text(json.dumps(self.summary, indent=2, sort_keys=True) + "\n")
            phase = "publishing report"
            temporary.replace(self.summary_path)
        except OSError as error:
            # Preserve the last published bytes and the actual command result.
            # A later successful diagnostic write must still record this failure.
            self.summary["report_publication_error"] = f"{phase}: {error}"
            raise ReportPublicationError(error.errno, str(error)) from error

    def _fail_report_publication(self) -> None:
        self.summary["verification_status"] = "failed"
        if self.summary.get("exit_code") not in (None, 0):
            return  # Keep an existing command/refusal/interruption/stale result.
        if any(step["status"] == "failed" for step in self.summary["steps"]):
            self.summary.update(status="failed", exit_code=1)
        else:
            self.summary.update(status="incomplete", exit_code=2)

    def _capture_repository_state(self, key: str) -> bool:
        try:
            commit = command_output(["git", "rev-parse", "HEAD"], self.root, self.env)
            top_level = command_output(["git", "rev-parse", "--show-toplevel"], self.root, self.env)
            common_dir = command_output(["git", "rev-parse", "--git-common-dir"], self.root, self.env)
            status = command_output(["git", "status", "--porcelain=v1", "--untracked-files=all"], self.root, self.env)
            tracked = subprocess.run(
                ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
                cwd=self.root,
                env=self.env,
                capture_output=True,
                check=False,
            )
        except OSError as error:
            self.summary["repository_error"] = str(error)
            return False
        if (
            commit.returncode != 0
            or top_level.returncode != 0
            or common_dir.returncode != 0
            or status.returncode != 0
            or tracked.returncode != 0
        ):
            self.summary["repository_error"] = {
                "commit_exit_code": commit.returncode,
                "top_level_exit_code": top_level.returncode,
                "common_dir_exit_code": common_dir.returncode,
                "status_exit_code": status.returncode,
                "files_exit_code": tracked.returncode,
                "commit_stderr": commit.stderr.strip(),
                "top_level_stderr": top_level.stderr.strip(),
                "common_dir_stderr": common_dir.stderr.strip(),
                "status_stderr": status.stderr.strip(),
                "files_stderr": tracked.stderr.decode("utf-8", errors="replace").strip(),
            }
            return False
        dirty_files = [line for line in status.stdout.splitlines() if line]
        digest = hashlib.sha256()
        paths = sorted(path for path in tracked.stdout.split(b"\0") if path)
        try:
            for relative in paths:
                digest.update(len(relative).to_bytes(8, "big"))
                digest.update(relative)
                path = self.root / os.fsdecode(relative)
                try:
                    metadata = path.lstat()
                except FileNotFoundError:
                    digest.update(b"missing\0")
                    continue
                if path.is_symlink():
                    digest.update(b"symlink\0")
                    digest.update(os.fsencode(os.readlink(path)))
                elif path.is_file():
                    digest.update(b"file\0")
                    with path.open("rb") as source:
                        while chunk := source.read(1024 * 1024):
                            digest.update(chunk)
                    digest.update(str(metadata.st_mode & 0o111).encode("ascii"))
                else:
                    digest.update(f"other:{metadata.st_mode}\0".encode("ascii"))
        except OSError as error:
            self.summary["repository_error"] = f"could not hash repository contents: {error}"
            return False
        snapshot = {
            "commit": commit.stdout.strip(),
            "dirty": bool(dirty_files),
            "dirty_files": dirty_files,
            "content_sha256": digest.hexdigest(),
        }
        self.summary[key] = snapshot
        if key == "start_tree":
            self.summary["runtime_inputs"] = ci_plan.runtime_inputs(self.root, self.env)
            if self.mode in {"full", "merge", "release"}:
                try:
                    ci_plan.require_qualifying_inputs(self.summary["runtime_inputs"])
                except ValueError as error:
                    self.summary["repository_error"] = str(error)
                    return False
            if re.fullmatch(r"[0-9a-f]{40}", snapshot["commit"]):
                self.env["HEX_GUARDED_BUILD_SOURCE"] = (
                    snapshot["commit"] + ":" + snapshot["content_sha256"] + ":"
                    + ("dirty" if snapshot["dirty"] else "clean"))
            root_path = Path(top_level.stdout.strip()).resolve()
            common_path = Path(common_dir.stdout.strip())
            if not common_path.is_absolute():
                common_path = root_path / common_path
            self.git_identity = {
                "workspace_root": str(root_path),
                "git_common_dir": str(common_path.resolve()),
                "commit": snapshot["commit"],
                "dirty": snapshot["dirty"],
                "dirty_files": snapshot["dirty_files"],
                "content_sha256": snapshot["content_sha256"],
            }
            self.summary["source_identity"] = self.git_identity
            self.summary["commit"] = snapshot["commit"]
            self.summary["dirty"] = snapshot["dirty"]
            self.summary["dirty_files"] = snapshot["dirty_files"]
            self.summary["repository_commands"] = [
                ["git", "rev-parse", "HEAD"],
                ["git", "rev-parse", "--show-toplevel"],
                ["git", "rev-parse", "--git-common-dir"],
                ["git", "status", "--porcelain=v1", "--untracked-files=all"],
                ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
            ]
        return True

    def _resolve_cargo_identity(self) -> bool:
        command = [
            "cargo",
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--locked",
            "--manifest-path",
            str(self.root / "Cargo.toml"),
        ]
        configured_target = self.env.get("CARGO_TARGET_DIR")
        try:
            result = subprocess.run(
                command,
                cwd=self.root,
                env=self.env,
                capture_output=True,
                text=True,
                check=False,
            )
        except OSError as error:
            self.summary["cargo_metadata_error"] = f"could not resolve Cargo target: {error}"
            return False
        if result.returncode != 0:
            self.summary["cargo_metadata_error"] = {
                "exit_code": result.returncode,
                "stderr": result.stderr.strip(),
            }
            return False
        try:
            metadata = json.loads(result.stdout)
            raw_target = metadata["target_directory"]
            raw_workspace_root = metadata["workspace_root"]
            members = metadata["workspace_members"]
            packages = metadata["packages"]
            if (
                not isinstance(raw_target, str)
                or not isinstance(raw_workspace_root, str)
                or not isinstance(members, list)
                or not all(isinstance(member, str) for member in members)
                or not isinstance(packages, list)
            ):
                raise ValueError("Cargo metadata fields have invalid types")
            workspace_root = Path(raw_workspace_root).resolve()
            target = Path(raw_target)
            if not target.is_absolute():
                target = self.root / target
            self._lexical_cargo_target = target.absolute()
            target = target.resolve()
            by_id = {
                package["id"]: package
                for package in packages
                if isinstance(package, dict)
                and isinstance(package.get("id"), str)
                and isinstance(package.get("name"), str)
                and isinstance(package.get("version"), str)
            }
            workspace_packages = []
            for member in members:
                package = by_id.get(member)
                if package is None:
                    raise ValueError("workspace package metadata is incomplete")
                workspace_packages.append(
                    {
                        "id": member,
                        "name": package["name"],
                        "version": package["version"],
                        "spec": member,
                    }
                )
            if not workspace_packages:
                raise ValueError("Cargo workspace has no members to clean safely")
        except (KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
            self.summary["cargo_metadata_error"] = f"Cargo metadata was incomplete or invalid: {error}"
            return False

        if workspace_root != self.root:
            self.summary["cargo_metadata_error"] = (
                f"Cargo workspace root {workspace_root} does not match local CI root {self.root}"
            )
            return False
        self.env["CARGO_TARGET_DIR"] = str(target)
        lock_path, owner_path = target_state_paths(target, self.target_state_dir)
        self.target_owner_path = owner_path
        self.cargo_identity = {
            "manifest_path": str(self.root / "Cargo.toml"),
            "workspace_root": str(workspace_root),
            "target_directory": str(target),
            "target_source": "environment" if configured_target is not None else "cargo metadata/config/default",
            "configured_target_value": configured_target,
            "metadata_command": command,
            "workspace_packages": workspace_packages,
            "lock_path": str(lock_path),
            "owner_path": str(owner_path),
        }
        self.summary["cargo_identity"] = self.cargo_identity
        return True

    @staticmethod
    def _uses_cargo_target(step: Step) -> bool:
        return step.uses_cargo_target or bool(
            len(step.argv) > 1
            and step.argv[0] == "cargo"
            and step.argv[1] in {"build", "check", "test", "clippy", "run", "bench", "doc", "clean", "rustc"}
        )

    @staticmethod
    def _directory_identity(path: Path) -> dict[str, int] | None:
        try:
            status = path.stat()
        except FileNotFoundError:
            return None
        except OSError as error:
            raise TargetCacheError(f"could not inspect Cargo target identity: {error}") from error
        if not path.is_dir():
            raise TargetCacheError("resolved Cargo target path is not a directory")
        return {"device": status.st_dev, "inode": status.st_ino}

    def _load_target_owner(self, owner_path: Path, target: Path) -> dict[str, object] | None:
        if not owner_path.exists() and not owner_path.is_symlink():
            return None
        try:
            if owner_path.is_symlink():
                raise ValueError("owner marker is a symlink")
            value = json.loads(owner_path.read_text(encoding="utf-8"))
            if not isinstance(value, dict):
                raise ValueError("owner marker is not an object")
            owner = value.get("workspace_owner")
            target_identity = value.get("target_identity")
            if (
                type(value.get("schema_version")) is not int or value["schema_version"] not in {1, 2}
                or value.get("target_directory") != str(target)
                or not isinstance(owner, dict)
                or not isinstance(owner.get("workspace_root"), str)
                or not isinstance(owner.get("git_common_dir"), str)
                or (
                    target_identity is not None
                    and (
                        not isinstance(target_identity, dict)
                        or set(target_identity) != {"device", "inode"}
                        or not all(isinstance(item, int) and not isinstance(item, bool) for item in target_identity.values())
                    )
                )
            ):
                raise ValueError("owner fields have invalid types")
            if value["schema_version"] == 2 and not isinstance(value.get("dependency_inputs"), dict):
                raise ValueError("dependency input identity is missing")
            if "dependency_inputs" in value:
                dependency = value["dependency_inputs"]
                if (not isinstance(dependency, dict) or set(dependency) != {"version", "sha256", "packages"}
                        or type(dependency["version"]) is not int or dependency["version"] != 1
                        or not isinstance(dependency["sha256"], str)
                        or not re.fullmatch(r"[0-9a-f]{64}", dependency["sha256"])
                        or not isinstance(dependency["packages"], list)
                        or any(not isinstance(p, dict) or set(p) != {"name", "version", "spec", "sha256"}
                               or not all(isinstance(v, str) and v for v in p.values())
                               or not re.fullmatch(r"[0-9a-f]{64}", p["sha256"])
                               for p in dependency["packages"])):
                    raise ValueError("malformed dependency input identity")
            return value
        except (OSError, json.JSONDecodeError, ValueError) as error:
            raise TargetCacheError(
                f"Cargo target ownership marker is malformed at {owner_path}; "
                "use a fresh target or explicitly adopt it after checking ownership"
            ) from error

    def _write_target_owner(self, path: Path, owner: dict[str, object], *, checked=False) -> None:
        temporary = path.with_name(f"{path.name}.{os.getpid()}.{self.run_id}.tmp")
        payload = {
            "schema_version": 2,
            "dependency_inputs": self._dependency_inputs,
            "cache_inputs": self._cache_provenance(checked=checked),
            "target_directory": self.cargo_identity["target_directory"] if self.cargo_identity else None,
            "workspace_owner": {
                "workspace_root": owner["workspace_root"],
                "git_common_dir": owner["git_common_dir"],
            },
            "target_identity": self._directory_identity(
                Path(str(self.cargo_identity["target_directory"]))
            ) if self.cargo_identity else None,
            "recorded_at": utc_now(),
        }
        temporary.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        temporary.replace(path)

    def _check_target_filesystem(self, target: Path) -> None:
        try:
            observation = ci_target_storage.inspect(target, self.env)
        except ValueError as error:
            raise TargetCacheError(str(error) + "; target preparation refused before cleanup") from error
        self.summary["target_filesystem"] = observation
        if observation.get("filesystem_type") == "exfat":
            raise TargetCacheError(
                "macOS ExFAT Cargo targets are unsupported: cleanup can fail on AppleDouble entries "
                "and test executables have exhibited launch stalls; choose a fresh APFS "
                "CARGO_TARGET_DIR and rerun normal local CI. The target and previous owner were preserved")

    def _prepare_target_cache(self) -> bool:
        if self.cargo_identity is None or self.git_identity is None:
            self.summary["target_cache_error"] = "Cargo or Git identity is unavailable"
            return False
        target = Path(str(self.cargo_identity["target_directory"]))
        lock_path, owner_path = target_state_paths(target, self.target_state_dir)
        holder = {
            "run_id": self.run_id,
            "workspace_root": self.git_identity["workspace_root"],
            "commit": self.git_identity["commit"],
            "target_directory": str(target),
        }
        lock = CargoTargetLock(lock_path, holder)
        try:
            lock.acquire()
        except TargetCacheError as error:
            self.summary["target_cache_error"] = str(error)
            return False
        self.target_lock = lock
        try:
            self._check_target_filesystem(target)
            malformed_owner = False
            try:
                owner_record = self._load_target_owner(owner_path, target)
            except TargetCacheError:
                if not self.adopt_target:
                    raise
                owner_record = None
                malformed_owner = True
            current_workspace = {
                "workspace_root": self.git_identity["workspace_root"],
                "git_common_dir": self.git_identity["git_common_dir"],
            }
            target_identity = self._directory_identity(target)
            dependency_inputs = self._dependency_context()
            self._dependency_inputs = dependency_inputs
            self._path_inputs = dependency_inputs["packages"]
            self._workspace_inputs = build_inputs.workspace_snapshot(self.root, self.env, self._release_metadata)
            self.env["HEX_GUARDED_CARGO_CONFIG_FINGERPRINT"] = dependency_inputs["sha256"]
            if self._resume_evidence is not None:
                expected = self._resume_evidence["checkpoint"]["target"]
                actual = {"owner_sha256": ci_checkpoint.owner_digest(owner_path),
                          "target_identity": target_identity, "cargo_identity": self.cargo_identity,
                          "dependency_inputs": dependency_inputs}
                if self.adopt_target or actual != expected or owner_record["workspace_owner"] != current_workspace:
                    raise TargetCacheError("checkpoint target ownership or guarded inputs changed; start a fresh run")
                ci_checkpoint.require_no_jobs()
                if (not self._capture_repository_state("resume_locked_tree")
                        or self.summary["resume_locked_tree"] != self.summary["start_tree"]):
                    raise TargetCacheError("checkpoint source changed before target lock")
            ownership_action = "reuse"
            cleanup_argv: list[str] | None = None
            previous_workspace = None
            if malformed_owner:
                ownership_action = "adopted"
                cleanup_argv = self._workspace_clean_argv(target)
            elif owner_record is None:
                nonempty = False
                if target.exists():
                    try:
                        nonempty = next(target.iterdir(), None) is not None
                    except OSError as error:
                        raise TargetCacheError(f"could not inspect unowned Cargo target: {error}") from error
                if nonempty and not self.adopt_target:
                    raise TargetCacheError(
                        f"Cargo target ownership is unknown at {target}; choose an empty target or pass --adopt-target"
                    )
                if nonempty:
                    ownership_action = "adopted"
                    cleanup_argv = self._workspace_clean_argv(target)
                else:
                    ownership_action = "claim-empty"
            else:
                previous_workspace = owner_record["workspace_owner"]
                prior_target_identity = owner_record.get("target_identity")
                same_workspace = previous_workspace == current_workspace
                if prior_target_identity != target_identity:
                    if not self.adopt_target:
                        raise TargetCacheError(
                            "Cargo target directory identity changed; choose a fresh target or pass --adopt-target"
                        )
                    ownership_action = "adopted"
                    cleanup_argv = self._workspace_clean_argv(target)
                elif not same_workspace and previous_workspace.get("git_common_dir") != current_workspace["git_common_dir"]:
                    if not self.adopt_target:
                        raise TargetCacheError(
                            "Cargo target is owned by a different repository; choose a fresh target or pass --adopt-target"
                        )
                    ownership_action = "adopted"
                    cleanup_argv = self._workspace_clean_argv(target)
                elif not same_workspace:
                    ownership_action = "worktree-handoff"
                    cleanup_argv = self._workspace_clean_argv(target)

            if owner_record and owner_record.get("dependency_inputs") != dependency_inputs:
                if ownership_action == "reuse":
                    ownership_action = "dependency-input-handoff"
                cleanup_argv = self._workspace_clean_argv(target)
            invalidated = self._provenance_cleanup(owner_record) if owner_record else []
            if self._invalidate_dependency_profile:
                ownership_action = "compiler-profile-handoff"
                cleanup_argv = self._workspace_clean_argv(target)[:8]
            if invalidated:
                if ownership_action == "reuse":
                    ownership_action = "source-or-compiler-handoff"
                if cleanup_argv is None:
                    cleanup_argv = self._workspace_clean_argv(target)[:8]
                for spec in invalidated:
                    name = self._clean_package_name(spec)
                    if name not in cleanup_argv:
                        cleanup_argv.extend(("--package", name))
            if cleanup_argv is not None:
                current = {p["name"]: p for p in dependency_inputs["packages"]}
                previous = owner_record.get("dependency_inputs", {}) if owner_record else {}
                for old in (previous or {}).get("packages", []):
                    if old["name"] not in current or current[old["name"]]["version"] != old["version"]:
                        raise TargetCacheError("removed/version-changed path dependency requires a fresh target")
                old_by_name = {p["name"]: p for p in (previous or {}).get("packages", [])}
                for package in dependency_inputs["packages"]:
                    if not self._invalidate_dependency_profile and old_by_name.get(package["name"]) != package:
                        cleanup_argv.extend(("--package", self._clean_package_name(package["spec"])))

            self.summary["target_cache"] = {
                "action": ownership_action,
                "previous_workspace_owner": previous_workspace,
                "current_workspace_owner": current_workspace,
                "target_identity_before": target_identity,
                "cleanup_argv": cleanup_argv,
                "cleanup_status": "not_needed" if cleanup_argv is None else "pending",
                "lock_path": str(lock_path),
                "lock_acquired": True,
                "dependency_inputs": dependency_inputs,
            }
            if cleanup_argv is not None:
                result = self._run_target_cleanup(cleanup_argv)
                target_cache = self.summary["target_cache"]
                assert isinstance(target_cache, dict)
                target_cache["cleanup_status"] = "passed" if result else "failed"
                if not result:
                    self.summary["target_cache_error"] = "workspace dev-artifact cleanup failed; previous owner was preserved"
                    return False

            self._write_target_owner(owner_path, current_workspace)
            self.target_owner_claimed = True
            return True
        except (TargetCacheError, OSError, ValueError) as error:
            self.summary["target_cache_error"] = str(error)
            release_error = lock.release()
            if release_error is not None:
                self.summary["target_lock_release_error"] = release_error
            self.target_lock = None
            return False
        except BaseException:
            lock.release()
            self.target_lock = None
            raise

    def _workspace_clean_argv(self, target: Path) -> list[str]:
        if self.cargo_identity is None:
            raise TargetCacheError("workspace package metadata is unavailable")
        packages = self.cargo_identity["workspace_packages"]
        if not isinstance(packages, list) or not packages:
            raise TargetCacheError("workspace package list is unavailable")
        argv = [
            "cargo",
            "clean",
            "--profile",
            "dev",
            "--target-dir",
            str(target),
            "--manifest-path",
            str(self.root / "Cargo.toml"),
        ]
        for package in packages:
            if not isinstance(package, dict) or not isinstance(package.get("spec"), str):
                raise TargetCacheError("workspace package metadata is malformed")
            argv.extend(("--package", self._clean_package_name(package["spec"])))
        return argv

    def _clean_package_name(self, spec: str) -> str:
        """Cargo 1.94 clean matches names, ignoring ID URL/version qualifiers.

        Keep full IDs in provenance, but only clean a name when current complete
        metadata proves it identifies exactly one workspace or path package.
        Never use a qualifier to imply a narrower cleanup than Cargo performs.
        """
        if not isinstance(self._release_metadata, dict):
            raise TargetCacheError("complete cleanup package metadata is unavailable")
        packages = self._release_metadata["packages"]
        matches = [p for p in packages if p["id"] == spec]
        if len(matches) != 1:
            raise TargetCacheError("cleanup package is absent or ambiguous in current metadata")
        name = matches[0]["name"]
        if (not isinstance(name, str) or not name
                or sum(p["name"] == name for p in packages) != 1
                or matches[0].get("source") is not None):
            raise TargetCacheError("cleanup requires a unique workspace/path package name")
        return name

    def _release_scope(self) -> list[dict[str, str]]:
        """Cargo clean ignores URL/version qualifiers: reject external name collisions."""
        result = command_output(
            ["cargo", "metadata", "--format-version", "1", "--locked", "--all-features"], self.root, self.env
        )
        if result.returncode != 0:
            raise TargetCacheError(f"release package metadata failed: {result.stderr.strip()}")
        try:
            metadata = json.loads(result.stdout)
            self._release_metadata = metadata
            members = set(metadata["workspace_members"])
            packages = metadata["packages"]
            scope = sorted(
                ({"name": p["name"], "version": p["version"], "spec": p["id"]}
                 for p in packages if p["id"] in members), key=lambda p: p["name"]
            )
            names = {p["name"] for p in scope}
            if not scope or len(names) != len(scope) or any(
                p["id"] not in members and p["name"] in names for p in packages
            ):
                raise ValueError("workspace package names overlap other packages")
            if any(not all(isinstance(v, str) and v for v in p.values()) for p in scope):
                raise ValueError("invalid package fields")
            if self.cargo_identity is None or {p["spec"] for p in scope} != {
                p["spec"] for p in self.cargo_identity["workspace_packages"]
            }:
                raise ValueError("workspace package metadata changed")
            return scope
        except (KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
            raise TargetCacheError(f"unsafe release package selection: {error}") from error

    def _release_build_inputs(self, argv: list[str]) -> str:
        tools = {}
        search = os.pathsep.join(str((self.root / entry).resolve()) if not Path(entry).is_absolute() else entry
                                 for entry in self.env.get("PATH", os.defpath).split(os.pathsep))
        for tool in ("cargo", "rustc"):
            result = command_output([tool, "-Vv"], self.root, self.env)
            resolved = shutil.which(tool, path=search)
            if result.returncode != 0 or not result.stdout.strip() or resolved is None:
                raise TargetCacheError("cannot identify release toolchain: " + tool)
            path = Path(resolved).resolve(strict=True)
            info = path.stat()
            digest = hashlib.sha256()
            with path.open("rb") as source:
                while chunk := source.read(1024 * 1024):
                    digest.update(chunk)
            tools[tool] = {"path": str(path), "version": result.stdout,
                           "sha256": digest.hexdigest(), "mode": info.st_mode}
        key = build_inputs.fingerprint(self.root, self.env, self._release_metadata, tools)
        self.summary["toolchain"] = tools
        self._compiler_inputs = build_inputs.fingerprint(self.root, self.env, self._release_metadata, tools, compiler_only=True)
        return hashlib.sha256((key + json.dumps(argv)).encode()).hexdigest()

    def _dependency_context(self) -> dict:
        # Full feature metadata includes optional/dev path dependencies. Read it
        # inside the target lock, before even Python's private Cargo fixtures.
        self._release_scope()
        self._path_inputs = self._path_dependency_inputs()
        return {"version": 1, "sha256": self._release_build_inputs(["guarded-dev-inputs-v1"]),
                "packages": self._path_inputs}

    def _path_dependency_inputs(self) -> list[dict]:
        metadata = self._release_metadata
        members = set(metadata["workspace_members"])
        packages = [{"name": p["name"], "version": p["version"], "spec": p["id"],
                     "sha256": build_inputs.fingerprint(self.root, {}, metadata, {},
                                                         paths=(Path(p["manifest_path"]).parent,))}
                    for p in metadata["packages"] if p["id"] not in members and p.get("source") is None]
        for package in packages:
            if sum(p["name"] == package["name"] for p in metadata["packages"]) != 1:
                raise TargetCacheError("path dependency cleanup has an ambiguous package name")
        return sorted(packages, key=lambda p: p["name"])

    def _cache_provenance(self, *, checked=False) -> dict | None:
        if self._compiler_inputs is None:
            return None
        witness = dict(self._workspace_inputs)
        source = self.summary.get("start_tree", {})
        witness.update(commit=source.get("commit"), dirty=source.get("dirty"), checked=checked)
        return {"compiler_sha256": self._compiler_inputs, "workspace": witness, "dependencies": self._path_inputs}

    def _provenance_cleanup(self, owner: dict) -> list[str]:
        self._invalidate_dependency_profile = False
        previous = owner.get("cache_inputs")
        # Existing bounded ownership records without cache witnesses get a
        # one-time conservative invalidation; ownership/adoption is independent.
        if previous is not None:
            if (not isinstance(previous, dict) or set(previous) not in ({"compiler_sha256", "workspace"}, {"compiler_sha256", "workspace", "dependencies"})
                    or not isinstance(previous["compiler_sha256"], str)
                    or not re.fullmatch(r"[0-9a-f]{64}", previous["compiler_sha256"])
                    or not isinstance(previous["workspace"], dict)):
                raise TargetCacheError("malformed cache input provenance")
            witness = previous["workspace"]
            if (set(witness) != {"packages", "observed_ns", "commit", "dirty", "checked"}
                    or type(witness["observed_ns"]) is not int or witness["observed_ns"] <= 0
                    or type(witness["checked"]) is not bool or type(witness["dirty"]) is not bool
                    or not isinstance(witness["commit"], str) or not isinstance(witness["packages"], list)
                    or any(not isinstance(p, dict) or set(p) != {"spec", "path", "sha256", "timestamps"}
                           or not all(isinstance(v, str) and v for v in p.values())
                           or any(not re.fullmatch(r"[0-9a-f]{64}", p[k]) for k in ("sha256", "timestamps"))
                           for p in witness["packages"])):
                raise TargetCacheError("malformed workspace cache provenance")
        dependencies = previous.get("dependencies", []) if previous else []
        if (not isinstance(dependencies, list) or any(not isinstance(p, dict)
                or set(p) != {"name", "version", "spec", "sha256"}
                or not all(isinstance(v, str) and v for v in p.values())
                or not re.fullmatch(r"[0-9a-f]{64}", p["sha256"]) for p in dependencies)):
            raise TargetCacheError("malformed path dependency cache provenance")
        current = {p["name"]: p for p in self._path_inputs}
        for old in dependencies:
            if old["name"] not in current or current[old["name"]]["version"] != old["version"]:
                raise TargetCacheError("removed/version-changed path dependency requires a fresh target")
        if previous is None or previous["compiler_sha256"] != self._compiler_inputs:
            # Toolchain changes invalidate this owned profile's dependencies,
            # including registry/previously selected versions, not just members.
            # Cargo owns the profile cleanup; dev and release remain separate.
            self._invalidate_dependency_profile = True
            return []
        changed = [p["spec"] for p in self._path_inputs if p not in dependencies]
        return changed + build_inputs.stale_workspace_packages(self.root, self.env, previous["workspace"], self._workspace_inputs)

    def _completed_workspace_inputs(self) -> None:
        current = build_inputs.workspace_snapshot(self.root, self.env, self._release_metadata)
        if current["packages"] != self._workspace_inputs["packages"]:
            raise TargetCacheError("workspace source content/timestamps changed during guarded build")
        self._workspace_inputs = current

    def validate_release_inputs(self, report: dict) -> None:
        """Refuse drift before handing an executable to a staging consumer."""
        if report.get("build_cache_key") is not None and self._release_build_inputs(report["build_argv"]) != report["build_cache_key"]:
            raise TargetCacheError("release toolchain/config/dependency inputs changed during build")

    @staticmethod
    def _write_release_owner(path: Path, payload: dict[str, object]) -> None:
        if path.is_symlink():
            raise TargetCacheError("release ownership marker is a symlink")
        temporary = path.with_name(f"{path.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
        with temporary.open("x", encoding="utf-8") as stream:
            json.dump(payload, stream, indent=2, sort_keys=True)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        temporary.replace(path)
        descriptor = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)

    def inspect_recreated_release_target(self) -> dict[str, object]:
        """Read-only explicit recovery authority, while the target lock is held.

        This never adopts an unknown/foreign owner or certifies its old artifact.
        The caller must revalidate the returned hash and replacement identity at
        the guarded mutation boundary; ordinary automatic restart stays stricter.
        """
        if self.target_lock is None or self.cargo_identity is None:
            raise TargetCacheError("recreated-target inspection requires target ownership")
        target = Path(str(self.cargo_identity["target_directory"]))
        lexical = self._lexical_cargo_target
        if lexical is None:
            raise TargetCacheError("recreated target has no lexical path evidence")
        for path in (lexical, *lexical.parents):
            if path.is_symlink():
                raise TargetCacheError("recreated target path contains a symlink")
        identity = self._directory_identity(target)
        if identity is None:
            raise TargetCacheError("recreated target must exist")
        _, dev_owner = target_state_paths(target, self.target_state_dir)
        owner = dev_owner.with_name(dev_owner.name.replace(".owner.json", ".release-owner.json"))
        try:
            descriptor = os.open(owner, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
            with os.fdopen(descriptor, "rb") as stream:
                if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
                    raise ValueError("owner is not regular")
                raw = stream.read(128 * 1024 + 1)
            if len(raw) > 128 * 1024:
                raise ValueError("owner exceeds bounded metadata size")
            record = json.loads(raw)
            workspace = {k: self.git_identity[k] for k in ("workspace_root", "git_common_dir")}
            previous = record["source"]
            old_identity = record["target_identity"]
            def hex_string(value, size):
                return (isinstance(value, str) and len(value) == size
                        and all(c in "0123456789abcdef" for c in value))
            if (type(record.get("schema_version")) is not int or record["schema_version"] != 1
                    or record.get("profile") != "release"
                    or record.get("state") not in {"pending", "ready"}
                    or record.get("target_directory") != str(target)
                    or record.get("workspace_owner") != workspace
                    or not isinstance(old_identity, dict) or set(old_identity) != {"device", "inode"}
                    or any(type(old_identity[k]) is not int or old_identity[k] < (1 if k == "inode" else 0)
                           for k in ("device", "inode"))
                    or (old_identity == identity and record["state"] != "pending")
                    or not isinstance(previous, dict)
                    or set(previous) != {"commit", "dirty", "dirty_files", "content_sha256"}
                    or previous["dirty"] is not False or previous["dirty_files"] != []
                    or not hex_string(previous["commit"], 40)
                    or not hex_string(previous["content_sha256"], 64)):
                raise ValueError("owner is not this repository's prior target")
            scope = self._release_scope()
            packages = record["packages"]
            if (not isinstance(packages, list) or not packages
                    or any(not isinstance(p, dict) or set(p) != {"name", "version", "spec"}
                           or not all(isinstance(v, str) and v for v in p.values()) for p in packages)):
                raise ValueError("invalid prior package scope")
            names = {p["name"] for p in packages}
            if len(names) != len(packages) or packages != [p for p in scope if p["name"] in names]:
                raise ValueError("prior package ownership changed")
            source = self.summary["start_tree"]
            if not hex_string(source["commit"], 40):
                raise ValueError("current source has no exact commit")
            ancestry = subprocess.run(["git", "merge-base", "--is-ancestor", previous["commit"],
                                      source["commit"]], cwd=self.root, env=self.env,
                                     capture_output=True, check=False)
            if ancestry.returncode != 0:
                raise ValueError("prior owner source is not an ancestor of current source")
            if record["state"] == "ready":
                artifact = record["artifact"]
                argv = record["build_argv"]
                if (not isinstance(artifact, dict) or set(artifact) != {"path", "sha256"}
                        or not isinstance(artifact["path"], str)
                        or not Path(artifact["path"]).is_absolute()
                        or ".." in Path(artifact["path"]).parts
                        or target / "release" not in Path(artifact["path"]).parents
                        or not hex_string(artifact["sha256"], 64)
                        or not isinstance(argv, list) or not all(isinstance(a, str) for a in argv)
                        or argv[:2] != ["cargo", "build"] or "--release" not in argv):
                    raise ValueError("malformed historical release artifact")
            if (not self._capture_repository_state("recovery_inspected_tree")
                    or self.summary["recovery_inspected_tree"] != source
                    or self._directory_identity(target) != identity):
                raise ValueError("source or replacement changed during inspection")
            return {"owner_sha256": hashlib.sha256(raw).hexdigest(), "target_identity": identity}
        except (OSError, KeyError, TypeError, ValueError) as error:
            raise TargetCacheError(f"recreated release target inspection refused: {error}") from error

    @contextmanager
    def guarded_release(self, *, adopt: bool = False, expected_owner_sha256: str | None = None,
                        expected_recreated_target_identity: dict[str, int] | None = None,
                        recovery_inactive_check=None, requested_build_argv: list[str] | None = None,
                        verification_check=None):
        """Explicit release handoff; caller must identify its artifact before exit.

        Uses the dev driver's SAME target-wide lock, but a separate durable
        marker. Exceptions leave pending ownership, requiring explicit recovery.
        Raw Cargo commands do not participate in this cooperative contract.
        """
        if self.target_lock is not None:
            raise TargetCacheError("release preparation requires an unheld runner")
        self.output_dir.mkdir(parents=True, exist_ok=True)
        if not self._capture_repository_state("start_tree") or self.summary["start_tree"]["dirty"]:
            raise TargetCacheError("release preparation requires exact clean source")
        try:
            ci_plan.require_qualifying_inputs(self.summary["runtime_inputs"])
        except ValueError as error:
            raise TargetCacheError(str(error)) from error
        if not self._resolve_cargo_identity() or not self._check_disk_headroom():
            raise TargetCacheError(f"release preparation failed: {self.summary}")
        target = Path(str(self.cargo_identity["target_directory"]))
        lock_path, dev_owner_path = target_state_paths(target, self.target_state_dir)
        owner_path = dev_owner_path.with_name(dev_owner_path.name.replace(".owner.json", ".release-owner.json"))
        lock = CargoTargetLock(lock_path, {
            "run_id": self.run_id, "profile": "release", "target_directory": str(target),
            "workspace_root": str(self.root), "commit": self.git_identity["commit"],
        })
        lock.acquire()
        self.target_lock = lock
        try:
            self._check_target_filesystem(target)
            # Full Cargo metadata can create .rustc_info.json. Record the target
            # before that command while holding the same lock used for handoff.
            initial_identity = self._directory_identity(target)
            initially_empty = initial_identity is None or not any(target.iterdir())
            if not initially_empty and not adopt and not os.path.lexists(owner_path):
                raise TargetCacheError("release ownership is unknown; explicit adoption or an empty target required")
            replacement = expected_recreated_target_identity
            if replacement is not None:
                if (not adopt or expected_owner_sha256 is None
                        or not isinstance(replacement, dict) or set(replacement) != {"device", "inode"}
                        or any(type(replacement[k]) is not int or replacement[k] < (1 if k == "inode" else 0)
                               for k in ("device", "inode"))
                        or not callable(recovery_inactive_check)):
                    raise TargetCacheError("explicit recreated-target recovery requires identity, owner hash and inactivity check")
                inspected = self.inspect_recreated_release_target()
                if inspected != {"owner_sha256": expected_owner_sha256, "target_identity": replacement}:
                    raise TargetCacheError("recreated target or owner changed since inspection")
            expected_owner_bytes = None
            if expected_owner_sha256 is not None:
                if (not adopt or len(expected_owner_sha256) != 64
                        or any(c not in "0123456789abcdef" for c in expected_owner_sha256)):
                    raise TargetCacheError("expected release owner requires explicit adoption and its SHA256")
                try:
                    descriptor = os.open(owner_path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
                    with os.fdopen(descriptor, "rb") as stream:
                        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
                            raise TargetCacheError("expected release owner is not a regular file")
                        expected_owner_bytes = stream.read(128 * 1024 + 1)
                    if (len(expected_owner_bytes) > 128 * 1024
                            or hashlib.sha256(expected_owner_bytes).hexdigest() != expected_owner_sha256):
                        raise TargetCacheError("release owner changed since restart inspection; preserved unchanged")
                except OSError as error:
                    raise TargetCacheError("expected release owner is unavailable; preserved unchanged") from error
            # Recheck source after acquiring ownership; a changed checkout refuses.
            if not self._capture_repository_state("release_locked_tree") or self.summary["release_locked_tree"] != self.summary["start_tree"]:
                raise TargetCacheError("release source changed before lock acquisition")
            scope = self._release_scope()
            if verification_check is not None:
                verification_check()
            cache_key = None
            if requested_build_argv is not None:
                try:
                    cache_key = self._release_build_inputs(requested_build_argv)
                except (KeyError, ValueError, TypeError, OSError) as error:
                    raise TargetCacheError(f"cannot identify incremental release inputs: {error}") from error
            self._workspace_inputs = build_inputs.workspace_snapshot(self.root, self.env, self._release_metadata)
            self._path_inputs = self._path_dependency_inputs()
            workspace = {k: self.git_identity[k] for k in ("workspace_root", "git_common_dir")}
            source = self.summary["start_tree"]
            identity = self._directory_identity(target)
            record = None
            malformed = False
            if owner_path.exists() or owner_path.is_symlink():
                try:
                    if owner_path.is_symlink():
                        raise ValueError("symlink marker")
                    record = json.loads(expected_owner_bytes if expected_owner_bytes is not None else owner_path.read_text())
                    if (not isinstance(record, dict) or record.get("schema_version") != 1
                            or not {"target_identity", "source", "workspace_owner", "packages"}.issubset(record)
                            or record.get("profile") != "release" or record.get("target_directory") != str(target)
                            or record.get("state") not in {"pending", "ready"}
                            or not isinstance(record.get("workspace_owner"), dict)
                            or set(record["workspace_owner"]) != {"workspace_root", "git_common_dir"}
                            or not all(isinstance(v, str) and v for v in record["workspace_owner"].values())
                            or not isinstance(record.get("source"), dict)
                            or set(record["source"]) != {"commit", "dirty", "dirty_files", "content_sha256"}
                            or record["source"].get("dirty") is not False
                            or record["source"].get("dirty_files") != []
                            or not isinstance(record["source"].get("commit"), str)
                            or not isinstance(record["source"].get("content_sha256"), str)
                            or len(record["source"]["content_sha256"]) != 64
                            or any(c not in "0123456789abcdef" for c in record["source"]["content_sha256"])
                            or not isinstance(record.get("target_identity"), (dict, type(None)))
                            or (record["target_identity"] is not None and
                                (set(record["target_identity"]) != {"device", "inode"} or
                                 not all(type(v) is int and v >= 0 for v in record["target_identity"].values())))
                            or not isinstance(record.get("packages"), list)
                            or not record["packages"]
                            or any(not isinstance(p, dict) or set(p) != {"name", "version", "spec"}
                                   or not all(isinstance(v, str) and v for v in p.values()) for p in record["packages"])):
                        raise ValueError("invalid release owner fields")
                    if record["state"] == "ready":
                        artifact = record.get("artifact")
                        if (not isinstance(artifact, dict) or set(artifact) != {"path", "sha256"}
                                or not all(isinstance(v, str) for v in artifact.values())
                                or len(artifact["sha256"]) != 64
                                or any(c not in "0123456789abcdef" for c in artifact["sha256"])
                                or not isinstance(record.get("build_argv"), list)):
                            raise ValueError("invalid release artifact fields")
                except (ValueError, TypeError, OSError) as error:
                    if not adopt or expected_owner_sha256 is not None:
                        raise TargetCacheError(f"malformed release ownership: {error}") from error
                    record = None
                    malformed = True
            if record is not None:
                # Automatic restart recovery retains the inspected owner, not
                # manual adoption's permission to take over another target.
                if expected_owner_sha256 is not None and (
                        (replacement is None and record["target_identity"] != identity)
                        or record["workspace_owner"] != workspace):
                    raise TargetCacheError("release target identity/workspace changed since restart inspection; preserved unchanged")
                old_scope = [(p["name"], p["version"]) for p in record["packages"]]
                old_names = {name for name, _version in old_scope}
                retained_scope = [(p["name"], p["version"]) for p in scope if p["name"] in old_names]
                # New members are cleanable through the current workspace; every
                # old member must still be present with its original name/version.
                if old_scope != retained_scope:
                    raise TargetCacheError("removed, renamed or version-changed workspace packages require a fresh release target")
                if record["state"] == "pending" and not adopt:
                    raise TargetCacheError("release handoff is pending; explicit adoption/recovery required")
                if (record.get("target_identity") != identity or record["workspace_owner"].get("git_common_dir") != workspace["git_common_dir"]) and not adopt:
                    raise TargetCacheError("release target identity/repository changed; explicit adoption required")
            # An existing dev marker is NOT evidence of release ownership.
            populated = target.exists() and any(target.iterdir())
            if record is None and not adopt:
                if not initially_empty:
                    raise TargetCacheError("release ownership is unknown; explicit adoption or an empty target required")
                if target.is_symlink() or (initial_identity is not None and identity != initial_identity):
                    raise TargetCacheError("fresh release target identity changed during preflight")
                # Only Cargo's metadata cache may have appeared since the
                # locked empty-target observation; never accept a symlink or
                # an unrelated file as evidence of an empty target.
                entries = list(target.iterdir()) if target.exists() else []
                if any(p.name != ".rustc_info.json" or not stat.S_ISREG(p.lstat().st_mode)
                       for p in entries):
                    raise TargetCacheError("fresh release target changed during preflight")
                populated = False
            reuse = bool(record and record["state"] == "ready" and not adopt
                         and record.get("target_identity") == identity
                         and record["workspace_owner"] == workspace
                         and (record["source"] == source if requested_build_argv is None else
                              cache_key is not None and record.get("build_cache_key") == cache_key))
            invalidated = self._provenance_cleanup(record) if record else []
            pending = {"schema_version": 1, "profile": "release", "state": "pending",
                       "target_directory": str(target), "target_identity": identity,
                       "workspace_owner": workspace, "source": source, "packages": scope,
                       "recorded_at": utc_now(), "cache_inputs": self._cache_provenance()}
            if cache_key is not None:
                pending["build_cache_key"] = cache_key
                self.env["HEX_GUARDED_CARGO_CONFIG_FINGERPRINT"] = cache_key
            report = {**pending, "owner_path": str(owner_path), "lock_path": str(lock_path),
                      "action": ("compiler-profile-handoff" if self._invalidate_dependency_profile else
                                 "source-or-dependency-handoff" if invalidated and reuse else
                                 "reuse" if reuse else ("adopted" if adopt else "handoff")),
                      "cleanup_status": "not_needed", "previous_owner": record,
                      "malformed_previous_owner": malformed}
            if replacement is not None:
                # Last check before pending ownership/cleanup, under the SAME
                # target lock. No process control is permitted by this callback.
                recovery_inactive_check()
                if self.inspect_recreated_release_target() != inspected:
                    raise TargetCacheError("recreated target or owner changed before mutation")
                report["explicit_recreated_target"] = inspected
            self.summary["release_target_cache"] = report
            self._write_release_owner(owner_path, pending)
            if (not reuse or invalidated or self._invalidate_dependency_profile) and populated:
                argv = self._workspace_clean_argv(target) if not reuse and not self._invalidate_dependency_profile else self._workspace_clean_argv(target)[:8]
                argv[3] = "release"
                for spec in invalidated:
                    name = self._clean_package_name(spec)
                    if name not in argv:
                        argv.extend(("--package", name))
                report["cleanup_argv"] = argv
                report["cleanup_status"] = "pending"
                # Keep release and dev cleanup evidence independent.
                log_path = self.output_dir / "release-handoff-clean.log"
                with log_path.open("w") as log:
                    result = subprocess.run(argv, cwd=self.root, env=self.env, stdout=log,
                                            stderr=subprocess.STDOUT, check=False)
                report.update(cleanup_exit_code=result.returncode, cleanup_log=str(log_path))
                if result.returncode != 0:
                    report["cleanup_status"] = "failed"
                    raise TargetCacheError("release cleanup failed; pending owner retained")
                report["cleanup_status"] = "passed"
            yield report
            if requested_build_argv is not None and report.get("build_argv") != requested_build_argv:
                raise TargetCacheError("release build arguments differ from locked cache identity")
            if requested_build_argv is not None and self._release_build_inputs(requested_build_argv) != cache_key:
                raise TargetCacheError("release toolchain/config/dependency inputs changed during build")
            if not isinstance(report.get("artifact"), dict) or report.get("build_exit_code") != 0:
                raise TargetCacheError("release artifact identification is required")
            artifact = report["artifact"]
            if self.identify_release_artifact(Path(artifact["path"]), report["build_argv"]) != artifact:
                raise TargetCacheError("release artifact changed before ownership completion")
            if not self._capture_repository_state("release_end_tree") or self.summary["release_end_tree"] != source:
                raise TargetCacheError("release source changed during build")
            self._completed_workspace_inputs()
            pending["cache_inputs"] = self._cache_provenance(checked=True)
            pending.update(state="ready", target_identity=self._directory_identity(target),
                           artifact=report["artifact"], build_argv=report.get("build_argv"), recorded_at=utc_now())
            self._write_release_owner(owner_path, pending)
            report["state"] = "ready"
        except BaseException as error:
            self.summary["release_target_error"] = {"type": type(error).__name__, "message": str(error)}
            raise
        finally:
            try:
                self._write_summary()
            finally:
                error = lock.release()
                self.target_lock = None
            if error:
                raise TargetCacheError(error)

    def identify_release_artifact(self, path: Path, argv: list[str]) -> dict[str, str]:
        """Identify the Cargo-reported optimized executable while still locked."""
        if self.target_lock is None or self.cargo_identity is None:
            raise TargetCacheError("release artifact identification requires target ownership")
        if argv[:2] != ["cargo", "build"] or "--release" not in argv or "--target-dir" in argv:
            raise TargetCacheError("unsupported release build arguments")
        target = Path(str(self.cargo_identity["target_directory"]))
        lexical = path.absolute()
        resolved = lexical.resolve()
        # Cargo's owned target is canonical. System aliases above it (macOS
        # /var -> /private/var, for example) are not artifact symlinks. Locate
        # the outermost lexical spelling of that same root, then inspect every
        # component below it without resolving away an in-target symlink.
        roots = [p for p in reversed(lexical.parents) if p.resolve() == target]
        root = roots[0] if roots else None
        if (target.resolve() != target or root is None or root.is_symlink()
                or self._directory_identity(root) != self._directory_identity(target)
                or not resolved.is_relative_to(target) or not lexical.is_file()):
            raise TargetCacheError("release artifact is not a plain file inside the owned target")
        relative = lexical.relative_to(root)
        component = root
        for part in relative.parts:
            if part == "..":
                raise TargetCacheError("release artifact traverses outside its target-relative path")
            component = component / part
            if component.is_symlink():
                raise TargetCacheError("release artifact is not a plain file inside the owned target")
        digest = hashlib.sha256()
        with path.open("rb") as stream:
            while chunk := stream.read(1024 * 1024):
                digest.update(chunk)
        return {"path": str(resolved), "sha256": digest.hexdigest()}

    def _run_target_cleanup(self, argv: list[str]) -> bool:
        started = time.monotonic()
        log_path = self.output_dir / "target-handoff-clean.log"
        try:
            with log_path.open("w", encoding="utf-8") as log:
                result = subprocess.run(
                    argv,
                    cwd=self.root,
                    env=self.env,
                    stdout=log,
                    stderr=subprocess.STDOUT,
                    check=False,
                )
        except OSError as error:
            log_path.write_text(f"workspace dev cleanup could not run: {error}\n", encoding="utf-8")
            self.summary["target_cache"]["cleanup_error"] = str(error)  # type: ignore[index]
            self.summary["target_cache"]["cleanup_log"] = str(log_path.relative_to(self.root))  # type: ignore[index]
            return False
        finally:
            self.summary["target_cache"]["cleanup_duration_seconds"] = round(time.monotonic() - started, 6)
        target_cache = self.summary["target_cache"]
        assert isinstance(target_cache, dict)
        target_cache["cleanup_exit_code"] = result.returncode
        target_cache["cleanup_log"] = str(log_path.relative_to(self.root))
        return result.returncode == 0

    def _check_disk_headroom(self) -> bool:
        """Read-only floor check; never remove caches, logs, source or artifacts."""
        configured = self.env.get("CARGO_TARGET_DIR")
        target = Path(configured) if configured else self.root / "target"
        if not target.is_absolute():
            target = self.root / target
        probes = []
        self.summary["disk_headroom"] = {
            "checked_at": utc_now(),
            "minimum_free_bytes": self.min_free_bytes,
            "target_source": "CARGO_TARGET_DIR" if configured else "workspace default",
            "probes": probes,
        }
        self.summary.setdefault("disk_history", []).append(self.summary["disk_headroom"])
        for label, requested in (("workspace", self.root), ("target", target)):
            probe = requested.absolute()
            while not probe.exists() and probe != probe.parent:
                probe = probe.parent
            entry = {"role": label, "requested_path": str(requested), "probed_path": str(probe)}
            probes.append(entry)
            try:
                usage = shutil.disk_usage(probe)
            except OSError as error:
                entry["error"] = str(error)
                self.summary["disk_error"] = f"could not inspect disk headroom for {label}: {error}"
                return False
            entry.update({"total_bytes": usage.total, "free_bytes": usage.free})
            if usage.free < self.min_free_bytes:
                self.summary["disk_error"] = (
                    f"{label} has {usage.free / GIB:.2f} GiB free; "
                    f"checks require the configured floor of {self.min_free_bytes / GIB:.2f} GiB. "
                    "Inspect inactive generated caches manually; preserve active/shared builds, "
                    "source/media and accepted artifacts. No files were removed. See docs/local-ci.md."
                )
                return False
        return True

    def _prepare_carry(self) -> None:
        """Decide per step from an earlier passed report; any refusal runs everything."""
        record: dict[str, object] = {"report": str(self.carry_report)}
        self.summary["carry"] = record
        try:
            raw = self.carry_report.read_bytes()
            evidence = json.loads(raw)
            record["sha256"] = hashlib.sha256(raw).hexdigest()
            if not self.plan:
                raise ValueError("base-advance carry requires a resolved merge plan")
            basis = ci_plan.carry_basis(self.root, self.plan, evidence, env=self.env)
            decisions = carry_decisions(self.steps, evidence, basis, self.plan["cargo_inventory"])
            dependency = evidence.get("target_cache", {}).get("dependency_inputs")
            if not isinstance(dependency, dict):
                raise ValueError("carried report lacks guarded toolchain/dependency identity")
        except (ValueError, OSError, KeyError, TypeError) as error:
            record["refused"] = f"{error}; every step runs fresh"
            print(f"carry refused: {record['refused']}", flush=True)
            return
        record.update(report_commit=basis["report_commit"], delta_paths=basis["delta_paths"],
                      delta_owners=basis["delta_owners"],
                      decisions=[{"step_id": step.step_id, "action": d["action"], "reason": d["reason"]}
                                 for step, d in zip(self.steps, decisions)])
        self._carry = decisions
        self._carry_dependency_inputs = dependency
        self._carry_report_path = str(self.carry_report.resolve())

    def _drop_carry(self, reason: str) -> None:
        self._carry = None
        self.summary["carry"]["refused"] = f"{reason}; remaining steps run fresh"
        print(f"carry refused: {self.summary['carry']['refused']}", flush=True)

    def _carry_origin(self, entry: dict) -> dict:
        """The run that actually executed a result, preserved through carry chains."""
        return entry.get("carried_from") or entry.get("carry", {}).get("carried_remainder_from") or {
            "report": self._carry_report_path, "commit": self.summary["carry"]["report_commit"],
            "log": entry.get("log")}

    def _record_carried(self, step: Step, index: int, decision: dict) -> None:
        entry = decision["entry"]
        self.summary["steps"].append({
            "step_id": step.step_id, "argv": list(step.argv), "status": "carried",
            "started_at": None, "ended_at": None, "start_offset_seconds": None,
            "duration_seconds": 0.0, "timing_estimates": None, "exit_code": 0,
            "log": None, "error": None, "execution": "carried",
            "test_results": entry.get("test_results", {}),
            "carried_from": self._carry_origin(entry), "carry_reason": decision["reason"],
        })
        self._write_summary()
        print(f"[{index}/{len(self.steps)}] {step.step_id}: carried ({decision['reason']})", flush=True)

    def _run_step(self, step: Step, index: int) -> dict[str, object]:
        started = time.monotonic()
        text = None
        phase = "opening step log"
        log_path = self.output_dir / f"{index:02d}-{step.step_id}.log"
        entry: dict[str, object] = {
            "step_id": step.step_id,
            "argv": list(step.argv),
            "status": "running",
            "started_at": utc_now(),
            "ended_at": None,
            "start_offset_seconds": round(started - self.started_monotonic, 6),
            "duration_seconds": None,
            "timing_estimates": None,
            "exit_code": None,
            "log": str(log_path.relative_to(self.root)),
            "error": None,
            "execution": "executed",
        }
        steps = self.summary["steps"]
        assert isinstance(steps, list)
        steps.append(entry)
        try:
            self._write_summary()
        except ReportPublicationError:
            entry.update(status="not_run", started_at=None, start_offset_seconds=None,
                         log=None, error="report publication failed before command admission")
            raise
        print(f"[{index}/{len(self.steps)}] {step.step_id}: running", flush=True)
        if step.step_id == "runner-tests" and self.summary.get("plan") and "python_tests" in self.summary["plan"]:
            selection = self.summary["plan"]["python_tests"]
            print("python self-tests: " + json.dumps(selection, sort_keys=True), flush=True)
        try:
            with log_path.open("w", encoding="utf-8") as log, \
                    terminal_progress(f"checking {step.step_id}", announce=False):
                phase = "running command"
                result = subprocess.run(
                    list(step.argv),
                    cwd=self.root,
                    env=self.env,
                    stdout=log,
                    stderr=subprocess.STDOUT,
                    check=False,
                )
            entry["exit_code"] = result.returncode
            entry["status"] = "passed" if result.returncode == 0 else "failed"
            phase = "reading step log"
            text = log_path.read_text(errors="replace")
            rust_counts = [int(n) for n in re.findall(r"test result: ok\. (\d+) passed;", text)]
            python_counts = [int(n) for n in re.findall(r"^Ran (\d+) tests? in ", text, re.MULTILINE)]
            skipped = [int(n) for n in re.findall(r"^OK \(skipped=(\d+)\)", text, re.MULTILINE)]
            entry["test_results"] = {"rust_passed_by_binary": rust_counts,
                                     "python_executions": python_counts, "python_skipped": skipped}
            if step.requires_tests and result.returncode == 0 and sum(rust_counts) + sum(python_counts) - sum(skipped) <= 0:
                entry["status"] = "failed"
                entry["error"] = "requested test scope executed no passing tests; zero-match/all-skipped is not qualification"
        except FileNotFoundError as error:
            entry["status"] = "failed"
            entry["error"] = (f"required executable not found: {error.filename}"
                              if phase == "running command" else f"log I/O failure while {phase}: {error}")
            self._retain_step_error(log_path, entry)
        except InterruptedRun:
            entry["status"] = "interrupted"
            entry["error"] = "pipeline interrupted"
            entry["exit_code"] = 130
            entry["ended_at"] = utc_now()
            self._retain_step_error(log_path, entry)
            raise
        except OSError as error:
            entry["status"] = "failed"
            entry["error"] = (f"command could not run: {error}" if phase == "running command"
                              else f"log I/O failure while {phase}: {error}")
            self._retain_step_error(log_path, entry)
        finally:
            ended = time.monotonic()
            duration = ended - started
            entry["ended_at"] = utc_now()
            entry["duration_seconds"] = round(duration, 6)
            if self.summary["time_to_first_result_seconds"] is None:
                self.summary["time_to_first_result_seconds"] = round(ended - self.started_monotonic, 6)
            if text is None:
                try:
                    text = log_path.read_text(errors="replace")
                except OSError:
                    pass  # Missing diagnostics do not change the step's result.
            if text is not None:
                entry["timing_estimates"] = log_timing_estimates(text, duration)
        self._write_summary()
        print(f"[{index}/{len(self.steps)}] {step.step_id}: {entry['status']}", flush=True)
        return entry

    @staticmethod
    def _retain_step_error(log_path: Path, entry: dict[str, object]) -> None:
        # Never replace captured output or let a second log error hide the actual failure.
        try:
            with log_path.open("a", encoding="utf-8") as log:
                log.write(f"\n{entry['error']}\n")
        except OSError as error:
            entry["log_error"] = str(error)

    def _diagnostic_text(self, text: str) -> str:
        # Console excerpts are not raw logs. Remove terminal controls and known sensitive
        # environment values without printing the environment or altering retained evidence.
        text = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", text)
        text = re.sub(r"[\x00-\x08\x0b-\x1f\x7f]", "", text)
        sensitive = r"token|password|passwd|secret|credential|authorization|(?:api|access|private)[_-]?key"
        for key, value in sorted(self.env.items(), key=lambda item: len(item[1]), reverse=True):
            if value and re.search(sensitive, key, re.IGNORECASE):
                text = text.replace(value, "[redacted]")
        text = re.sub(r"(?im)(\b[\w-]*(?:" + sensitive + r")[\w-]*\s*[:=]\s*)[^\n]+",
                      r"\1[redacted]", text)
        text = re.sub(r"(?i)(--[\w-]*(?:" + sensitive + r")[\w-]*\s+)(?:'[^']*'|\"[^\"]*\"|\S+)",
                      r"\1[redacted]", text)
        return text

    def _print_failure_details(self) -> None:
        """Present the settled result; diagnostics never confer qualification or execute retries."""
        status = self.summary.get("status")
        if status not in {"failed", "interrupted", "stale", "incomplete"}:
            return
        publication = self.summary.get("report_publication_error")
        stream = sys.stderr if publication else sys.stdout
        report_note = (["report publication failed: " + self._diagnostic_text(str(publication))[:2000],
                        "durable summary is unavailable or incomplete; this run does not qualify"]
                       if publication else [])
        preparation = self.summary.get("target_preparation_failure")
        if preparation:
            cache = self.summary.get("target_cache", {})
            log = cache.get("cleanup_log")
            details = [f"local ci diagnostic: {status}", f"profile: {self.mode}",
                       "step: cargo-target-preparation (preflight refusal)",
                       f"before step: {preparation['before_step']}",
                       f"summary: {self.summary_path}",
                       f"log: {self.root / log if log else 'no cleanup started; see summary'}",
                       str(self.summary.get("target_cache_error", "target preparation refused"))]
            if cache.get("cleanup_argv"):
                details.append("cleanup command: " + shlex.join(cache["cleanup_argv"])[:2000])
            details.append("rerun the original local CI invocation after resolving the refusal; prior passing stages are not a complete check set")
            print(self._diagnostic_text("\n".join(details + report_note)), file=stream, flush=True)
            return
        executed = [s for s in self.summary["steps"] if s["status"] != "not_run"]
        step = next((s for s in executed if s["status"] in {"failed", "interrupted"}),
                    executed[-1] if executed else None)
        category = "preflight refusal"
        excerpt = ""
        log_note = "log unavailable; see summary"
        if step:
            if step["log"]:
                path = self.root / step["log"]
                log_note = str(path)
                try:
                    with path.open("rb") as log:
                        log.seek(0, os.SEEK_END)
                        log.seek(max(0, log.tell() - 65536))
                        tail = log.read(65536).decode("utf-8", errors="replace")
                    lines = self._diagnostic_text(tail).splitlines()
                    marker = r"^\s*(?:error(?:\[|:)|thread .*panicked at|---- .* stdout ----|FAIL:|ERROR:|Traceback \()"
                    start = next((i for i, line in enumerate(lines) if re.search(marker, line)), None)
                    if start is not None:
                        excerpt = "\n".join(line[:240] for line in lines[start:start + 20])[:4000]
                except OSError:
                    log_note += " (unreadable; failure retained in summary)"
            error = str(step.get("error") or "")
            category = ("interrupted" if step["status"] == "interrupted" else
                        "no passing tests" if "no passing tests" in error else
                        "missing executable" if "required executable not found" in error else
                        "log I/O failure" if "log I/O failure" in error else
                        "compiler failure" if re.search(r"error\[E\d+\]|error:.*could not compile|--> .*:\d+:\d+", excerpt) else
                        "test failure" if re.search(r"panicked at|stdout ----|FAIL:|ERROR:|Traceback", excerpt) else
                        "command failure")
        if publication and (step is None or step["status"] not in {"failed", "interrupted"}):
            category = "report publication failure"
        result = "stale (source changed; prior results do not qualify)" if status == "stale" else status
        details = [f"local ci diagnostic: {result}", f"profile: {self.mode}",
                   f"step: {step['step_id'] if step else 'not started'} ({category})",
                   f"summary: {self.summary_path}", f"log: {log_note}"]
        if step:
            command = self._diagnostic_text(shlex.join(step["argv"]))
            truncated = len(command) > 2000
            if truncated:
                command = command[:2000] + " ... [truncated; exact argv retained in summary]"
            details.extend((f"command: {command}", f"cwd: {self.root}"))
            if publication:
                details.append(f"command result: {step['status']}; exit code: {step['exit_code']}")
            if step.get("error"):
                details.append(self._diagnostic_text(str(step["error"]))[:2000])
            if excerpt:
                details.extend(("failure excerpt (at most 20 lines / 4000 characters from last 64 KiB):", excerpt))
            else:
                details.append("no recognized compiler/test excerpt; inspect the retained log")
            rerun = "use the exact argv retained in the summary" if truncated else command
            details.append(f"rerun (diagnostic only; partial, not merge qualification): {rerun}")
        else:
            details.extend(self._diagnostic_text(str(value))[:2000]
                           for key, value in self.summary.items() if key.endswith("_error"))
        if status == "stale":
            details.append("freeze the intended candidate and rerun its required plan before qualification")
        print(self._diagnostic_text("\n".join(details + report_note)), file=stream, flush=True)

    def run(self) -> int:
        self.summary["native_acceptance_opt_in"] = bool(
            ci_plan.python_tests.native_acceptance_inputs(self.env))
        self.output_dir.mkdir(parents=True, exist_ok=False)
        try:
            self._write_summary()
        except ReportPublicationError:
            self._fail_report_publication()
            try:
                self._print_failure_details()
            except (OSError, ValueError):
                pass  # An unavailable console cannot turn missing evidence into success.
            return int(self.summary["exit_code"])
        previous_handlers: dict[int, object] = {}

        def interrupt_handler(_signum: int, _frame: object) -> None:
            raise InterruptedRun

        for signum in (signal.SIGINT, signal.SIGTERM):
            previous_handlers[signum] = signal.signal(signum, interrupt_handler)

        failed = False
        try:
            invalid_ids = [step.step_id for step in self.steps
                           if re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]{0,99}", step.step_id) is None]
            if invalid_ids:
                self.summary.update(status="failed", verification_status="failed", exit_code=2,
                                    step_id_error=f"invalid step IDs: {invalid_ids!r}; expected 1–100 ASCII characters: "
                                                  "alphanumeric first, then alphanumeric, underscore or hyphen")
                print(self.summary["step_id_error"], file=sys.stderr, flush=True)
            elif not self._capture_repository_state("start_tree"):
                self.summary["status"] = "failed"
                self.summary["exit_code"] = 2
                self.summary["metadata_error"] = "could not capture exact commit and dirty state"
            elif self.plan and self.plan.get("reuse") and (
                    self.summary["native_acceptance_opt_in"]
                    or self.plan["reuse"].get("native_acceptance_opt_in") is not False):
                # Recheck the actual runner environment, not only the earlier
                # resolver process, before Cargo identity/target preparation.
                self.summary.update(status="failed", exit_code=2,
                                    reuse_error="native/external acceptance opt-in is present or unknown; fresh checks required")
            elif self.plan and self.plan["qualifies_merge"] and (
                    self.summary["start_tree"]["dirty"]
                    or self.summary["start_tree"]["commit"] != self.plan["source"]
                    or ci_plan.runtime_inputs(self.root) != self.plan["runtime_inputs"]):
                self.summary.update(status="failed", exit_code=2,
                                    metadata_error="resolved plan source/inputs changed or are dirty")
            elif any(self._uses_cargo_target(step) for step in self.steps) and not self._resolve_cargo_identity():
                self.summary["status"] = "failed"
                self.summary["exit_code"] = 2
                self.summary["metadata_error"] = "could not resolve exact Cargo workspace and target identity"
                print(self.summary["cargo_metadata_error"], file=sys.stderr, flush=True)
            elif not self._check_disk_headroom():
                self.summary["status"] = "failed"
                self.summary["exit_code"] = 2
                print(self.summary["disk_error"], file=sys.stderr, flush=True)
            else:
                if self.mode in {"full", "merge", "release"}:
                    try:
                        self._checkpoint_inputs = ci_checkpoint.execution_inputs(self.root, self.env, self.steps)
                    except (ValueError, OSError) as error:
                        # Ordinary validation remains available when a checkpoint
                        # cannot identify every command. No inheritance is allowed.
                        self.summary["checkpoint_unavailable"] = str(error)
                if self.resume_report is not None:
                    try:
                        if self._checkpoint_inputs is None:
                            raise ValueError("continuation executable inputs are unavailable")
                        self._resume_evidence, lineage = ci_checkpoint.load(
                            self.root, self.resume_report, self.summary, self.steps, self._checkpoint_inputs)
                        if not self._prepare_target_cache():
                            raise ValueError(self.summary.get("target_cache_error", "target guard refused"))
                        self.summary["resume"] = lineage
                        self.summary["steps"] = ci_checkpoint.inherited_steps(self._resume_evidence, lineage)
                    except (ValueError, OSError, KeyError, TypeError) as error:
                        self.summary["resume_error"] = str(error)
                        failed = True
                if self.carry_report is not None:
                    self._prepare_carry()
                self._write_summary()
                for index, step in enumerate(self.steps, start=1):
                    if failed:
                        break
                    if index <= len(self.summary["steps"]):
                        continue
                    if self.mode in {"full", "merge", "release"} and self._uses_cargo_target(step) and not self._check_disk_headroom():
                        print(self.summary["disk_error"], file=sys.stderr, flush=True)
                        if all("free_bytes" in p for p in self.summary["disk_headroom"]["probes"]):
                            self.summary["disk_refusal"] = {"before_step": index - 1}
                        failed = True
                        break
                    if self._uses_cargo_target(step) and self.target_lock is None:
                        if not self._prepare_target_cache():
                            self.summary["target_preparation_failure"] = {"before_step": step.step_id}
                            print(
                                self.summary.get("target_cache_error", "could not prepare Cargo target ownership"),
                                file=sys.stderr,
                                flush=True,
                            )
                            failed = True
                            break
                        if self.plan and self.plan.get("reuse") and self.plan["reuse"]["dependency_inputs"] != self._dependency_inputs:
                            self.summary["reuse_error"] = "runtime report toolchain/config/dependency inputs differ"
                            failed = True
                            break
                        if self._carry is not None and self._carry_dependency_inputs != self._dependency_inputs:
                            self._drop_carry("carried report toolchain/config/dependency inputs differ")
                    decision = self._carry[index - 1] if self._carry is not None else {"action": "run"}
                    if decision["action"] == "carry" and self.target_lock is not None:
                        self._record_carried(step, index, decision)
                        continue
                    if decision["action"] == "narrow" and self.target_lock is not None:
                        result = self._run_step(Step(step.step_id, tuple(decision["argv"]),
                                                     step.uses_cargo_target, step.requires_tests), index)
                        result["carry"] = {"planned_argv": list(step.argv),
                                           "carried_remainder_from": self._carry_origin(decision["entry"]),
                                           "reason": decision["reason"]}
                        self._write_summary()
                    else:
                        result = self._run_step(step, index)
                    if result["status"] != "passed":
                        failed = True
                        break
                if self.summary.get("resume") and not failed:
                    try:
                        ci_checkpoint.validate_inheritance(self.root, self.summary)
                        if self._checkpoint_inputs != ci_checkpoint.execution_inputs(self.root, self.env, self.steps):
                            raise ValueError("continuation execution inputs changed during checks")
                    except (ValueError, OSError, KeyError, TypeError) as error:
                        self.summary["resume_error"] = str(error)
                        failed = True
                if self.mode == "merge" and not failed:
                    scoped_passed = [sum(entry.get("test_results", {}).get("rust_passed_by_binary", []))
                                     for entry in self.summary["steps"]
                                     if isinstance(entry, dict) and module_scoped(tuple(entry.get("argv") or ()))
                                     and "--doc" not in (entry.get("argv") or ())]
                    if scoped_passed and sum(scoped_passed) <= 0:
                        self.summary["module_scope_error"] = (
                            "changed modules ran no tests: add boundary tests to the changed module")
                        failed = True
                if self.mode == "merge" and not failed:
                    self.summary["test_budget"] = merge_test_budget(
                        self.summary["steps"], self.over_budget_reason)
                partial = self.mode == "quick" or self.plan and not self.plan["qualifies_merge"]
                self.summary["verification_status"] = "failed" if failed else ("partial" if partial else "passed")
                self.summary["status"] = self.summary["verification_status"]
                self.summary["exit_code"] = (2 if "disk_error" in self.summary else 1) if failed else 0
        except ReportPublicationError:
            self._fail_report_publication()
        except InterruptedRun:
            self.summary["status"] = "interrupted"
            self.summary["exit_code"] = 130
            steps = self.summary["steps"]
            if isinstance(steps, list):
                for entry in steps:
                    if isinstance(entry, dict) and entry["status"] == "running":
                        entry["status"] = "interrupted"
                        entry["ended_at"] = utc_now()
                        entry["error"] = "pipeline interrupted"
        finally:
            try:
                if self.target_lock is not None:
                    try:
                        if self.target_owner_claimed and self.target_owner_path is not None and self.git_identity is not None:
                            try:
                                if self._dependency_context() != self._dependency_inputs:
                                    raise TargetCacheError("dependency inputs changed during guarded checks")
                                self._completed_workspace_inputs()
                                self._write_target_owner(self.target_owner_path, self.git_identity, checked=self.summary.get("exit_code") == 0)
                                if self.summary.get("disk_refusal"):
                                    self._checkpoint_target = {
                                        "owner_sha256": ci_checkpoint.owner_digest(self.target_owner_path),
                                        "target_identity": self._directory_identity(Path(self.cargo_identity["target_directory"])),
                                        "cargo_identity": self.cargo_identity,
                                        "dependency_inputs": self._dependency_inputs,
                                    }
                            except InterruptedRun:
                                self.summary.update(status="interrupted", exit_code=130,
                                                    target_owner_update_error="dependency identity interrupted; owner not advanced")
                            except Exception as error:
                                # Fingerprinting can refuse malformed paths, cycles or
                                # unsupported file types. None may bypass cleanup.
                                self.summary.update(status="incomplete", exit_code=2,
                                                    target_owner_update_error=f"{type(error).__name__}: {error}")
                    finally:
                        try:
                            release_error = self.target_lock.release()
                            if release_error is not None:
                                self.summary["target_lock_release_error"] = release_error
                        except InterruptedRun:
                            self.summary.update(status="interrupted", exit_code=130,
                                                target_lock_release_error="lock release interrupted")
                        except Exception as error:
                            self.summary.update(status="incomplete", exit_code=2,
                                                target_lock_release_error=f"{type(error).__name__}: {error}")
                        finally:
                            self.target_lock = None
                self.summary["ended_at"] = utc_now()
                if self.summary.get("start_tree") is not None:
                    if self._capture_repository_state("end_tree"):
                        start_tree = self.summary["start_tree"]
                        end_tree = self.summary["end_tree"]
                        stale = start_tree != end_tree
                        self.summary["stale"] = stale
                        if stale:
                            self.summary["verification_status"] = self.summary.get("status")
                            self.summary["status"] = "stale"
                            self.summary["exit_code"] = 3
                    else:
                        self.summary["stale"] = None
                        self.summary["status"] = "incomplete"
                        self.summary["exit_code"] = 2
                steps = self.summary["steps"]
                if isinstance(steps, list):
                    next_index = len(steps) + 1
                    for index, step in enumerate(self.steps[next_index - 1 :], start=next_index):
                        steps.append(
                            {
                                "step_id": step.step_id,
                                "argv": list(step.argv),
                                "status": "not_run",
                                "started_at": None,
                                "ended_at": None,
                                "start_offset_seconds": None,
                                "duration_seconds": None,
                                "timing_estimates": None,
                                "exit_code": None,
                                "log": None,
                                "error": "pipeline stopped before this step",
                            }
                        )
                self.summary["duration_seconds"] = round(time.monotonic() - self.started_monotonic, 6)
                if self._checkpoint_inputs is not None and self.summary.get("disk_refusal"):
                    try:
                        if self._checkpoint_inputs != ci_checkpoint.execution_inputs(self.root, self.env, self.steps):
                            raise ValueError("execution inputs changed during original checks")
                        self.summary["checkpoint"] = ci_checkpoint.seal(
                            self.root, self.summary, self.steps, self._checkpoint_inputs, self._checkpoint_target)
                    except (ValueError, OSError, KeyError, TypeError) as error:
                        if self.summary.get("disk_refusal"):
                            self.summary["checkpoint_unavailable"] = str(error)
                if self.summary.get("report_publication_error"):
                    self._fail_report_publication()
                try:
                    self._write_summary()
                except ReportPublicationError:
                    self._fail_report_publication()
            finally:
                # Even an end-snapshot or summary-write exception must restore
                # the caller's process-wide signal handlers.
                for signum, handler in previous_handlers.items():
                    signal.signal(signum, handler)
        try:
            self._print_failure_details()
        except (OSError, ValueError):
            pass  # Console failure must not replace the recorded result or cleanup.
        return int(self.summary["exit_code"] if self.summary["exit_code"] is not None else 2)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--quick",
        action="store_true",
        help="run architecture/UI/GPUI guards and formatting only; this is not full CI",
    )
    parser.add_argument("--profile", choices=("edit", "merge", "full", "release", "health"),
                        help="explicit deterministic scope; default remains full audit")
    parser.add_argument("--base", help="immutable integrated comparison base for scoped checks")
    parser.add_argument("--source", help="exact current candidate commit for scoped checks")
    parser.add_argument("--reuse-report", type=Path,
                        help="explicit prior runtime certificate for a prose-only continuation")
    parser.add_argument("--over-budget-reason",
                        help="record why a merge run exceeds the one-minute test budget (cite the Pulse issue)")
    parser.add_argument("--carry-report", type=Path,
                        help="after merging a newer base, reuse unaffected steps of this passed merge summary")
    parser.add_argument("--resume-report", type=Path,
                        help="continue an exact-source plan stopped solely by a pre-step disk refusal")
    parser.add_argument("--package", help="explicit workspace package for partial edit feedback")
    parser.add_argument("--test-filter", help="explicit nonempty Rust test filter for partial edit feedback")
    parser.add_argument("--features", default="", help="explicit package features for partial edit feedback")
    target = parser.add_mutually_exclusive_group()
    target.add_argument("--lib", action="store_true", help="test only the package library in an explicit edit plan")
    target.add_argument("--test", metavar="NAME", help="test only this named integration target in an explicit edit plan")
    parser.add_argument(
        "--min-free-gib", type=int,
        help="read-only free-space floor in GiB; default 2 for full and 0 for quick; never deletes files",
    )
    parser.add_argument(
        "--adopt-target",
        action="store_true",
        help="assert sole ownership of an unowned Cargo target and clean only this workspace's dev artifacts",
    )
    parser.add_argument("--prepare-changelog", action="store_true",
                        help="generate notes before freezing source, then exit without running sealed checks")
    parser.add_argument("--changelog-base", help="explicit first-generation Git baseline")
    parser.add_argument("--changelog-through", default="HEAD", help="immutable generation range end")
    arguments = parser.parse_args(argv)
    if arguments.resume_report and (arguments.reuse_report or arguments.adopt_target
                                    or arguments.quick or arguments.profile == "edit"
                                    or arguments.prepare_changelog):
        parser.error("--resume-report cannot combine reuse/adoption/quick/edit/changelog preparation")
    if arguments.carry_report and (arguments.reuse_report or arguments.resume_report or arguments.quick
                                   or arguments.profile != "merge"):
        parser.error("--carry-report requires --profile merge and cannot combine reuse/resume/quick")
    if arguments.prepare_changelog:
        command = [sys.executable, str(ROOT / "scripts/changelog.py"), "prepare",
                   "--source", arguments.changelog_through]
        if arguments.changelog_base:
            command.extend(["--base", arguments.changelog_base])
        return subprocess.run(command, cwd=ROOT).returncode
    if arguments.changelog_base or arguments.changelog_through != "HEAD":
        parser.error("changelog range arguments require --prepare-changelog")
    if arguments.min_free_gib is not None and not 0 <= arguments.min_free_gib <= sys.maxsize // GIB:
        parser.error("--min-free-gib must be a bounded nonnegative integer")
    mode = "quick" if arguments.quick else "full"
    plan = None
    if arguments.profile in {"merge", "edit", "health"} or arguments.base or arguments.source:
        if arguments.quick or not arguments.profile or not arguments.base or not arguments.source:
            parser.error("scoped profiles require --profile, --base and --source; cannot combine --quick")
        try:
            metadata = json.loads(command_output(["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"], ROOT, dict(os.environ)).stdout)
            plan = ci_plan.resolve(ROOT, profile=arguments.profile, base=arguments.base,
                                   source=arguments.source, metadata=metadata)
            if arguments.reuse_report:
                plan = reusable_plan(ROOT, plan, arguments.reuse_report)
            if arguments.package or arguments.test_filter or arguments.features or arguments.lib or arguments.test is not None:
                if arguments.profile != "edit" or not arguments.package or not arguments.test_filter:
                    raise ValueError("package/filter/features require --profile edit with a nonempty package and test filter")
                plan["edit"] = edit_selection(plan["cargo_inventory"], package=arguments.package,
                                              test_filter=arguments.test_filter, features=arguments.features,
                                              lib=arguments.lib, test=arguments.test)
                plan.pop("plan_sha256")
                plan["plan_sha256"] = hashlib.sha256(json.dumps(plan, sort_keys=True).encode()).hexdigest()
        except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
            parser.error(f"cannot resolve conservative check plan: {error}")
        mode = arguments.profile
    elif arguments.profile and arguments.quick:
        parser.error("--profile cannot be combined with --quick")
    elif arguments.reuse_report:
        parser.error("--reuse-report requires an explicit merge plan")
    elif arguments.package or arguments.test_filter or arguments.features or arguments.lib or arguments.test is not None:
        parser.error("package/filter/features require an explicit edit plan")
    runner = LocalCiRunner(
        mode=mode,
        min_free_bytes=None if arguments.min_free_gib is None else arguments.min_free_gib * GIB,
        adopt_target=arguments.adopt_target,
        plan=plan,
        resume_report=arguments.resume_report,
        carry_report=arguments.carry_report,
        over_budget_reason=arguments.over_budget_reason,
    )
    try:
        exit_code = runner.run()
    except FileExistsError:
        print(f"run output already exists: {runner.output_dir}", file=sys.stderr)
        return 2
    except OSError as error:
        print(f"local ci could not retain its complete report at {runner.summary_path}: {error}", file=sys.stderr)
        return 2
    carried = [s for s in runner.summary.get("steps", []) if isinstance(s, dict) and s.get("execution") == "carried"]
    if carried:
        print(f"{len(carried)} step(s) carried from {runner.summary['carry']['report']}, not re-executed")
    budget = runner.summary.get("test_budget")
    if isinstance(budget, dict) and budget.get("exceeded"):
        slowest = ", ".join(f"{s['step_id']} {s['seconds']:.0f}s" for s in budget["slowest"])
        verdict = (f"reason: {budget['over_budget_reason']}" if budget.get("over_budget_reason") else
                   "not a merge failure; record a recurring slow suite in Pulse")
        print(f"warning: merge test time {budget['test_seconds']:.0f}s exceeds the "
              f"{budget['limit_seconds']:.0f}s budget ({slowest}); {verdict}", file=sys.stderr)
    print(f"local ci {runner.summary['status']}: {runner.summary_path}")
    if mode == "health" and runner.summary["status"] == "failed":
        print(f"main is red after merge {plan['source']}: owning checks did not pass; "
              "fix forward before further merges", file=sys.stderr)
    if runner.summary["status"] == "passed" and mode == "merge":
        print(f"if the base advances: merge it, then rerun with --carry-report {runner.summary_path}")
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
