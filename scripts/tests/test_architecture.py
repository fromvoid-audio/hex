"""Source-owned architecture documentation and dependency refusal controls."""
import contextlib
import io
import json
from pathlib import Path
import runpy
import tempfile
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parents[1] / "check-architecture.py"
DOCS = """//! # Purpose
//! Render fixtures.
//! # Dependencies
//! No workspace dependencies.
//! # Does not own
//! No UI or device I/O.
"""


class ArchitectureDocumentationTests(unittest.TestCase):
    def run_guard(self, docs=DOCS, dependencies=(), package_name="hex-dsp"):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "src").mkdir()
            (root / "src/lib.rs").write_text(docs)
            package = {"name": package_name, "id": "fixture", "manifest_path": str(root / "Cargo.toml"),
                       "dependencies": list(dependencies)}
            metadata = {"packages": [package], "workspace_members": ["fixture"]}
            output = io.StringIO()
            with patch("subprocess.check_output", return_value=json.dumps(metadata)), \
                    contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
                try:
                    runpy.run_path(str(SCRIPT), run_name="__main__")
                except SystemExit as error:
                    return error.code, output.getvalue()
            return 0, output.getvalue()

    def test_source_docs_qualify_without_readme(self):
        self.assertEqual(self.run_guard()[0], 0)

    def test_missing_or_empty_source_section_refuses(self):
        for docs in ("", DOCS.replace("//! No UI or device I/O.\n", "")):
            with self.subTest(docs=docs):
                code, output = self.run_guard(docs)
                self.assertEqual(code, 1)
                self.assertIn("crate doc comments must define", output)

    def test_documented_crate_still_refuses_forbidden_platform_dependency(self):
        for name in ("cpal", "qos-threads"):
            with self.subTest(dependency=name):
                code, output = self.run_guard(dependencies=[{"name": name, "kind": None}])
                self.assertEqual(code, 1)
                self.assertIn(f"{name} belongs in its declared platform/UI boundary", output)

    def test_audio_profile_clock_does_not_admit_production_or_process_api(self):
        for kind, features, expected in (("dev", ["time"], 0),
                                         (None, ["time"], 1),
                                         ("dev", ["fs"], 1),
                                         ("dev", ["process"], 1)):
            with self.subTest(kind=kind, features=features):
                code, _ = self.run_guard(package_name="hex-audio", dependencies=[
                    {"name": "rustix", "kind": kind, "features": features}])
                self.assertEqual(code, expected)
