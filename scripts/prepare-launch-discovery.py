#!/usr/bin/env python3
"""Prepare a signed VST3 helper profile for the raw Cargo desktop."""
import argparse
from contextlib import redirect_stdout
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent


def _delivery_source(path: Path) -> bytes:
    """Read the delivery source from an opened regular file without links."""
    directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        descriptor = os.open(path.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
        try:
            if not stat.S_ISREG(os.fstat(descriptor).st_mode):
                raise RuntimeError("delivery source is not a regular file")
            with os.fdopen(descriptor, "rb", closefd=False) as source:
                return source.read()
        finally:
            os.close(descriptor)
    finally:
        os.close(directory)


SPEC = importlib.util.spec_from_file_location("hex_launch_delivery", ROOT / "scripts/hex-test-build.py")
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("launch delivery dependency unavailable")
delivery = importlib.util.module_from_spec(SPEC)
# Delivery authority must reflect source bytes, not a timestamp-valid pyc.
_SOURCE = _delivery_source(Path(SPEC.origin))
delivery._EXECUTED_DELIVERY_SOURCE_SHA256 = hashlib.sha256(_SOURCE).hexdigest()
exec(compile(_SOURCE, SPEC.origin, "exec", dont_inherit=True), delivery.__dict__)


def optimized_scanner() -> Path:
    result = subprocess.run(
        ["cargo", "build", "--release", "-p", "hex-plugin-scan", "--bin",
         "hex-vst3-scan-macos", "--locked", "--message-format=json"],
        cwd=ROOT, stdout=subprocess.PIPE, text=True, check=True,
    )
    artifacts = [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]
    paths = [Path(event["executable"]) for event in artifacts
             if event.get("reason") == "compiler-artifact" and event.get("executable")
             and event.get("target", {}).get("name") == "hex-vst3-scan-macos"
             and event.get("profile", {}).get("opt_level") == "3"]
    if len(paths) != 1 or not paths[0].is_absolute():
        raise delivery.DeliveryError("no unique optimized discovery scanner from Cargo")
    return paths[0]


def verify_tools(app: Path, signing: dict, instance: bool) -> None:
    receipt_path = app / "Contents/Resources/test-build.json"
    delivery.assert_plain_path(receipt_path)
    receipt = json.loads(receipt_path.read_bytes())
    if (receipt.get("schema") != 2 or receipt.get("profile") != "release"
            or receipt.get("signing") != signing
            or receipt.get("files") != delivery.inventory(app)):
        raise delivery.DeliveryError(f"invalid launch tools cache: {app}")
    if instance:
        delivery.verify_instance_package(app, receipt)
    else:
        delivery.verify_discovery_package(app, receipt)


def native_source_checkout(root: Path, staging: Path) -> Path:
    if delivery.run(["git", "status", "--porcelain", "--untracked-files=no"], root=root):
        raise delivery.DeliveryError("prepare new vst3 tools with unmodified tracked files")
    if not delivery.run(["git", "status", "--porcelain"], root=root):
        return root
    # Keep untracked files out of the compiler and preserve the existing exact
    # clean-checkout receipt. The private clone shares only Git objects and is
    # removed with the rest of the owned staging directory.
    candidate = delivery.run(["git", "rev-parse", "HEAD"], root=root)
    source = staging / "source"
    delivery.run(["git", "clone", "--quiet", "--shared", "--no-checkout", "--", str(root), str(source)], root=root)
    delivery.run(["git", "checkout", "--quiet", "--detach", candidate], root=source)
    return source


def build_native(builder, sdk: Path, instance: bool, staging: Path) -> Path:
    source = native_source_checkout(ROOT, staging)
    local = source / "artifacts"
    local.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="launch-native-", dir=local) as output:
        output = Path(output) / "native"
        if source == ROOT:
            receipt = builder.build(sdk, output, discovery_only=not instance, instance_only=instance)
        else:
            delivery.run([sys.executable, str(source / "crates/hex-plugin-scan/native/macos_bootstrap.py"),
                          "build", "--sdk", str(sdk.resolve()), "--output", str(output),
                          "--instance-only" if instance else "--discovery-only"], root=source)
            receipt = output / "controlled-macos.json"
        if builder.verify(receipt).get("source_dirty") is not False:
            raise delivery.DeliveryError("vst3 helper build did not use a clean source checkout")
        destination = staging / "native"
        shutil.copytree(receipt.parent, destination)
        return destination


