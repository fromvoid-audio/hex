"""Native wrapper registrations keep raw text and unrelated components guarded."""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class BrowserScrollbarStyleGuardTests(unittest.TestCase):
    def test_wrapper_names_do_not_escape_their_owners(self):
        self.check_registered_views(outside_owner=True)

    def test_registered_owners_reject_raw_and_unknown_children(self):
        self.check_registered_views(outside_owner=False)

    def check_registered_views(self, *, outside_owner):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "scripts").mkdir()
            shutil.copy2(ROOT / "scripts/check-ui-style.py", root / "scripts/check-ui-style.py")
            files = {
                "crates/hex-ui/src/app.rs": ".font_family(theme::FONT_FAMILY)",
                "crates/hex-ui/src/editor/render.rs": '.child(error.display_text()) ErrorTooltip(message.clone()) .font_family(theme::FONT_FAMILY) self.transport(window, cx)',
                "crates/hex-ui/src/editor/display_error.rs": '.child(self.display_text()) self.0.display_text()',
                "crates/hex-ui/src/editor/header.rs": '.pl(px(theme::TITLEBAR_LEADING_INSET))',
                "crates/hex-ui/src/layout/transport.rs": '.pl(px(theme::TITLEBAR_LEADING_INSET))',
                "scripts/hex-test-build.py": '"CFBundleName": "hex"\n"CFBundleDisplayName": "hex"',
                "crates/hex-ui/src/editor/track_performance/tests.rs": '.child(super::super::render(&self.state, self.owner, self.now))',
                "crates/hex-ui/src/editor/browser.rs": '.child(self.browser_results(rows, count, cx)) .child(super::clip_name::passive_name_header(name, fill, true))',
                "crates/hex-ui/src/editor/browser/scrollbar.rs": '.child(list) .child(self.browser_scrollbar(cx))',
                "crates/hex-ui/src/editor/track_meters.rs": '.child(bar) .child(bars.id(id))',
                "crates/hex-ui/src/editor/tracks.rs": '.child(toggle_button_with_style(label, tooltip, id, style)) .child(framed_clip) .child(super::clip_name::passive_name_header(name, fill, true))',
                "crates/hex-ui/src/editor/clip_boundary.rs": '.child(paint)',
                "crates/hex-ui/src/editor/clip_border_ui_test.rs": '.child(super::pixel_backdrop_shell(body, bounds, style, true, selector))',
                "crates/hex-ui/src/editor/time_move_preview.rs": '.child(super::clip_name::passive_name_header(name, fill, true))',
                "crates/hex-ui/src/editor/console_mixer.rs": '.child(activator) .child(solo) .child(gain) .child(pan) .child(toggles) .child(controls) .child(column) .child(super::track_meters::render(id, peaks, height))',
                "crates/hex-ui/src/editor/session_grid.rs": '.child(session_clip_backdrop(clip, fill, stroke)) .child(super::clip_boundary::backdrop_shell(body, left, width, style, true, selector)) .child(self.console_mixer_row(page, width, scenes, cx))',
                "crates/hex-ui/src/editor/return_tracks.rs": '.child(super::waveform::toggle_button_with_style(label, tooltip, id, style))',
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
                ("other.rs", '.child(bar)'),
                ("other.rs", '.child(framed_clip)'),
                ("other.rs", '.child(paint)'),
                ("other.rs", '.child(session_clip_backdrop(clip, fill, stroke))'),
                ("editor/console_mixer.rs", '.child("Unnormalized")'),
                ("editor/console_mixer.rs", '.child(self.unregistered_component(cx))'),
                ("other.rs", '.child(self.console_mixer_row(page, width, scenes, cx))'),
                ("editor/session_grid.rs", '.child("Unnormalized")'),
                ("editor/session_grid.rs", '.child(self.unregistered_component(cx))'),
                ("editor/time_move_preview.rs", '.child("Unnormalized")'),
                ("editor/time_move_preview.rs", '.child(self.unregistered_component(cx))'),
                ("other.rs", '.child(super::pixel_backdrop_shell(body, bounds, style, true, selector))'),
                ("editor/clip_border_ui_test.rs", '.child("Unnormalized")'),
                ("editor/clip_border_ui_test.rs", '.child(self.unregistered_component(cx))'),
                ("other.rs", '.child(backdrop_shell(body, left, width, style, true, selector))'),
                ("other.rs", '.child(super::clip_name::passive_name_header(name, fill, true))'),
                ("other.rs", '.child(trailing_edge(notch, color, hover))'),
                ("editor/clip_boundary.rs", '.child("Unnormalized")'),
                ("editor/clip_boundary.rs", '.child(self.unregistered_component(cx))'),
                ("other.rs", '.child(toggle_button_with_style(label, tooltip, id, style))'),
                ("other.rs", '.child(super::waveform::toggle_button_with_style(label, tooltip, id, style))'),
                ("editor/tracks.rs", '.child("Unnormalized")'),
                ("editor/tracks.rs", '.child(self.unregistered_component(cx))'),
                ("editor/return_tracks.rs", '.child("Unnormalized")'),
                ("editor/return_tracks.rs", '.child(self.unregistered_component(cx))'),
                ("other.rs", '.child(bars.id(id))'),
                ("editor/track_meters.rs", '.child("Unnormalized")'),
                ("editor/track_meters.rs", '.child(self.unregistered_component(cx))'),
                ("editor/browser.rs", '.child("Unnormalized")'),
                ("editor/browser.rs", '.child(self.unregistered_component(cx))'),
                ("editor/browser/scrollbar.rs", '.child("Unnormalized")'),
                ("editor/browser/scrollbar.rs", '.child(self.unregistered_component(cx))'),
            ]:
                if (name == "other.rs") != outside_owner:
                    continue
                path = root / "crates/hex-ui/src" / name
                original = path.read_text() if path.exists() else None
                path.write_text(content)
                with self.subTest(name=name, content=content):
                    result = run()
                    self.assertIn(name, result.stderr)
                    self.assertNotEqual(result.returncode, 0)
                if original is None:
                    path.unlink()
                else:
                    path.write_text(original)



if __name__ == "__main__":
    unittest.main()
