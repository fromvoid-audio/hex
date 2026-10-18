//! Browser composition and transient visual review state. Tag storage stays outside the UI.
use super::*;

const SECTIONS: [(BrowserSection, &str); 4] = [
    (BrowserSection::Samples, "samples"),
    (BrowserSection::Instruments, "devices"),
    (BrowserSection::Projects, "projects"),
    (BrowserSection::All, "all"),
];
// Match existing browser buttons: a 22px hit/centering slot plus the
// sidebar's existing 1px right border. Glyph padding lives inside the slot.
const PICKER_CONTROL_SIZE: f32 = 22.0;
const PICKER_BORDER_WIDTH: f32 = 1.0;
const PICKER_MIN_WIDTH: f32 = PICKER_CONTROL_SIZE + PICKER_BORDER_WIDTH;
const PICKER_RESIZE_WIDTH: f32 = 4.0;
// Capture within one divider width; an extra divider width of intentional
// expansion releases the current gesture, avoiding threshold chatter.
fn picker_resize_width(raw: f32, maximum: f32, locked: bool) -> (f32, bool) {
    let width = raw.clamp(PICKER_MIN_WIDTH, maximum);
    let band = PICKER_RESIZE_WIDTH * if locked { 2.0 } else { 1.0 };
    let locked = width <= PICKER_MIN_WIDTH + band;
    (if locked { PICKER_MIN_WIDTH } else { width }, locked)
}

const ICONS: [&str; 7] = ["♫", "⌘", "▤", "◈", "≋", "□", "⌕"];
const TAG_GROUPS: [(&str, &[&str]); 6] = [
    ("content", &["audio clip", "midi clip", "preset", "sample"]),
    ("type", &["loop", "one shot", "impulse response"]),
    ("sounds", &["bass", "keys", "lead", "pad", "texture"]),
    ("drums", &["kick", "snare", "clap", "hat", "percussion"]),
    ("character", &["acoustic", "analog", "bright", "dark"]),
    ("devices", &["instrument", "audio effect", "midi effect"]),
];

pub(super) struct BrowserLayout {
    sidebar_width: f32,
    pub(super) sidebar_drag: Option<(f32, f32, bool)>,
    tags_visible: bool,
    editor_visible: bool,
    settings_visible: bool,
    group_open: [bool; 6],
    visible: [bool; 4],
    icons: [usize; 4],
}

impl Default for BrowserLayout {
    fn default() -> Self {
        Self {
            sidebar_width: 124.0,
            sidebar_drag: None,
            tags_visible: true,
            editor_visible: false,
            settings_visible: false,
            group_open: [true, true, false, false, false, false],
            visible: [true; 4],
            icons: [0, 1, 2, 6],
        }
    }
}

impl BrowserLayout {
    pub(super) fn reveal_all(&mut self) {
        if let Some(index) = SECTIONS
            .iter()
            .position(|(section, _)| *section == BrowserSection::All)
        {
            self.visible[index] = true;
        }
    }
}

impl BrowserLayout {
    pub(super) fn editor_width(&self, base_width: f32) -> f32 {
        if self.editor_visible {
            (base_width * 0.3).clamp(90.0, 190.0)
        } else {
            0.0
        }
    }

    pub(super) fn base_width_for_total(&self, total_width: f32) -> f32 {
        if self.editor_visible {
            total_width - (total_width * 0.3 / 1.3).clamp(90.0, 190.0)
        } else {
            total_width
        }
    }

    fn sidebar_width(&self, base_width: f32) -> f32 {
        self.sidebar_width
            .clamp(PICKER_MIN_WIDTH, (base_width - 124.0).max(PICKER_MIN_WIDTH))
    }
}

pub(super) fn browser_label_tooltip(label: impl AsRef<str>, cx: &mut App) -> gpui::AnyView {
    cx.new(|_| super::super::transport::MiniTooltip {
        description: theme::text(label),
    })
    .into()
}

fn browser_caption(value: impl AsRef<str>) -> Div {
    div().text_xs().child(theme::text(value))
}

fn picker_icon(icon: Div) -> Div {
    let icon = icon.whitespace_nowrap();
    div()
        .size(px(PICKER_CONTROL_SIZE))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .child(icon)
}

fn browser_divider() -> Div {
    div()
        .border_b_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)))
}

// Keep the same line box at every picker width: labels clip rather than
// wrapping, ellipsizing or removing rows when the icon-only width is reached.
fn picker_label(value: impl AsRef<str>) -> Div {
    browser_caption(value)
        .flex_1()
        .min_w_0()
        .whitespace_nowrap()
        .overflow_hidden()
}