def prepare(instance: bool) -> Path:
    delivery.require_delivery_source(ROOT)
    common = Path(delivery.run(["git", "rev-parse", "--path-format=absolute", "--git-common-dir"]))
    artifacts = common.parent / "artifacts"
    sdk = Path(os.environ.get("HEX_VST3_SDK") or artifacts / "vst3-sdk")
    builder = delivery.discovery_builder()
    native_digest = builder.source_digest()
    cache = artifacts / ("launch-instance" if instance else "launch-discovery")
    native = cache / ("native-" + native_digest)
    if not native.exists() and not sdk.is_dir():
        raise delivery.DeliveryError("set HEX_VST3_SDK to the pinned clean VST3 SDK checkout")
    signing = delivery.signing_configuration(json.loads((ROOT / "scripts/hex-test-build.json").read_bytes()))
    scanner = None
    if not instance:
        path = optimized_scanner()
        delivery.assert_plain_path(path)
        scanner = {"path": str(path), "sha256": delivery.sha(path)}
    key = hashlib.sha256(json.dumps(
        [scanner and scanner["sha256"], native_digest, signing], sort_keys=True
    ).encode()).hexdigest()
    delivery.assert_plain_path(cache)
    cache.mkdir(parents=True, exist_ok=True)
    generation = cache / key
    app = generation / "hex.app"
    # Only one launcher publishes a generation; readers use immutable directories.
    lock_path = cache / "prepare.lock"
    delivery.assert_plain_path(lock_path)
    with lock_path.open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if not generation.exists():
            with tempfile.TemporaryDirectory(prefix="stage-", dir=cache) as staging:
                staging = Path(staging)
                if not native.exists():
                    built = build_native(builder, sdk, instance, staging)
                    delivery.require_delivery_source(ROOT)
                    built.rename(native)
                receipt = native / "controlled-macos.json"
                delivery.assert_plain_path(receipt)
                source = json.loads(receipt.read_bytes()).get("source_commit", "")
                if not isinstance(source, str) or re.fullmatch(r"[0-9a-f]{40}", source) is None:
                    raise delivery.DeliveryError("cannot identify source for launch tools")
                tools = staging / "tools" / "hex.app"
                (tools / "Contents/Resources").mkdir(parents=True)
                if instance:
                    evidence = {"vst3_instance": delivery.stage_instance(tools, receipt, source, signing)}
                else:
                    evidence = {"vst3_discovery": delivery.stage_discovery(
                        tools, receipt, source, scanner, signing)}
                delivery.write_json(tools / "Contents/Resources/test-build.json", {
                    "schema": 2, "profile": "release", "source": source,
                    "signing": signing, **evidence,
                    "files": delivery.inventory(tools),
                })
                verify_tools(tools, signing, instance)
                delivery.require_delivery_source(ROOT)
                tools.parent.rename(generation)
        else:
            verify_tools(app, signing, instance)
    return app


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--instance", action="store_true")
    options = parser.parse_args()
    try:
        with redirect_stdout(sys.stderr):
            app = prepare(options.instance)
        print(app)
    except (delivery.DeliveryError, OSError, ValueError, KeyError, TypeError,
            subprocess.SubprocessError) as error:
        role = "instance" if options.instance else "discovery"
        print(f"vst3 {role} unavailable: {error}", file=sys.stderr)
        raise SystemExit(1)
