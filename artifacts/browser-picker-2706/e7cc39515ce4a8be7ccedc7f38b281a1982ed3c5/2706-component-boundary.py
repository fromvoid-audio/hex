from pathlib import Path
p=Path('crates/hex-ui/src/editor/browser/layout.rs');s=p.read_text();s=s.replace('fn picker_icon(glyph: Div) -> Div {\n    div()', 'fn picker_icon(icon: Div) -> Div {\n    let icon = icon.whitespace_nowrap();\n    div()').replace('.child(glyph.whitespace_nowrap())', '.child(icon)')
old='''                    .child(picker_icon(
                        browser_caption("■")
                            .text_color(rgb(theme::element_colors(color).accent))
                            .debug_selector(move || {
                                format!("browser-picker-glyph-collection-{index}")
                            }),
                    ))''';assert old in s;s=s.replace(old,'                    .child(icon)')
anchor='            ][index];\n            sidebar = sidebar.child('
new='''            ][index];
            let icon = picker_icon(
                browser_caption("■")
                    .text_color(rgb(theme::element_colors(color).accent))
                    .debug_selector(move || format!("browser-picker-glyph-collection-{index}")),
            );
            sidebar = sidebar.child('''
assert anchor in s;s=s.replace(anchor,new,1)
old='''                    .child(picker_icon(
                        browser_caption(ICONS[self.browser.layout.icons[index]]).debug_selector(
                            move || format!("browser-picker-glyph-section-{index}"),
                        ),
                    ))''';assert old in s;s=s.replace(old,'                    .child(icon)')
anchor='            let active = self.browser.section == section;'
new='''            let active = self.browser.section == section;
            let icon = picker_icon(
                browser_caption(ICONS[self.browser.layout.icons[index]])
                    .debug_selector(move || format!("browser-picker-glyph-section-{index}")),
            );''';assert anchor in s;s=s.replace(anchor,new,1)
old='''                    .child(picker_icon(browser_caption("□").debug_selector(
                        move || format!("browser-picker-glyph-place-{index}"),
                    )))''';assert old in s;s=s.replace(old,'                    .child(icon)')
anchor='            let tooltip = name.to_owned();\n            sidebar = sidebar.child('
new='''            let tooltip = name.to_owned();
            let icon = picker_icon(
                browser_caption("□")
                    .debug_selector(move || format!("browser-picker-glyph-place-{index}")),
            );
            sidebar = sidebar.child('''
assert anchor in s;s=s.replace(anchor,new,1)
p.write_text(s)
