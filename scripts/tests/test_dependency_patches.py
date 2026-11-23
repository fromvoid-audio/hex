"""Finder metadata and source integrity for the pinned dependency patches."""
import importlib.util
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import build_inputs


def load_script(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class DependencyPatchTests(unittest.TestCase):
    def test_finder_metadata_is_ignored_but_unexpected_files_and_links_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "scripts").mkdir()
            for name in ("Cargo.toml", "Cargo.lock", "scripts/check-dependency-patches.py"):
                shutil.copyfile(ROOT / name, root / name)
            for crate in ("block", "proc-macro-error2"):
                shutil.copytree(ROOT / "third_party" / crate, root / "third_party" / crate,
                                ignore=shutil.ignore_patterns(".DS_Store"))
            vendor = root / "third_party/proc-macro-error2"

            def check():
                return subprocess.run(
                    [sys.executable, str(root / "scripts/check-dependency-patches.py")],
                    capture_output=True, text=True, check=False)

            baseline = check()
            self.assertEqual(baseline.returncode, 0, baseline.stderr)
            # The file observed in the owner's checkout on 30 October 2026.
            (vendor / ".DS_Store").write_bytes(b"Finder metadata")
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

            (vendor / ".DS_Store").unlink()
            link = vendor / "src/.DS_Store"
            link.symlink_to("lib.rs")
            result = check()
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("unexpected files", result.stderr)

    def test_cpal_and_xattr_ignore_finder_metadata_but_reject_symlinks(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "scripts").mkdir()
            for name in (
                "Cargo.toml",
                "Cargo.lock",
                "scripts/check-cpal-patch.py",
                "scripts/check-xattr-compat.py",
            ):
                shutil.copyfile(ROOT / name, root / name)
            for crate in ("cpal", "xattr"):
                shutil.copytree(
                    ROOT / "third_party" / crate,
                    root / "third_party" / crate,
                    ignore=shutil.ignore_patterns(".DS_Store"),
                )

            cpal_vendor = root / "third_party/cpal"
            (cpal_vendor / ".DS_Store").write_bytes(b"Finder metadata")
            cpal_ok = subprocess.run(
                [sys.executable, str(root / "scripts/check-cpal-patch.py")],
                capture_output=True, text=True, check=False,
            )
            self.assertEqual(cpal_ok.returncode, 0, cpal_ok.stderr)

            (cpal_vendor / ".DS_Store").unlink()
            (cpal_vendor / ".DS_Store").symlink_to("Cargo.toml")
            cpal_link = subprocess.run(
                [sys.executable, str(root / "scripts/check-cpal-patch.py")],
                capture_output=True, text=True, check=False,
            )
            self.assertNotEqual(cpal_link.returncode, 0)
            self.assertIn(".DS_Store", cpal_link.stderr)

            xattr_mod = load_script("xattr_compat_fixture", root / "scripts/check-xattr-compat.py")
            xattr_vendor = root / "third_party/xattr"
            (xattr_vendor / ".DS_Store").write_bytes(b"Finder metadata")
            xattr_mod.verify()

            (xattr_vendor / ".DS_Store").unlink()
            (xattr_vendor / ".DS_Store").symlink_to("Cargo.toml")
            with self.assertRaises(AssertionError):
                xattr_mod.verify()

    def test_future_lints_rustflags_env_handles_empty_encoded_rustflags(self):
        future_lints = load_script(
            "future_lints_fixture", ROOT / "scripts/check-dependency-future-lints.py"
        )
        empty = future_lints.rustflags_env({"CARGO_ENCODED_RUSTFLAGS": ""}, "/tmp/t")
        self.assertEqual(
            empty["CARGO_ENCODED_RUSTFLAGS"],
            "-D\x1fpub_use_of_private_extern_crate",
        )
        self.assertFalse(empty["CARGO_ENCODED_RUSTFLAGS"].startswith("\x1f"))

        populated = future_lints.rustflags_env(
            {"CARGO_ENCODED_RUSTFLAGS": "-C\x1fopt-level=2"}, "/tmp/t"
        )
        self.assertEqual(
            populated["CARGO_ENCODED_RUSTFLAGS"],
            "-C\x1fopt-level=2\x1f-D\x1fpub_use_of_private_extern_crate",
        )

        fallback = future_lints.rustflags_env({}, "/tmp/t")
        self.assertNotIn("CARGO_ENCODED_RUSTFLAGS", fallback)
        self.assertIn("-D pub_use_of_private_extern_crate", fallback["RUSTFLAGS"])

    def test_build_inputs_ignores_finder_metadata_but_observes_ds_store_symlink(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            folder = root / "input"
            folder.mkdir()
            (folder / "data.txt").write_text("ok")
            meta = {"target_directory": str(root / "target")}
            before = build_inputs.fingerprint(root, {}, meta, {}, paths=[folder])
            (folder / ".DS_Store").write_bytes(b"Finder metadata")
            with_ds = build_inputs.fingerprint(root, {}, meta, {}, paths=[folder])
            self.assertEqual(before, with_ds)
            (folder / ".DS_Store").unlink()
            (folder / ".DS_Store").symlink_to(folder / "data.txt")
            with_link = build_inputs.fingerprint(root, {}, meta, {}, paths=[folder])
            self.assertNotEqual(before, with_link)


if __name__ == "__main__":
    unittest.main()
