"""The log-viewer exception preserves source text without opening other routes."""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class LogViewerStyleGuardTests(unittest.TestCase):
    def test_only_the_log_viewer_source_and_typed_components_are_exempt(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "scripts").mkdir()
            shutil.copy2(ROOT / "scripts/check-ui-style.py", root / "scripts/check-ui-style.py")
            files = {
                "crates/hex-ui/src/app.rs": ".font_family(theme::FONT_FAMILY)",
                "crates/hex-ui/src/editor/render.rs": '.child(error.display_text()) ErrorTooltip(message.clone()) .font_family(theme::FONT_FAMILY) header::title_bar',
                "crates/hex-ui/src/editor/display_error.rs": '.child(self.display_text()) self.0.display_text()',
                "crates/hex-ui/src/editor/header.rs": 'crate::build_info::HEADER_INSET.max(theme::TITLEBAR_LEADING_INSET) .px(px(',
                "crates/hex-ui/src/layout/transport.rs": '.pl(px(theme::TITLEBAR_LEADING_INSET))',
                "scripts/hex-test-build.py": '"CFBundleName": "hex"\n"CFBundleDisplayName": "hex"',
                "crates/hex-ui/src/log_viewer.rs": '.child(display) .child(session) .child(status) .child(self.query.clone()) .child(list) .child(self.log_scrollbar(cx))',
            }
            for name, text in files.items():
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(text)
            def run():
                return subprocess.run([sys.executable, str(root / "scripts/check-ui-style.py")], capture_output=True, text=True)
            self.assertEqual(run().returncode, 0)
            other = root / "crates/hex-ui/src/other.rs"
            other.write_text('.child(display)')
            self.assertIn("other.rs", run().stderr)
            other.write_text('.child(self.log_scrollbar(cx))')
            self.assertIn("other.rs", run().stderr)
            other.unlink()
            viewer = root / "crates/hex-ui/src/log_viewer.rs"
            viewer.write_text('.child("Unnormalized")')
            self.assertIn("log_viewer.rs", run().stderr)
            viewer.write_text('.child(self.unregistered_component(cx))')
            self.assertIn("log_viewer.rs", run().stderr)


if __name__ == "__main__":
    unittest.main()
