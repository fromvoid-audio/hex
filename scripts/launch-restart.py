"""Explicit launcher restart; no process action is used by ordinary launch."""
from __future__ import annotations

from dataclasses import asdict, dataclass
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile
import time


class RestartError(Exception):
    """Identity, exclusion or ownership could not be established; fail closed."""


@dataclass(frozen=True)
class Identity:
    pid: int
    started: str
    bundle: str
    executable: str
    identifier: str


def exact_identity(value: dict, app: Path) -> Identity:
    if (not isinstance(value, dict) or set(value) != set(Identity.__dataclass_fields__)
            or type(value["pid"]) is not int or value["pid"] <= 0
            or not isinstance(value["started"], str) or not value["started"]
            or value["bundle"] != str(app)
            or value["executable"] != str(app / "Contents/MacOS/hex")
            or value["identifier"] != "org.portalsurfer.hex.dev"):
        raise RestartError("refusing an ambiguous or unrelated development process")
    return Identity(**value)


class NativeController:
    def __init__(self, root: Path, app: Path, output: Path, delivery):
        self.root, self.app, self.output, self.delivery = root, app, output, delivery
        self.seal = self.disk_identity() if app.exists() else None

    def disk_identity(self):
        self.delivery.verify(self.app)
        executable = self.app / "Contents/MacOS/hex"
        info = executable.stat()
        return (info.st_dev, info.st_ino, self.delivery.sha(executable))

    def call(self, operation: str, expected: Identity | None = None):
        if operation != "snapshot" and (self.seal is None or self.disk_identity() != self.seal):
            raise RestartError("installed application changed; no termination requested")
        argv = ["/usr/bin/swift", "-module-cache-path", str(self.output / "swift-module-cache"),
                str(self.root / "scripts/launch-restart.swift"), operation, str(self.app)]
        if expected is not None:
            argv.append(json.dumps(asdict(expected), separators=(",", ":")))
        try:
            result = subprocess.run(argv, cwd=self.root, capture_output=True, text=True,
                                    check=False, timeout=15)
        except subprocess.TimeoutExpired as error:
            raise RestartError("native app control timed out; escalation stopped") from error
        with (self.output / "native-control.log").open("a") as log:
            log.write(f"{operation}: exit {result.returncode}\n{result.stdout}{result.stderr}")
        if result.returncode:
            raise RestartError(f"native app control refused; see {self.output / 'native-control.log'}")
        return json.loads(result.stdout)

    def snapshot(self) -> list[Identity]:
        values = self.call("snapshot")
        if not isinstance(values, list) or len(values) > 1:
            raise RestartError("multiple or ambiguous stable hex processes; close them manually")
        identities = [exact_identity(value, self.app) for value in values]
        if identities and self.seal is None:
            raise RestartError("running application has no verifiable installed bundle")
        return identities

    def request(self, operation: str, expected: Identity) -> bool:
        result = self.call(operation, expected)
        if type(result) is not bool:
            raise RestartError("invalid native app-control result")
        return result


def stop_exact(controller, *, normal_timeout=10.0, force_timeout=5.0,
               clock=time.monotonic, sleep=time.sleep, announce=print) -> None:
    """Quit one identity, escalate once, and never retarget a replacement."""
    initial = controller.snapshot()
    if not initial:
        return
    if len(initial) != 1:
        raise RestartError("multiple stable hex processes; close them manually")
    expected = initial[0]
    announce("--force-restart: requesting normal quit; unsaved work may be lost if force is needed", flush=True)

    def current():
        identities = controller.snapshot()
        if identities and identities != [expected]:
            raise RestartError("hex process identity changed; restart stopped without retargeting")
        return bool(identities)

    for operation, timeout in (("quit", normal_timeout), ("force", force_timeout)):
        if not current():
            return
        controller.request(operation, expected)
        deadline = clock() + timeout
        while current():
            remaining = deadline - clock()
            if remaining <= 0:
                break
            sleep(min(0.2, remaining))
        else:
            return
    raise RestartError("the exact hex process did not exit; no build, cleanup or launch performed")


def marker_bytes(path: Path) -> bytes:
    """Read only a bounded regular marker without following a link or FIFO."""
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as stream:
            if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
                raise RestartError("release owner is not a regular file")
            raw = stream.read(128 * 1024 + 1)
        if len(raw) > 128 * 1024:
            raise RestartError("release owner exceeds its bounded metadata size")
        return raw
    except OSError as error:
        raise RestartError("release owner cannot be read safely; preserved unchanged") from error


def hexadecimal(value, length):
    return isinstance(value, str) and len(value) == length and all(c in "0123456789abcdef" for c in value)


