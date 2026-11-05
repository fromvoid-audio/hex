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
INSTANCE_APP = Path("hex-vst3-instance.app")
INSTANCE_EXECUTABLE = INSTANCE_APP / "Contents/MacOS/hex-vst3-instance"
SOURCES = ("macos_bootstrap.py", "macos_helper.mm", "macos_fixture.cpp", "build.py",
           "macos_scan.mm", "macos_scan_wire.hpp", "macos_factory_fixture.cpp", "launch_ignored.cpp",
           "macos_instance.hpp", "macos_lifecycle_fixture.cpp", "macos_lifecycle_case.hpp",
           "macos_host_messages.hpp", "macos_host.hpp", "macos_parameters.hpp", "macos_parameter_page.hpp", "macos_host_case.hpp", "macos_stream_case.hpp",
           "macos_processing.hpp", "macos_processing_fixture.cpp", "macos_processing_case.hpp", "macos_bundle.hpp", "macos_state_stream.hpp", "macos_instance_wire.hpp", "macos_input_notes.hpp", "macos_editor.hpp", "macos_instance_worker.mm")
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


def macho(path, filetype, architecture="arm64"):
    if path.is_symlink() or not path.is_file() or path.stat().st_size > MAX_FILE:
        raise ValueError("missing, linked or oversized native binary: " + str(path))
    with path.open("rb") as f:
        header = f.read(32)
    if len(header) != 32:
        raise ValueError("truncated Mach-O binary")
    magic, cpu, subtype, kind, commands, size, _, reserved = struct.unpack("<8I", header)
    expected_cpu, expected_subtype = (0x0100000C, 0) if architecture == "arm64" else (0x01000007, 3)
    if (magic, cpu, subtype, kind, reserved) != (0xFEEDFACF, expected_cpu, expected_subtype, filetype, 0):
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


def verify(receipt, run=False, expected_source_digest=None, expected_sdk_pins=None):
    receipt = receipt.resolve()
    if receipt.stat().st_size > MAX_FILE:
        raise ValueError("oversized receipt")
    data = json.loads(receipt.read_text())
    instance = data.get("scope") == "native-macos-instance"
    minimal = data.get("scope") == "native-macos-factory-discovery" or instance
    app = INSTANCE_APP if instance else APP
    if (data.get("version") != 1 or data.get("platform") != "darwin-arm64"
            or data.get("scope") not in ("original-macos-bootstrap", "native-macos-factory-discovery", "native-macos-instance")
            or data.get("sdk_pins") != (PINS if expected_sdk_pins is None else expected_sdk_pins)
            or data.get("source_sha256") != (source_digest() if expected_source_digest is None else expected_source_digest)):
        raise ValueError("unsupported or stale bootstrap receipt")
    root = receipt.parent
    if data.get("package_layout") is not None:
        expected_layout = "hex-app-instance-v1" if instance else "hex-app-discovery-v1"
        expected_receipt = "vst3-instance.json" if instance else "vst3-discovery.json"
        if (data["package_layout"] != expected_layout or not minimal
                or receipt.name != expected_receipt or root.name != "Resources"
                or root.parent.name != "Contents"):
            raise ValueError("unknown discovery package layout")
        root = root.parent / "Helpers"
    actual = inventory(root / app)
    if not actual or actual != data.get("bundle_files"):
        raise ValueError("missing or changed bundle contents")
    primary = INSTANCE_EXECUTABLE if instance else APP / "Contents/MacOS/hex-vst3-factory-scan"
    macho(root / primary, 2)
    info = plistlib.loads((root / app / "Contents/Info.plist").read_bytes())
    if minimal:
        name = "hex-vst3-instance" if instance else "hex-vst3-factory-scan"
        allowed = {str(app / member) for member in (
            "Contents/Info.plist", "Contents/MacOS/" + name,
            "Contents/Resources/sdk-notices.json", "Contents/_CodeSignature/CodeResources")}
        if (set(actual) != allowed or data.get("factory_fixtures") != []
                or data.get("factory_scan_fault_helper") is not None
                or (instance and (data.get("instance_helper") != str(INSTANCE_EXECUTABLE)
                                  or data.get("factory_scan_helper") is not None))):
            raise ValueError("minimal helper bundle contains unexpected or fault assets")
        expected_id = "org.portalsurfer.hex.vst3.instance" if instance else "org.portalsurfer.hex.vst3.discovery"
        if info.get("CFBundleExecutable") != name or info.get("CFBundleIdentifier") != expected_id:
            raise ValueError("unexpected helper package identity")
        if run:
            raise ValueError("instance helper requires owned duplex protocol" if instance else
                             "discovery helper requires owned factory protocol; use scanner fixture qualification")
        return data
    executable = root / EXECUTABLE
    macho(executable, 2)
    for name in ("effect", "instrument", "lifecycle", "processing"):
        fixture = root / APP / "Contents/Resources/fixtures" / (name + ".vst3")
        info = plistlib.loads((fixture / "Contents/Info.plist").read_bytes())
        if info.get("CFBundleExecutable") != "fixture" or info.get("CFBundleIdentifier") != "org.portalsurfer.hex.fixture." + name:
            raise ValueError("unexpected fixture executable selection or identity")
        macho(fixture / "Contents/MacOS/fixture", 8)
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


