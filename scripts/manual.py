#!/usr/bin/env python3
"""Versioned offline user manuals; Markdown/CSS/Typst are documentation only.

The repository owns content and rendering inputs. Pinned Pandoc/Typst CLI tools
produce self-contained HTML and a PDF using bundled Ioskeley Mono. No application
framework, script runtime, network asset or sibling repository is a dependency.
Builds stage a complete input snapshot and publish create-only output directories;
the manifest binds source, tool binaries, fonts, content and final output bytes.
"""
from __future__ import annotations

import argparse
import hashlib
from html.parser import HTMLParser
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import zipfile


class Refusal(ValueError):
    pass


# Same pinned release tools used by plugin-release, independently installed here.
TOOLS = {
    "pandoc": ("3.12", "https://github.com/jgm/pandoc/releases/download/3.12/pandoc-3.12-arm64-macOS.zip",
               "f148ca09c9f36594db527a9fc988ad736290ce428f79594c50208cd1ec58b3c0", "pandoc-3.12-arm64/bin/pandoc"),
    "typst": ("0.15.1", "https://github.com/typst/typst/releases/download/v0.15.1/typst-aarch64-apple-darwin.tar.xz",
              "48f62ed034aa3a7978309579ac6ca00045e2ef0da73114e8af27cfd8e74dc05a", "typst-aarch64-apple-darwin/typst"),
}


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def encode(value):
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()


def run(argv, directory):
    result = subprocess.run([str(a) for a in argv], cwd=directory, capture_output=True, text=True)
    if result.returncode:
        raise Refusal(f"documentation tool failed: {argv[0]}: {result.stderr[-4000:]}")
    return result.stdout.strip()


def setup(directory):
    """Install only hash-verified pinned native-arm64 documentation executables."""
    if os.uname().sysname != "Darwin" or os.uname().machine != "arm64":
        raise Refusal("automatic documentation tool setup currently supports macOS arm64")
    directory.mkdir(parents=True, exist_ok=True)
    result = {}
    for name, (version, url, digest, member) in TOOLS.items():
        destination = directory / (name + "-" + version)
        receipt = destination.with_suffix(destination.suffix + ".json")
        if destination.is_symlink() or receipt.is_symlink():
            raise Refusal("documentation tools must be regular files")
        try:
            proof = json.loads(receipt.read_bytes())
            installed = (destination.is_file() and proof.get("archive_sha256") == digest
                         and proof.get("executable_sha256") == sha(destination))
        except (FileNotFoundError, ValueError, TypeError, AttributeError):
            installed = False
        if not installed:
            with tempfile.TemporaryDirectory(prefix=".doc-tool-", dir=directory) as temporary:
                stage = Path(temporary)
                archive = stage / "download"
                run(["curl", "--fail", "--location", "--proto", "=https", "--tlsv1.2",
                     "--output", archive, url], stage)
                if sha(archive) != digest:
                    raise Refusal("documentation tool archive hash mismatch: " + name)
                # Read one exact named member; never extract paths from an archive.
                if name == "pandoc":
                    with zipfile.ZipFile(archive) as source:
                        raw = source.read(member)
                else:
                    with tarfile.open(archive, "r:xz") as source:
                        entry = source.getmember(member)
                        if not entry.isfile() or entry.size > 256 * 1024 * 1024:
                            raise Refusal("invalid pinned documentation executable")
                        with source.extractfile(entry) as executable:
                            raw = executable.read()
                binary = stage / name
                binary.write_bytes(raw)
                binary.chmod(0o755)
                proof = {"name": name, "version": version, "url": url,
                         "archive_sha256": digest, "executable_sha256": sha(binary)}
                # Recover a crash between executable and receipt publication by
                # re-verifying the pinned archive and comparing the existing
                # binary. Never overwrite an unknown/different executable.
                if destination.exists():
                    if not destination.is_file() or sha(destination) != sha(binary):
                        raise Refusal("existing documentation executable differs from pinned download")
                else:
                    os.link(binary, destination)
                temporary_receipt = stage / "receipt.json"
                with temporary_receipt.open("wb") as output:
                    output.write(encode(proof))
                    output.flush()
                    os.fsync(output.fileno())
                os.replace(temporary_receipt, receipt)
        proof = json.loads(receipt.read_bytes())
        if proof.get("archive_sha256") != digest or proof.get("executable_sha256") != sha(destination):
            raise Refusal("documentation tool differs from verified installation: " + name)
        actual = run([destination, "--version"], directory).splitlines()[0]
        if not actual.startswith(name + " " + version) or (len(actual) > len(name + " " + version)
                                                         and actual[len(name + " " + version)] not in " (\t"):
            raise Refusal("documentation tool version mismatch: " + actual)
        result[name] = {"path": str(destination.resolve()), "sha256": sha(destination), "version": version}
    return result


def inputs(root):
    files = list((root / "docs/manual").rglob("*"))
    files += list((root / "assets/fonts/ioskeley-mono").glob("*.ttf"))
    files += [root / "assets/fonts/ioskeley-mono/OFL.txt"]
    if any(p.is_symlink() for p in files):
        raise Refusal("manual inputs cannot be symlinks")
    return {p.relative_to(root).as_posix(): sha(p) for p in sorted(files) if p.is_file()}


