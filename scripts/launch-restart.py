"""Explicit launcher restart; no process action is used by ordinary launch."""
from __future__ import annotations

from dataclasses import asdict, dataclass
import hashlib
import json
import os
from pathlib import Path
import stat
import signal
import shutil
import subprocess
import tempfile
import time


SWIFTC = "/usr/bin/swiftc"
COMPILE_TIMEOUT = 120
OPERATION_TIMEOUT = 15


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
        self._helper_dir = None
        self._binary = None
        self._closed = False
        self._failed = False
        self._cleanup_blocked = False

    def disk_identity(self):
        self.delivery.verify(self.app)
        executable = self.app / "Contents/MacOS/hex"
        info = executable.stat()
        return (info.st_dev, info.st_ino, self.delivery.sha(executable))

    def _file_identity(self, path: Path, *, executable=False, owned=True):
        # Source and private executable paths cannot pass through a symlink.
        for component in (path, *path.parents):
            if component.is_symlink():
                raise RestartError("native helper path contains a symlink; no app control requested")
        try:
            descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
            try:
                if not stat.S_ISREG(os.fstat(descriptor).st_mode):
                    raise RestartError("native helper file identity is ambiguous; app control stopped")
                stream = os.fdopen(descriptor, "rb")
            except BaseException:
                os.close(descriptor)
                raise
            with stream:
                before = os.fstat(stream.fileno())
                if (not stat.S_ISREG(before.st_mode) or before.st_nlink != 1
                        or (owned and before.st_uid != os.getuid())
                        or before.st_mode & 0o022
                        or (executable and not before.st_mode & stat.S_IXUSR)
                        or before.st_size > 64 * 1024 * 1024):
                    raise RestartError("native helper file identity is ambiguous; app control stopped")
                digest = hashlib.sha256()
                while block := stream.read(64 * 1024):
                    digest.update(block)
                after = os.fstat(stream.fileno())
                def stamp(info):
                    return (info.st_dev, info.st_ino, info.st_size, info.st_mode,
                            info.st_mtime_ns, info.st_ctime_ns, info.st_uid, info.st_nlink)
                if stamp(before) != stamp(after) or stamp(path.lstat()) != stamp(after):
                    raise RestartError("native helper file changed while inspected; app control stopped")
                return (after.st_dev, after.st_ino, after.st_size, after.st_mode,
                        after.st_mtime_ns, after.st_ctime_ns, digest.hexdigest())
        except OSError as error:
            raise RestartError(f"native helper/toolchain unavailable: {path}; app control stopped") from error

    def _check_directory(self):
        if self._helper_dir is None:
            raise RestartError("native helper has no owned compilation workspace")
        try:
            info = self._helper_dir.lstat()
        except OSError as error:
            raise RestartError("native helper workspace is unavailable; cleanup refused") from error
        if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid()
                or stat.S_IMODE(info.st_mode) != 0o700
                or (info.st_dev, info.st_ino) != self._directory_identity):
            raise RestartError("native helper workspace identity changed; preserved without app control")

    def _run_owned(self, argv, timeout, phase):
        """Bound only this newly created session, never a discovered app/PID."""
        self._check_directory()
        started = time.monotonic()
        status = "not started"
        # Files avoid unbounded capture_output buffers and descendant pipe holds.
        with tempfile.TemporaryFile(dir=self._helper_dir) as stdout, \
                tempfile.TemporaryFile(dir=self._helper_dir) as stderr:
            process = None
            try:
                process = subprocess.Popen(argv, cwd=self.root, stdin=subprocess.DEVNULL,
                                           stdout=stdout, stderr=stderr, start_new_session=True)
                try:
                    code = process.wait(timeout=timeout)
                except BaseException:
                    # The group was created for this compiler/helper invocation;
                    # its unreaped leader cannot be reused as another process.
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    except OSError as error:
                        self._cleanup_blocked = True
                        status = "owned child cleanup refused; workspace retained"
                        raise RestartError(status) from error
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired as error:
                        self._cleanup_blocked = True
                        status = "owned child exit unconfirmed; workspace retained"
                        raise RestartError(status) from error
                    raise
                status = f"exit {code}"
                stdout.seek(0); stderr.seek(0)
                out, err = stdout.read(1024 * 1024 + 1), stderr.read(1024 * 1024 + 1)
                if len(out) > 1024 * 1024 or len(err) > 1024 * 1024:
                    raise RestartError("native helper output exceeds bounded diagnostics")
                return subprocess.CompletedProcess(argv, code, out.decode("utf-8"), err.decode("utf-8"))
            except subprocess.TimeoutExpired as error:
                status = "timed out; owned child stopped"
                raise RestartError(f"native {phase} timed out; escalation stopped") from error
            except OSError as error:
                status = "could not start"
                raise RestartError(f"native {phase} unavailable; escalation stopped") from error
            except KeyboardInterrupt:
                status = "interrupted; owned child stopped"
                raise
            finally:
                stdout.seek(0); stderr.seek(0)
                with (self.output / ("native-compile.log" if phase == "compile" else "native-control.log")).open("a") as log:
                    log.write(f"{phase}: {status}; child_pid {process.pid if process else 'none'}; "
                              f"budget {timeout}s; elapsed {time.monotonic()-started:.6f}s\n")
                    log.write(stdout.read(1024 * 1024).decode("utf-8", errors="replace"))
                    log.write(stderr.read(1024 * 1024).decode("utf-8", errors="replace"))

    def _validate_helper(self):
        self._check_directory()
        if (self._file_identity(self.root / "scripts/launch-restart.swift") != self._source_identity
                or self._file_identity(self._helper_dir / "launch-restart.swift") != self._copy_identity
                or self._file_identity(self._binary, executable=True) != self._binary_identity):
            raise RestartError("native helper source or executable changed; escalation stopped")

    def _prepare_helper(self):
        if self._closed or self._failed:
            raise RestartError("native helper is closed or previously failed; no automatic retry")
        if self._binary is not None:
            self._validate_helper()
            return self._binary
        self._failed = True  # No second compiler attempt after abnormal exit.
        source = self.root / "scripts/launch-restart.swift"
        self._source_identity = self._file_identity(source)
        compiler_identity = self._file_identity(Path(SWIFTC), executable=True, owned=False)
        self._helper_dir = Path(tempfile.mkdtemp(prefix="native-helper-", dir=self.output))
        info = self._helper_dir.lstat()
        self._directory_identity = (info.st_dev, info.st_ino)
        self._check_directory()
        snapshot = self._helper_dir / "launch-restart.swift"
        descriptor = os.open(source, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        try:
            original = os.fdopen(descriptor, "rb")
        except BaseException:
            os.close(descriptor)
            raise
        with original, snapshot.open("xb") as stream:
            raw = original.read(128 * 1024 + 1)
            if len(raw) > 128 * 1024:
                raise RestartError("native helper source exceeds its bounded size")
            stream.write(raw)
        snapshot.chmod(0o600)
        self._copy_identity = self._file_identity(snapshot)
        if self._copy_identity[-1] != self._source_identity[-1]:
            raise RestartError("native helper source changed before compile; app control stopped")
        self._binary = self._helper_dir / "launch-restart"
        argv = [SWIFTC, "-module-cache-path", str(self._helper_dir / "modules"),
                str(snapshot), "-o", str(self._binary)]
        attempt = {"source": str(source), "source_identity": self._source_identity,
                   "source_sha256": self._source_identity[-1], "source_copy": str(snapshot),
                   "compiler": SWIFTC, "compiler_identity": compiler_identity,
                   "compile_argv": argv, "compile_timeout_seconds": COMPILE_TIMEOUT,
                   "operation_timeout_seconds": OPERATION_TIMEOUT}
        # Diagnostic only, never read as authority or used for binary adoption.
        with (self.output / "native-compile.json").open("x") as stream:
            json.dump(attempt, stream, sort_keys=True, indent=2)
            stream.write("\n")
        result = self._run_owned(argv, COMPILE_TIMEOUT, "compile")
        if result.returncode:
            raise RestartError(f"native helper compile refused; see {self.output / 'native-compile.log'}")
        self._binary_identity = self._file_identity(self._binary, executable=True)
        if self._file_identity(Path(SWIFTC), executable=True, owned=False) != compiler_identity:
            raise RestartError("native compiler changed during compilation; app control stopped")
        self._validate_helper()
        provenance = {"source": str(source), "source_identity": self._source_identity,
                      "source_sha256": self._source_identity[-1], "source_copy": str(snapshot),
                      "binary": str(self._binary), "binary_identity": self._binary_identity,
                      "binary_sha256": self._binary_identity[-1], "compiler": SWIFTC,
                      "compiler_identity": compiler_identity, "compile_argv": argv,
                      "compile_timeout_seconds": COMPILE_TIMEOUT,
                      "operation_timeout_seconds": OPERATION_TIMEOUT}
        with (self.output / "native-helper.json").open("x") as stream:
            json.dump(provenance, stream, sort_keys=True, indent=2)
            stream.write("\n")
        self._failed = False
        return self._binary

    def close(self):
        self._closed = True
        if self._cleanup_blocked:
            raise RestartError("owned native child exit is unconfirmed; workspace retained")
        if self._helper_dir is not None:
            self._check_directory()
            shutil.rmtree(self._helper_dir)
            self._helper_dir = None

    def call(self, operation: str, expected: Identity | None = None):
        try:
            if operation != "snapshot" and (self.seal is None or self.disk_identity() != self.seal):
                raise RestartError("installed application changed; no termination requested")
            binary = self._prepare_helper()
            # Compilation may be cold/slow. Repeat the installed seal check at
            # the actual operation boundary, before even invoking the helper.
            if operation != "snapshot" and self.disk_identity() != self.seal:
                raise RestartError("installed application changed; no termination requested")
            argv = [str(binary), operation, str(self.app)]
            if expected is not None:
                argv.append(json.dumps(asdict(expected), separators=(",", ":")))
            result = self._run_owned(argv, OPERATION_TIMEOUT, operation)
            self._validate_helper()
            if result.returncode:
                raise RestartError(f"native app control refused; see {self.output / 'native-control.log'}")
            try:
                return json.loads(result.stdout)
            except ValueError as error:
                raise RestartError("invalid native app-control result; escalation stopped") from error
        except BaseException:
            self._failed = True
            self.close()
            raise

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
            if not isinstance(artifact, dict) or set(artifact) != {"path", "sha256"}:
                raise RestartError("ready release artifact is inconsistent; preserved for inspection")
            try:
                path = Path(artifact["path"])
                if not path.is_absolute() or not path.resolve().is_relative_to(target / "release"):
                    raise RestartError("ready release artifact is outside release; preserved for inspection")
                actual = runner.identify_release_artifact(path, record["build_argv"])
            except (ci.TargetCacheError, OSError, TypeError, ValueError) as error:
                raise RestartError("ready release artifact is inconsistent; preserved for inspection") from error
            if actual != artifact:
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
            runner.target_lock = lock
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
                try:
                    stop_exact(controller)
                finally:
                    close = getattr(controller, "close", None)
                    if close is not None:
                        close()
                delivery.assert_inactive(app)
                assert_no_raw(root)
                delivery.validate_source(runner, source, expected=frozen)
                delivery.recover(app)
                report.update(status="ready_for_build", expected_release_owner_sha256=expected,
                              source=frozen)
            finally:
                error = lock.release()
                runner.target_lock = None
                if error:
                    raise RestartError(error)
        return expected
    except BaseException as error:
        report.update(status="interrupted" if isinstance(error, KeyboardInterrupt) else "refused",
                      error=str(error))
        raise
    finally:
        delivery.write_json(output / "restart-preflight.json", report)
