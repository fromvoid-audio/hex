from pathlib import Path
p=Path('crates/hex-ui/src/editor/browser/layout.rs');s=p.read_text();anchor='fn browser_divider() -> Div {'
s=s.replace(anchor,'''// Keep the full capture gutter independent of its visible separator. The leading
// line adjoins the existing 1px panel border without consuming layout width.
pub(super) fn resize_divider_line(group: &'static str, selector: &'static str, active: bool) -> Div {
    div()
        .absolute()
        .left(px(0.0))
        .top(px(0.0))
        .w(px(1.0))
        .h_full()
        .bg(rgb(theme::color(if active { theme::PaletteRole::Secondary } else { theme::PaletteRole::Border })))
        .group_hover(group, |style| style.bg(rgb(theme::color(theme::PaletteRole::Secondary))))
        .debug_selector(move || selector.to_owned())
}

'''+anchor,1)
a=s.index('    pub(super) fn browser_sidebar_resize(');b=s.index('    pub(super) fn browser_tag_selector(',a);block=s[a:b]
block=block.replace('''            .cursor_col_resize()
            .bg(''','''            .cursor_col_resize()
            .relative()
            .group("browser-sidebar-resize")
            .bg(''',1)
start=block.index('            .bg(');end=block.index('            .child(\n                canvas(',start)
block=block[:start]+'''            .child(resize_divider_line(
                "browser-sidebar-resize", "browser-sidebar-resize-line", self.browser.layout.sidebar_drag.is_some(),
            ))
'''+block[end:]
s=s[:a]+block+s[b:];p.write_text(s)
p=Path('crates/hex-ui/src/editor/browser.rs');s=p.read_text();a=s.index('        let mut resize = div()\n            .id("browser-resize-handle")');b=s.index('        resize = resize.child(',a);block=s[a:b];start=block.index('            .bg(')
block=block[:start]+'''            .relative()
            .group("browser-resize-handle")
            .child(layout::resize_divider_line(
                "browser-resize-handle", "browser-resize-line", self.browser.resizing,
            ));
'''
s=s[:a]+block+s[b:];p.write_text(s)
p=Path('crates/hex-ui/src/editor/browser_resize_feedback_ui_test.rs');s=p.read_text();anchor='fn palette('
s=s.replace(anchor,'''// PR323 moves the same feedback paint into a thin child; retain all original
// gesture, state, cursor/capture and color assertions against the actual paint.
fn paint_selector(handle: &str) -> &'static str {
    if handle == "browser-sidebar-resize" { "browser-sidebar-resize-line" } else { "browser-resize-line" }
}

'''+anchor,1).replace('visual.rendered_background(selector)', 'visual.rendered_background(paint_selector(selector))').replace('visual.rendered_background(other)', 'visual.rendered_background(paint_selector(other))');p.write_text(s)
