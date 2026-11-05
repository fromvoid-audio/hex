"""GPUI source integrity and offline cumulative patch sensitivity."""
import difflib
import hashlib
import importlib.util
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/check-gpui-patch.py"
SPEC = importlib.util.spec_from_file_location("gpui_patch_check", SCRIPT)
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)


class GpuiPatchTests(unittest.TestCase):
    def test_finder_metadata_and_source_integrity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "scripts").mkdir()
            for name in ("Cargo.toml", "Cargo.lock", "scripts/check-gpui-patch.py"):
                shutil.copyfile(ROOT / name, root / name)
            vendor = root / "third_party/gpui"
            shutil.copytree(ROOT / "third_party/gpui", vendor,
                            ignore=shutil.ignore_patterns(".DS_Store"))

            def check():
                return subprocess.run([sys.executable, str(root / "scripts/check-gpui-patch.py")],
                                      capture_output=True, text=True, check=False)

            baseline = check()
            self.assertEqual(baseline.returncode, 0, baseline.stderr)
            # Same files and locations observed in the user's failing checkout.
            for name in (".DS_Store", "src/.DS_Store"):
                (vendor / name).write_bytes(b"Finder metadata")
            metadata = check()
            self.assertEqual(metadata.returncode, 0, metadata.stderr)

            for name in ("src/unexpected.rs", "src/.DS_Store.rs"):
                with self.subTest(unexpected=name):
                    path = vendor / name
                    path.write_text("fn unexpected() {}\n")
                    result = check()
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("unexpected files", result.stderr)
                    path.unlink()

            link = vendor / "src/.DS_Store"
            link.unlink()
            for target in ("interactive.rs", "missing-finder-target"):
                with self.subTest(symlink=target):
                    link.symlink_to(target)
                    result = check()
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("unexpected files", result.stderr)
                    link.unlink()

            for name in ("src/interactive.rs", "src/element.rs", "src/accessibility.rs"):
                with self.subTest(changed=name):
                    path = vendor / name
                    original = path.read_bytes()
                    path.write_bytes(original + b"\n// changed source\n")
                    result = check()
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("source/provenance mismatch", result.stderr)
                    path.write_bytes(original)

            missing = vendor / "src/interactive.rs"
            original = missing.read_bytes()
            missing.unlink()
            result = check()
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("source/provenance mismatch", result.stderr)
            missing.write_bytes(original)
            self.assertEqual(check().returncode, 0)



class CumulativePatchTests(unittest.TestCase):
    def fixture(self, root, omit=None):
        original = {"src/first.rs": b"one\nold\n", "src/second.rs": b"two\nold\n"}
        current = {name: data.replace(b"old", b"new") for name, data in original.items()}
        added = {"src/added.rs": b"addition\n"}
        digest = lambda data: hashlib.sha256(data).hexdigest()
        manifest = {"original_files": {n: digest(b) for n, b in original.items()},
                    "patched_files": {n: digest(b) for n, b in current.items()},
                    "added_files": {n: digest(b) for n, b in added.items()}}
        blocks = []
        for name, data in {**current, **added}.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
            if name == omit:
                continue
            before = original.get(name, b"")
            blocks.append(f"diff --git a/{name} b/{name}\n")
            if name in added:
                blocks.append("new file mode 100644\n")
            blocks.extend(difflib.unified_diff(
                before.decode().splitlines(keepends=True), data.decode().splitlines(keepends=True),
                fromfile=f"a/{name}" if name in original else "/dev/null", tofile=f"b/{name}"))
        (root / "hex-patches.diff").write_text("".join(blocks))
        # These hashes still pass for an omitted patch hunk: the old guard's gap.
        for name, expected in {**manifest["patched_files"], **manifest["added_files"]}.items():
            self.assertEqual(digest((root / name).read_bytes()), expected)
        return manifest

    def test_exact_patch_restores_originals_and_removes_additions_without_mutation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest = self.fixture(root)
            before = {p.relative_to(root): p.read_bytes() for p in root.rglob("*") if p.is_file()}
            self.assertEqual(CHECK.verify_cumulative_patch(root, manifest), [])
            after = {p.relative_to(root): p.read_bytes() for p in root.rglob("*") if p.is_file()}
            self.assertEqual(after, before)

    def test_missing_existing_or_added_hunk_cannot_hide_behind_valid_source_hashes(self):
        for omitted in ["src/second.rs", "src/added.rs"]:
            with self.subTest(omitted=omitted), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                manifest = self.fixture(root, omit=omitted)
                errors = CHECK.verify_cumulative_patch(root, manifest)
                self.assertTrue(any(omitted in error for error in errors), errors)



if __name__ == "__main__":
    unittest.main()
