"""Boundary guard regressions, including mutations of actual production source."""
import importlib.util
from pathlib import Path
import shutil
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "check-device-layout.py"
SPEC = importlib.util.spec_from_file_location("device_layout_guard", SCRIPT)
guard = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = guard
SPEC.loader.exec_module(guard)


class DeviceLayoutGuardTests(unittest.TestCase):
    def test_comments_literals_nested_blocks_and_return_types_are_not_chrome(self):
        source = '''
        /* .bg(rgb(0x123456)) /* nested */ fn false_header() {} */
        fn render<'a>() -> hex_widgets::ControlStyle {
            let label = r##"{ fn fake() { .rounded(px(9)) } }"##;
            let escaped = "quote\\\" } .bg(color)";
            let brace = '}';
            shared::body().child(label)
        }
        '''
        stream = guard.tokens(source)
        self.assertEqual(set(guard.functions(stream)), {"render"})
        self.assertEqual(guard.chrome_errors(stream, "fixture"), [])
        self.assertEqual(guard.require_calls(guard.functions(stream)["render"], ("shared::body",), "fixture"), [])

    def test_style_literal_is_distinct_from_return_type(self):
        errors = guard.chrome_errors(guard.tokens("fn style() -> ControlStyle { ControlStyle { surface: palette } }"), "fixture")
        self.assertEqual(len(errors), 1)
        self.assertIn("resolve device styles", errors[0])

    def production_copy(self, root, *, gallery=False):
        paths = list(guard.ENTRY_POINTS) + [guard.UI / "device_composition.rs", guard.PANEL / "operator_panel/preview.rs"]
        paths += [path.relative_to(guard.ROOT) for path in (guard.ROOT / guard.PANEL).glob("*_panel.rs")]
        if gallery:
            paths += [Path("crates/hex-ui/examples/layout_review") / name for name in ("gallery.rs", "compositions.rs")]
            paths += [Path("crates/hex-ui/examples/layout_review.rs"), Path("crates/hex-sandbox/src/main.rs")]
        for path in paths:
            (root / path).parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(guard.ROOT / path, root / path)

    def test_production_boundary_and_negative_controls(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.production_copy(root)
            self.assertEqual(guard.analyze(root, gallery=False), [])

            rows = root / guard.PANEL / "parameter_rows.rs"
            rows_original = rows.read_text()
            rows.write_text(rows_original.replace("crate::device_composition::windowed_body(", "local_unstyled_body("))
            errors = guard.analyze(root, gallery=False)
            self.assertTrue(any("parameter_rows.rs::body" in error and "missing shared call" in error for error in errors), errors)
            rows.write_text(rows_original)

            panel = root / guard.PANEL / "utility_panel.rs"
            original = panel.read_text()
            # A local header/surface fork must fail even when its legitimate
            # shared composition call remains elsewhere in the same function.
            panel.write_text(original.replace("crate::device_composition::utility(input, output)",
                                              "crate::device_composition::utility(input, output).bg(rgb(0x474747)).rounded(px(7.0))"))
            errors = guard.analyze(root, gallery=False)
            self.assertTrue(any("device chrome" in error and "bg" in error for error in errors), errors)
            self.assertTrue(any("color literal" in error for error in errors), errors)
            panel.write_text(original)

            host = root / guard.UI / "editor/device_panel.rs"
            host_original = host.read_text()
            host.write_text(host_original.replace("device_frame(", "local_header_and_frame("))
            errors = guard.analyze(root, gallery=False)
            self.assertTrue(any("render_instrument" in error and "missing shared call device_frame" in error for error in errors), errors)
            host.write_text(host_original)

            helper = "pub(super) fn control_style() -> ControlStyle {"
            for injection in [
                "let fork = hex_widgets::ActionStyle { background: 0x123456, text: 0xffffff, hover_background: 0xabcdef, hover_text: Some(0) };",
                "let fork = ControlStyle { surface: 0x123456, ..theme::device_style(theme::DeviceStyleRole::Control) };",
                "let mut fork = theme::device_style(theme::DeviceStyleRole::Control); fork.surface = theme::color(theme::PaletteRole::Primary);",
            ]:
                host.write_text(host_original.replace(helper, helper + injection))
                errors = guard.analyze(root, gallery=False)
                self.assertTrue(any("style" in error and "theme/device.rs" in error for error in errors), errors)
            host.write_text(host_original)

            owner = root / guard.PANEL / "return_view.rs"
            original = owner.read_text()
            owner.write_text(original.replace("crate::device_composition::unsupported_body", "custom_error_body"))
            errors = guard.analyze(root, gallery=False)
            self.assertTrue(any("return_view.rs::render_return_effect" in error and "unsupported_body" in error for error in errors), errors)
            owner.write_text(original)

            # A future dedicated panel is discovered automatically; both effect
            # owners must opt into it instead of silently diverging.
            (root / guard.PANEL / "future_panel.rs").write_text("fn render_body() { crate::device_composition::body() }")
            errors = guard.analyze(root, gallery=False)
            self.assertEqual(sum("future_panel::render_body" in error for error in errors), 2, errors)

    def test_gallery_uses_product_styles_and_production_compositions(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.production_copy(root, gallery=True)
            self.assertEqual(guard.analyze(root), [])
            preview = root / "crates/hex-ui/examples/layout_review/compositions.rs"
            original = preview.read_text()
            preview.write_text(original + "\nfn local_skin() { ControlStyle { surface: palette } }\n")
            self.assertTrue(any("resolve device styles" in error for error in guard.analyze(root)))
            preview.write_text(original.replace("composition::utility", "local_utility").replace("device_composition::utility", "local_utility"))
            self.assertTrue(any("missing shared call device_composition::utility" in error for error in guard.analyze(root)))
            preview.write_text(original)
            entry = root / "crates/hex-ui/examples/layout_review.rs"
            entry.write_text(entry.read_text().replace("../src/device_composition.rs", "layout_review/copied_composition.rs"))
            self.assertTrue(any("include the owning" in error for error in guard.analyze(root)))
            entry.write_text((guard.ROOT / entry.relative_to(root)).read_text())
            entry = root / "crates/hex-sandbox/src/main.rs"
            entry.write_text(entry.read_text().replace("../../hex-ui/src/device_composition.rs", "copied_composition.rs"))
            self.assertTrue(any("hex-sandbox" in error and "include the owning" in error for error in guard.analyze(root)))


if __name__ == "__main__":
    unittest.main()
