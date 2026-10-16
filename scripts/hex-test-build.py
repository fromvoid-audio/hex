#!/usr/bin/env python3
"""Build, verify and safely activate the configured optimized native test app."""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import shutil
import signal
import subprocess
import sys
import tempfile
import uuid

ROOT = Path(__file__).resolve().parent.parent
IDENTITY = "org.portalsurfer.hex.dev"
SPEC = importlib.util.spec_from_file_location("hex_local_ci", ROOT / "scripts/local-ci.py")
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("local CI module unavailable")
ci = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = ci
SPEC.loader.exec_module(ci)


class DeliveryError(Exception):
    """Fail closed without changing projects, preferences or unrelated apps."""


def run(argv: list[str], root: Path = ROOT) -> str:
    return subprocess.run(argv, cwd=root, check=True, capture_output=True, text=True).stdout.strip()


def sha(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


@contextmanager
def optimized_artifact(runner, *, package: str = "hex-desktop", adopt: bool = False):
    """Build/identify only; shared by delivery and nonactivating review staging.

    The target lock spans compilation, source validation and the caller's copy.
    This helper never installs, activates or launches an application.
    """
    with runner.guarded_release(adopt=adopt) as ownership:
        argv = ["cargo", "build", "--release", "-p", package, "--locked", "--message-format=json"]
        log_path = runner.output_dir / "optimized-build.log"
        with log_path.open("w") as log:
            process = subprocess.run(argv, cwd=runner.root, env=runner.env,
                                     stdout=subprocess.PIPE, stderr=log, text=True, check=False)
            log.write(process.stdout)
        ownership.update(build_argv=argv, build_exit_code=process.returncode, build_log=str(log_path))
        if process.returncode != 0:
            raise DeliveryError(f"optimized build failed; retained log: {log_path}")
        events = [json.loads(line) for line in process.stdout.splitlines() if line.startswith("{")]
        binaries = [Path(event["executable"]) for event in events
                    if event.get("reason") == "compiler-artifact" and event.get("executable")
                    and event.get("target", {}).get("name") == package
                    and event.get("profile", {}).get("opt_level") == "3"]
        if len(binaries) != 1:
            raise DeliveryError("no unique optimized artifact from Cargo")
        ownership["artifact"] = runner.identify_release_artifact(binaries[0], argv)
        if (not runner._capture_repository_state("release_artifact_tree")
                or runner.summary["release_artifact_tree"] != runner.summary["start_tree"]):
            raise DeliveryError("source changed during optimized compilation")
        yield binaries[0], ownership


def write_json(path: Path, value: object) -> None:
    temporary = path.with_name(path.name + ".tmp")
    with temporary.open("w") as output:
        json.dump(value, output, indent=2, sort_keys=True)
        output.write("\n")
        output.flush()
        os.fsync(output.fileno())
    temporary.replace(path)
    sync_directory(path.parent)


def sync_directory(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def assert_plain_path(path: Path) -> None:
    for component in (path, *path.parents):
        if component.is_symlink():
            raise DeliveryError(f"refusing symlink in application path: {component}")


def assert_identity(app: Path) -> None:
    assert_plain_path(app)
    with (app / "Contents/Info.plist").open("rb") as source:
        info = plistlib.load(source)
    if info.get("CFBundleIdentifier") != IDENTITY or info.get("CFBundleExecutable") != "hex":
        raise DeliveryError(f"refusing unrelated application: {app}")
    if not (app / "Contents/MacOS/hex").is_file():
        raise DeliveryError(f"missing development executable: {app}")


def assert_inactive(app: Path) -> None:
    prefix = str(app.absolute()).casefold() + "/"
    for line in run(["ps", "-ww", "-axo", "pid=,comm="]).splitlines():
        fields = line.strip().split(None, 1)
        if len(fields) == 2 and str(Path(fields[1]).absolute()).casefold().startswith(prefix):
            raise DeliveryError(f"hex is running (pid {fields[0]}); save, quit and retry. "
                                "no application was force-quit; disk replacement cannot reload a process")


def inventory(app: Path) -> dict[str, str]:
    result = {}
    for path in sorted(app.rglob("*")):
        if path.is_symlink():
            raise DeliveryError(f"symlink in staged app: {path}")
        if path.is_file() and path.relative_to(app).as_posix() != "Contents/Resources/test-build.json":
            result[path.relative_to(app).as_posix()] = sha(path)
    return result


def verify_native_executable(binary: Path) -> None:
    # The local bundle is unsigned; validate Mach-O code, not a bundle seal.
    # verify() independently checks every packaged resource against the receipt.
    run(["codesign", "--verify", "--strict", "--ignore-resources", str(binary)])


def verify(app: Path) -> dict:
    assert_identity(app)
    receipt = json.loads((app / "Contents/Resources/test-build.json").read_text())
    if receipt.get("files") != inventory(app):
        raise DeliveryError(f"artifact checksum mismatch: {app}")
    if not os.access(app / "Contents/MacOS/hex", os.X_OK):
        raise DeliveryError("packaged binary is not executable")
    verify_native_executable(app / "Contents/MacOS/hex")
    return receipt


@contextmanager
def activation_lock(app: Path):
    lock_path = app.parent / ".hex-test-build.lock"
    assert_plain_path(lock_path)
    with lock_path.open("a+") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise DeliveryError("another test-build activation owns this location") from error
        yield


def recover(app: Path, journal: Path) -> None:
    if not journal.exists():
        return
    assert_plain_path(journal)
    transaction = json.loads(journal.read_text())
    if str(app).casefold() != str(transaction.get("destination", app)).casefold():
        raise DeliveryError("activation journal belongs to another destination")
    backup = Path(transaction["backup"])
    stage = Path(transaction["stage"])
    # Never act on arbitrary paths supplied by a malformed journal.
    for path, prefix in ((backup, ".hex-rollback-"), (stage, ".hex-stage-")):
        if path.parent != app.parent or not path.name.startswith(prefix):
            raise DeliveryError("invalid activation journal; preserve it for manual recovery")
        assert_plain_path(path)
    assert_inactive(app)
    assert_inactive(backup)
    if not app.exists() and backup.exists():
        assert_identity(backup)
        backup.rename(app)
    elif app.exists() and not stage.exists():
        # New candidate has been renamed. Complete only if fully verified.
        try:
            verify(app)
        except (DeliveryError, OSError, ValueError, subprocess.CalledProcessError):
            if not backup.exists():
                raise
            assert_identity(backup)
            app.rename(stage)
            backup.rename(app)
    elif app.exists() and stage.exists():
        assert_identity(app)  # interruption before old bundle was moved
    else:
        raise DeliveryError("interrupted first installation; retained stage requires manual inspection")
    journal.unlink()
    sync_directory(app.parent)


def activate(app: Path, stage: Path, backup: Path | None = None) -> Path | None:
    journal = app.parent / ".hex-test-build-transaction.json"
    if journal.exists():
        raise DeliveryError("unrecovered activation journal")
    verify(stage)
    for path in stage.rglob("*"):
        if path.is_file():
            with path.open("rb") as source:
                os.fsync(source.fileno())
    sync_directory(stage)
    assert_inactive(app)
    if app.exists():
        assert_identity(app)
    backup = backup or app.parent / (".hex-rollback-" + uuid.uuid4().hex + ".app")
    if backup.exists():
        raise DeliveryError("rollback destination already exists")
    write_json(journal, {"backup": str(backup), "stage": str(stage), "destination": str(app)})
    try:
        # Check again immediately before replacement; never terminate a process.
        assert_inactive(app)
        if app.exists():
            app.rename(backup)
            sync_directory(app.parent)
        stage.rename(app)
        sync_directory(app.parent)
        verify(app)
    except (OSError, DeliveryError, ValueError, subprocess.CalledProcessError):
        # Retain failed candidate for inspection, then restore the intact old app.
        if backup.exists():
            assert_inactive(app)
            assert_inactive(backup)
            if app.exists():
                app.rename(stage)
            backup.rename(app)
            sync_directory(app.parent)
        raise
    else:
        journal.unlink()
        sync_directory(app.parent)
    return backup if backup.exists() else None


def validate_ci(report: Path, runner, candidate: str) -> dict:
    evidence = json.loads(report.read_text())
    if not runner._capture_repository_state("start_tree"):
        raise DeliveryError("cannot identify current source")
    current = runner.summary["start_tree"]
    expected_steps = [(step.step_id, list(step.argv)) for step in ci.FULL_STEPS]
    actual_steps = [(step.get("step_id"), step.get("argv")) for step in evidence.get("steps", [])]
    if (evidence.get("mode") != "full" or evidence.get("status") != "passed"
            or evidence.get("stale") is not False or evidence.get("exit_code") != 0
            or actual_steps != expected_steps
            or any(step.get("status") != "passed" for step in evidence["steps"])
            or evidence.get("start_tree") != evidence.get("end_tree")
            or current["dirty"] or evidence.get("dirty") is not False
            or evidence.get("commit") != candidate
            or evidence["start_tree"].get("content_sha256") != current["content_sha256"]):
        raise DeliveryError("full exact-source local CI evidence is missing, failed, dirty or stale")
    head = current["commit"]
    if head != candidate:
        # A merge commit is eligible only with identical complete Git tree and ancestry.
        if run(["git", "rev-parse", f"{head}^{{tree}}"], runner.root) != run(
                ["git", "rev-parse", f"{candidate}^{{tree}}"], runner.root):
            raise DeliveryError("merged source differs from locally verified candidate")
        run(["git", "merge-base", "--is-ancestor", candidate, head], runner.root)
    return evidence


def build(args) -> dict:
    if sys.platform != "darwin":
        raise DeliveryError("native test delivery requires macOS")
    common = Path(run(["git", "rev-parse", "--git-common-dir"]))
    if not common.is_absolute():
        common = ROOT / common
    configuration = json.loads((ROOT / "scripts/hex-test-build.json").read_text())
    configured = Path(os.environ.get("HEX_DEV_APP_PATH", configuration["app_path"]))
    app = configured if configured.is_absolute() else common.resolve().parent / configured
    app = app.absolute()
    if app.name.casefold() != "hex.app":
        raise DeliveryError("stable development target must be named hex.app")
    assert_plain_path(app)
    app.parent.mkdir(parents=True, exist_ok=True)
    with activation_lock(app):
        recover(app, app.parent / ".hex-test-build-transaction.json")
        assert_inactive(app)
        if app.exists():
            assert_identity(app)
        runner = ci.LocalCiRunner(ROOT, adopt_target=False)
        validate_ci(args.ci_report, runner, args.candidate)
        included = json.loads(args.included.read_text())
        if not isinstance(included, list) or not included:
            raise DeliveryError("include ticket revisions and PR head revisions")
        for item in included:
            if (not isinstance(item, dict) or not isinstance(item.get("ticket"), int)
                    or not isinstance(item.get("ticket_revision"), int) or item["ticket_revision"] <= 0
                    or not isinstance(item.get("pr"), str)
                    or not isinstance(item.get("head"), str) or len(item["head"]) != 40
                    or any(char not in "0123456789abcdef" for char in item["head"])):
                raise DeliveryError("invalid included ticket/PR revision")
        checklist = args.checklist.read_text()
        if not checklist.strip():
            raise DeliveryError("finite user checklist required")
        runner.output_dir.mkdir(parents=True)
        if not runner._resolve_cargo_identity() or not runner._check_disk_headroom():
            raise DeliveryError(str(runner.summary))
        with optimized_artifact(runner, adopt=getattr(args, "adopt_release_target", False)) as (binary, release_ownership):
            metadata = json.loads(run(["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"]))
            version = next(package["version"] for package in metadata["packages"] if package["name"] == "hex-desktop")
            argv = release_ownership["build_argv"]
            validate_ci(args.ci_report, runner, args.candidate)
            stage = Path(tempfile.mkdtemp(prefix=".hex-stage-", suffix=".app", dir=app.parent))
            contents = stage / "Contents"
            (contents / "MacOS").mkdir(parents=True)
            (contents / "Resources/licenses").mkdir(parents=True)
            shutil.copy2(binary, contents / "MacOS/hex")
            for source, name in ((ROOT / "assets/fonts/ioskeley-mono/OFL.txt", "ioskeley-mono-OFL.txt"),
                                 (ROOT / "third_party/gpui/LICENSE-APACHE", "gpui-LICENSE-APACHE")):
                shutil.copy2(source, contents / "Resources/licenses" / name)
            source = runner.summary["start_tree"]["commit"]
            info = {"CFBundleExecutable": "hex", "CFBundleIdentifier": IDENTITY,
                    "CFBundleName": "hex", "CFBundleDisplayName": "hex", "CFBundlePackageType": "APPL",
                    "CFBundleShortVersionString": version, "CFBundleVersion": str(int(source[:8], 16)),
                    "NSHighResolutionCapable": True, "HexSourceRevision": source}
            with (contents / "Info.plist").open("wb") as output:
                plistlib.dump(info, output)
            (contents / "Resources/USER-TEST.md").write_text(checklist)
            shutil.copy2(args.ci_report, contents / "Resources/local-ci-summary.json")
            # Rust's native linker ad-hoc signs the executable. The bundle is local,
            # unsigned packaging; never mutate that binary after hashing it.
            verify_native_executable(contents / "MacOS/hex")
            backup = app.parent / (".hex-rollback-" + uuid.uuid4().hex + ".app")
            receipt = {"schema": 1, "version": version, "source": source, "candidate": args.candidate,
                       "source_sha256": runner.summary["start_tree"]["content_sha256"],
                       "profile": "release", "build_argv": argv, "target_directory": str(runner.cargo_identity["target_directory"]),
                       "release_target_ownership": {key: release_ownership[key] for key in
                           ("profile", "source", "packages", "target_directory", "owner_path",
                            "lock_path", "action", "cleanup_status", "artifact")},
                       "ci_report": str(args.ci_report.resolve()), "ci_report_sha256": sha(args.ci_report),
                       "included": included, "app_path": str(app), "created_at": ci.utc_now(),
                       "activation": "installed on disk; launch or relaunch required; not launched by delivery",
                       "signature": "executable ad-hoc; bundle unsigned",
                       "rollback_path": str(backup) if app.exists() else None, "files": inventory(stage)}
            write_json(contents / "Resources/test-build.json", receipt)
            verify(stage)
            verify_native_executable(contents / "MacOS/hex")
            validate_ci(args.ci_report, runner, args.candidate)
            backup = activate(app, stage, backup)
            verify_native_executable(app / "Contents/MacOS/hex")
            receipt["rollback_path"] = str(backup) if backup else None
            write_json(app.parent / "hex-test-build-receipt.json", receipt)
            return receipt


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ci-report", type=Path, required=True)
    parser.add_argument("--candidate", required=True, help="full locally verified PR head")
    parser.add_argument("--included", type=Path, required=True, help="JSON list of ticket/revision/pr/head objects")
    parser.add_argument("--checklist", type=Path, required=True)
    parser.add_argument("--adopt-release-target", action="store_true",
                        help="explicitly recover unknown/pending release ownership; selectively clean workspace release artifacts")
    args = parser.parse_args()
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    try:
        receipt = build(args)
        print(json.dumps(receipt, indent=2, sort_keys=True))
        return 0
    except (DeliveryError, ci.TargetCacheError, ci.InterruptedRun, OSError, ValueError,
            KeyError, TypeError, StopIteration, subprocess.CalledProcessError, KeyboardInterrupt) as error:
        print(f"test delivery incomplete: {error}; previous app/rollback and staged artifacts retained. "
              "save and quit hex before retrying; no process was terminated", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