def build(sdk, output, *, discovery_only=False, instance_only=False):
    if discovery_only and instance_only:
        raise ValueError("choose one minimal helper role")
    app = INSTANCE_APP if instance_only else APP
    minimal = discovery_only or instance_only
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

    def compile_one(source, destination, extra, architecture="arm64"):
        destination.parent.mkdir(parents=True, exist_ok=True)
        dep = output / (destination.parent.parent.parent.name + "-" + source + ".d")
        flags = common.copy()
        flags[flags.index("-arch") + 1] = architecture
        args = flags + ["-MD", "-MF", str(dep), str(NATIVE / source), "-o", str(destination)] + extra
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
        macho(destination, 8 if "-bundle" in extra else 6 if "-dynamiclib" in extra else 2, architecture)
        # Ad hoc signatures qualify local original fixtures only, not distribution.
        subprocess.run(["/usr/bin/codesign", "--force", "--sign", "-", "--timestamp=none", str(destination if "-dynamiclib" in extra else destination.parent.parent.parent)], check=True, timeout=120)
        runtime[str(destination.relative_to(output))] = command(["/usr/bin/otool", "-L", str(destination)]).splitlines()[1:]

    resources = output / app / "Contents/Resources"
    resources.mkdir(parents=True, exist_ok=True)
    digests, factory_fixtures = {}, []
    if not minimal:
        for instrument, name, source in ((0, "effect", "macos_fixture.cpp"),
                                         (1, "instrument", "macos_fixture.cpp"),
                                         (0, "lifecycle", "macos_lifecycle_fixture.cpp"),
                                         (0, "processing", "macos_processing_fixture.cpp"),
                                         (0, "tail", "macos_processing_fixture.cpp"),
                                         (0, "infinite", "macos_processing_fixture.cpp")):
            bundle = resources / "fixtures" / (name + ".vst3")
            (bundle / "Contents").mkdir(parents=True)
            info = {"CFBundleExecutable": "fixture", "CFBundleIdentifier": "org.portalsurfer.hex.fixture." + name,
                    "CFBundlePackageType": "BNDL", "CFBundleVersion": "1"}
            (bundle / "Contents/Info.plist").write_bytes(plistlib.dumps(info, sort_keys=True))
            binary = bundle / "Contents/MacOS/fixture"
            tail_flags = (["-DHEX_FIXTURE_RESIDUAL_ECHO=1"] if name == "tail" else
                          ["-DHEX_FIXTURE_INFINITE_TAIL=1"] if name == "infinite" else [])
            compile_one(source, binary, ["-bundle", "-framework", "CoreFoundation", "-DHEX_INSTRUMENT=" + str(instrument), *tail_flags])
            digests[name] = sha(binary)
        modes = ["normal", "entry-hang", "entry-crash", "enumerate-hang", "malformed-count", "malformed-string",
                 "duplicate-uid", "exit-fail", "exit-hang", "exit-crash", "flood", "open-channel-child",
                 "leave-group-hang", "enumerate-crash", "load-hang", "load-crash",
                 "factory2-instrument", "factory2-effect", "factory2-dual", "factory2-unknown",
                 "factory2-mismatch", "factory2-malformed", "factory2-query-fail", "factory2-invalid-utf8",
                 "factory2-class-query-fail", "factory2-null-interface"]

        def factory_bundle(name):
            bundle = resources / "factory-fixtures" / (name + ".vst3")
            (bundle / "Contents/MacOS").mkdir(parents=True)
            (bundle / "Contents/Info.plist").write_bytes(plistlib.dumps({
                "CFBundleExecutable": "fixture", "CFBundleIdentifier": "org.portalsurfer.hex.factory-fixture." + name,
                "CFBundlePackageType": "BNDL", "CFBundleVersion": "1"}, sort_keys=True))
            return bundle

        for mode, name in enumerate(modes):
            bundle = factory_bundle(name)
            compile_one("macos_factory_fixture.cpp", bundle / "Contents/MacOS/fixture",
                        ["-bundle", "-framework", "CoreFoundation", "-DHEX_FACTORY_MODE=" + str(mode)])
            factory_fixtures.append({"name": name, "bundle": str(bundle.relative_to(output))})
        wrong = factory_bundle("wrong-architecture")
        compile_one("macos_factory_fixture.cpp", wrong / "Contents/MacOS/fixture",
                    ["-bundle", "-framework", "CoreFoundation"], architecture="x86_64")
        factory_fixtures.append({"name": "wrong-architecture", "bundle": str(wrong.relative_to(output))})
        universal = factory_bundle("universal")
        normal = resources / "factory-fixtures/normal.vst3/Contents/MacOS/fixture"
        args = ["xcrun", "lipo", "-create", str(normal), str(wrong / "Contents/MacOS/fixture"), "-output", str(universal / "Contents/MacOS/fixture")]
        commands.append(args)
        subprocess.run(args, check=True, timeout=120)
        subprocess.run(["/usr/bin/codesign", "--force", "--sign", "-", "--timestamp=none", str(universal)], check=True, timeout=120)
        if set(command(["xcrun", "lipo", "-archs", str(universal / "Contents/MacOS/fixture")]).split()) != {"arm64", "x86_64"}:
            raise ValueError("universal fixture architectures changed")
        runtime[str((universal / "Contents/MacOS/fixture").relative_to(output))] = command(["/usr/bin/otool", "-L", str(universal / "Contents/MacOS/fixture")]).splitlines()[1:]
        factory_fixtures.append({"name": "universal", "bundle": str(universal.relative_to(output))})
    header = output / "fixture_identity.hpp"
    header.write_text('#define HEX_SDK_PIN "' + PINS["sdk"] + '"\n' +
                      '#define HEX_PUBLIC_SDK_PIN "' + PINS["public.sdk"] + '"\n' + "".join(
        '#define HEX_' + name.upper() + '_SHA256 "' + digest + '"\n' for name, digest in digests.items()))
    (output / app / "Contents/Info.plist").write_bytes(plistlib.dumps({
        "CFBundleExecutable": "hex-vst3-instance" if instance_only else ("hex-vst3-factory-scan" if discovery_only else "hex-vst3-host"),
        "CFBundleIdentifier": "org.portalsurfer.hex.vst3.instance" if instance_only else ("org.portalsurfer.hex.vst3.discovery" if discovery_only else "org.portalsurfer.hex.vst3.bootstrap"),
        "CFBundlePackageType": "APPL", "CFBundleVersion": "1",
        ("LSUIElement" if instance_only else "LSBackgroundOnly"): True,
    }, sort_keys=True))
    # CommonCrypto's legacy one-shot function remains available for local identity.
    if not minimal:
        compile_one("macos_helper.mm", output / EXECUTABLE, ["-I" + str(output), "-framework", "Foundation", "-Wno-deprecated-declarations"])
    scan_executable = APP / "Contents/MacOS/hex-vst3-factory-scan"
    if instance_only:
        compile_one("macos_instance_worker.mm", output / INSTANCE_EXECUTABLE, ["-framework", "AppKit", "-framework", "Foundation", "-Wno-deprecated-declarations"])
    else:
        compile_one("macos_scan.mm", output / scan_executable, ["-I" + str(output), "-framework", "Foundation", "-Wno-deprecated-declarations"])
    reply_hang = APP / "Contents/MacOS/hex-vst3-factory-scan-reply-hang"
    if not minimal:
        compile_one("macos_scan.mm", output / reply_hang,
                    ["-I" + str(output), "-framework", "Foundation", "-Wno-deprecated-declarations", "-DHEX_SCAN_HANG_AFTER_REPLY=1"])
        compile_one("launch_ignored.cpp", output / app / "Contents/MacOS/hex-vst3-launch-ignored", [])
        compile_one("launch_ignored.cpp", output / app / "Contents/MacOS/hex-vst3-autoreap-driver.dylib",
                    ["-dynamiclib", "-DHEX_LAUNCH_IGNORED_DYLIB=1"])
    (resources / "sdk-notices.json").write_text(json.dumps(licenses, indent=2) + "\n")
    subprocess.run(["/usr/bin/codesign", "--force", "--sign", "-", "--timestamp=none", str(output / app)], check=True, timeout=120)
    receipt = {"version": 1, "scope": "native-macos-instance" if instance_only else ("native-macos-factory-discovery" if discovery_only else "original-macos-bootstrap"), "platform": "darwin-arm64",
               "sdk_pins": PINS, "source_sha256": source_digest(), "factory_scan_helper": None if instance_only else str(scan_executable),
               "instance_helper": str(INSTANCE_EXECUTABLE) if instance_only else None,
               "factory_fixtures": factory_fixtures,
               "factory_scan_fault_helper": None if minimal else str(reply_hang),
               "source_commit": command(["git", "rev-parse", "HEAD"], cwd=ROOT),
               "source_dirty": bool(command(["git", "status", "--porcelain"], cwd=ROOT)),
               "compiler": {"path": str(compiler), "sha256": sha(compiler), "version": command([str(compiler), "--version"])},
               "build_tools": {name: {"path": path, "sha256": sha(Path(path))} for name, path in
                               [("linker", command(["xcrun", "--find", "ld"])),
                                ("codesign", "/usr/bin/codesign"), ("otool", "/usr/bin/otool"),
                                ("lipo", command(["xcrun", "--find", "lipo"]))]},
               "apple_sdk": {"path": str(apple_sdk), "version": command(["xcrun", "--sdk", "macosx", "--show-sdk-version"]),
                             "settings_sha256": sha(apple_sdk / "SDKSettings.json")},
               "runtime": {"os_build": command(["/usr/bin/sw_vers", "-buildVersion"]), "dependencies": runtime,
                           "qualification": "system runtime install names and OS build; shared-cache images are not individually hashed or compatibility-qualified"},
               "build_commands": commands, "build_closure": closure, "bundle_files": inventory(output / app),
               "signing": "ad-hoc local helper build; no entitlements or notarization"}
    path = output / "controlled-macos.json"
    path.write_text(json.dumps(receipt, indent=2) + "\n")
    verify(path, run=not minimal)
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="mode", required=True)
    builder = commands.add_parser("build")
    roles = builder.add_mutually_exclusive_group()
    roles.add_argument("--discovery-only", action="store_true", help="minimal deployable factory scanner, without bootstrap/fault fixtures")
    roles.add_argument("--instance-only", action="store_true", help="minimal source-bound ordinary-instance worker, without discovery/fault fixtures")
    builder.add_argument("--sdk", type=Path, required=True)
    builder.add_argument("--output", type=Path, required=True)
    checker = commands.add_parser("verify")
    checker.add_argument("--receipt", type=Path, required=True)
    checker.add_argument("--run", action="store_true")
    args = parser.parse_args()
    try:
        result = build(args.sdk, args.output, discovery_only=args.discovery_only, instance_only=args.instance_only) if args.mode == "build" else verify(args.receipt, args.run)
        print(result if isinstance(result, Path) else json.dumps({"verified": True, "scope": result["scope"]}))
    except (ValueError, OSError, KeyError, subprocess.SubprocessError) as error:
        parser.exit(1, "controlled macOS bootstrap refused: " + str(error) + "\n")


if __name__ == "__main__":
    main()
