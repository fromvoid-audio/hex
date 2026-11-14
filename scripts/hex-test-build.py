#!/usr/bin/env python3
"""Build, verify and safely activate the configured optimized native test app.

Direct execution and the release and launch loaders bind the delivery script's
compiled bytes to candidate source. Other Python importers lack this witness.
Source checks detect drift at qualification and publication boundaries. Keep the
candidate checkout stable throughout delivery.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import errno
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import signal
import stat
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
IDENTITY = "org.portalsurfer.hex.dev"


def _read_regular_source(path: Path, owner: str) -> bytes:
    directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        descriptor = os.open(path.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,
                             dir_fd=directory)
        try:
            if not stat.S_ISREG(os.fstat(descriptor).st_mode):
                raise RuntimeError(f"{owner} source is not a regular file")
            with os.fdopen(descriptor, "rb", closefd=False) as stream:
                return stream.read()
        finally:
            os.close(descriptor)
    finally:
        os.close(directory)


# Python has already parsed the direct entry script. Before loading any build
# tooling, execute the delivery implementation from the same buffer we hash.
if __name__ == "__main__" and "_EXECUTED_DELIVERY_SOURCE_SHA256" not in globals():
    _direct_path = ROOT / "scripts/hex-test-build.py"
    _direct_source = _read_regular_source(_direct_path, "delivery")
    _EXECUTED_DELIVERY_SOURCE_SHA256 = hashlib.sha256(_direct_source).hexdigest()
    exec(compile(_direct_source, str(_direct_path), "exec", dont_inherit=True), globals())
    raise SystemExit("delivery entry point did not exit")


# The sealed update helper imports this same verifier/publication owner under a
# fixed runtime-only name. Quit-time publication must not load the CI runner,
# changelog tooling, or a source checkout. Normal build callers retain them.
if __name__ != "hex_update_runtime":
    SPEC = importlib.util.spec_from_file_location("hex_local_ci", ROOT / "scripts/local-ci.py")
    if SPEC is None or SPEC.loader is None:
        raise RuntimeError("local CI module unavailable")
    ci = importlib.util.module_from_spec(SPEC)
    sys.modules[SPEC.name] = ci
    # Bind execution to a regular source file, without following a symlink or
    # blocking on a substituted FIFO, and do not trust timestamp-valid pyc.
    source = _read_regular_source(Path(SPEC.origin), "local CI")
    ci._DELIVERY_RUNNER_SOURCE_SHA256 = hashlib.sha256(source).hexdigest()
    exec(compile(source, SPEC.origin, "exec", dont_inherit=True), ci.__dict__)

    CHANGELOG_SPEC = importlib.util.spec_from_file_location("hex_changelog", ROOT / "scripts/changelog.py")
    if CHANGELOG_SPEC is None or CHANGELOG_SPEC.loader is None:
        raise RuntimeError("changelog validator unavailable")
    changelog = importlib.util.module_from_spec(CHANGELOG_SPEC)
    CHANGELOG_SPEC.loader.exec_module(changelog)


class DeliveryError(Exception):
    """Fail closed without changing projects, preferences or unrelated apps."""


class PublicationUncertain(DeliveryError):
    """The target may have changed; inspect or recover before retrying."""


class PublicationFollowupFailed(DeliveryError):
    """The new target verified, but its post-publication reporting failed."""


def require_delivery_source(root: Path) -> None:
    """Require the candidate's delivery script to match the bytes we executed."""
    witness = globals().get("_EXECUTED_DELIVERY_SOURCE_SHA256")
    if not isinstance(witness, str) or re.fullmatch(r"[0-9a-f]{64}", witness) is None:
        raise DeliveryError("executed delivery source witness is unavailable")
    try:
        source = _read_regular_source(root / "scripts/hex-test-build.py", "delivery")
    except (OSError, RuntimeError) as error:
        raise DeliveryError(f"candidate delivery source is unavailable: {error}") from error
    if hashlib.sha256(source).hexdigest() != witness:
        raise DeliveryError("delivery source mismatch: executed delivery differs from candidate source")


def run(argv: list[str], root: Path = ROOT, *, env: dict[str, str] | None = None) -> str:
    bounded = {"timeout": 30} if __name__ == "hex_update_runtime" else {}
    if env is not None:
        bounded["env"] = env
    return subprocess.run(argv, cwd=root, check=True, capture_output=True, text=True, **bounded).stdout.strip()


