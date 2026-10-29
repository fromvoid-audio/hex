"""Read-only macOS Cargo target volume identification, before cache mutation.

Probe the nearest existing ancestor for a not-yet-created target. Resolve its
device with df, then read diskutil's structured filesystem metadata. Neither
tool writes the volume, moves executables, changes permissions or owns cleanup.
Other platforms retain the existing runner behavior, without a storage claim.
"""

from pathlib import Path
import plistlib
import subprocess
import sys
import time
from xml.parsers.expat import ExpatError


def inspect(target: Path, env: dict[str, str]) -> dict:
    observation = {"platform": sys.platform, "target_directory": str(target)}
    if sys.platform != "darwin":
        return {**observation, "status": "not_checked"}
    started = time.monotonic()
    ancestor = target
    while not ancestor.exists() and ancestor != ancestor.parent:
        ancestor = ancestor.parent
    try:
        df = subprocess.run(["df", "-P", str(ancestor)], env=env,
                            capture_output=True, check=False, timeout=5)
        lines = df.stdout.decode("utf-8").splitlines()
        device = lines[1].split()[0] if df.returncode == 0 and len(lines) == 2 else ""
        if not device.startswith("/dev/"):
            raise ValueError("df did not identify a local target device")
        result = subprocess.run(["diskutil", "info", "-plist", device], env=env,
                                capture_output=True, check=False, timeout=5)
        metadata = plistlib.loads(result.stdout) if result.returncode == 0 else {}
        filesystem = metadata.get("FilesystemType") if isinstance(metadata, dict) else None
        if not isinstance(filesystem, str) or not filesystem.strip():
            raise ValueError("diskutil did not identify the target filesystem")
    except (OSError, ValueError, IndexError, ExpatError, subprocess.TimeoutExpired) as error:
        raise ValueError(f"cannot identify macOS Cargo target storage: {error}") from error
    return {**observation, "status": "identified", "probe_path": str(ancestor),
            "device": device, "filesystem_type": filesystem.strip().lower(),
            "duration_seconds": round(time.monotonic() - started, 6)}
