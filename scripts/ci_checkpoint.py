"""Immutable passed-stage observations for one bounded disk-refusal continuation.

This does not certify cached Cargo artifacts. The runner still owns target locking,
input identification and conservative cache invalidation. Local reports are not
signed: hashes detect drift/corruption, not deliberate coordinated forgery.
"""

from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
from types import SimpleNamespace


def digest(value) -> str:
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


def require_no_jobs() -> None:
    """Conservative process preflight; unknown Cargo writers cannot be inherited.

    The cooperative target lock remains necessary. This snapshot cannot prevent
    a later uncooperative writer; operators must keep raw Cargo off the target.
    Refuse any visible Cargo/compiler job rather than guess its target from argv.
    """
    if sys.platform not in {"darwin", "linux"}:
        raise ValueError("checkpoint process inspection is unsupported on this platform")
    try:
        result = subprocess.run(["/bin/ps", "-axo", "pid=,comm="], capture_output=True,
                                text=True, check=False, timeout=10)
    except (OSError, subprocess.SubprocessError) as error:
        raise ValueError("cannot establish absence of Cargo/compiler jobs") from error
    if result.returncode != 0 or not result.stdout.strip():
        raise ValueError("cannot establish absence of Cargo/compiler jobs")
    for row in result.stdout.splitlines():
        fields = row.strip().split(None, 1)
        if len(fields) != 2 or not fields[0].isdigit():
            raise ValueError("unrecognized process inventory; checkpoint continuation refused")
        if Path(fields[1]).name in {"cargo", "rustc", "rustdoc", "rustup"}:
            raise ValueError("Cargo/compiler job is active; wait for it to finish before continuing")


def file_digest(path: Path) -> str:
    path = Path(path).absolute()
    if any(p.is_symlink() for p in (path, *path.parents)):
        raise ValueError("checkpoint evidence contains a symlink")
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as stream:
        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
            raise ValueError("checkpoint evidence is not a regular file")
        value = hashlib.sha256()
        while chunk := stream.read(1024 * 1024):
            value.update(chunk)
        return value.hexdigest()


def owner_digest(path: Path) -> str:
    """Normalize the existing guard's state directory (macOS /var is an alias).

    This is only for the runner-selected target owner path. The final marker
    remains a regular non-symlink file; report/log evidence keeps stricter paths.
    """
    return file_digest(path.parent.resolve(strict=True) / path.name)


def execution_inputs(root: Path, env: dict, steps) -> dict:
    """Hash environment values without publishing secrets, plus command binaries."""
    if any(value and (key.startswith("HEX_VST3_") or key == "HEX_TEST_SIGNING_IDENTITY")
           for key, value in env.items()):
        raise ValueError("external native fixtures/signing inputs require fresh checks; checkpointing unavailable")
    search = os.pathsep.join(str((root / p).resolve()) if not Path(p).is_absolute() else p
                             for p in env.get("PATH", os.defpath).split(os.pathsep))
    tools = {}
    for name in sorted({s.argv[0] for s in steps} | {sys.executable}):
        resolved = str(root / name) if os.path.dirname(name) and not Path(name).is_absolute() else shutil.which(name, path=search)
        if resolved is None:
            raise ValueError("cannot identify checkpoint executable: " + name)
        path = Path(resolved).resolve(strict=True)
        tools[name] = {"path": str(path), "sha256": file_digest(path),
                       "mode": path.stat().st_mode}
    return {"environment_sha256": digest({k: v for k, v in env.items()
            # Tool-call identity changes between invocations and is not consumed
            # by repository checks/builds. Keep actual configuration values bound.
            if k not in {"_", "CODEX_TOOL_CALL_ID", "HEX_GUARDED_CARGO_CONFIG_FINGERPRINT"}}),
            "tools": tools, "python_version": sys.version, "platform": sys.platform}


