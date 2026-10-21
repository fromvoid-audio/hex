"""Identity refusal tests; native evidence requires explicit original receipts."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest

NATIVE = Path(__file__).resolve().parents[2] / "crates/hex-plugin-scan/native"
sys.path.insert(0, str(NATIVE))
try:
    SPEC = importlib.util.spec_from_file_location("hex_macos_bootstrap", NATIVE / "macos_bootstrap.py")
    BOOT = importlib.util.module_from_spec(SPEC)
    SPEC.loader.exec_module(BOOT)
finally:
    sys.path.pop(0)


class MachHeaderTests(unittest.TestCase):
    def test_wrong_role_and_architecture_refuse(self):
        # Parser component evidence only; not an executable native fixture.
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "header"
            for cpu, kind in [(0x01000007, 2), (0x0100000C, 8)]:
                path.write_bytes(struct.pack("<8I", 0xFEEDFACF, cpu, 0, kind, 1, 8, 0, 0) + b"\0" * 8)
                with self.subTest(cpu=cpu, kind=kind), self.assertRaisesRegex(ValueError, "native arm64"):
                    BOOT.macho(path, 2)

    def test_truncation_and_command_bounds_refuse(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "header"
            for data in [b"\xcf\xfa", struct.pack("<8I", 0xFEEDFACF, 0x0100000C, 0, 2, 1, 8, 0, 0)]:
                path.write_bytes(data)
                with self.subTest(size=len(data)), self.assertRaises(ValueError):
                    BOOT.macho(path, 2)


@unittest.skipUnless(os.environ.get("HEX_VST3_MACOS_RECEIPT"), "explicit original macOS fixture receipt required")
class NativeBootstrapTests(unittest.TestCase):
    def setUp(self):
        self.original = Path(os.environ["HEX_VST3_MACOS_RECEIPT"]).resolve()
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        shutil.copytree(self.original.parent / BOOT.APP, self.root / BOOT.APP)
        self.receipt = self.root / "controlled-macos.json"
        shutil.copyfile(self.original, self.receipt)

    def test_actual_native_identity_and_effect_instrument_pcm(self):
        self.assertEqual(BOOT.verify(self.receipt, run=True), {
            "version": 1, "scope": "original-macos-bootstrap", "effect_frames": 64, "instrument_frames": 64})

    def test_absent_helper_refuses_before_execution(self):
        (self.root / BOOT.EXECUTABLE).unlink()
        with self.assertRaisesRegex(ValueError, "missing or changed"):
            BOOT.verify(self.receipt, run=True)

    def test_changed_fixture_and_helper_refuse_before_execution(self):
        for relative in [BOOT.EXECUTABLE, BOOT.APP / "Contents/Resources/fixtures/effect.vst3/Contents/MacOS/fixture"]:
            path = self.root / relative
            original = path.read_bytes()
            path.write_bytes(original + b"changed")
            with self.subTest(path=str(relative)), self.assertRaisesRegex(ValueError, "missing or changed"):
                BOOT.verify(self.receipt, run=True)
            path.write_bytes(original)

    def test_actual_x86_64_binary_refuses_even_with_updated_manifest(self):
        # A real wrong-architecture Mach-O, not a patched header or hash-only refusal.
        binary = self.root / BOOT.EXECUTABLE
        compiler = BOOT.command(["xcrun", "--find", "clang++"])
        subprocess.run([compiler, "-arch", "x86_64", "-x", "c++", "-", "-o", str(binary)],
                       input="int main() { return 0; }\n", text=True, check=True, timeout=120)
        data = json.loads(self.receipt.read_text())
        data["bundle_files"] = BOOT.inventory(self.root / BOOT.APP)
        self.receipt.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, "native arm64"):
            BOOT.verify(self.receipt, run=True)

    def test_helper_itself_refuses_changed_fixture_before_loading(self):
        path = self.root / BOOT.APP / "Contents/Resources/fixtures/effect.vst3/Contents/MacOS/fixture"
        path.write_bytes(path.read_bytes() + b"changed")
        # Bypass the Python verifier deliberately: native compiled-in identity
        # must still refuse the changed original module, before CFBundle load.
        result = subprocess.run([str(self.root / BOOT.EXECUTABLE), "--self-test"],
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 1)
        self.assertIn("original fixture smoke failed", result.stderr)

    def test_helper_does_not_accept_an_installed_module_path(self):
        result = subprocess.run([str(self.root / BOOT.EXECUTABLE), "/Library/Audio/Plug-Ins/VST3"],
                                capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 2)
        self.assertIn("arbitrary paths refused", result.stderr)

    def test_stale_source_and_linked_member_refuse(self):
        data = json.loads(self.receipt.read_text())
        data["source_sha256"] = "0" * 64
        self.receipt.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, "stale bootstrap"):
            BOOT.verify(self.receipt, run=True)
        shutil.copyfile(self.original, self.receipt)
        (self.root / BOOT.APP / "Contents/linked").symlink_to(self.original)
        with self.assertRaisesRegex(ValueError, "linked bundle"):
            BOOT.verify(self.receipt, run=True)


if __name__ == "__main__":
    unittest.main()
