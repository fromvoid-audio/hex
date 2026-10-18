from pathlib import Path
p=Path('crates/hex-ui/src/editor/browser.rs');s=p.read_text()
a=s.index('        panel = div()\n            .w(px(pane_width + editor_width))');b=s.index('\n\n        let entity = cx.entity();',a)
old=s[a:b]
assert '.child(toolbar)' in old and '.children(editor)' in old
s=s[:a]+'''        // The rail belongs to the fixed base column. The optional editor is a
        // sibling of that WHOLE column, so neither opening control can move.
        panel = div()
            .w(px(pane_width + editor_width))
            .flex_shrink_0()
            .min_h_0()
            .flex()
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .text_color(rgb(theme::color(theme::PaletteRole::Text)))
            .child(
                div()
                    .w(px(pane_width))
                    .flex_shrink_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(toolbar)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .child(sidebar)
                            .child(self.browser_sidebar_resize(pane_width, cx))
                            .child(panel),
                    ),
            )
            .children(editor);'''+s[b:];p.write_text(s)
p=Path('crates/hex-ui/src/editor/browser/layout.rs');s=p.read_text().replace('"open tag editor",','"show or hide tag editor",',1).replace('''                        "open full tag editor",
                        |view, _, cx| {
                            view.browser.layout.editor_visible = true;''','''                        "show or hide tag editor",
                        |view, _, cx| {
                            view.browser.layout.editor_visible = !view.browser.layout.editor_visible;''',1);p.write_text(s)