def require_source(root, source):
    if (run(["git", "rev-parse", "HEAD"], root) != source
            or run(["git", "status", "--porcelain", "--untracked-files=normal"], root)):
        raise Refusal("commit the clean exact manual source before rendering qualified documentation")


class OfflineHtml(HTMLParser):
    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag in {"script", "iframe", "object", "embed", "audio", "video", "base", "link"}:
            raise Refusal("offline manual contains a script or external/embedded document")
        if any(key.startswith("on") for key in attrs):
            raise Refusal("offline manual contains event handlers")
        if "srcset" in attrs or ("src" in attrs and not attrs["src"].startswith("data:")):
            raise Refusal("offline manual contains an external asset")


def validate(directory, version):
    raw = (directory / "hex-manual.html").read_text()
    OfflineHtml().feed(raw)
    if version not in raw:
        raise Refusal("manual HTML is missing its release version")
    for match in re.finditer(r"url\(\s*['\"]?([^)'\"]+)", raw, re.I):
        if not match[1].startswith("data:"):
            raise Refusal("manual CSS contains an external asset")
    pdf = (directory / "hex-manual.pdf").read_bytes()
    if not pdf.startswith(b"%PDF-") or b"%%EOF" not in pdf[-1024:]:
        raise Refusal("invalid manual PDF")
    fonts = re.findall(rb"/(?:BaseFont|FontName)\s*/([^\s>/]+)", pdf)
    if not fonts or any(re.fullmatch(rb"(?:[A-Z]{6}\+)?Ioskeley(?:-Mono|Mono)(?:-(?:Bold|Medium|Italic))?", font) is None
                        for font in fonts):
        raise Refusal("manual PDF contains a missing or alternate font")


def verify(directory, version=None, source=None):
    manifest = json.loads((directory / "documentation.json").read_bytes())
    if manifest.get("schema") != 1 or (version and manifest["version"] != version) or (source and manifest["source"] != source):
        raise Refusal("manual provenance differs from requested release")
    if set(manifest["outputs"]) != {"hex-manual.html", "hex-manual.pdf"}:
        raise Refusal("manual output inventory differs")
    for name, digest in manifest["outputs"].items():
        if (directory / name).is_symlink() or sha(directory / name) != digest:
            raise Refusal("manual output hash mismatch: " + name)
    validate(directory, manifest["version"])
    return manifest


def build(root, destination, version, source, tools=None):
    """Render exact version/source-bound manuals and return their frozen manifest."""
    root, destination = Path(root).resolve(), Path(destination).absolute()
    if re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[a-z0-9.]+)?", version) is None or re.fullmatch(r"[0-9a-f]{40}", source) is None:
        raise Refusal("manual requires a release version and full source SHA")
    require_source(root, source)
    identity = inputs(root)
    tools = tools or setup(root / "artifacts/documentation-tools")
    if destination.exists():
        manifest = verify(destination, version, source)
        if manifest["inputs"] != identity or manifest["tools"] != tools:
            raise Refusal("manual inputs changed; previous version preserved")
        return manifest
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".manual-", dir=destination.parent) as temporary:
        stage = Path(temporary)
        for relative in identity:
            target = stage / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(root / relative, target)
        working = stage / "docs/manual"
        output = stage / "output"
        output.mkdir()
        metadata = ["--metadata", "title:hex manual", "--metadata", f"subtitle:{version} · {source[:8]}",
                    "--metadata", f"version:{version}", "--metadata", "lang:en"]
        run([tools["pandoc"]["path"], "manual.md", "--from", "markdown+smart", "--to", "html5", "--standalone",
             "--embed-resources", "--toc", "--css", "style.css", *metadata, "--output", output / "hex-manual.html"], working)
        run([tools["pandoc"]["path"], "manual.md", "--from", "markdown+smart", "--to", "typst", "--standalone",
             "--template", "template.typ", *metadata, "--output", "manual.generated.typ"], working)
        run([tools["typst"]["path"], "compile", "--creation-timestamp", "0", "--ignore-system-fonts",
             "--font-path", stage / "assets/fonts/ioskeley-mono", "--root", stage,
             "manual.generated.typ", output / "hex-manual.pdf"], working)
        if inputs(root) != identity:
            raise Refusal("manual source changed during rendering; no output published")
        require_source(root, source)
        validate(output, version)
        manifest = {"schema": 1, "version": version, "source": source, "inputs": identity, "tools": tools,
                    "outputs": {name: sha(output / name) for name in ("hex-manual.html", "hex-manual.pdf")}}
        (output / "documentation.json").write_bytes(encode(manifest))
        output.rename(destination)
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("build", "verify", "setup"))
    parser.add_argument("--version")
    parser.add_argument("--source")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    try:
        if args.operation == "setup":
            result = setup(root / "artifacts/documentation-tools")
        elif args.operation == "verify":
            result = verify(args.output, args.version, args.source)
        else:
            result = build(root, args.output, args.version, args.source)
        print(json.dumps(result, indent=2, sort_keys=True))
    except (Refusal, OSError, ValueError, KeyError, TypeError) as error:
        parser.exit(1, f"manual incomplete: {error}\n")


if __name__ == "__main__":
    main()