def sha(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


@contextmanager
def optimized_artifact(runner, *, package: str = "hex-desktop", adopt: bool = False,
                       expected_owner_sha256: str | None = None,
                       expected_recreated_target_identity: dict[str, int] | None = None,
                       recovery_inactive_check=None, verification_check=None, discovery=False,
                       reservation=None, on_admission=None):
    """Build/identify only; shared by delivery and nonactivating review staging.

    The target lock spans compilation, source validation and the caller's copy.
    This helper never installs, activates or launches an application.
    """
    recovery = {"adopt": adopt}
    if reservation is not None:
        recovery["reservation"] = reservation
    argv = ["cargo", "build", "--release", "-p", package, "--locked", "--message-format=json"]
    if discovery:
        if package != "hex-desktop":
            raise DeliveryError("discovery tools require the desktop build")
        argv[5:5] = ["-p", "hex-plugin-scan", "--bin", "hex-desktop", "--bin", "hex-vst3-scan-macos"]
    recovery["requested_build_argv"] = argv
    if verification_check is not None:
        recovery["verification_check"] = verification_check
    if expected_owner_sha256 is not None:
        recovery["expected_owner_sha256"] = expected_owner_sha256
    if expected_recreated_target_identity is not None:
        recovery.update(expected_recreated_target_identity=expected_recreated_target_identity,
                        recovery_inactive_check=recovery_inactive_check)
    with runner.guarded_release(**recovery) as ownership:
        if on_admission is not None:
            on_admission(ownership)
        log_path = runner.output_dir / "optimized-build.log"
        with log_path.open("w") as log, \
                ci.terminal_progress("building optimized hex"):
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
        if discovery:
            scanners = [Path(event["executable"]) for event in events
                        if event.get("reason") == "compiler-artifact" and event.get("executable")
                        and event.get("target", {}).get("name") == "hex-vst3-scan-macos"
                        and event.get("profile", {}).get("opt_level") == "3"]
            if len(scanners) != 1:
                raise DeliveryError("no unique optimized discovery scanner from Cargo")
            ownership["discovery_scanner"] = runner.identify_release_artifact(scanners[0], argv)
        runner.validate_release_inputs(ownership)
        if verification_check is not None:
            verification_check()
        if (not runner._capture_repository_state("release_artifact_tree")
                or runner.summary["release_artifact_tree"] != runner.summary["start_tree"]):
            raise DeliveryError("source changed during optimized compilation")
        yield binaries[0], ownership


DISCOVERY_ROOT = Path("Contents/Helpers")
DISCOVERY_RECEIPT = Path("Contents/Resources/vst3-discovery.json")
DISCOVERY_APP = "hex-vst3-host.app"
DISCOVERY_SCANNER = "hex-vst3-scan-macos"
INSTANCE_RECEIPT = Path("Contents/Resources/vst3-instance.json")
INSTANCE_APP = "hex-vst3-instance.app"


def discovery_builder():
    native = ROOT / "crates/hex-plugin-scan/native"
    sys.path.insert(0, str(native))
    try:
        spec = importlib.util.spec_from_file_location("hex_discovery_builder", native / "macos_bootstrap.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module
    finally:
        sys.path.pop(0)


def validate_discovery_input(path: Path, candidate: str, *, packaged=False, provenance=None) -> dict:
    assert_plain_path(path.absolute())
    kwargs = {} if provenance is None else {"expected_source_digest": provenance[0], "expected_sdk_pins": provenance[1]}
    data = discovery_builder().verify(path, **kwargs)
    if (data.get("scope") != "native-macos-factory-discovery"
            or data.get("source_commit") != candidate or data.get("source_dirty") is not False
            or data.get("package_layout") != ("hex-app-discovery-v1" if packaged else None)):
        raise DeliveryError("discovery helper must be minimal and built from the exact clean candidate")
    return data


def prepare_discovery_input(args, output_dir: Path) -> Path | None:
    """Resolve or build the exact native input before staging the app."""
    receipt = args.vst3_discovery_receipt
    if args.without_vst3_discovery:
        return None
    if receipt is None:
        sdk = args.vst3_sdk or os.environ.get("HEX_VST3_SDK")
        if not sdk:
            raise DeliveryError(
                "set HEX_VST3_SDK or --vst3-sdk to the pinned clean VST3 SDK checkout, "
                "supply --vst3-discovery-receipt, or explicitly use --without-vst3-discovery"
            )
        receipt = discovery_builder().build(
            Path(sdk), output_dir / "vst3-discovery", discovery_only=True
        )
    validate_discovery_input(receipt, args.candidate)
    return receipt


def sign_discovery(path: Path, identifier: str, signing: dict) -> None:
    run(["codesign", "--force", "--sign", signing["certificate_sha1"], "--identifier", identifier,
         "--timestamp=none", str(path)])
    run(["codesign", "--verify", "--strict", str(path)])
    actual = native_signature(path)
    if actual["identifier"] != identifier or actual["team_identifier"] != signing["team_identifier"]:
        raise DeliveryError("discovery tool signer/identifier differs from configuration")


def stage_discovery(stage: Path, source: Path, candidate: str, scanner: dict, signing: dict) -> dict:
    data = validate_discovery_input(source, candidate)
    scanner_path = Path(scanner["path"])
    assert_plain_path(scanner_path)
    if sha(scanner_path) != scanner["sha256"]:
        raise DeliveryError("optimized discovery scanner changed before staging")
    builder = discovery_builder()
    builder.macho(scanner_path, 2)
    destination = stage / DISCOVERY_ROOT
    destination.mkdir(parents=True, exist_ok=True)
    shutil.copytree(source.parent / DISCOVERY_APP, destination / DISCOVERY_APP)
    shutil.copy2(scanner_path, destination / DISCOVERY_SCANNER)
    if (builder.inventory(destination / DISCOVERY_APP) != data["bundle_files"]
            or sha(destination / DISCOVERY_SCANNER) != scanner["sha256"]):
        raise DeliveryError("discovery inputs changed during staging")
    if validate_discovery_input(source, candidate) != data:
        raise DeliveryError("discovery native receipt changed during staging")
    input_receipt_sha256 = sha(source)
    sign_discovery(destination / DISCOVERY_APP, "org.portalsurfer.hex.vst3.discovery", signing)
    sign_discovery(destination / DISCOVERY_SCANNER, "org.portalsurfer.hex.vst3.scan", signing)
    data["bundle_files"] = builder.inventory(destination / DISCOVERY_APP)
    data["signing"] = {"mode": "Developer ID local discovery; not notarized; no added entitlements", **signing}
    data["package_layout"] = "hex-app-discovery-v1"
    packaged_receipt = stage / DISCOVERY_RECEIPT
    write_json(packaged_receipt, data)
    builder.verify(packaged_receipt)
    # Re-signing is explicit: preserve input provenance and identify final bytes.
    return {"source": candidate, "input_receipt_sha256": input_receipt_sha256, "input_scanner": scanner,
            "root": str(DISCOVERY_ROOT), "scanner_sha256": sha(destination / DISCOVERY_SCANNER),
            "receipt_sha256": sha(packaged_receipt), "signing": signing}


def verify_discovery_package(app: Path, receipt: dict) -> None:
    evidence = receipt["vst3_discovery"]
    if evidence.get("source") != receipt.get("source") or evidence.get("root") != str(DISCOVERY_ROOT):
        raise DeliveryError("discovery package source/location mismatch")
    root = app / DISCOVERY_ROOT
    packaged_receipt = app / DISCOVERY_RECEIPT
    provenance = candidate_native_provenance(app, receipt) if __name__ == "hex_update_runtime" else None
    data = validate_discovery_input(packaged_receipt, receipt["source"], packaged=True, provenance=provenance)
    if sha(packaged_receipt) != evidence.get("receipt_sha256") or sha(root / DISCOVERY_SCANNER) != evidence.get("scanner_sha256"):
        raise DeliveryError("discovery package changed")
    discovery_builder().macho(root / DISCOVERY_SCANNER, 2)
    for path, identifier in [(root / DISCOVERY_APP, "org.portalsurfer.hex.vst3.discovery"),
                             (root / DISCOVERY_SCANNER, "org.portalsurfer.hex.vst3.scan")]:
        run(["codesign", "--verify", "--strict", str(path)])
        actual = native_signature(path)
        if actual["identifier"] != identifier or actual["team_identifier"] != receipt["signing"]["team_identifier"]:
            raise DeliveryError("discovery package signer/identifier mismatch")
    if data.get("signing", {}).get("certificate_sha1") != receipt["signing"]["certificate_sha1"]:
        raise DeliveryError("discovery native receipt signer mismatch")


def validate_instance_input(path: Path, candidate: str, *, packaged=False, provenance=None) -> dict:
    assert_plain_path(path.absolute())
    kwargs = {} if provenance is None else {"expected_source_digest": provenance[0], "expected_sdk_pins": provenance[1]}
    data = discovery_builder().verify(path, **kwargs)
    if (data.get("scope") != "native-macos-instance"
            or data.get("source_commit") != candidate or data.get("source_dirty") is not False
            or data.get("package_layout") != ("hex-app-instance-v1" if packaged else None)):
        raise DeliveryError("instance helper must be minimal and built from the exact clean candidate")
    return data


def stage_instance(stage: Path, source: Path, candidate: str, signing: dict) -> dict:
    data = validate_instance_input(source, candidate)
    builder = discovery_builder()
    destination = stage / DISCOVERY_ROOT / INSTANCE_APP
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copytree(source.parent / INSTANCE_APP, destination)
    if (builder.inventory(destination) != data["bundle_files"]
            or validate_instance_input(source, candidate) != data):
        raise DeliveryError("instance inputs changed during staging")
    input_receipt_sha256 = sha(source)
    sign_discovery(destination, "org.portalsurfer.hex.vst3.instance", signing)
    data["bundle_files"] = builder.inventory(destination)
    data["signing"] = {"mode": "Developer ID local instance; not notarized; no added entitlements", **signing}
    data["package_layout"] = "hex-app-instance-v1"
    packaged_receipt = stage / INSTANCE_RECEIPT
    write_json(packaged_receipt, data)
    builder.verify(packaged_receipt)
    return {"source": candidate, "input_receipt_sha256": input_receipt_sha256,
            "root": str(DISCOVERY_ROOT), "receipt_sha256": sha(packaged_receipt), "signing": signing}


def verify_instance_package(app: Path, receipt: dict) -> None:
    evidence = receipt["vst3_instance"]
    if evidence.get("source") != receipt.get("source") or evidence.get("root") != str(DISCOVERY_ROOT):
        raise DeliveryError("instance package source/location mismatch")
    packaged_receipt = app / INSTANCE_RECEIPT
    provenance = candidate_native_provenance(app, receipt) if __name__ == "hex_update_runtime" else None
    data = validate_instance_input(packaged_receipt, receipt["source"], packaged=True, provenance=provenance)
    if sha(packaged_receipt) != evidence.get("receipt_sha256"):
        raise DeliveryError("instance package changed")
    helper = app / DISCOVERY_ROOT / INSTANCE_APP
    run(["codesign", "--verify", "--strict", str(helper)])
    actual = native_signature(helper)
    if (actual["identifier"] != "org.portalsurfer.hex.vst3.instance"
            or actual["team_identifier"] != receipt["signing"]["team_identifier"]
            or data.get("signing", {}).get("certificate_sha1") != receipt["signing"]["certificate_sha1"]):
        raise DeliveryError("instance package signer mismatch")


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


def assert_recovery_inactive(app: Path) -> None:
    """Never quit; recheck both configured app and raw desktops before mutation."""
    assert_inactive(app)
    for line in run(["ps", "-ww", "-axo", "pid=,comm="]).splitlines():
        fields = line.strip().split(None, 1)
        if len(fields) == 2 and Path(fields[1].rstrip("/")).name.casefold() == "hex-desktop":
            raise DeliveryError(f"raw hex-desktop is running (pid {fields[0]}); save, quit and retry; "
                                "no process was terminated")


def inventory(app: Path, *, signed_payload: bool = False, include_receipt: bool = False) -> dict[str, str]:
    result = {}
    for path in sorted(app.rglob("*")):
        if path.is_symlink():
            raise DeliveryError(f"symlink in staged app: {path}")
        relative = path.relative_to(app).as_posix()
        if not include_receipt and relative == "Contents/Resources/test-build.json":
            continue
        # Signing changes native code and creates the resource seal. The seal
        # itself authenticates those files and the pre-signing bundled receipt.
        if signed_payload and (relative == "Contents/MacOS/hex" or relative.startswith("Contents/_CodeSignature/")):
            continue
        if path.is_file():
            result[relative] = sha(path)
    return result


def signing_configuration(configuration: dict) -> dict[str, str]:
    signing = configuration.get("signing")
    if (not isinstance(signing, dict) or set(signing) != {"certificate_sha1", "team_identifier"}
            or not isinstance(signing.get("certificate_sha1"), str)
            or re.fullmatch(r"[0-9A-F]{40}", signing["certificate_sha1"]) is None
            or not isinstance(signing.get("team_identifier"), str)
            or re.fullmatch(r"[A-Z0-9]{10}", signing["team_identifier"]) is None):
        raise DeliveryError("configure an explicit Developer ID certificate SHA1 and team in scripts/hex-test-build.json")
    return dict(signing)


def native_signature(binary: Path) -> dict[str, str | None]:
    bounded = {"timeout": 30} if __name__ == "hex_update_runtime" else {}
    result = subprocess.run(["codesign", "--display", "--verbose=4", "--requirements", "-", str(binary)],
                            check=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, **bounded)
    lines = result.stdout.splitlines()
    fields = dict(line.split("=", 1) for line in lines if "=" in line and not line.startswith("designated =>"))
    return {"identifier": fields.get("Identifier"), "team_identifier": fields.get("TeamIdentifier"),
            "designated_requirement": next((line for line in lines if line.startswith("designated =>")), None)}


def verify_signed_bundle(app: Path, signing: dict[str, str], actual: dict) -> None:
    signing = signing_configuration({"signing": signing})
    requirement = (f'identifier "{IDENTITY}" and anchor apple generic and '
                   f'certificate leaf[subject.OU] = "{signing["team_identifier"]}" and '
                   'certificate leaf[field.1.2.840.113635.100.6.1.13] exists and '
                   f'certificate leaf = H"{signing["certificate_sha1"]}"')
    run(["codesign", "--verify", "--strict", "--test-requirement", "=" + requirement, str(app)])
    designated = actual.get("designated_requirement") or ""
    if (actual.get("identifier") != IDENTITY or actual.get("team_identifier") != signing["team_identifier"]
            or "cdhash" in designated.lower() or f'identifier "{IDENTITY}"' not in designated
            or signing["team_identifier"] not in designated):
        raise DeliveryError("signed app has no stable configured identifier/team designated requirement")


UPDATE_HELPER_ID = "org.portalsurfer.hex.update"
UPDATE_HELPER = Path("Contents/Helpers/hex-update.app")
UPDATE_RUNTIME_SCRIPTS = ("hex-test-build.py", "pending_update_exit.py", "hex-test-build.json")


def _python_runtime_identity() -> dict:
    """Pin the already-running local interpreter; never invoke a tool installer."""
    process = subprocess.run(["/bin/ps", "-ww", "-p", str(os.getpid()), "-o", "comm="],
                             capture_output=True, text=True, check=True, timeout=10)
    lines = process.stdout.splitlines()
    if len(lines) != 1:
        raise DeliveryError("cannot identify local Python interpreter")
    path = Path(lines[0].strip())
    if not path.is_absolute():
        raise DeliveryError("local Python interpreter path is ambiguous")
    assert_plain_path(path)
    metadata = path.stat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > 128 * 1024 * 1024:
        raise DeliveryError("local Python interpreter is not a supported executable")
    digest = sha(path)
    after = path.stat()
    if (metadata.st_dev, metadata.st_ino, metadata.st_size, metadata.st_mtime_ns) != (
            after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns):
        raise DeliveryError("local Python interpreter changed while inspected")
    return {"version": 1, "interpreter": str(path), "interpreter_sha256": digest,
            "interpreter_device": metadata.st_dev, "interpreter_inode": metadata.st_ino}


def stage_update_supervisor(stage: Path, signing: dict) -> dict:
    """Create the fixed-purpose signed helper as a sealed nested app."""
    helper = stage / UPDATE_HELPER
    contents = helper / "Contents"
    executable = contents / "MacOS/hex-update"
    resources = contents / "Resources"
    executable.parent.mkdir(parents=True)
    scripts = resources / "runtime/scripts"
    scripts.mkdir(parents=True)
    for name in UPDATE_RUNTIME_SCRIPTS:
        shutil.copy2(ROOT / "scripts" / name, scripts / name)
    shutil.copy2(ROOT / "scripts/hex-update-entry.py", resources / "entry.py")
    # The existing verifier validates optional discovery/instance receipts using
    # this exact original source closure; keep it inside the signed helper.
    native = discovery_builder()
    native_destination = resources / "runtime/crates/hex-plugin-scan/native"
    native_destination.mkdir(parents=True)
    for name in native.SOURCES:
        shutil.copy2(ROOT / "crates/hex-plugin-scan/native" / name, native_destination / name)
    write_json(resources / "policy.json", _python_runtime_identity())
    with (contents / "Info.plist").open("wb") as output:
        plistlib.dump({"CFBundleExecutable": "hex-update", "CFBundleIdentifier": UPDATE_HELPER_ID,
                       "CFBundlePackageType": "APPL", "CFBundleVersion": "1", "LSBackgroundOnly": True}, output)
    subprocess.run(["/usr/bin/swiftc", "-O", str(ROOT / "scripts/hex-update-launcher.swift"),
                    "-o", str(executable)], check=True, capture_output=True, timeout=120)
    subprocess.run(["/usr/bin/codesign", "--force", "--sign", signing["certificate_sha1"],
                    "--identifier", UPDATE_HELPER_ID, "--timestamp=none", str(helper)],
                   check=True, capture_output=True, timeout=120)
    evidence = {"identifier": UPDATE_HELPER_ID, "executable_sha256": sha(executable),
                "native_sources": list(native.SOURCES), "native_sdk_pins": native.PINS}
    verify_update_supervisor(stage, {"update_supervisor": evidence, "signing": signing})
    return evidence


def candidate_native_provenance(app: Path, receipt: dict):
    """Hash the candidate's signed source closure without executing its code."""
    evidence = receipt.get("update_supervisor")
    if not isinstance(evidence, dict):
        raise DeliveryError("candidate native provenance unavailable")
    names = evidence.get("native_sources")
    pins = evidence.get("native_sdk_pins")
    if (not isinstance(names, list) or not 0 < len(names) <= 256
            or not isinstance(pins, dict) or not pins
            or any(not isinstance(name, str) or re.fullmatch(r"[A-Za-z0-9_.-]+", name) is None for name in names)
            or len(set(names)) != len(names)):
        raise DeliveryError("candidate native provenance is invalid")
    root = app / UPDATE_HELPER / "Contents/Resources/runtime/crates/hex-plugin-scan/native"
    digest = hashlib.sha256()
    total = 0
    for name in names:
        path = root / name
        assert_plain_path(path)
        info = path.stat()
        if not stat.S_ISREG(info.st_mode) or info.st_size > 16 * 1024 * 1024:
            raise DeliveryError("candidate native source is invalid")
        total += info.st_size
        if total > 64 * 1024 * 1024:
            raise DeliveryError("candidate native sources are too large")
        with path.open("rb") as source:
            content = source.read(info.st_size + 1)
        if len(content) != info.st_size:
            raise DeliveryError("candidate native source changed")
        digest.update(name.encode() + b"\0" + content + b"\0")
    return digest.hexdigest(), pins


def verify_update_supervisor(app: Path, receipt: dict) -> None:
    evidence = receipt.get("update_supervisor")
    if not isinstance(evidence, dict) or evidence.get("identifier") != UPDATE_HELPER_ID:
        raise DeliveryError("invalid update helper receipt")
    helper = app / UPDATE_HELPER
    executable = helper / "Contents/MacOS/hex-update"
    assert_plain_path(executable)
    if sha(executable) != evidence.get("executable_sha256"):
        raise DeliveryError("update helper executable changed")
    signing = signing_configuration({"signing": receipt.get("signing")})
    requirement = (f'identifier "{UPDATE_HELPER_ID}" and anchor apple generic and '
                   f'certificate leaf[subject.OU] = "{signing["team_identifier"]}" and '
                   'certificate leaf[field.1.2.840.113635.100.6.1.13] exists and '
                   f'certificate leaf = H"{signing["certificate_sha1"]}"')
    subprocess.run(["/usr/bin/codesign", "--verify", "--strict", "--test-requirement",
                    "=" + requirement, str(helper)], check=True, capture_output=True, timeout=15)
    actual = native_signature(executable)
    if actual.get("identifier") != UPDATE_HELPER_ID or actual.get("team_identifier") != signing["team_identifier"]:
        raise DeliveryError("update helper signer or identifier differs")


def sign_bundle(app: Path, signing: dict[str, str]) -> None:
    # No identity search or ad-hoc fallback: keychain/signing failures abort
    # before publication. Local test signing does not request notarization.
    run(["codesign", "--force", "--sign", signing["certificate_sha1"], "--identifier", IDENTITY,
         "--timestamp=none", str(app)])


def verify_native_executable(binary: Path) -> None:
    # Covers legacy linker-signed code and the input/output native executable;
    # schema-2 verification additionally requires the full signed bundle seal.
    run(["codesign", "--verify", "--strict", "--ignore-resources", str(binary)])


def verify(app: Path) -> dict:
    if (app / "Contents/Resources/release-build.json").exists():
        return verify_distribution(app)
    assert_identity(app)
    receipt = json.loads((app / "Contents/Resources/test-build.json").read_text())
    actual = native_signature(app / "Contents/MacOS/hex")
    signed = actual.get("team_identifier") not in (None, "not set")
    if receipt.get("schema") == 2:
        configured = signing_configuration(json.loads((ROOT / "scripts/hex-test-build.json").read_text()))
        if receipt.get("signing") != configured:
            raise DeliveryError("signed receipt does not match the configured certificate/team")
        verify_signed_bundle(app, configured, actual)
    elif signed:
        raise DeliveryError("signed app requires a sealed schema-2 receipt; refusing legacy verification")
    if receipt.get("files") != inventory(app, signed_payload=receipt.get("schema") == 2):
        raise DeliveryError(f"artifact checksum mismatch: {app}")
    if not os.access(app / "Contents/MacOS/hex", os.X_OK):
        raise DeliveryError("packaged binary is not executable")
    verify_native_executable(app / "Contents/MacOS/hex")
    if receipt.get("update_supervisor") is not None:
        verify_update_supervisor(app, receipt)
    if receipt.get("vst3_discovery") is not None:
        verify_discovery_package(app, receipt)
    if receipt.get("vst3_instance") is not None:
        verify_instance_package(app, receipt)
    return receipt


def verify_distribution(app: Path) -> dict:
    """Verify the sealed distribution receipt, shared by release and installer.

    The full bundle seal authenticates all resources. Distribution archive hashes
    and notarization/stapling are checked by hex-release; local launch verification
    never needs a network call. No distribution receipt is a test-build receipt.
    """
    assert_identity(app)
    resources = app / "Contents/Resources"
    receipt = json.loads((resources / "release-build.json").read_bytes())
    if (not isinstance(receipt, dict) or receipt.get("schema") != 1
            or receipt.get("channel") not in {"beta", "nightly", "stable"}
            or receipt.get("profile") != "release"
            or receipt.get("verification") != {"mode": "full-ci", "tests": "passed"}
            or type(receipt.get("build")) is not int or receipt["build"] < 1
            or not isinstance(receipt.get("source"), str)
            or re.fullmatch(r"[0-9a-f]{40}", receipt["source"]) is None):
        raise DeliveryError("invalid distribution receipt")
    signing = signing_configuration(json.loads((ROOT / "scripts/hex-test-build.json").read_bytes()))
    if receipt.get("signing") != signing:
        raise DeliveryError("distribution signer differs from configuration")
    verify_signed_bundle(app, signing, native_signature(app / "Contents/MacOS/hex"))
    signature = subprocess.run(["codesign", "--display", "--verbose=4", "--entitlements", "-", str(app)],
                               check=True, capture_output=True)
    entitlements = plistlib.loads(signature.stdout)
    details = signature.stderr.decode("utf-8", errors="replace")
    if (not re.search(r"flags=.*\bruntime\b", details)
            or not re.search(r"(?m)^Timestamp=.+", details)
            or entitlements != {"com.apple.security.device.audio-input": True}):
        raise DeliveryError("distribution app requires timestamp, hardened runtime and exact audio-input entitlement")
    info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
    if info.get("NSMicrophoneUsageDescription") != "hex uses audio input to record audio into your project.":
        raise DeliveryError("distribution microphone usage description differs")
    for key, expected in (("CFBundleVersion", str(receipt["build"])),
                          ("CFBundleShortVersionString", receipt["target"]),
                          ("HexReleaseVersion", receipt["version"]),
                          ("HexReleaseChannel", receipt["channel"]),
                          ("HexSourceRevision", receipt["source"])):
        if info.get(key) != expected:
            raise DeliveryError("distribution metadata differs from sealed receipt")
    for filename, key in (("notes.json", "notes_sha256"), ("snapshot.json", "snapshot_sha256"),
                          ("TESTING.md", "checklist_sha256"), ("local-ci-summary.json", "ci_report_sha256"),
                          ("manual/documentation.json", "documentation_sha256")):
        if sha(resources / filename) != receipt.get(key):
            raise DeliveryError("distribution resource differs from sealed receipt")
    documentation = json.loads((resources / "manual/documentation.json").read_bytes())
    if (documentation.get("schema") != 1 or documentation.get("version") != receipt["version"]
            or documentation.get("source") != receipt["source"]
            or set(documentation.get("outputs", {})) != {"hex-manual.html", "hex-manual.pdf"}):
        raise DeliveryError("distribution manual provenance differs")
    if any(sha(resources / "manual" / name) != digest for name, digest in documentation["outputs"].items()):
        raise DeliveryError("distribution manual bytes differ")
    if not os.access(app / "Contents/MacOS/hex", os.X_OK):
        raise DeliveryError("distribution executable is not executable")
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


def install_workspace(app: Path) -> Path:
    return app.parent / ".hex-install"


def pending_candidate(target: Path) -> Path:
    """Reserve a fixed private candidate store without touching the installed app.

    A pre-existing directory must carry our exact target-bound marker. Unknown
    contents are preserved, including incomplete initialization after a crash.
    """
    root = target.parent / ".hex-pending-update"
    marker = root / "owner.json"
    assert_plain_path(root)
    owner = {"schema": 1, "target": str(target)}
    try:
        root.mkdir(mode=0o700)
    except FileExistsError:
        if not root.is_dir():
            raise DeliveryError("pending update location is not a directory")
        assert_plain_path(marker)
        try:
            matches = marker.is_file() and json.loads(marker.read_text()) == owner
        except (OSError, ValueError) as error:
            raise DeliveryError("pending update owner is unreadable; preserved for inspection") from error
        if not matches:
            raise DeliveryError("pending update location is not owned by this target; preserved for inspection")
    else:
        write_json(marker, owner)
        sync_directory(root.parent)
    allowed = {"owner.json", ".hex-test-build.lock", ".hex-install", "hex.app",
               "hex-test-build-receipt.json", ".DS_Store"}
    for member in root.iterdir():
        if member.name not in allowed:
            raise DeliveryError("unexpected pending update contents; preserved for inspection")
        assert_plain_path(member)
        if member.name in {".hex-install", "hex.app"}:
            valid = member.is_dir()
        else:
            valid = stat.S_ISREG(member.lstat().st_mode)
        if not valid:
            raise DeliveryError("invalid pending update member; preserved for inspection")
    return root / "hex.app"


def pending_target_identity(target: Path) -> dict:
    """Observe the installed bundle for later comparison, without modifying it."""
    assert_identity(target)
    binary = target / "Contents/MacOS/hex"
    info = target / "Contents/Info.plist"
    assert_plain_path(binary)
    assert_plain_path(info)
    verify_native_executable(binary)
    metadata = target.stat()
    return {"device": metadata.st_dev, "inode": metadata.st_ino,
            "executable_sha256": sha(binary), "info_sha256": sha(info),
            "signature": native_signature(binary)}


def check_install_workspace(workspace: Path, *, apps: bool = True) -> None:
    """Allow only fixed installer members and Finder's regular metadata file."""
    assert_plain_path(workspace)
    if not workspace.is_dir():
        raise DeliveryError("unexpected temporary installation contents; preserved for inspection")
    for path in workspace.iterdir():
        if apps and path.name in ("stage.app", "previous.app"):
            assert_plain_path(path)
        elif path.name == ".DS_Store" and stat.S_ISREG(path.lstat().st_mode):
            continue
        else:
            raise DeliveryError("unexpected temporary installation contents; preserved for inspection")


def remove_install_workspace(workspace: Path) -> None:
    # Never recursively remove this directory: it can acquire Finder metadata
    # after publication. Inspect every member before unlinking that one file.
    for attempt in range(2):
        check_install_workspace(workspace, apps=False)
        metadata = workspace / ".DS_Store"
        if metadata.exists():
            metadata.unlink()
        try:
            workspace.rmdir()
            break
        except OSError as error:
            if error.errno != errno.ENOTEMPTY:
                raise
            # Finder can recreate metadata immediately before rmdir. Reinspect
            # every member, refuse foreign data, and retry only once. Continued
            # interference remains an honest incomplete installation.
            check_install_workspace(workspace, apps=False)
            if attempt == 1:
                raise
    sync_directory(workspace.parent)


def recover(app: Path) -> None:
    # Older installers own this history. Never guess which app their journal owns.
    legacy = app.parent / ".hex-test-build-transaction.json"
    if os.path.lexists(legacy):
        raise DeliveryError("legacy installation journal requires inspection; preserved unchanged")
    workspace = install_workspace(app)
    assert_plain_path(workspace)
    if not workspace.exists():
        return
    check_install_workspace(workspace)
    stage, previous = workspace / "stage.app", workspace / "previous.app"
    for path in (app, stage, previous):
        assert_plain_path(path)
        assert_inactive(path)
    if previous.exists():
        if app.exists():
            if stage.exists():
                raise DeliveryError("ambiguous interrupted installation; temporary apps preserved")
            try:
                verify(app)
            except (DeliveryError, OSError, ValueError, subprocess.SubprocessError):
                assert_identity(previous)
                app.rename(stage)
                previous.rename(app)
                sync_directory(workspace)
                sync_directory(app.parent)
        else:
            assert_identity(previous)
            previous.rename(app)
            sync_directory(workspace)
            sync_directory(app.parent)
    elif app.exists():
        # A partial stage never displaced this existing app. After publication,
        # stage is absent and only a verified new bundle permits cleanup.
        if stage.exists():
            assert_identity(app)
        else:
            verify(app)
    # Only these fixed temporary paths belong to this installer. Historical
    # review/rollback apps and legacy journals are never collected.
    for path in (stage, previous):
        if path.exists():
            shutil.rmtree(path)
    remove_install_workspace(workspace)


def activate(app: Path, stage: Path) -> None:
    workspace = install_workspace(app)
    if stage != workspace / "stage.app":
        raise DeliveryError("candidate must use the fixed temporary installation workspace")
    previous = workspace / "previous.app"
    assert_plain_path(workspace)
    check_install_workspace(workspace)
    assert_plain_path(previous)
    assert_plain_path(app)
    if previous.exists():
        raise DeliveryError("unfinished installation; retry before publishing another candidate")
    verify(stage)
    for path in stage.rglob("*"):
        if path.is_file():
            with path.open("rb") as source:
                os.fsync(source.fileno())
    for path in sorted((p for p in stage.rglob("*") if p.is_dir()), reverse=True):
        sync_directory(path)
    sync_directory(stage)
    sync_directory(workspace)
    sync_directory(app.parent)
    assert_inactive(app)
    if app.exists():
        assert_identity(app)
    try:
        # Recheck immediately before publication; never terminate a process.
        assert_inactive(app)
        if app.exists():
            app.rename(previous)
            sync_directory(workspace)
            sync_directory(app.parent)
        stage.rename(app)
        sync_directory(workspace)
        sync_directory(app.parent)
        verify(app)
    except (OSError, DeliveryError, ValueError, subprocess.SubprocessError):
        if previous.exists():
            assert_inactive(app)
            assert_inactive(previous)
            if app.exists():
                app.rename(stage)
            previous.rename(app)
            sync_directory(workspace)
            sync_directory(app.parent)
        raise
    # previous.app is publication custody only, never a retained version backup.
    check_install_workspace(workspace)
    if previous.exists():
        shutil.rmtree(previous)
    remove_install_workspace(workspace)



def _apply_verified_pending(target: Path, selected_receipt_sha256: str) -> dict:
    """Publish one selected pending candidate after an independently approved exit.

    The caller must first establish the exact parent session's approved normal
    exit. This operation is intentionally not exposed as an automatic launcher
    action. Both publication and pending selection are locked through the final
    verification and the existing recoverable activation transaction.
    """
    if (not isinstance(selected_receipt_sha256, str)
            or re.fullmatch(r"[0-9a-f]{64}", selected_receipt_sha256) is None):
        raise DeliveryError("invalid selected pending receipt")
    if not target.is_absolute() or target.name.casefold() != "hex.app":
        raise DeliveryError("pending update target must be an absolute hex.app path")
    assert_plain_path(target)
    root = target.parent / ".hex-pending-update"
    if not root.exists():
        raise DeliveryError("selected pending update is unavailable")
    with activation_lock(target):
        pending = pending_candidate(target)
        with activation_lock(pending):
            receipt_path = pending / "Contents/Resources/test-build.json"
            assert_plain_path(receipt_path)
            if not receipt_path.is_file() or sha(receipt_path) != selected_receipt_sha256:
                raise DeliveryError("pending update selection changed")
            receipt = verify(pending)
            if (receipt.get("schema") != 2 or receipt.get("app_path") != str(target)
                    or receipt.get("verification") != {"mode": "full-ci", "tests": "passed"}
                    or receipt.get("profile") != "release"
                    or not isinstance(receipt.get("pending_target_identity"), dict)):
                raise DeliveryError("pending update is not qualified for this target")
            assert_inactive(target)
            if pending_target_identity(target) != receipt["pending_target_identity"]:
                raise DeliveryError("installed app changed since preparation")
            old_signing = receipt["pending_target_identity"].get("signature")
            new_signing = native_signature(pending / "Contents/MacOS/hex")
            if not isinstance(old_signing, dict):
                raise DeliveryError("installed app signing identity is unavailable")
            if old_signing.get("team_identifier") not in (None, "not set") and (
                    not old_signing.get("designated_requirement")
                    or old_signing.get("team_identifier") != new_signing.get("team_identifier")
                    or old_signing.get("designated_requirement") != new_signing.get("designated_requirement")):
                raise DeliveryError("pending update changes the installed signing requirement")
            recover(target)
            if pending_target_identity(target) != receipt["pending_target_identity"]:
                raise DeliveryError("installed app changed during recovery")
            workspace = install_workspace(target)
            workspace.mkdir()
            stage = workspace / "stage.app"
            shutil.copytree(pending, stage, symlinks=True)
            if sha(receipt_path) != selected_receipt_sha256 or sha(stage / "Contents/Resources/test-build.json") != selected_receipt_sha256:
                raise DeliveryError("pending update changed during publication")
            if verify(stage) != receipt:
                raise DeliveryError("pending update changed during publication")
            assert_inactive(target)
            if pending_target_identity(target) != receipt["pending_target_identity"]:
                raise DeliveryError("installed app changed during publication")
            try:
                activate(target, stage)
            except Exception as error:
                # Activation normally rolls back on verification failure, but
                # cleanup can fail after the new app has already been verified.
                # Inspect only the selected receipt; do not assert success.
                try:
                    installed_receipt = target / "Contents/Resources/test-build.json"
                    assert_plain_path(installed_receipt)
                    may_have_published = installed_receipt.is_file() and sha(installed_receipt) == selected_receipt_sha256
                except (OSError, DeliveryError):
                    may_have_published = True
                if may_have_published:
                    raise PublicationUncertain("update publication may have completed; inspect the installed app before retry") from error
                raise
            try:
                installed = verify(target)
                if installed != receipt:
                    raise DeliveryError("installed update does not match selected receipt")
            except Exception as error:
                raise PublicationUncertain("update was published but its final verification failed; inspect before retry") from error
            try:
                result = {**receipt, "artifact_files": inventory(target, include_receipt=True),
                          "native_signature": native_signature(target / "Contents/MacOS/hex"),
                          "activation": "installed on disk; not launched by delivery"}
                write_json(target.parent / "hex-test-build-receipt.json", result)
            except Exception as error:
                raise PublicationFollowupFailed("update installed but its final receipt could not be written") from error
            return result



def apply_after_approved_parent_exit(parent_pid: int, approval_fd: int, ready_fd: int,
                                     target: Path, selected_receipt_sha256: str,
                                     *, observer_timeout: float = 60.0) -> dict:
    """Authorize publication only after the inherited parent handshake and clean exit."""
    spec = importlib.util.spec_from_file_location(
        "hex_pending_update_exit", Path(__file__).with_name("pending_update_exit.py"))
    if spec is None or spec.loader is None:
        raise DeliveryError("approved shutdown observer unavailable")
    observer = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(observer)
    try:
        observer.observe_parent_exit(parent_pid, approval_fd, ready_fd,
                                     target / "Contents/MacOS/hex", timeout=observer_timeout)
    except observer.ExitObservationError as error:
        raise DeliveryError(str(error)) from error
    return _apply_verified_pending(target, selected_receipt_sha256)


def _require_delivery_sources(runner) -> None:
    """Check both policy consumers without looking up mutable module names."""
    require_delivery_source(runner.root)
    error = ci.delivery_source_error(runner.root)
    if error is not None:
        raise DeliveryError("delivery CI source mismatch: " + error)
    verify = getattr(type(runner), "delivery_source_error", None)
    if not callable(verify):
        raise DeliveryError("runner source witness is unavailable")
    error = verify(runner)
    if error is not None:
        raise DeliveryError("runner CI source mismatch: " + error)


def validate_ci(report: Path, runner, candidate: str, *, allow_dev_profile: bool = False,
                reservation=None) -> dict:
    _require_delivery_sources(runner)
    evidence = json.loads(report.read_text())
    if not isinstance(evidence, dict):
        raise DeliveryError("CI certificate must be an object")
    if type(evidence.get("schema_version")) is not int or evidence["schema_version"] != 2:
        raise DeliveryError("CI certificate requires schema 2 with executed test counts")
    if (not isinstance(evidence.get("steps"), list)
            or any(not isinstance(step, dict) for step in evidence["steps"])
            or any(not isinstance(evidence.get(key), dict) for key in ("start_tree", "end_tree"))
            or "target_cache" in evidence and not isinstance(evidence["target_cache"], dict)):
        raise DeliveryError("CI certificate has malformed steps/source/input records")
    for step in evidence["steps"]:
        counts = step.get("test_results", {})
        if (not isinstance(counts, dict) or any(not isinstance(counts.get(key, []), list)
                or any(type(n) is not int or n < 0 for n in counts.get(key, []))
                for key in ("rust_passed_by_binary", "python_executions", "python_skipped"))):
            raise DeliveryError("CI certificate has malformed executed test counts")
    if not runner._capture_repository_state("start_tree"):
        raise DeliveryError("cannot identify current source")
    current = runner.summary["start_tree"]
    plan = evidence.get("plan")
    expected_steps = [(step.step_id, list(step.argv)) for step in ci.FULL_STEPS]
    allowed_mode = evidence.get("mode") == "full"
    if plan is not None:
        if not isinstance(plan, dict):
            raise DeliveryError("invalid scoped verification plan")
        if not allow_dev_profile and (plan.get("scope") != "full" or plan.get("reuse")):
            raise DeliveryError("beta/stable/native qualified delivery requires the full audit, not scoped dev checks")
        if (plan.get("profile") not in {"merge", "full", "release"}
                or not plan.get("qualifies_merge") or plan.get("source") != candidate):
            raise DeliveryError("invalid scoped verification plan")
        if reservation is not None:
            reservation.revalidate(runner.env.get("CARGO_TARGET_DIR"))
        metadata = json.loads(run(["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"],
                                  runner.root, env=runner.env))
        if reservation is not None:
            reservation.revalidate(metadata.get("target_directory"))
            if metadata.get("workspace_root") != str(runner.root):
                raise DeliveryError("CI metadata changed the reserved workspace")
        _require_delivery_sources(runner)
        resolved = ci.ci_plan.resolve(runner.root, profile=plan["profile"], base=plan["base"], source=candidate,
                                      metadata=metadata, allow_same_tree_merge=True)
        if plan.get("reuse"):
            resolved = ci.reusable_plan(runner.root, resolved, Path(plan["reuse"]["report"]))
        _require_delivery_sources(runner)
        if resolved != plan:
            raise DeliveryError("recorded scope differs from the current source/dependency plan")
        expected_steps = [(step.step_id, list(step.argv)) for step in ci.planned_steps(plan)]
        # Full Rust scope can use reduced Python coverage for ordinary merges.
        # Qualified delivery still requires the complete freshly resolved audit.
        if not allow_dev_profile and expected_steps != [
                (step.step_id, list(step.argv)) for step in ci.FULL_STEPS]:
            raise DeliveryError("beta/stable/native qualified delivery requires the full audit, not scoped dev checks")
        allowed_mode = evidence.get("mode") == plan["profile"]
    if evidence.get("schema_version") == 2:
        required = ci.planned_steps(plan or {"scope": "full"})
        for step, recorded in zip(required, evidence.get("steps", [])):
            counts = recorded.get("test_results", {})
            if step.requires_tests and (sum(counts.get("rust_passed_by_binary", []))
                    + sum(counts.get("python_executions", [])) - sum(counts.get("python_skipped", []))) <= 0:
                raise DeliveryError("requested test configuration has no executed positive cases")
    actual_steps = [(step.get("step_id"), step.get("argv")) for step in evidence.get("steps", [])]
    if (not allowed_mode or evidence.get("status") != "passed"
            or evidence.get("stale") is not False or evidence.get("exit_code") != 0
            or actual_steps != expected_steps
            or any(step.get("status") != "passed" for step in evidence["steps"])
            or evidence.get("start_tree") != evidence.get("end_tree")
            or current["dirty"] or evidence.get("dirty") is not False
            or evidence.get("commit") != candidate
            or evidence["start_tree"].get("content_sha256") != current["content_sha256"]):
        raise DeliveryError("full exact-source local CI evidence is missing, failed, dirty or stale")
    try:
        ci.ci_checkpoint.validate_inheritance(runner.root, evidence)
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise DeliveryError(f"CI inherited stage evidence is invalid: {error}") from error
    head = current["commit"]
    if head != candidate:
        # A merge commit is eligible only with identical complete Git tree and ancestry.
        if run(["git", "rev-parse", f"{head}^{{tree}}"], runner.root) != run(
                ["git", "rev-parse", f"{candidate}^{{tree}}"], runner.root):
            raise DeliveryError("merged source differs from locally verified candidate")
        run(["git", "merge-base", "--is-ancestor", candidate, head], runner.root)
    if runner.target_lock is not None:
        # The release driver calls this under the SAME target lock before
        # pending/cleanup/build, again before consumption, and before publish.
        try:
            expected = evidence.get("target_cache", {}).get("dependency_inputs")
            if not isinstance(expected, dict) or runner._dependency_context() != expected:
                raise DeliveryError("CI toolchain/configuration inputs differ from the guarded delivery")
        except (OSError, ValueError, RuntimeError, ci.TargetCacheError) as error:
            raise DeliveryError(f"cannot revalidate CI toolchain/configuration inputs: {error}") from error
    _require_delivery_sources(runner)
    return evidence


def validate_source(runner, candidate: str, expected: dict | None = None) -> dict:
    """Freeze clean exact source for build-only launch; this is not CI evidence."""
    _require_delivery_sources(runner)
    if not runner._capture_repository_state("start_tree"):
        raise DeliveryError("cannot identify current source")
    current = runner.summary["start_tree"]
    if (len(candidate) != 40 or any(char not in "0123456789abcdef" for char in candidate)
            or current["dirty"] or current["commit"] != candidate
            or (expected is not None and current != expected)):
        raise DeliveryError("build-only source is dirty, changed or differs from the exact candidate")
    _require_delivery_sources(runner)
    return dict(current)


def changelog_payload(root: Path) -> tuple[bytes | None, dict]:
    """Return validated exact committed bytes for sealing; never generate during build."""
    try:
        dataset = changelog.validate_current(root, if_present=True)
    except (ValueError, OSError, KeyError, TypeError) as error:
        raise DeliveryError(f"invalid changelog; previous app preserved: {error}") from error
    path = root / changelog.DATASET
    if not path.exists():
        return None, {"status": "unavailable", "coverage": []}
    raw = path.read_bytes()
    return raw, {"status": "available" if dataset['batches'] else "unavailable",
                 "sha256": hashlib.sha256(raw).hexdigest(),
                 "coverage": [batch['input']['source'] for batch in dataset['batches']]}


def build(args) -> dict:
    require_delivery_source(ROOT)
    if not getattr(args, "fresh_release_target", False):
        return _build(args)
    if (getattr(args, "adopt_release_target", False)
            or getattr(args, "expected_release_owner_sha256", None) is not None
            or getattr(args, "expected_recreated_target_device", None) is not None
            or getattr(args, "expected_recreated_target_inode", None) is not None):
        raise DeliveryError("fresh release target cannot adopt or recover an owner")
    target = os.environ.get("CARGO_TARGET_DIR")
    if not target or os.environ.get("CARGO_CACHE_RUSTC_INFO") != "0":
        raise DeliveryError("fresh release target requires explicit CARGO_TARGET_DIR and CARGO_CACHE_RUSTC_INFO=0")
    with ci.FreshTargetReservation.acquire(ROOT, target, {"operation": "fresh-release-delivery"}) as reservation:
        return _build(args, reservation=reservation)


def _build(args, *, reservation=None) -> dict:
    require_delivery_source(ROOT)
    if sys.platform != "darwin":
        raise DeliveryError("native test delivery requires macOS")
    common = Path(run(["git", "rev-parse", "--git-common-dir"]))
    if not common.is_absolute():
        common = ROOT / common
    configuration = json.loads((ROOT / "scripts/hex-test-build.json").read_text())
    signing = signing_configuration(configuration)
    configured = Path(os.environ.get("HEX_DEV_APP_PATH", configuration["app_path"]))
    app = configured if configured.is_absolute() else common.resolve().parent / configured
    app = app.absolute()
    if app.name.casefold() != "hex.app":
        raise DeliveryError("stable development target must be named hex.app")
    assert_plain_path(app)
    app.parent.mkdir(parents=True, exist_ok=True)
    target = app
    prepare_only = getattr(args, "prepare_only", False)
    target_identity = None
    if prepare_only:
        if getattr(args, "build_only", False) or getattr(args, "dev_profile", False):
            raise DeliveryError("pending updates require full exact-source CI evidence")
        if not target.exists():
            raise DeliveryError("pending update requires an existing installed hex app")
        if getattr(args, "adopt_release_target", False):
            # Recovery can clean an unknown release target and retains the
            # ordinary installer's stronger running-process refusal.
            assert_recovery_inactive(target)
        target_identity = pending_target_identity(target)
        app = pending_candidate(target)
    with activation_lock(app):
        recover(app)
        assert_inactive(app)
        if app.exists():
            assert_identity(app)
            if prepare_only and verify(app).get("app_path") != str(target):
                raise DeliveryError("pending candidate targets a different application")
        runner = ci.LocalCiRunner(ROOT, adopt_target=False)
        build_only = getattr(args, "build_only", False)
        source_evidence = validate_source(runner, args.candidate) if build_only else None
        release_ownership = None

        def admitted(ownership) -> None:
            nonlocal release_ownership
            release_ownership = ownership

        def revalidate() -> None:
            if build_only:
                validate_source(runner, args.candidate, expected=source_evidence)
            else:
                validate_ci(args.ci_report, runner, args.candidate,
                            **({"allow_dev_profile": True} if getattr(args, "dev_profile", False) else {}),
                            **({"reservation": reservation} if reservation is not None and release_ownership is None else {}))
            if release_ownership is not None:
                runner.validate_release_inputs(release_ownership)

        revalidate()
        changelog_bytes, changelog_identity = changelog_payload(ROOT)
        revalidate()
        included = json.loads(args.included.read_text())
        if not isinstance(included, list) or not included:
            raise DeliveryError("include PR head revisions")
        for item in included:
            if (not isinstance(item, dict) or not isinstance(item.get("pr"), str)
                    or not item["pr"].startswith("https://github.com/PORTALSURFER/hex/pull/")
                    or not isinstance(item.get("head"), str) or len(item["head"]) != 40
                    or any(char not in "0123456789abcdef" for char in item["head"])):
                raise DeliveryError("invalid included PR revision")
        checklist = args.checklist.read_text()
        if not checklist.strip():
            raise DeliveryError("finite user checklist required")
        runner.output_dir.mkdir(parents=True)
        resolved = (runner._resolve_cargo_identity() if reservation is None
                    else runner._resolve_cargo_identity(reservation))
        if not resolved or not runner._check_disk_headroom():
            raise DeliveryError(str(runner.summary))
        replacement = None
        device = getattr(args, "expected_recreated_target_device", None)
        inode = getattr(args, "expected_recreated_target_inode", None)
        if device is not None or inode is not None:
            if (type(device) is not int or device < 0 or type(inode) is not int or inode <= 0
                    or not getattr(args, "adopt_release_target", False)
                    or not getattr(args, "expected_release_owner_sha256", None)):
                raise DeliveryError("explicit recreated target requires both identity integers and owner CAS")
            replacement = {"device": device, "inode": inode}
        recovery = {}
        if replacement is not None:
            recovery = {"expected_recreated_target_identity": replacement,
                        "recovery_inactive_check": lambda: assert_recovery_inactive(target)}
        instance_receipt = getattr(args, "vst3_instance_receipt", None)
        if instance_receipt is not None:
            validate_instance_input(instance_receipt, args.candidate)
        discovery_receipt = prepare_discovery_input(args, runner.output_dir)
        with optimized_artifact(runner, **({"discovery": True} if discovery_receipt else {}), adopt=getattr(args, "adopt_release_target", False),
                                expected_owner_sha256=getattr(args, "expected_release_owner_sha256", None),
                                verification_check=revalidate,
                                **({"reservation": reservation, "on_admission": admitted} if reservation is not None else {}),
                                **recovery) as (binary, release_ownership):
            metadata = json.loads(run(["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"],
                                      runner.root, env=runner.env))
            version = next(package["version"] for package in metadata["packages"] if package["name"] == "hex-desktop")
            argv = release_ownership["build_argv"]
            revalidate()
            workspace = install_workspace(app)
            workspace.mkdir()
            stage = workspace / "stage.app"
            stage.mkdir()
            contents = stage / "Contents"
            (contents / "MacOS").mkdir(parents=True)
            (contents / "Resources/licenses").mkdir(parents=True)
            shutil.copy2(binary, contents / "MacOS/hex")
            instance_evidence = None
            if instance_receipt is not None:
                instance_evidence = stage_instance(stage, instance_receipt, args.candidate, signing)
            discovery_evidence = None
            if discovery_receipt is not None:
                discovery_evidence = stage_discovery(
                    stage, discovery_receipt, args.candidate, release_ownership["discovery_scanner"], signing)
            for source, name in ((ROOT / "assets/fonts/ioskeley-mono/OFL.txt", "ioskeley-mono-OFL.txt"),
                                 (ROOT / "third_party/gpui/LICENSE-APACHE", "gpui-LICENSE-APACHE")):
                shutil.copy2(source, contents / "Resources/licenses" / name)
            update_supervisor = stage_update_supervisor(stage, signing)
            source = runner.summary["start_tree"]["commit"]
            info = {"CFBundleExecutable": "hex", "CFBundleIdentifier": IDENTITY,
                    "CFBundleName": "hex", "CFBundleDisplayName": "hex", "CFBundlePackageType": "APPL",
                    "CFBundleShortVersionString": version, "CFBundleVersion": str(int(source[:8], 16)),
                    "NSHighResolutionCapable": True, "HexSourceRevision": source}
            with (contents / "Info.plist").open("wb") as output:
                plistlib.dump(info, output)
            (contents / "Resources/USER-TEST.md").write_text(checklist)
            if changelog_bytes is not None:
                (contents / "Resources/changelog.json").write_bytes(changelog_bytes)
            if build_only:
                write_json(contents / "Resources/source-verification.json", {
                    "mode": "source-only", "tests": "not_run", "source": source_evidence,
                })
            else:
                shutil.copy2(args.ci_report, contents / "Resources/local-ci-summary.json")
            verify_native_executable(contents / "MacOS/hex")
            receipt = {"schema": 2, "version": version, "source": source, "candidate": args.candidate,
                       "diagnostic_capabilities": ["actions-v1", "audio-v1"],
                       "source_sha256": runner.summary["start_tree"]["content_sha256"],
                       "profile": "release", "build_argv": argv, "target_directory": str(runner.cargo_identity["target_directory"]),
                       "release_target_ownership": {key: release_ownership[key] for key in
                           ("profile", "source", "packages", "target_directory", "owner_path",
                            "lock_path", "action", "cleanup_status", "artifact")},
                       "ci_report": None if build_only else str(args.ci_report.resolve()),
                       "ci_report_sha256": None if build_only else sha(args.ci_report),
                       "verification": {"mode": "source-only" if build_only else "scoped-dev-ci" if getattr(args, "dev_profile", False) else "full-ci",
                                        "tests": "not_run" if build_only else "passed"},
                       "update_supervisor": update_supervisor,
                       "vst3_discovery": discovery_evidence,
                       "vst3_instance": instance_evidence,
                       "included": included, "changelog": changelog_identity, "app_path": str(target), "created_at": ci.utc_now(),
                       "activation": ("prepared pending update; not installed or launched" if prepare_only else
                                      "installed on disk; launch or relaunch required; not launched by delivery"),
                       "signature": "Developer ID sealed local test bundle; not notarized",
                       "signing": signing,
                       "input_executable_sha256": sha(contents / "MacOS/hex"),
                       "files": inventory(stage, signed_payload=True)}
            if prepare_only:
                receipt["pending_target_identity"] = target_identity
            write_json(contents / "Resources/test-build.json", receipt)
            sign_bundle(stage, signing)
            verify(stage)
            verify_native_executable(contents / "MacOS/hex")
            revalidate()
            if prepare_only and pending_target_identity(target) != target_identity:
                raise DeliveryError("installed app changed during preparation; pending candidate was not replaced")
            activate(app, stage)
            verify_native_executable(app / "Contents/MacOS/hex")
            receipt = {**receipt, "artifact_files": inventory(app, include_receipt=True),
                       "native_signature": native_signature(app / "Contents/MacOS/hex")}
            write_json(app.parent / "hex-test-build-receipt.json", receipt)
            return receipt


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    verification = parser.add_mutually_exclusive_group(required=True)
    verification.add_argument("--ci-report", type=Path, help="full exact-source local CI report")
    parser.add_argument("--dev-profile", action="store_true",
                        help="explicit development delivery only; allow a resolved scoped merge report, never beta/stable full qualification")
    parser.add_argument("--prepare-only", action="store_true",
                        help="prepare a fully verified pending update without replacing the installed app")
    verification.add_argument("--build-only", action="store_true",
                              help="verify clean exact source and build without running or requiring tests")
    parser.add_argument("--candidate", required=True, help="full exact candidate commit")
    parser.add_argument("--included", type=Path, required=True, help="JSON list of pr/head objects (ticket references optional)")
    parser.add_argument("--checklist", type=Path, required=True)
    parser.add_argument("--adopt-release-target", action="store_true",
                        help="explicitly recover unknown/pending release ownership; selectively clean workspace release artifacts")
    parser.add_argument("--fresh-release-target", action="store_true",
                        help="reserve the explicit existing empty CARGO_TARGET_DIR before metadata")
    parser.add_argument("--vst3-instance-receipt", type=Path,
                        help="include exact-source minimal native instance helper")
    discovery = parser.add_mutually_exclusive_group()
    discovery.add_argument("--vst3-discovery-receipt", type=Path,
                           help="use a prebuilt exact-source discovery helper instead of building with HEX_VST3_SDK")
    discovery.add_argument("--vst3-sdk", type=Path,
                           help="use this pinned clean SDK checkout to build the default discovery helper")
    discovery.add_argument("--without-vst3-discovery", action="store_true",
                           help="explicitly build a development app without VST3 discovery")
    parser.add_argument("--expected-release-owner-sha256",
                        help="require this exact previously inspected owner under the release mutation lock")
    parser.add_argument("--expected-recreated-target-device", type=int,
                        help="explicit recovery: require this inspected replacement target device")
    parser.add_argument("--expected-recreated-target-inode", type=int,
                        help="explicit recovery: require this inspected replacement target inode")
    args = parser.parse_args()
    if args.dev_profile and args.build_only:
        parser.error("--dev-profile requires --ci-report, not --build-only")
    if args.expected_recreated_target_device is not None or args.expected_recreated_target_inode is not None:
        if (args.expected_recreated_target_device is None or args.expected_recreated_target_device < 0
                or args.expected_recreated_target_inode is None or args.expected_recreated_target_inode <= 0
                or not args.expected_release_owner_sha256 or not args.adopt_release_target):
            parser.error("recreated-target recovery requires device/inode, owner SHA256 and explicit adoption")
    if args.expected_release_owner_sha256 and not args.adopt_release_target:
        parser.error("--expected-release-owner-sha256 requires --adopt-release-target")
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    try:
        receipt = build(args)
        print(json.dumps(receipt, indent=2, sort_keys=True))
        return 0
    except (DeliveryError, ci.TargetCacheError, ci.InterruptedRun, OSError, ValueError,
            KeyError, TypeError, StopIteration, subprocess.SubprocessError, KeyboardInterrupt) as error:
        operation = "pending update preparation" if args.prepare_only else "test installation"
        print(f"{operation} incomplete: {error}; temporary publication files preserved for retry. "
              "inspect the reported process/ownership refusal before retrying; no process was terminated", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
