#!/usr/bin/env python3
"""Offline builder for the complete 11-device Hex factory `.hexdevice` library (#3004).

Compiles and packages all eleven native factory devices (7 audio effects + 4 instruments)
using the exact same `scripts/build_device_package.py` pipeline and pinned C ABI v1 header
as any third-party package, emitting deterministic `.hexdevice` archives, receipts, and a
`factory-library-manifest.json` index with zero network access.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT / "scripts"))
import build_device_package as pkg_builder  # noqa: E402

FIXTURES_DIR = REPO_ROOT / "third_party" / "hex-device-abi-qual" / "fixtures"

CAP_AUDIO_EFFECT = 1 << 0
CAP_INSTRUMENT = 1 << 1
CAP_PARAMETER_EVENTS = 1 << 2
CAP_NOTE_EVENTS = 1 << 3
CAP_ZERO_LATENCY = 1 << 4
CAP_STATE_PERSISTENCE = 1 << 5
CAP_NOTE_EXPRESSION = 1 << 6
CAP_CHANNEL_EXPRESSION = 1 << 7
CAP_BORROWED_RESOURCES = 1 << 8

EFFECT_BASE_CAPS = (
    CAP_AUDIO_EFFECT | CAP_PARAMETER_EVENTS | CAP_ZERO_LATENCY | CAP_STATE_PERSISTENCE
)
SYNTH_EXPR_CAPS = (
    CAP_INSTRUMENT
    | CAP_PARAMETER_EVENTS
    | CAP_NOTE_EVENTS
    | CAP_ZERO_LATENCY
    | CAP_STATE_PERSISTENCE
    | CAP_NOTE_EXPRESSION
    | CAP_CHANNEL_EXPRESSION
)
SINE_CAPS = (
    CAP_INSTRUMENT
    | CAP_PARAMETER_EVENTS
    | CAP_NOTE_EVENTS
    | CAP_ZERO_LATENCY
    | CAP_STATE_PERSISTENCE
)
SAMPLER_CAPS = SINE_CAPS | CAP_BORROWED_RESOURCES

FACTORY_DEVICES: list[dict[str, Any]] = [
    # 7 stock audio effects
    {
        "source": "utility_v1.c",
        "type_id": "hex.pkg.utility",
        "display_name": "utility",
        "filename": "hex.pkg.utility.hexdevice",
        "device_kind": 1,
        "capabilities": EFFECT_BASE_CAPS,
    },
    {
        "source": "saturator_v1.c",
        "type_id": "hex.pkg.saturator",
        "display_name": "saturator",
        "filename": "hex.pkg.saturator.hexdevice",
        "device_kind": 1,
        "capabilities": EFFECT_BASE_CAPS,
    },
    {
        "source": "compressor_v1.c",
        "type_id": "hex.pkg.compressor",
        "display_name": "compressor",
        "filename": "hex.pkg.compressor.hexdevice",
        "device_kind": 1,
        "capabilities": EFFECT_BASE_CAPS,
    },
    {
        "source": "gate_v1.c",
        "type_id": "hex.pkg.gate",
        "display_name": "gate",
        "filename": "hex.pkg.gate.hexdevice",
        "device_kind": 1,
        "capabilities": EFFECT_BASE_CAPS,
    },
    {
        "source": "auto_pan_v1.c",
        "type_id": "hex.pkg.auto_pan",
        "display_name": "auto pan",
        "filename": "hex.pkg.auto_pan.hexdevice",
        "device_kind": 1,
        "capabilities": EFFECT_BASE_CAPS,
    },
    {
        "source": "eq_v1.c",
        "type_id": "hex.pkg.eq",
        "display_name": "eq",
        "filename": "hex.pkg.eq.hexdevice",
        "device_kind": 1,
        "capabilities": EFFECT_BASE_CAPS,
    },
    {
        "source": "eq8_v1.c",
        "type_id": "hex.pkg.eq8",
        "display_name": "eq8",
        "filename": "hex.pkg.eq8.hexdevice",
        "device_kind": 1,
        "capabilities": EFFECT_BASE_CAPS,
    },
    # 4 stock instruments
    {
        "source": "sine_v1.c",
        "type_id": "hex.pkg.sine",
        "display_name": "sine",
        "filename": "hex.pkg.sine.hexdevice",
        "device_kind": 2,
        "capabilities": SINE_CAPS,
    },
    {
        "source": "operator_v1.c",
        "type_id": "hex.pkg.operator",
        "display_name": "vector",
        "filename": "hex.pkg.operator.hexdevice",
        "device_kind": 2,
        "capabilities": SYNTH_EXPR_CAPS,
    },
    {
        "source": "fm4_v1.c",
        "type_id": "hex.pkg.fm4",
        "display_name": "fm4",
        "filename": "hex.pkg.fm4.hexdevice",
        "device_kind": 2,
        "capabilities": SYNTH_EXPR_CAPS,
    },
    {
        "source": "sampler_v1.c",
        "type_id": "hex.pkg.sampler",
        "display_name": "sampler",
        "filename": "hex.pkg.sampler.hexdevice",
        "device_kind": 2,
        "capabilities": SAMPLER_CAPS,
    },
]


def parse_version(raw: str) -> tuple[int, int, int]:
    parts = [int(p) for p in raw.split(".")]
    if len(parts) != 3:
        raise ValueError(f"expected major.minor.patch, got {raw!r}")
    return (parts[0], parts[1], parts[2])


def build_factory_library(
    *,
    out_dir: Path,
    version: tuple[int, int, int],
    auth_token: str,
    publisher_id: str = "hex.factory",
    cc: str = "clang",
) -> dict[str, Any]:
    out_dir.mkdir(parents=True, exist_ok=True)
    receipts_dir = out_dir / "receipts"
    receipts_dir.mkdir(parents=True, exist_ok=True)
    entries: list[dict[str, Any]] = []
    for spec in FACTORY_DEVICES:
        archive_path = out_dir / spec["filename"]
        receipt_path = receipts_dir / f"{spec['type_id']}.receipt.json"
        receipt = pkg_builder.build_package(
            c_source=FIXTURES_DIR / spec["source"],
            type_id=spec["type_id"],
            display_name=spec["display_name"],
            publisher_id=publisher_id,
            device_kind=spec["device_kind"],
            capabilities=spec["capabilities"],
            version=version,
            state_version=1,
            ui_version=1,
            ui_schema=None,
            notice_text=f"hex offline factory package: {spec['display_name']}",
            auth_token=auth_token,
            reviewer_note="offline factory library build (#3004)",
            out_archive=archive_path,
            out_receipt=receipt_path,
            cc=cc,
        )
        entries.append(
            {
                "type_id": spec["type_id"],
                "display_name": spec["display_name"],
                "device_kind": spec["device_kind"],
                "filename": spec["filename"],
                "version": f"{version[0]}.{version[1]}.{version[2]}",
                "package_sha256": receipt["archive_sha256"],
            }
        )
    index = {
        "schema_version": 1,
        "publisher_id": publisher_id,
        "factory_count": len(entries),
        "packages": entries,
    }
    (out_dir / "factory-library-manifest.json").write_text(
        json.dumps(index, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return index


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Build offline 11-device Hex factory .hexdevice library")
    parser.add_argument("--out-dir", type=Path, required=True)
    parser.add_argument("--version", default="1.0.0")
    parser.add_argument("--auth-token", required=True)
    parser.add_argument("--publisher-id", default="hex.factory")
    parser.add_argument("--cc", default="clang")
    args = parser.parse_args(argv)
    index = build_factory_library(
        out_dir=args.out_dir,
        version=parse_version(args.version),
        auth_token=args.auth_token,
        publisher_id=args.publisher_id,
        cc=args.cc,
    )
    print(f"built {index['factory_count']} offline factory packages in {args.out_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
