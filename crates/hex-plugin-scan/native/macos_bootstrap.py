#!/usr/bin/env python3
"""Build/verify original Darwin arm64 fixtures; never scan installed plugins."""
import argparse
import hashlib
import json
import platform
import plistlib
import shlex
import shutil
import struct
import subprocess
from pathlib import Path

from build import PINS

NATIVE = Path(__file__).resolve().parent
ROOT = NATIVE.parents[2]
APP = Path("hex-vst3-host.app")
EXECUTABLE = APP / "Contents/MacOS/hex-vst3-host"
SOURCES = ("macos_bootstrap.py", "macos_helper.mm", "macos_fixture.cpp", "build.py")
MAX_FILE = 16 * 1024 * 1024


def sha(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def source_digest():
    h = hashlib.sha256()
    for name in SOURCES:
        h.update(name.encode() + b"\0" + (NATIVE / name).read_bytes() + b"\0")
    return h.hexdigest()


def command(args, **kwargs):
    return subprocess.check_output(args, text=True, timeout=120, **kwargs).strip()


def macho(path, filetype):
    if path.is_symlink() or not path.is_file() or path.stat().st_size > MAX_FILE:
        raise ValueError("missing, linked or oversized native binary: " + str(path))
    with path.open("rb") as f:
        header = f.read(32)
    if len(header) != 32:
        raise ValueError("truncated Mach-O binary")
    magic, cpu, subtype, kind, commands, size, _, reserved = struct.unpack("<8I", header)
    if (magic, cpu, subtype, kind, reserved) != (0xFEEDFACF, 0x0100000C, 0, filetype, 0):
        raise ValueError("expected native arm64 Mach-O with the declared binary role")
    if commands < 1 or commands > 4096 or size < commands * 8 or size > path.stat().st_size - 32:
        raise ValueError("invalid Mach-O command bounds")


def inventory(root):
    if root.is_symlink() or not root.is_dir():
        raise ValueError("missing or linked helper bundle")
    files = {}
    total = 0
    for index, path in enumerate(root.rglob("*")):
        if index >= 256:
            raise ValueError("too many bundle members")
        if path.is_symlink():
            raise ValueError("linked bundle member refused")
        if path.is_file():
            if path.stat().st_size > MAX_FILE:
                raise ValueError("oversized bundle member")
            total += path.stat().st_size
            if total > 32 * 1024 * 1024:
                raise ValueError("oversized bootstrap bundle")
            files[str(path.relative_to(root.parent))] = sha(path)
        elif not path.is_dir():
            raise ValueError("nonregular bundle member")
    return files


def verify(receipt, run=False):
    receipt = receipt.resolve()
    if receipt.stat().st_size > MAX_FILE:
        raise ValueError("oversized receipt")
    data = json.loads(receipt.read_text())
    if (data.get("version"), data.get("scope"), data.get("platform")) != (
        1, "original-macos-bootstrap", "darwin-arm64"
    ) or data.get("sdk_pins") != PINS or data.get("source_sha256") != source_digest():
        raise ValueError("unsupported or stale bootstrap receipt")
    root = receipt.parent
    actual = inventory(root / APP)
    if not actual or actual != data.get("bundle_files"):
        raise ValueError("missing or changed bundle contents")
    executable = root / EXECUTABLE
    macho(executable, 2)
    for name in ("effect", "instrument"):
        macho(root / APP / "Contents/Resources/fixtures" / (name + ".vst3") / "Contents/MacOS/fixture", 8)
    info = plistlib.loads((root / APP / "Contents/Info.plist").read_bytes())
    if info.get("CFBundleExecutable") != "hex-vst3-host" or info.get("CFBundleIdentifier") != "org.portalsurfer.hex.vst3.bootstrap":
        raise ValueError("unexpected helper package identity")
    # Verify immediately before and after each owned executable attempt.
    if run:
        if platform.system() != "Darwin" or platform.machine() != "arm64":
            raise ValueError("native execution requires Darwin arm64")
        if data.get("runtime", {}).get("os_build") != command(["/usr/bin/sw_vers", "-buildVersion"]):
            raise ValueError("runtime OS build changed since fixture build")
        command(["/usr/bin/codesign", "--verify", "--deep", "--strict", str(root / APP)])
        identity = json.loads(command([str(executable), "--identity"]))
        if identity != {"version": 1, "scope": data["scope"], "platform": data["platform"], "sdk": PINS["sdk"]}:
            raise ValueError("helper identity mismatch")
        verify(receipt)
        smoke = json.loads(command([str(executable), "--self-test"]))
        if smoke != {"version": 1, "scope": data["scope"], "effect_frames": 64, "instrument_frames": 64}:
            raise ValueError("helper fixture smoke mismatch")
        verify(receipt)
        return smoke
    return data


def build(sdk, output):
    if platform.system() != "Darwin" or platform.machine() != "arm64":
        raise ValueError("native bootstrap builds require Darwin arm64")
    sdk, output = sdk.resolve(), output.resolve()
    artifacts = (ROOT / "artifacts").resolve()
    if not output.is_relative_to(artifacts) or output == artifacts or output.is_relative_to(sdk):
        raise ValueError("output must be a fresh owned folder under this checkout's artifacts")
    licenses = {}
    for name, commit in PINS.items():
        path = sdk if name == "sdk" else sdk / name
        if command(["git", "rev-parse", "HEAD"], cwd=path) != commit:
            raise ValueError("SDK pin mismatch: " + name)
        if command(["git", "status", "--porcelain", "--untracked-files=all"], cwd=path):
            raise ValueError("SDK contains modified or untracked files: " + name)
        notice = path / "LICENSE.txt"
        if not notice.exists():
            notice = path / "LICENSE"
        text = notice.read_text()
        if "MIT License" not in text:
            raise ValueError("unreviewed SDK notice: " + name)
        licenses[name] = {"sha256": sha(notice), "text": text}
    output.mkdir(parents=True, exist_ok=False)
    compiler = Path(command(["xcrun", "--find", "clang++"]))
    apple_sdk = Path(command(["xcrun", "--sdk", "macosx", "--show-sdk-path"])).resolve()
    common = [str(compiler), "-std=c++17", "-O2", "-arch", "arm64", "-mmacosx-version-min=13.0",
              "-isysroot", str(apple_sdk), "-I" + str(sdk), "-Wall", "-Wextra", "-Werror",
              "-ffile-prefix-map=" + str(ROOT) + "=hex-source"]
    commands, closure, runtime = [], {}, {}

    def compile_one(source, destination, extra):
        destination.parent.mkdir(parents=True, exist_ok=True)
        dep = output / (destination.parent.parent.parent.name + "-" + source + ".d")
        args = common + ["-MD", "-MF", str(dep), str(NATIVE / source), "-o", str(destination)] + extra
        commands.append(args)
        subprocess.run(args, check=True, timeout=120)
        # Clang's Make dependency output escapes whitespace with backslashes.
        dependencies = shlex.split(dep.read_text().replace("\\\n", " ").split(":", 1)[1])
        for name in dependencies:
            path = Path(name).resolve()
            roots = [(sdk, "vst-sdk"), (NATIVE, "native"), (output, "generated"),
                     (apple_sdk, "apple-sdk"), (compiler.parent.parent, "apple-toolchain")]
            owner = next(((base, label) for base, label in roots if path.is_relative_to(base)), None)
            if owner is None:
                raise ValueError("unreviewed compiler input: " + str(path))
            base, label = owner
            closure[label + "/" + str(path.relative_to(base))] = sha(path)
        macho(destination, 8 if "-bundle" in extra else 2)
        # Ad hoc signatures qualify local original fixtures only, not distribution.
        subprocess.run(["/usr/bin/codesign", "--force", "--sign", "-", "--timestamp=none", str(destination.parent.parent.parent)], check=True)
        runtime[str(destination.relative_to(output))] = command(["/usr/bin/otool", "-L", str(destination)]).splitlines()[1:]

    resources = output / APP / "Contents/Resources"
    digests = {}
    for instrument, name in enumerate(("effect", "instrument")):
        bundle = resources / "fixtures" / (name + ".vst3")
        (bundle / "Contents").mkdir(parents=True)
        info = {"CFBundleExecutable": "fixture", "CFBundleIdentifier": "org.portalsurfer.hex.fixture." + name,
                "CFBundlePackageType": "BNDL", "CFBundleVersion": "1"}
        (bundle / "Contents/Info.plist").write_bytes(plistlib.dumps(info, sort_keys=True))
        binary = bundle / "Contents/MacOS/fixture"
        compile_one("macos_fixture.cpp", binary, ["-bundle", "-framework", "CoreFoundation", "-DHEX_INSTRUMENT=" + str(instrument)])
        digests[name] = sha(binary)
    header = output / "fixture_identity.hpp"
    header.write_text('#define HEX_SDK_PIN "' + PINS["sdk"] + '"\n' + "".join(
        '#define HEX_' + name.upper() + '_SHA256 "' + digest + '"\n' for name, digest in digests.items()))
    (output / APP / "Contents/Info.plist").write_bytes(plistlib.dumps({
        "CFBundleExecutable": "hex-vst3-host", "CFBundleIdentifier": "org.portalsurfer.hex.vst3.bootstrap",
        "CFBundlePackageType": "APPL", "CFBundleVersion": "1", "LSBackgroundOnly": True,
    }, sort_keys=True))
    # CommonCrypto's legacy one-shot function remains available for local identity.
    compile_one("macos_helper.mm", output / EXECUTABLE, ["-I" + str(output), "-framework", "Foundation", "-Wno-deprecated-declarations"])
    (resources / "sdk-notices.json").write_text(json.dumps(licenses, indent=2) + "\n")
    subprocess.run(["/usr/bin/codesign", "--force", "--sign", "-", "--timestamp=none", str(output / APP)], check=True)
    receipt = {"version": 1, "scope": "original-macos-bootstrap", "platform": "darwin-arm64",
               "sdk_pins": PINS, "source_sha256": source_digest(),
               "source_commit": command(["git", "rev-parse", "HEAD"], cwd=ROOT),
               "source_dirty": bool(command(["git", "status", "--porcelain"], cwd=ROOT)),
               "compiler": {"path": str(compiler), "sha256": sha(compiler), "version": command([str(compiler), "--version"])},
               "build_tools": {name: {"path": path, "sha256": sha(Path(path))} for name, path in
                               [("linker", command(["xcrun", "--find", "ld"])),
                                ("codesign", "/usr/bin/codesign"), ("otool", "/usr/bin/otool")]},
               "apple_sdk": {"path": str(apple_sdk), "version": command(["xcrun", "--sdk", "macosx", "--show-sdk-version"]),
                             "settings_sha256": sha(apple_sdk / "SDKSettings.json")},
               "runtime": {"os_build": command(["/usr/bin/sw_vers", "-buildVersion"]), "dependencies": runtime,
                           "qualification": "system runtime install names and OS build; shared-cache images are not individually hashed or compatibility-qualified"},
               "build_commands": commands, "build_closure": closure, "bundle_files": inventory(output / APP),
               "signing": "ad-hoc, local original fixtures only; no entitlements or notarization"}
    path = output / "controlled-macos.json"
    path.write_text(json.dumps(receipt, indent=2) + "\n")
    verify(path, run=True)
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="mode", required=True)
    builder = commands.add_parser("build")
    builder.add_argument("--sdk", type=Path, required=True)
    builder.add_argument("--output", type=Path, required=True)
    checker = commands.add_parser("verify")
    checker.add_argument("--receipt", type=Path, required=True)
    checker.add_argument("--run", action="store_true")
    args = parser.parse_args()
    try:
        result = build(args.sdk, args.output) if args.mode == "build" else verify(args.receipt, args.run)
        print(result if isinstance(result, Path) else json.dumps({"verified": True, "scope": result["scope"]}))
    except (ValueError, OSError, KeyError, subprocess.SubprocessError) as error:
        parser.exit(1, "controlled macOS bootstrap refused: " + str(error) + "\n")


if __name__ == "__main__":
    main()