def step_plan(steps) -> list[dict]:
    return [{"step_id": s.step_id, "argv": list(s.argv),
             "requires_tests": s.requires_tests, "uses_cargo_target": s.uses_cargo_target}
            for s in steps]


def passed_prefix(report: dict, plan: list[dict]) -> int:
    rows = report["steps"]
    if (not isinstance(rows, list) or any(not isinstance(r, dict) for r in rows)
            or len(rows) != len(plan) or len({p["step_id"] for p in plan}) != len(plan)):
        raise ValueError("checkpoint required plan is incomplete or ambiguous")
    count = 0
    stopped = False
    for row, required in zip(rows, plan):
        if (row["step_id"], row["argv"]) != (required["step_id"], required["argv"]):
            raise ValueError("checkpoint step order or command changed")
        if row["status"] == "not_run":
            stopped = True
            if row.get("exit_code") is not None or row.get("log") is not None:
                raise ValueError("checkpoint has an ambiguous unexecuted stage")
            continue
        if stopped or row["status"] != "passed" or row.get("exit_code") != 0 or row.get("error"):
            raise ValueError("checkpoint contains a failed, interrupted or unknown stage")
        if required["requires_tests"]:
            counts = row["test_results"]
            if not isinstance(counts, dict):
                raise ValueError("checkpoint test counts are malformed")
            values = [counts.get(k, []) for k in
                      ("rust_passed_by_binary", "python_executions", "python_skipped")]
            if (any(not isinstance(v, list) or any(type(n) is not int or n < 0 for n in v) for v in values)
                    or sum(values[0]) + sum(values[1]) - sum(values[2]) <= 0):
                raise ValueError("checkpoint has no positive executed tests")
        count += 1
    if not 0 < count < len(plan):
        raise ValueError("checkpoint needs a nonempty passed prefix and unexecuted suffix")
    return count


def log_records(root: Path, report: dict, count: int) -> list[dict]:
    records = []
    for row in report["steps"][:count]:
        path = root / row["log"]
        if not path.resolve().is_relative_to((root / "artifacts/local-ci").resolve()):
            raise ValueError("checkpoint log is outside local CI evidence")
        records.append({"path": str(path.absolute()), "sha256": file_digest(path)})
    return records


def seal(root: Path, report: dict, steps, inputs: dict, target: dict) -> dict:
    """Return an eligible checkpoint only for a sole pre-step capacity refusal."""
    if (not isinstance(report, dict) or not isinstance(report.get("start_tree"), dict)
            or not isinstance(target, dict) or not isinstance(report.get("disk_refusal"), dict)
            or type(report.get("minimum_free_bytes")) is not int):
        raise ValueError("checkpoint source, target or resource records are malformed")
    if (report.get("status") != "failed" or report.get("exit_code") != 2
            or report.get("stale") is not False or report.get("resume")
            or report.get("mode") not in {"full", "merge", "release"}
            or report.get("start_tree") != report.get("end_tree")
            or report["start_tree"].get("dirty") is not False
            or any(k.endswith("_error") and k != "disk_error" for k in report)
            or not report.get("disk_refusal") or not target or not target.get("target_identity")):
        raise ValueError("only clean, unchanged, sole disk-headroom refusals can continue")
    if report.get("minimum_free_bytes", 0) < 2 * 1024 ** 3:
        raise ValueError("continuation requires the unchanged operational disk floor")
    plan = step_plan(steps)
    count = passed_prefix(report, plan)
    if report["disk_refusal"]["before_step"] != count:
        raise ValueError("resource refusal does not match the passed prefix")
    return {"version": 1, "report_sha256": digest({k: v for k, v in report.items() if k != "checkpoint"}),
            "plan": plan, "execution_inputs": inputs,
            "target": target, "passed_count": count, "logs": log_records(root, report, count)}


