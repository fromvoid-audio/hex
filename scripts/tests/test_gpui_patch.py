"""Offline cumulative patch sensitivity without a downloaded/native dependency."""
import difflib
import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "check-gpui-patch.py"
SPEC = importlib.util.spec_from_file_location("gpui_patch_check", SCRIPT)
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)


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