def recovery_hash(runner, ci, source: str, ancestor) -> str | None:
    target = Path(runner.cargo_identity["target_directory"])
    _, dev_owner = ci.target_state_paths(target, runner.target_state_dir)
    owner = dev_owner.with_name(dev_owner.name.replace(".owner.json", ".release-owner.json"))
    if not os.path.lexists(owner):
        if target.exists() and any(target.iterdir()):
            raise RestartError("release ownership is unknown; no implicit adoption or cleanup")
        return None
    raw = marker_bytes(owner)
    try:
        record = json.loads(raw)
        workspace = {k: runner.git_identity[k] for k in ("workspace_root", "git_common_dir")}
        previous = record["source"]
        packages = record["packages"]
        scope = runner._release_scope()
        if (record.get("schema_version") != 1 or record.get("profile") != "release"
                or record.get("state") not in {"pending", "ready"}
                or record.get("target_directory") != str(target)
                or record.get("target_identity") != runner._directory_identity(target)
                or record.get("workspace_owner") != workspace
                or set(previous) != {"commit", "dirty", "dirty_files", "content_sha256"}
                or previous["dirty"] is not False or previous["dirty_files"] != []
                or not hexadecimal(previous["commit"], 40)
                or not hexadecimal(previous["content_sha256"], 64)
                or not isinstance(packages, list) or not packages
                or any(not isinstance(p, dict) or set(p) != {"name", "version", "spec"}
                       or not all(isinstance(v, str) and v for v in p.values()) for p in packages)):
            raise RestartError("release ownership does not match this checkout/target; no implicit adoption")
        names = {p["name"] for p in packages}
        if packages != [p for p in scope if p["name"] in names]:
            raise RestartError("release package ownership changed; use inspected operator recovery")
        if not ancestor(previous["commit"], source):
            raise RestartError("release owner source is not known prior main; no implicit adoption")
        if record["state"] == "ready":
            artifact = record["artifact"]
            path = Path(artifact["path"])
            if (not hexadecimal(artifact["sha256"], 64) or not isinstance(record["build_argv"], list)
                    or not path.is_absolute() or not path.resolve().is_relative_to(target / "release")
                    or not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != artifact["sha256"]):
                raise RestartError("ready release artifact is inconsistent; preserved for inspection")
            return None
        return hashlib.sha256(raw).hexdigest()
    except (KeyError, TypeError, ValueError) as error:
        raise RestartError("malformed release ownership; no implicit operator adoption") from error


def prepare(root: Path, app: Path, source: str, ci, delivery, assert_no_raw,
            *, controller_factory=NativeController) -> str | None:
    """Inspect/quit/recover under existing exclusions; never write release ownership."""
    if "HEX_DEV_APP_PATH" in os.environ:
        raise RestartError("--force-restart requires the repository-configured stable app path")
    delivery.assert_plain_path(app)
    app.parent.mkdir(parents=True, exist_ok=True)
    runner = ci.LocalCiRunner(root, adopt_target=False)
    artifacts = root / "artifacts/launch"
    artifacts.mkdir(parents=True, exist_ok=True)
    output = Path(tempfile.mkdtemp(prefix="restart-", dir=artifacts))
    report = {"kind": "opt-in restart preflight; tests not run", "candidate": source,
              "status": "incomplete", "app": str(app)}
    try:
        with delivery.activation_lock(app):
            frozen = delivery.validate_source(runner, source)
            if not runner._resolve_cargo_identity() or not runner._check_disk_headroom():
                raise RestartError(f"release prerequisites failed before app quit: {runner.summary}")
            target = Path(runner.cargo_identity["target_directory"])
            lock_path, _ = ci.target_state_paths(target, runner.target_state_dir)
            lock = ci.CargoTargetLock(lock_path, {"operation": "force-restart-preflight",
                                                "workspace_root": str(root), "commit": source})
            lock.acquire()
            try:
                expected = recovery_hash(runner, ci, source, lambda old, new:
                    subprocess.run(["git", "merge-base", "--is-ancestor", old, new], cwd=root,
                                   env=runner.env, check=False, capture_output=True).returncode == 0)
                legacy = app.parent / ".hex-test-build-transaction.json"
                if os.path.lexists(legacy):
                    raise RestartError("legacy install journal requires inspection; preserved unchanged")
                workspace = delivery.install_workspace(app)
                if os.path.lexists(workspace):
                    delivery.check_install_workspace(workspace)
                assert_no_raw(root)
                controller = controller_factory(root, app, output, delivery)
                stop_exact(controller)
                delivery.assert_inactive(app)
                assert_no_raw(root)
                delivery.validate_source(runner, source, expected=frozen)
                delivery.recover(app)
                report.update(status="ready_for_build", expected_release_owner_sha256=expected,
                              source=frozen)
            finally:
                error = lock.release()
                if error:
                    raise RestartError(error)
        return expected
    except BaseException as error:
        report.update(status="interrupted" if isinstance(error, KeyboardInterrupt) else "refused",
                      error=str(error))
        raise
    finally:
        delivery.write_json(output / "restart-preflight.json", report)