def load(root: Path, path: Path, summary: dict, steps, inputs: dict) -> tuple[dict, dict]:
    path = Path(path).absolute()
    report_hash = file_digest(path)
    report = json.loads(path.read_bytes())
    if not isinstance(report, dict) or not isinstance(report.get("checkpoint"), dict):
        raise ValueError("resume report has no valid checkpoint")
    checkpoint = report["checkpoint"]
    expected = seal(root, report, steps, inputs, checkpoint["target"])
    if checkpoint != expected:
        raise ValueError("checkpoint inputs, plan or log evidence changed")
    for key in ("start_tree", "runtime_inputs", "plan", "mode", "repo_root", "python_version", "platform", "minimum_free_bytes"):
        if report.get(key) != summary.get(key):
            raise ValueError("checkpoint source/runtime/plan differs: " + key)
    return report, {"report": str(path), "sha256": report_hash,
                    "passed_count": checkpoint["passed_count"],
                    "logs": checkpoint["logs"], "reason": "exact-input pre-step disk refusal"}


def inherited_steps(report: dict, lineage: dict) -> list[dict]:
    rows = deepcopy(report["steps"][:lineage["passed_count"]])
    for row, log in zip(rows, lineage["logs"]):
        row.update(execution="inherited", inherited_from={"report": lineage["report"],
                   "sha256": lineage["sha256"], "log": log}, duration_seconds=None,
                   start_offset_seconds=None, timing_estimates=None, started_at=None, ended_at=None,
                   log=log["path"])
    return rows


def validate_inheritance(root: Path, report: dict) -> None:
    """Consumers verify every inherited row against its unchanged original record."""
    lineage = report.get("resume")
    inherited = [r for r in report["steps"] if r.get("execution") == "inherited"]
    if not lineage:
        if inherited:
            raise ValueError("inherited stages have no checkpoint lineage")
        return
    if not isinstance(lineage, dict):
        raise ValueError("checkpoint lineage is malformed")
    path = Path(lineage["report"])
    if file_digest(path) != lineage["sha256"]:
        raise ValueError("original checkpoint report changed")
    original = json.loads(path.read_bytes())
    if not isinstance(original, dict) or not isinstance(original.get("checkpoint"), dict):
        raise ValueError("original checkpoint is malformed")
    checkpoint = original["checkpoint"]
    # Re-establish eligibility as well as hashes. This consumer has the complete
    # recorded plan; the caller separately checks it against its required plan.
    plan = checkpoint["plan"]
    if (not isinstance(plan, list) or any(not isinstance(p, dict)
            or set(p) != {"step_id", "argv", "requires_tests", "uses_cargo_target"}
            or not isinstance(p["step_id"], str) or not isinstance(p["argv"], list)
            or not p["argv"] or any(not isinstance(a, str) for a in p["argv"])
            or type(p["requires_tests"]) is not bool or type(p["uses_cargo_target"]) is not bool
            for p in plan)):
        raise ValueError("original checkpoint plan is malformed")
    required = tuple(SimpleNamespace(**p) for p in plan)
    if checkpoint != seal(root, original, required, checkpoint["execution_inputs"], checkpoint["target"]):
        raise ValueError("original checkpoint is no longer eligible")
    if (lineage["passed_count"] != checkpoint["passed_count"]
            or lineage["logs"] != log_records(root, original, checkpoint["passed_count"])
            or lineage["logs"] != checkpoint["logs"]
            or report["steps"][:len(inherited)] != inherited_steps(original, lineage)
            or len(inherited) != lineage["passed_count"]):
        raise ValueError("inherited stage evidence differs from the original checkpoint")
    for key in ("start_tree", "runtime_inputs", "plan", "mode", "repo_root", "python_version", "platform", "minimum_free_bytes"):
        if report.get(key) != original.get(key):
            raise ValueError("inherited runtime inputs differ: " + key)
    if report.get("target_cache", {}).get("dependency_inputs") != checkpoint["target"]["dependency_inputs"]:
        raise ValueError("inherited guarded dependency inputs differ")
