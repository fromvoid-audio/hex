from pathlib import Path
p=Path('crates/hex-ui/src/editor/browser/layout.rs');s=p.read_text()
s=s.replace('const ICONS:', '''// Match existing browser buttons: a 22px hit/centering slot plus the
// sidebar's existing 1px right border. Glyph padding lives inside the slot.
const PICKER_CONTROL_SIZE: f32 = 22.0;
const PICKER_BORDER_WIDTH: f32 = 1.0;
const PICKER_MIN_WIDTH: f32 = PICKER_CONTROL_SIZE + PICKER_BORDER_WIDTH;
const PICKER_LABEL_THRESHOLD: f32 = 64.0;

const ICONS:''',1)
s=s.replace('''        let width = self
            .sidebar_width
            .clamp(36.0, (base_width - 124.0).max(36.0));
        if width < 64.0 { 36.0 } else { width }''','''        self.sidebar_width
            .clamp(PICKER_MIN_WIDTH, (base_width - 124.0).max(PICKER_MIN_WIDTH))''')
s=s.replace('let compact = width < 64.0;', 'let compact = width < PICKER_LABEL_THRESHOLD;')
s=s.replace('''.clamp(36.0, (width - 124.0).max(36.0));''','''.clamp(PICKER_MIN_WIDTH, (width - 124.0).max(PICKER_MIN_WIDTH));''')
anchor='fn browser_divider() -> Div {'
s=s.replace(anchor,'''fn picker_icon(glyph: Div) -> Div {
    div()
        .size(px(PICKER_CONTROL_SIZE))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .child(glyph.whitespace_nowrap())
}

'''+anchor)
# Only the three persistent source-row types; settings/tag rows stay unchanged.
for height in ['22.0','25.0','23.0']:
 old=f'''                    .h(px({height}))
                    .px_2()
                    .flex()'''
 new=f'''                    .h(px({height}))
                    .w_full()
                    .min_w_0()
                    .flex_shrink_0()
                    .overflow_hidden()
                    .flex()'''
 assert s.count(old)==1,(height,s.count(old));s=s.replace(old,new)
s=s.replace('''                    .child(
                        browser_caption("■")''','''                    .child(picker_icon(
                        browser_caption("■")''',1)
s=s.replace('''                                format!("browser-picker-glyph-collection-{index}")
                            }),
                    )''','''                                format!("browser-picker-glyph-collection-{index}")
                            }),
                    ))''',1)
s=s.replace('''                    .child(
                        browser_caption(ICONS[self.browser.layout.icons[index]]).debug_selector(''','''                    .child(picker_icon(
                        browser_caption(ICONS[self.browser.layout.icons[index]]).debug_selector(''',1)
s=s.replace('''                            move || format!("browser-picker-glyph-section-{index}"),
                        ),
                    )''','''                            move || format!("browser-picker-glyph-section-{index}"),
                        ),
                    ))''',1)
s=s.replace('''                    .child(
                        browser_caption("□")''','''                    .child(picker_icon(
                        browser_caption("□")''',1)
s=s.replace('''                            .debug_selector(move || format!("browser-picker-glyph-place-{index}")),
                    )''','''                            .debug_selector(move || format!("browser-picker-glyph-place-{index}")),
                    ))''',1)
# Instrumentation is only for persistent picker glyphs, never configuration duplicates.
s=s.replace('''                                    browser_caption(ICONS[self.browser.layout.icons[index]])
                                        .debug_selector(move || {
                                            format!("browser-picker-glyph-section-{index}")
                                        }),''','''                                    browser_caption(ICONS[self.browser.layout.icons[index]]),''')
s=s.replace('''(!compact).then(|| browser_caption(name).overflow_hidden().text_ellipsis())''','''(!compact).then(|| {
                            browser_caption(name)
                                .flex_1()
                                .min_w_0()
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis()
                        })''')
p.write_text(s)
p=Path('crates/hex-ui/src/editor/browser_ui_test.rs');s=p.read_text()
s=s.replace('''    assert_eq!(compact.size.width, px(36.0));''','''    assert_eq!(
        compact.size.width,
        visual.debug_bounds("browser-control-⚙").unwrap().size.width + px(1.0)
    );''')
s=s.replace('''            assert_eq!(glyph.center().y, row.center().y);''','''            // GPUI rounds text origins to pixels while font line boxes may be fractional.
            assert!(f32::from(glyph.center().y - row.center().y).abs() <= 0.5);''')
p.write_text(s)
