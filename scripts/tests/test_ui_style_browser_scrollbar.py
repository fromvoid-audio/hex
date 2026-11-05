"""Native wrapper registrations keep raw text and unrelated components guarded."""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class BrowserScrollbarStyleGuardTests(unittest.TestCase):
    def test_only_registered_native_views_are_exempt(self):
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
                "crates/hex-ui/src/editor/track_performance/tests.rs": '.child(super::super::render(&self.state, self.owner, self.now))',
                "crates/hex-ui/src/editor/browser.rs": '.child(self.browser_results(rows, count, cx))',
                "crates/hex-ui/src/editor/browser/scrollbar.rs": '.child(list) .child(self.browser_scrollbar(cx))',
            }
            for name, text in files.items():
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(text)
            def run():
                return subprocess.run([sys.executable, str(root / "scripts/check-ui-style.py")], capture_output=True, text=True)
            self.assertEqual(run().returncode, 0)
            for name, content in [
                ("other.rs", '.child(super::super::render(&self.state, self.owner, self.now))'),
                ("editor/track_performance/tests.rs", '.child("Unnormalized")'),
                ("editor/track_performance/tests.rs", '.child(self.unregistered_component(cx))'),
                ("other.rs", '.child(self.browser_results(rows, count, cx))'),
                ("other.rs", '.child(self.browser_scrollbar(cx))'),
                ("other.rs", '.child(list)'),
                ("editor/browser.rs", '.child("Unnormalized")'),
                ("editor/browser.rs", '.child(self.unregistered_component(cx))'),
                ("editor/browser/scrollbar.rs", '.child("Unnormalized")'),
                ("editor/browser/scrollbar.rs", '.child(self.unregistered_component(cx))'),
            ]:
                path = root / "crates/hex-ui/src" / name
                original = path.read_text() if path.exists() else None
                path.write_text(content)
                with self.subTest(name=name, content=content):
                    self.assertIn(name, run().stderr)
                    self.assertNotEqual(run().returncode, 0)
                if original is None:
                    path.unlink()
                else:
                    path.write_text(original)



if __name__ == "__main__":
    unittest.main()
