#!/usr/bin/env python3
"""Prepare signed VST3 discovery tools for the raw Cargo desktop."""
from contextlib import redirect_stdout
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location("hex_launch_delivery", ROOT / "scripts/hex-test-build.py")
delivery = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(delivery)


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


def verify_tools(app: Path, signing: dict) -> None:
    receipt_path = app / "Contents/Resources/test-build.json"
    delivery.assert_plain_path(receipt_path)
    receipt = json.loads(receipt_path.read_bytes())
    if (receipt.get("schema") != 2 or receipt.get("profile") != "release"
            or receipt.get("signing") != signing
            or receipt.get("files") != delivery.inventory(app)):
        raise delivery.DeliveryError(f"invalid discovery tools cache: {app}")
    delivery.verify_discovery_package(app, receipt)


def prepare() -> Path:
    common = Path(delivery.run(["git", "rev-parse", "--path-format=absolute", "--git-common-dir"]))
    artifacts = common.parent / "artifacts"
    sdk = Path(os.environ.get("HEX_VST3_SDK") or artifacts / "vst3-sdk")
    builder = delivery.discovery_builder()
    native_digest = builder.source_digest()
    cache = artifacts / "launch-discovery"
    native = cache / ("native-" + native_digest)
    if not native.exists() and not sdk.is_dir():
        raise delivery.DeliveryError("set HEX_VST3_SDK to the pinned clean VST3 SDK checkout")
    signing = delivery.signing_configuration(json.loads((ROOT / "scripts/hex-test-build.json").read_bytes()))
    scanner = optimized_scanner()
    delivery.assert_plain_path(scanner)
    scanner_hash = delivery.sha(scanner)
    key = hashlib.sha256(json.dumps(
        [scanner_hash, native_digest, signing], sort_keys=True
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
                    if delivery.run(["git", "status", "--porcelain"]):
                        raise delivery.DeliveryError("prepare new vst3 discovery tools from a clean checkout first")
                    local = ROOT / "artifacts"
                    local.mkdir(exist_ok=True)
                    with tempfile.TemporaryDirectory(prefix="launch-native-", dir=local) as output:
                        built = builder.build(sdk, Path(output) / "native", discovery_only=True)
                        shutil.copytree(built.parent, staging / "native")
                    (staging / "native").rename(native)
                receipt = native / "controlled-macos.json"
                delivery.assert_plain_path(receipt)
                source = json.loads(receipt.read_bytes()).get("source_commit", "")
                if not isinstance(source, str) or re.fullmatch(r"[0-9a-f]{40}", source) is None:
                    raise delivery.DeliveryError("cannot identify source for discovery")
                tools = staging / "tools" / "hex.app"
                evidence = delivery.stage_discovery(tools, receipt, source, {
                    "path": str(scanner), "sha256": scanner_hash,
                }, signing)
                delivery.write_json(tools / "Contents/Resources/test-build.json", {
                    "schema": 2, "profile": "release", "source": source,
                    "signing": signing, "vst3_discovery": evidence,
                    "files": delivery.inventory(tools),
                })
                verify_tools(tools, signing)
                tools.parent.rename(generation)
        else:
            verify_tools(app, signing)
    return app


if __name__ == "__main__":
    try:
        with redirect_stdout(sys.stderr):
            app = prepare()
        print(app)
    except (delivery.DeliveryError, OSError, ValueError, KeyError, TypeError,
            subprocess.SubprocessError) as error:
        print(f"vst3 discovery unavailable: {error}", file=sys.stderr)
        raise SystemExit(1)