fn picker_heading(value: &str) -> Div {
    browser_heading(value)
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .whitespace_nowrap()
}

// Keep the original heading's text line box laid out, but do not paint its
// descendants at the exact icon-only minimum. The separator is an overlay, so
// neither its height nor text length can move subsequent source rows.
fn picker_group_heading(value: &'static str, compact: bool) -> gpui::Stateful<Div> {
    div()
        .id(gpui::SharedString::from(format!(
            "browser-picker-heading-{value}"
        )))
        .debug_selector(move || format!("browser-picker-heading-{value}"))
        .relative()
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .child(
            div()
                .w_full()
                .min_w_0()
                .when(compact, |line| line.invisible())
                .child(
                    picker_heading(value)
                        .debug_selector(move || format!("browser-picker-heading-text-{value}")),
                ),
        )
        .when(compact, |heading| {
            heading.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(PICKER_CONTROL_SIZE))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .w(px(PICKER_CONTROL_SIZE - 2.0 * PICKER_RESIZE_WIDTH))
                            .h(px(PICKER_BORDER_WIDTH))
                            .bg(rgb(theme::color(theme::PaletteRole::Muted)))
                            .debug_selector(move || format!("browser-picker-heading-mark-{value}")),
                    ),
            )
        })
        .tooltip(move |_, cx| browser_label_tooltip(value, cx))
}

fn browser_heading(value: &str) -> Div {
    browser_caption(value)
        .overflow_hidden()
        .px_2()
        .py_1()
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
}

// Disabled controls reserve the requested widgets without implying saved tags/collections.
fn browser_pending(value: &str) -> Div {
    browser_caption(value)
        .px_1()
        .py(px(2.0))
        .bg(rgb(theme::color(theme::PaletteRole::Hover)))
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
}

