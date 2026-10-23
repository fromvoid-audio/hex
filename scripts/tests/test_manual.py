"""Offline documentation integrity; real renderer output is a release-cut gate."""
import importlib.util
import hashlib
import io
from pathlib import Path
import tempfile
import subprocess
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zipfile

SPEC = importlib.util.spec_from_file_location("manual_fixture", Path(__file__).resolve().parents[1] / "manual.py")
manual = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(manual)


class ManualTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        (self.root / "hex-manual.pdf").write_bytes(b"%PDF-1.7\n/BaseFont /AAAAAA+Ioskeley-Mono\n%%EOF\n")
        (self.root / "hex-manual.html").write_text('<html><body>0.1.0-beta.1</body></html>')
        self.manifest = {"schema": 1, "version": "0.1.0-beta.1", "source": "a" * 40,
                         "outputs": {name: manual.sha(self.root / name) for name in ("hex-manual.html", "hex-manual.pdf")}}
        (self.root / "documentation.json").write_bytes(manual.encode(self.manifest))

    def test_version_source_and_output_corruption_are_refused(self):
        self.assertEqual(manual.verify(self.root, "0.1.0-beta.1", "a" * 40), self.manifest)
        with self.assertRaisesRegex(manual.Refusal, "provenance"):
            manual.verify(self.root, "0.1.0-beta.2", "a" * 40)
        with self.assertRaisesRegex(manual.Refusal, "provenance"):
            manual.verify(self.root, "0.1.0-beta.1", "b" * 40)
        (self.root / "hex-manual.pdf").write_bytes(b"modified")
        with self.assertRaisesRegex(manual.Refusal, "hash mismatch"):
            manual.verify(self.root)

    def test_offline_manual_refuses_script_network_fonts_and_event_handlers(self):
        for content in ('<script>alert(1)</script>', '<img src="https://invalid/asset.png">',
                        '<a onclick="x()">go</a>', '<style>@font-face{src:url(https://invalid/font)}</style>',
                        '<iframe src="data:text/html,fixture"></iframe>'):
            (self.root / "hex-manual.html").write_text('0.1.0-beta.1' + content)
            with self.subTest(content=content), self.assertRaises(manual.Refusal):
                manual.validate(self.root, "0.1.0-beta.1")
        (self.root / "hex-manual.html").write_text('0.1.0-beta.1<style>@font-face{src:url(data:font/ttf;base64,AAAA)}</style>')
        manual.validate(self.root, "0.1.0-beta.1")

    def test_complete_font_and_manual_inputs_are_content_bound(self):
        source = self.root / "source"
        (source / "docs/manual").mkdir(parents=True)
        (source / "assets/fonts/ioskeley-mono").mkdir(parents=True)
        (source / "docs/manual/manual.md").write_text("# first use\n")
        font = source / "assets/fonts/ioskeley-mono/ioskeley-mono-regular.ttf"
        font.write_bytes(b"font-one")
        (font.parent / "OFL.txt").write_text("license")
        before = manual.inputs(source)
        font.write_bytes(b"font-two")
        self.assertNotEqual(before, manual.inputs(source))
        font.unlink()
        font.symlink_to(self.root / "hex-manual.pdf")
        with self.assertRaisesRegex(manual.Refusal, "symlinks"):
            manual.inputs(source)

    def test_pdf_cannot_silently_substitute_another_font_for_code(self):
        (self.root / "hex-manual.pdf").write_bytes(b"%PDF-1.7\n/BaseFont /AAAAAA+Ioskeley-Mono\n/FontName /DejaVuSansMono\n%%EOF\n")
        with self.assertRaisesRegex(manual.Refusal, "alternate font"):
            manual.validate(self.root, "0.1.0-beta.1")

    def test_manifest_cannot_select_unexpected_output_paths(self):
        self.manifest["outputs"]["../outside.pdf"] = "a" * 64
        (self.root / "documentation.json").write_bytes(manual.encode(self.manifest))
        with self.assertRaisesRegex(manual.Refusal, "inventory"):
            manual.verify(self.root)

    def test_tool_setup_recovers_missing_or_partial_receipt_without_replacing_binary(self):
        memory = io.BytesIO()
        with zipfile.ZipFile(memory, "w") as archive:
            archive.writestr("pandoc/bin/pandoc", b"pinned executable fixture")
        raw = memory.getvalue()
        tools = {"pandoc": ("3.12", "https://invalid/pinned.zip", hashlib.sha256(raw).hexdigest(), "pandoc/bin/pandoc")}
        calls = []
        def run(argv, directory):
            if argv[0] == "curl":
                calls.append("download")
                Path(argv[argv.index("--output") + 1]).write_bytes(raw)
                return ""
            return "pandoc 3.12"
        with patch.object(manual, "TOOLS", tools), patch.object(manual, "run", side_effect=run), \
                patch.object(manual.os, "uname", return_value=SimpleNamespace(sysname="Darwin", machine="arm64")):
            cache = self.root / "tools"
            manual.setup(cache)
            executable = cache / "pandoc-3.12"
            inode = executable.stat().st_ino
            receipt = cache / "pandoc-3.12.json"
            receipt.unlink()
            manual.setup(cache)
            receipt.write_bytes(b"{")
            manual.setup(cache)
            manual.setup(cache)
            self.assertEqual(executable.stat().st_ino, inode)
            self.assertEqual(executable.read_bytes(), b"pinned executable fixture")
            self.assertEqual(len(calls), 3)

    def test_render_cannot_attribute_dirty_or_different_source_to_a_release(self):
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)
        subprocess.run(["git", "add", "."], cwd=self.root, check=True)
        subprocess.run(["git", "-c", "user.name=manual fixture", "-c", "user.email=fixture@invalid",
                        "commit", "-qm", "manual fixture"], cwd=self.root, check=True)
        source = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=self.root).decode().strip()
        with self.assertRaisesRegex(manual.Refusal, "exact manual source"):
            manual.build(self.root, self.root / "out", "0.1.0-beta.1", "a" * 40)
        (self.root / "new-uncommitted-manual.md").write_text("new manual")
        with self.assertRaisesRegex(manual.Refusal, "exact manual source"):
            manual.build(self.root, self.root / "out", "0.1.0-beta.1", source)


if __name__ == "__main__":
    unittest.main()
