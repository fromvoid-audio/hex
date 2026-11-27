#!/usr/bin/env python3
"""Reproducible local builder and verifier for `.hexdevice` native packages (#2997).

Compiles an independently authored C99 device source against the pinned Hex C ABI v1
header (`third_party/hex-device-abi-qual/include/hex_device_abi_v1.h`), emits a
deterministic `.hexdevice` archive (`HEXDEV01` format matching `hex-storage::device_packages`),
and writes a provenance/build receipt containing source, pinned SDK header, compiler,
target, and content SHA-256 hashes.

Never invokes production code signing or purchases certificates; development/test
packages use an explicitly isolated `reviewed_development_test` provenance tier.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import platform
import struct
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parent.parent
PINNED_ABI_HEADER = REPO_ROOT / "third_party" / "hex-device-abi-qual" / "include" / "hex_device_abi_v1.h"
ARCHIVE_MAGIC = b"HEXDEV01"


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    return sha256_bytes(path.read_bytes())


def host_target() -> tuple[str, str]:
    sys_name = platform.system().lower()
    os_name = "macos" if sys_name == "darwin" else sys_name
    machine = platform.machine().lower()
    arch_map = {"arm64": "aarch64", "amd64": "x86_64"}
    return os_name, arch_map.get(machine, machine)


def compiler_identity(cc: str) -> str:
    out = subprocess.check_output([cc, "--version"], text=True, stderr=subprocess.STDOUT)
    return out.splitlines()[0].strip() if out else cc


def build_package(
    *,
    c_source: Path,
    type_id: str,
    display_name: str,
    publisher_id: str,
    device_kind: int,
    capabilities: int,
    version: tuple[int, int, int],
    state_version: int,
    ui_version: int,
    ui_schema: dict[str, Any] | None,
    notice_text: str,
    auth_token: str,
    reviewer_note: str,
    out_archive: Path,
    out_receipt: Path,
    expected_sdk_header_sha256: str | None = None,
    cc: str = "clang",
    min_os_version: str = "14.0",
) -> dict[str, Any]:
    if not PINNED_ABI_HEADER.is_file():
        raise RuntimeError(f"pinned ABI header missing: {PINNED_ABI_HEADER}")
    actual_sdk_sha = sha256_file(PINNED_ABI_HEADER)
    if expected_sdk_header_sha256 and actual_sdk_sha != expected_sdk_header_sha256:
        raise ValueError(
            f"SDK ABI header mismatch: expected {expected_sdk_header_sha256}, got {actual_sdk_sha}"
        )
    if not c_source.is_file():
        raise FileNotFoundError(f"device source missing: {c_source}")

    target_os, target_arch = host_target()
    ext = "dylib" if target_os == "macos" else ("dll" if target_os == "windows" else "so")
    native_rel = f"bin/device.{ext}"

    with tempfile.TemporaryDirectory(prefix="hex-pkg-build-") as td:
        tmp_native = Path(td) / f"device.{ext}"
        cmd = [
            cc,
            "-std=c99",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-shared",
            "-fPIC",
            "-I",
            str(PINNED_ABI_HEADER.parent),
            str(c_source),
            "-o",
            str(tmp_native),
            "-lm",
        ]
        res = subprocess.run(cmd, capture_output=True, text=True)
        if res.returncode != 0:
            raise RuntimeError(f"native build failed:\n{res.stderr}")
        native_bytes = tmp_native.read_bytes()

    files: list[tuple[str, bytes]] = [(native_rel, native_bytes)]
    assets: list[str] = []
    ui_rel: str | None = None

    if ui_schema is not None:
        ui_rel = "ui/panel.json"
        ui_bytes = (json.dumps(ui_schema, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")
        files.append((ui_rel, ui_bytes))

    notice_rel = "notices/NOTICE.txt"
    files.append((notice_rel, notice_text.encode("utf-8")))
    assets.append(notice_rel)

    # Sort deterministically by path for reproducible archives.
    files.sort(key=lambda item: item[0])
    content_digests = {rel: sha256_bytes(data) for rel, data in files}
    auth_sha = sha256_bytes(auth_token.encode("utf-8"))

    manifest = {
        "schema_version": 1,
        "publisher_id": publisher_id,
        "type_id": type_id,
        "display_name": display_name,
        "package_major": version[0],
        "package_minor": version[1],
        "package_patch": version[2],
        "state_version": state_version,
        "abi_version": 1,
        "ui_version": ui_version,
        "device_kind": device_kind,
        "capabilities": capabilities,
        "target_os": target_os,
        "target_arch": target_arch,
        "min_os_version": min_os_version,
        "native_module_path": native_rel,
        "ui_schema_path": ui_rel,
        "presets": [],
        "assets": assets,
        "native_dependencies": [],
        "content_digests": content_digests,
        "provenance": {
            "tier": "reviewed_development_test",
            "authorization_sha256": auth_sha,
            "reviewer_note": reviewer_note,
        },
    }

    entries = [
        {
            "path": rel,
            "kind": "regular_file",
            "compression": "stored",
            "compressed_bytes": len(data),
            "expanded_bytes": len(data),
        }
        for rel, data in files
    ]

    header_json = json.dumps(
        {"manifest": manifest, "entries": entries},
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")

    archive_bytes = bytearray()
    archive_bytes.extend(ARCHIVE_MAGIC)
    archive_bytes.extend(struct.pack("<I", len(header_json)))
    archive_bytes.extend(header_json)
    for _, data in files:
        archive_bytes.extend(data)

    out_archive.parent.mkdir(parents=True, exist_ok=True)
    out_archive.write_bytes(archive_bytes)

    receipt = {
        "receipt_version": 1,
        "type_id": type_id,
        "publisher_id": publisher_id,
        "package_version": f"{version[0]}.{version[1]}.{version[2]}",
        "state_version": state_version,
        "abi_version": 1,
        "ui_version": ui_version,
        "target_os": target_os,
        "target_arch": target_arch,
        "compiler": compiler_identity(cc),
        "source_path": str(c_source),
        "source_sha256": sha256_file(c_source),
        "sdk_abi_header": str(PINNED_ABI_HEADER.relative_to(REPO_ROOT)),
        "sdk_abi_header_sha256": actual_sdk_sha,
        "archive_path": str(out_archive),
        "archive_sha256": sha256_bytes(bytes(archive_bytes)),
        "content_digests": content_digests,
        "provenance_tier": "reviewed_development_test",
        "authorization_sha256": auth_sha,
    }
    out_receipt.parent.mkdir(parents=True, exist_ok=True)
    out_receipt.write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return receipt


def parse_version(raw: str) -> tuple[int, int, int]:
    parts = raw.split(".")
    if len(parts) != 3:
        raise argparse.ArgumentTypeError("version must be MAJOR.MINOR.PATCH")
    return int(parts[0]), int(parts[1]), int(parts[2])


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Build a reproducible .hexdevice native package.")
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--type-id", required=True)
    parser.add_argument("--display-name", required=True)
    parser.add_argument("--publisher-id", default="hex.factory")
    parser.add_argument("--device-kind", type=int, required=True, choices=[1, 2])
    parser.add_argument("--capabilities", type=int, required=True)
    parser.add_argument("--version", type=parse_version, default=(1, 0, 0))
    parser.add_argument("--state-version", type=int, default=1)
    parser.add_argument("--ui-version", type=int, default=1)
    parser.add_argument("--ui-schema-json", type=Path, default=None)
    parser.add_argument("--notice", default="Hex original native device fixture.")
    parser.add_argument("--auth-token", default="reviewed-test-fixture-token-v1")
    parser.add_argument("--reviewer-note", default="local reproducible dev/test package build")
    parser.add_argument("--expected-sdk-sha256", default=None)
    parser.add_argument("--out-archive", required=True, type=Path)
    parser.add_argument("--out-receipt", required=True, type=Path)
    args = parser.parse_args(argv)

    ui_schema = json.loads(args.ui_schema_json.read_text(encoding="utf-8")) if args.ui_schema_json else None
    try:
        receipt = build_package(
            c_source=args.source,
            type_id=args.type_id,
            display_name=args.display_name,
            publisher_id=args.publisher_id,
            device_kind=args.device_kind,
            capabilities=args.capabilities,
            version=args.version,
            state_version=args.state_version,
            ui_version=args.ui_version,
            ui_schema=ui_schema,
            notice_text=args.notice,
            auth_token=args.auth_token,
            reviewer_note=args.reviewer_note,
            out_archive=args.out_archive,
            out_receipt=args.out_receipt,
            expected_sdk_header_sha256=args.expected_sdk_sha256,
        )
    except Exception as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    print(json.dumps(receipt, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