impl EditorView {
    pub(super) fn browser_toolbar(&self, cx: &mut Context<Self>) -> Div {
        browser_divider()
            .h(px(30.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .px_1()
            .gap_1()
            .debug_selector(|| "browser-toolbar".to_owned())
            .child(
                browser_pending("‹")
                    .id("browser-history-back")
                    .debug_selector(|| "browser-history-back".to_owned())
                    .tooltip(|_, cx| {
                        cx.new(|_| super::super::transport::MiniTooltip {
                            description: theme::text("back · history pending"),
                        })
                        .into()
                    }),
            )
            .child(
                browser_pending("›")
                    .id("browser-history-forward")
                    .tooltip(|_, cx| {
                        cx.new(|_| super::super::transport::MiniTooltip {
                            description: theme::text("forward · history pending"),
                        })
                        .into()
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .capture_any_mouse_down(cx.listener(|view, event: &MouseDownEvent, _, cx| {
                        if event.button == MouseButton::Left {
                            view.browser.results_focused = false;
                            cx.notify();
                        }
                    }))
                    .child(self.browser.search.clone()),
            )
            .child(self.browser_icon(
                "≡",
                "show or hide tag filters",
                |view, _, cx| {
                    view.browser.layout.tags_visible = !view.browser.layout.tags_visible;
                    cx.notify();
                },
                cx,
            ))
            .child(self.browser_icon(
                "⋮",
                "open tag editor",
                |view, _, cx| {
                    view.browser.layout.editor_visible = !view.browser.layout.editor_visible;
                    cx.notify();
                },
                cx,
            ))
            .child(self.browser_icon(
                "×",
                "close browser · cmd+alt+b",
                |view, window, cx| {
                    view.toggle_browser(window, cx);
                },
                cx,
            ))
    }

    pub(super) fn browser_sidebar(
        &self,
        width: f32,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let width = self.browser.layout.sidebar_width(width);
        let compact = width == PICKER_MIN_WIDTH;
        let mut sidebar = div()
            .id("browser-sidebar")
            .w(px(width))
            .flex_shrink_0()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .debug_selector(|| "browser-sidebar".to_owned())
            .child(picker_group_heading("collections", compact));
        for (index, name) in ["favorites", "inspiration", "recent finds"]
            .into_iter()
            .enumerate()
        {
            let color = [
                hex_project::ElementColor::Red,
                hex_project::ElementColor::Cyan,
                hex_project::ElementColor::Gray,
            ][index];
            let icon = picker_icon(
                browser_caption("■")
                    .text_color(rgb(theme::element_colors(color).accent))
                    .debug_selector(move || format!("browser-picker-glyph-collection-{index}")),
            );
            sidebar = sidebar.child(
                div()
                    .id(("browser-collection", index))
                    .debug_selector(move || format!("browser-collection-{index}"))
                    .tooltip(move |_, cx| {
                        browser_label_tooltip(format!("{name} · collections pending"), cx)
                    })
                    .h(px(22.0))
                    .w_full()
                    .min_w_0()
                    .flex_shrink_0()
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .bg(rgb(theme::color(
                        if self.browser.section == BrowserSection::Collection(index) {
                            theme::PaletteRole::Hover
                        } else {
                            theme::PaletteRole::Surface
                        },
                    )))
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, window, cx| {
                            view.select_browser_source(BrowserSection::Collection(index), cx);
                            view.browser.results_focus.focus(window);
                            window.prevent_default();
                            cx.notify();
                        }),
                    )
                    .child(icon)
                    .child(picker_label(name).debug_selector(move || {
                        format!("browser-picker-label-collection-{index}")
                    })),
            );
        }
        sidebar = sidebar
            .child(picker_group_heading("collections pending", compact))
            .child(picker_group_heading("library", compact));
        for (index, (section, name)) in SECTIONS.into_iter().enumerate() {
            if !self.browser.layout.visible[index] {
                continue;
            }
            let active = self.browser.section == section;
            let icon = picker_icon(
                browser_caption(ICONS[self.browser.layout.icons[index]])
                    .debug_selector(move || format!("browser-picker-glyph-section-{index}")),
            );
            sidebar =
                sidebar.child(
                    div()
                        .id(("browser-section", index))
                        .debug_selector(move || format!("browser-section-{name}"))
                        .tooltip(move |_, cx| browser_label_tooltip(name, cx))
                        .h(px(25.0))
                        .w_full()
                        .min_w_0()
                        .flex_shrink_0()
                        .overflow_hidden()
                        .flex()
                        .items_center()
                        .gap_1()
                        .cursor_pointer()
                        .bg(rgb(theme::color(if active {
                            theme::PaletteRole::Hover
                        } else {
                            theme::PaletteRole::Surface
                        })))
                        .text_color(rgb(theme::color(if active {
                            theme::PaletteRole::Secondary
                        } else {
                            theme::PaletteRole::Text
                        })))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, _, window, cx| {
                                view.select_browser_source(section.clone(), cx);
                                view.browser.results_focus.focus(window);
                                window.prevent_default();
                                cx.notify();
                            }),
                        )
                        .child(icon)
                        .child(picker_label(name).debug_selector(move || {
                            format!("browser-picker-label-section-{index}")
                        })),
                );
        }
        sidebar = sidebar.child(self.browser_icon(
            "⚙",
            "configure sections · session only",
            move |view, _, cx| {
                view.browser.layout.settings_visible = !view.browser.layout.settings_visible;
                if view.browser.layout.settings_visible {
                    view.browser.layout.sidebar_width =
                        view.browser.layout.sidebar_width.max(124.0);
                }
                cx.notify();
            },
            cx,
        ));
        if self.browser.layout.settings_visible {
            sidebar = sidebar.child(picker_group_heading("sections · session", compact));
            for (index, (_, name)) in SECTIONS.into_iter().enumerate() {
                let visible = self.browser.layout.visible[index];
                sidebar = sidebar.child(
                    div()
                        .h(px(PICKER_CONTROL_SIZE))
                        .flex_shrink_0()
                        .min_w_0()
                        .overflow_hidden()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .id(("browser-section-visibility", index))
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |view, _, _, cx| {
                                        let layout = &mut view.browser.layout;
                                        if layout.visible[index]
                                            && layout.visible.iter().filter(|v| **v).count() == 1
                                        {
                                            return;
                                        }
                                        layout.visible[index] = !layout.visible[index];
                                        if !layout.visible[index]
                                            && view.browser.section == SECTIONS[index].0
                                            && let Some(next) =
                                                layout.visible.iter().position(|v| *v)
                                        {
                                            view.select_browser_source(
                                                SECTIONS[next].0.clone(),
                                                cx,
                                            );
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child(picker_label(format!(
                                    "{} {name}",
                                    if visible { "☑" } else { "□" },
                                ))),
                        )
                        .child(
                            div()
                                .id(("browser-section-icon", index))
                                .w(px(PICKER_CONTROL_SIZE))
                                .flex_shrink_0()
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |view, _, _, cx| {
                                        let icon = &mut view.browser.layout.icons[index];
                                        *icon = (*icon + 1) % ICONS.len();
                                        cx.notify();
                                    }),
                                )
                                .child(browser_caption(ICONS[self.browser.layout.icons[index]])),
                        ),
                );
            }
        }
        sidebar = sidebar.child(picker_group_heading("places", compact));
        for (index, place) in self.browser.places.iter().take(PLACE_CAPACITY).enumerate() {
            let path = place.clone();
            let icon_path = place.clone();
            let row_bounds = Rc::new(std::cell::Cell::new(None::<gpui::Bounds<gpui::Pixels>>));
            let measured_bounds = Rc::clone(&row_bounds);
            let name = if *place == self.browser.music {
                "music"
            } else if *place == self.browser.home {
                "home"
            } else {
                place
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("folder")
            };
            let tooltip = name.to_owned();
            let icon = picker_icon(
                browser_caption(self.browser_place_glyph(place))
                    .debug_selector(move || format!("browser-picker-glyph-place-{index}")),
            );
            sidebar = sidebar.child(
                div()
                    .relative()
                    .id(("browser-place", index))
                    .debug_selector(move || format!("browser-place-{index}"))
                    .tooltip(move |_, cx| browser_label_tooltip(&tooltip, cx))
                    .h(px(23.0))
                    .w_full()
                    .min_w_0()
                    .flex_shrink_0()
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .bg(rgb(theme::color(
                        if self.browser.section == BrowserSection::Place(place.clone()) {
                            theme::PaletteRole::Hover
                        } else {
                            theme::PaletteRole::Surface
                        },
                    )))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, window, cx| {
                            view.select_browser_source(BrowserSection::Place(path.clone()), cx);
                            view.browser.results_focus.focus(window);
                            window.prevent_default();
                            cx.notify();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                            view.open_browser_place_icons(
                                icon_path.clone(),
                                row_bounds.get().map_or(event.position, |bounds| {
                                    gpui::point(bounds.left(), bounds.bottom())
                                }),
                                window,
                                cx,
                            );
                            window.prevent_default();
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        canvas(
                            move |bounds, _, _| measured_bounds.set(Some(bounds)),
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .child(icon)
                    .child(
                        picker_label(name)
                            .debug_selector(move || format!("browser-picker-label-place-{index}")),
                    ),
            );
        }
        if self.browser.places.len() < PLACE_CAPACITY {
            sidebar = sidebar.child(self.browser_icon(
                "+",
                "add folder place",
                |view, _, cx| view.open_browser_folder(cx),
                cx,
            ));
        }
        if let Some(status) = &self.browser.places_status {
            let tooltip = status.clone();
            sidebar = sidebar.child(
                picker_heading(status)
                    .id("browser-places-status")
                    .debug_selector(|| "browser-places-status".into())
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .tooltip(move |_, cx| browser_label_tooltip(&tooltip, cx)),
            );
        }
        sidebar
    }

    pub(super) fn browser_sidebar_resize(
        &self,
        width: f32,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let entity = cx.entity();
        let sidebar_width = self.browser.layout.sidebar_width(width);
        div()
            .id("browser-sidebar-resize")
            .debug_selector(|| "browser-sidebar-resize".to_owned())
            .w(px(PICKER_RESIZE_WIDTH))
            .h_full()
            .flex_shrink_0()
            .cursor_col_resize()
            .bg(rgb(theme::color(theme::PaletteRole::Border)))
            .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Secondary))))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let down_entity = entity.clone();
                        window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
                            if phase.bubble()
                                && event.button == MouseButton::Left
                                && bounds.contains(&event.position)
                            {
                                down_entity.update(cx, |view, _| {
                                    view.browser.layout.sidebar_drag = Some((
                                        f32::from(event.position.x),
                                        sidebar_width,
                                        sidebar_width == PICKER_MIN_WIDTH,
                                    ));
                                });
                            }
                        });
                        let move_entity = entity.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if !phase.bubble() {
                                return;
                            }
                            move_entity.update(cx, |view, cx| {
                                if !event.dragging() {
                                    view.browser.layout.sidebar_drag = None;
                                } else if let Some((start, initial, locked)) =
                                    view.browser.layout.sidebar_drag
                                {
                                    let (next, locked) = picker_resize_width(
                                        initial + f32::from(event.position.x) - start,
                                        (width - 124.0).max(PICKER_MIN_WIDTH),
                                        locked,
                                    );
                                    view.browser.layout.sidebar_width = next;
                                    view.browser.layout.sidebar_drag =
                                        Some((start, initial, locked));
                                    cx.notify();
                                }
                            });
                        });
                        let up_entity = entity.clone();
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                            if phase.bubble() && event.button == MouseButton::Left {
                                up_entity.update(cx, |view, _| {
                                    view.browser.layout.sidebar_drag = None;
                                });
                            }
                        });
                    },
                )
                .size_full(),
            )
    }

    pub(super) fn browser_tag_selector(&self, cx: &mut Context<Self>) -> Div {
        let mut tags = div()
            .id("browser-tag-selector")
            .max_h(px(180.0))
            .flex_shrink_0()
            .min_h_0()
            .overflow_y_scroll()
            .debug_selector(|| "browser-tag-selector".to_owned());
        if !self.browser.layout.tags_visible {
            return div();
        }
        for (index, (name, values)) in TAG_GROUPS.into_iter().enumerate() {
            let open = self.browser.layout.group_open[index];
            let mut group = browser_divider().child(
                div()
                    .id(("browser-tag-group", index))
                    .cursor_pointer()
                    .px_2()
                    .py(px(2.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _, cx| {
                            view.browser.layout.group_open[index] =
                                !view.browser.layout.group_open[index];
                            cx.notify();
                        }),
                    )
                    .child(browser_caption(format!(
                        "{name} {}",
                        if open { "▾" } else { "▸" }
                    ))),
            );
            if open {
                group = group.child(
                    div()
                        .px_2()
                        .pb_1()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .children(values.iter().map(|value| browser_pending(value))),
                );
            }
            tags = tags.child(group);
        }
        div().child(tags.child(browser_heading("tag filtering pending")))
    }

    pub(super) fn browser_tag_editor(&self, width: f32, cx: &mut Context<Self>) -> Option<Div> {
        if !self.browser.layout.editor_visible {
            return None;
        }
        let mut editor = div()
            .id("browser-tag-editor")
            .w(px(self.browser.layout.editor_width(width)))
            .flex_shrink_0()
            .min_h_0()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .debug_selector(|| "browser-tag-editor".to_owned())
            .child(
                div()
                    .h(px(26.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .bg(rgb(theme::color(theme::PaletteRole::Hover)))
                    .child(browser_caption("tag editor").px_1().flex_1())
                    .child(self.browser_icon(
                        "⊗",
                        "close tag editor",
                        |view, _, cx| {
                            view.browser.layout.editor_visible = false;
                            cx.notify();
                        },
                        cx,
                    )),
            )
            .child(browser_heading("assignment pending"))
            .child(
                browser_caption(self.browser_selected_label(cx))
                    .px_2()
                    .overflow_hidden()
                    .text_ellipsis(),
            );
        let mut groups = div()
            .id("browser-tag-editor-groups")
            .min_h_0()
            .overflow_y_scroll();
        for (name, values) in TAG_GROUPS {
            groups = groups.child(browser_divider().child(browser_heading(name)).children(
                values.iter().map(|value| {
                    browser_caption(format!("□ {value}"))
                        .px_2()
                        .py_1()
                        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                }),
            ));
        }
        editor = editor.child(groups);
        Some(div().flex().flex_shrink_0().min_h_0().child(editor))
    }

    pub(super) fn browser_selected_label(&self, cx: &App) -> String {
        let files = self.browser_visible_entries(cx);
        let index = self.browser.selected_index;
        if let Some(entry) = files.get(index) {
            entry.name.clone()
        } else {
            self.browser_visible_devices(cx)
                .get(index.saturating_sub(files.len()))
                .map_or_else(
                    || "no item selected".into(),
                    |(_, entry)| entry.name.to_string(),
                )
        }
    }

    pub(super) fn browser_selection_preview(&mut self, cx: &mut Context<Self>) -> Div {
        let entry = if self.browser.section == BrowserSection::Instruments {
            None
        } else {
            self.browser_visible_entries(cx)
                .get(self.browser.selected_index)
                .cloned()
        };
        browser_divider()
            .h(px(50.0))
            .flex_shrink_0()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .border_t_1()
            .debug_selector(|| "browser-selection-preview".to_owned())
            .child(
                browser_divider()
                    .h(px(26.0))
                    .flex_shrink_0()
                    .min_w_0()
                    .overflow_hidden()
                    .px_1()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(browser_caption("tags:").whitespace_nowrap().flex_shrink_0())
                    .child(browser_pending("add…").whitespace_nowrap().flex_shrink_0())
                    .child(div().flex_1().min_w_0())
                    .child(self.browser_icon(
                        "⋯",
                        "open full tag editor",
                        |view, _, cx| {
                            view.browser.layout.editor_visible = true;
                            cx.notify();
                        },
                        cx,
                    )),
            )
            .child(self.browser_audition_row(entry, cx))
    }
}
