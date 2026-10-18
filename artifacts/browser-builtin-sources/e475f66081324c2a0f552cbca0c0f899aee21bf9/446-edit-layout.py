from pathlib import Path
p=Path('crates/hex-ui/src/editor/browser/layout.rs');s=p.read_text()
s=s.replace('impl BrowserLayout {\n    pub(super) fn reveal_all', '''impl BrowserLayout {
    pub(super) fn samples_visible(&self) -> bool { self.visible[0] }
    pub(super) fn set_samples_visible(&mut self, visible: bool) { self.visible[0] = visible; }
    pub(super) fn reveal_all''',1)
s=s.replace('for (index, (section, name)) in SECTIONS.into_iter().enumerate() {','''for index in [3, 0, 1, 2] {
            let (section, name) = SECTIONS[index].clone();''',1)
s=s.replace('''                        .child(icon)
                        .child(picker_label(name)''','''                        .on_mouse_down(MouseButton::Right, cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                            if name == "samples" {
                                view.open_browser_samples_menu(event.position, window, cx);
                                cx.stop_propagation();
                                window.prevent_default();
                            }
                        }))
                        .child(icon)
                        .child(picker_label(name)''',1)
s=s.replace('"configure sections · session only"','"configure browser sources"').replace('picker_heading("sections · session")','picker_heading("sources")')
s=s.replace('''                                        let layout = &mut view.browser.layout;
                                        if layout.visible[index]''','''                                        if index == 0 {
                                            view.set_browser_source_visible(BrowserSection::Samples, !view.browser.layout.visible[index], cx);
                                            return;
                                        }
                                        let layout = &mut view.browser.layout;
                                        if layout.visible[index]''',1)
s=s.replace('''        sidebar = sidebar.child(picker_heading("places"));''','''        if self.browser.layout.settings_visible {
            for (index, place) in self.browser.places.iter().take(PLACE_CAPACITY).enumerate() {
                let path = place.clone();
                let visible = self.browser_place_visible(place);
                let name = if *place == self.browser.music { "music".to_owned() }
                    else if *place == self.browser.home { "home".to_owned() }
                    else { place.file_name().unwrap_or_default().to_string_lossy().into_owned() };
                let tooltip = name.clone();
                sidebar = sidebar.child(div().id(("browser-place-visibility", index))
                    .debug_selector(move || format!("browser-place-visibility-{index}"))
                    .h(px(PICKER_CONTROL_SIZE)).w_full().min_w_0().flex_shrink_0()
                    .overflow_hidden().whitespace_nowrap().cursor_pointer()
                    .tooltip(move |_, cx| browser_label_tooltip(&tooltip, cx))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |view, _, window, cx| {
                        view.set_browser_source_visible(BrowserSection::Place(path.clone()), !visible, cx);
                        view.browser.results_focus.focus(window);
                        window.prevent_default();
                    }))
                    .child(picker_label(format!("{} {name}", if visible { "☑" } else { "□" }))));
            }
        }
        sidebar = sidebar.child(picker_heading("places"));''',1)
s=s.replace('''        for (index, place) in self.browser.places.iter().take(PLACE_CAPACITY).enumerate() {
            let path''','''        for (index, place) in self.browser.places.iter().take(PLACE_CAPACITY).enumerate() {
            if !self.browser_place_visible(place) { continue; }
            let path''',1)
# Stable debug selector for real settings clicks, not replacing existing id.
s=s.replace('''.id(("browser-section-visibility", index))
                                .flex_1()''','''.id(("browser-section-visibility", index))
                                .debug_selector(move || format!("browser-section-visibility-{index}"))
                                .flex_1()''',1)
p.write_text(s)
p=Path('crates/hex-ui/src/editor/browser/place_icons.rs');s=p.read_text()
a=s.index('                    // Newly added paths')
b=s.index('                    if let Err(error)',a)
s=s[:a]+s[b:]
a=s.index('    fn browser_preferences_snapshot_for_visibility(')
b=s.index('    fn reconcile_browser_source_visibility',a)
s=s[:a]+s[b:]
p.write_text(s)
p=Path('crates/hex-ui/src/editor/browser.rs');s=p.read_text().replace('''            Ok(places) => {
                self.browser.pending_places_save''','''            Ok(places) => {
                self.browser.source_preferences = places.clone();
                self.browser.pending_places_save''',1);p.write_text(s)
