from pathlib import Path
p=Path('crates/hex-ui/src/editor/browser/layout.rs');s=p.read_text()
a=s.index('    pub(super) fn browser_sidebar_resize(');b=s.index('    pub(super) fn browser_tag_selector(',a);block=s[a:b]
block=block.replace('''.bg(rgb(theme::color(theme::PaletteRole::Border)))''','''.bg(rgb(theme::color(if self.browser.layout.sidebar_drag.is_some() {
                theme::PaletteRole::Secondary
            } else {
                theme::PaletteRole::Border
            })))''',1)
block=block.replace('''down_entity.update(cx, |view, _| {''','''down_entity.update(cx, |view, cx| {''',1).replace('''Some((f32::from(event.position.x), sidebar_width));
                                });''','''Some((f32::from(event.position.x), sidebar_width));
                                    cx.notify();
                                });''',1)
block=block.replace('''                                    view.browser.layout.sidebar_drag = None;
                                } else if''','''                                    if view.browser.layout.sidebar_drag.take().is_some() {
                                        cx.notify();
                                    }
                                } else if''',1)
block=block.replace('''up_entity.update(cx, |view, _| {
                                    view.browser.layout.sidebar_drag = None;
                                });''','''up_entity.update(cx, |view, cx| {
                                    if view.browser.layout.sidebar_drag.take().is_some() {
                                        cx.notify();
                                    }
                                });''',1)
s=s[:a]+block+s[b:];p.write_text(s)
p=Path('crates/hex-ui/src/editor/browser.rs');s=p.read_text();a=s.index('        let mut resize = div()\n            .id("browser-resize-handle")');b=s.index('        div()\n            .track_focus(&self.browser.results_focus)',a);block=s[a:b]
block=block.replace('''.bg(rgb(theme::color(theme::PaletteRole::Border)))''','''.bg(rgb(theme::color(if self.browser.resizing {
                theme::PaletteRole::Secondary
            } else {
                theme::PaletteRole::Border
            })))''',1)
block=block.replace('''                        if !phase.bubble() || !event.dragging() {
                            return;
                        }
                        move_entity.update(cx, |view, cx| {
                            if view.browser.resizing {''','''                        if !phase.bubble() {
                            return;
                        }
                        move_entity.update(cx, |view, cx| {
                            if !event.dragging() {
                                if view.browser.resizing {
                                    view.browser.resizing = false;
                                    cx.notify();
                                }
                                return;
                            }
                            if view.browser.resizing {''',1)
s=s[:a]+block+s[b:];p.write_text(s)
