//! Browser composition and transient visual review state. Tag storage stays outside the UI.
use super::*;

const SECTIONS: [(BrowserSection, &str); 3] = [
    (BrowserSection::Samples, "samples"),
    (BrowserSection::Instruments, "devices"),
    (BrowserSection::Projects, "projects"),
];
const ICONS: [&str; 6] = ["♫", "⌘", "▤", "◈", "≋", "□"];
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
    pub(super) sidebar_drag: Option<(f32, f32)>,
    tags_visible: bool,
    editor_visible: bool,
    settings_visible: bool,
    group_open: [bool; 6],
    visible: [bool; 3],
    icons: [usize; 3],
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
            visible: [true; 3],
            icons: [0, 1, 2],
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
        let width = self
            .sidebar_width
            .clamp(36.0, (base_width - 124.0).max(36.0));
        if width < 64.0 { 36.0 } else { width }
    }
}

fn browser_label_tooltip(label: impl AsRef<str>, cx: &mut App) -> gpui::AnyView {
    cx.new(|_| super::super::transport::MiniTooltip {
        description: theme::text(label),
    })
    .into()
}

fn browser_caption(value: impl AsRef<str>) -> Div {
    div().text_xs().child(theme::text(value))
}

fn browser_divider() -> Div {
    div()
        .border_b_1()
        .border_color(rgb(theme::color(theme::PaletteRole::Border)))
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
            .child(div().flex_1().min_w_0().child(self.browser.search.clone()))
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
        let compact = width < 64.0;
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
            .children((!compact).then(|| browser_heading("collections")));
        for (index, name) in ["favorites", "inspiration", "recent finds"]
            .into_iter()
            .enumerate()
        {
            let color = [
                hex_project::ElementColor::Red,
                hex_project::ElementColor::Cyan,
                hex_project::ElementColor::Gray,
            ][index];
            sidebar = sidebar.child(
                div()
                    .id(("browser-collection", index))
                    .tooltip(move |_, cx| {
                        browser_label_tooltip(format!("{name} · collections pending"), cx)
                    })
                    .h(px(22.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(
                        browser_caption("■").text_color(rgb(theme::element_colors(color).accent)),
                    )
                    .children(
                        (!compact).then(|| browser_caption(name).overflow_hidden().text_ellipsis()),
                    ),
            );
        }
        if !compact {
            sidebar = sidebar.child(browser_heading("collections pending"));
        }
        sidebar = sidebar.children((!compact).then(|| browser_heading("library")));
        for (index, (section, name)) in SECTIONS.into_iter().enumerate() {
            if !self.browser.layout.visible[index] {
                continue;
            }
            let active = self.browser.section == section;
            sidebar = sidebar.child(
                div()
                    .id(("browser-section", index))
                    .debug_selector(move || format!("browser-section-{name}"))
                    .tooltip(move |_, cx| browser_label_tooltip(name, cx))
                    .overflow_hidden()
                    .h(px(25.0))
                    .px_2()
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
                        cx.listener(move |view, _, _, cx| {
                            view.browser.section = section;
                            if section == BrowserSection::Samples {
                                view.browser.samples.source = None;
                            }
                            view.browser.selected_index = 0;
                            view.browser.results_focused = false;
                            cx.notify();
                        }),
                    )
                    .child(browser_caption(ICONS[self.browser.layout.icons[index]]))
                    .children(
                        (!compact).then(|| browser_caption(name).overflow_hidden().text_ellipsis()),
                    ),
            );
        }
        sidebar = sidebar.child(self.browser_icon(
            "⚙",
            "configure sections · session only",
            move |view, _, cx| {
                view.browser.layout.settings_visible =
                    compact || !view.browser.layout.settings_visible;
                if view.browser.layout.settings_visible {
                    view.browser.layout.sidebar_width =
                        view.browser.layout.sidebar_width.max(124.0);
                }
                cx.notify();
            },
            cx,
        ));
        if self.browser.layout.settings_visible && !compact {
            sidebar = sidebar.child(browser_heading("sections · session"));
            for (index, (_, name)) in SECTIONS.into_iter().enumerate() {
                let visible = self.browser.layout.visible[index];
                sidebar = sidebar.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .id(("browser-section-visibility", index))
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
                                            view.browser.section = SECTIONS[next].0;
                                            view.browser.selected_index = 0;
                                        }
                                        cx.notify();
                                    }),
                                )
                                .child(browser_caption(format!(
                                    "{} {}",
                                    if visible { "☑" } else { "□" },
                                    if compact { "" } else { name }
                                ))),
                        )
                        .child(
                            div()
                                .id(("browser-section-icon", index))
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
        sidebar = sidebar.children((!compact).then(|| browser_heading("places")));
        for (index, place) in self.browser.places.iter().take(PLACE_CAPACITY).enumerate() {
            let path = place.clone();
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
            sidebar = sidebar.child(
                div()
                    .id(("browser-place", index))
                    .tooltip(move |_, cx| browser_label_tooltip(&tooltip, cx))
                    .h(px(23.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .overflow_hidden()
                    .bg(rgb(theme::color(if self.browser.directory == *place {
                        theme::PaletteRole::Hover
                    } else {
                        theme::PaletteRole::Surface
                    })))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _, cx| {
                            if view.browser.section == BrowserSection::Instruments {
                                view.browser.section = BrowserSection::Samples;
                            }
                            if view.browser.section == BrowserSection::Samples
                                && path != view.browser.home
                            {
                                view.browser.samples.source = Some(path.clone());
                                view.browser.selected_index = 0;
                                cx.notify();
                            } else {
                                if path == view.browser.home {
                                    view.browser.section = BrowserSection::Projects;
                                }
                                view.request_browser_scan(path.clone(), cx);
                            }
                        }),
                    )
                    .child(browser_caption("□"))
                    .children(
                        (!compact).then(|| browser_caption(name).overflow_hidden().text_ellipsis()),
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
                browser_heading(if compact { "!" } else { status })
                    .id("browser-places-status")
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
            .w(px(4.0))
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
                                    view.browser.layout.sidebar_drag =
                                        Some((f32::from(event.position.x), sidebar_width));
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
                                } else if let Some((start, initial)) =
                                    view.browser.layout.sidebar_drag
                                {
                                    view.browser.layout.sidebar_width =
                                        (initial + f32::from(event.position.x) - start)
                                            .clamp(36.0, (width - 124.0).max(36.0));
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

    fn browser_selected_label(&self, cx: &App) -> String {
        if self.browser.section == BrowserSection::Instruments {
            let query = self.browser.search.read(cx).content.to_lowercase();
            self.browser
                .device_catalog
                .as_ref()
                .and_then(|catalog| catalog.as_ref().ok())
                .and_then(|entries| {
                    entries
                        .iter()
                        .filter(|(_, entry)| {
                            query.is_empty() || entry.name.to_lowercase().contains(&query)
                        })
                        .nth(self.browser.selected_index)
                })
                .map_or_else(
                    || "no item selected".into(),
                    |(_, entry)| entry.name.to_string(),
                )
        } else {
            self.browser_visible_entries(cx)
                .get(self.browser.selected_index)
                .map_or_else(|| "no item selected".into(), |entry| entry.name.clone())
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
        let mut preview = browser_divider()
            .h(px(108.0))
            .flex_shrink_0()
            .min_w_0()
            .flex()
            .flex_col()
            .border_t_1()
            .debug_selector(|| "browser-selection-preview".to_owned())
            .child(
                browser_divider()
                    .h(px(26.0))
                    .flex_shrink_0()
                    .px_1()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(browser_caption("tags:"))
                    .child(browser_pending("add…"))
                    .child(div().flex_1())
                    .child(self.browser_icon(
                        "⋯",
                        "open full tag editor",
                        |view, _, cx| {
                            view.browser.layout.editor_visible = true;
                            cx.notify();
                        },
                        cx,
                    )),
            );
        if let Some(entry) = entry {
            preview = preview.child(
                browser_caption(&entry.name)
                    .px_2()
                    .overflow_hidden()
                    .text_ellipsis(),
            );
            if entry.kind == EntryKind::WaveAudio {
                let rate = self
                    .state
                    .as_ref()
                    .map_or(48_000, |state| state.project.sample_rate());
                let slot = self.browser_preview_cell(entry.path.clone(), rate);
                if slot.metadata.borrow().is_none() && slot.error.borrow().is_none() {
                    self.queue_browser_audio_preview(
                        PreviewKey {
                            path: entry.path.clone(),
                            sample_rate: rate,
                        },
                        slot.clone(),
                        cx,
                    );
                }
                let metadata = slot.metadata.borrow().clone();
                let waveform = div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .bg(rgb(theme::color(theme::PaletteRole::ClipFill)));
                let waveform_panel = if let Some(metadata) = metadata {
                    waveform.child(super::super::waveform::waveform_summary_view(Arc::clone(
                        &metadata.waveform,
                    )))
                } else {
                    let key = PreviewKey {
                        path: entry.path.clone(),
                        sample_rate: rate,
                    };
                    let error = self
                        .browser
                        .preview_cache
                        .get(&key)
                        .is_some_and(|slot| slot.error.borrow().is_some());
                    waveform.child(browser_heading(if error {
                        "waveform unavailable"
                    } else {
                        "preparing waveform…"
                    }))
                };
                preview = preview.child(waveform_panel);
                preview = preview.child(
                    div()
                        .h(px(22.0))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .child(self.browser_icon(
                            "▶",
                            "audition selected sample",
                            move |view, _, cx| {
                                view.play_browser_sample(entry.path.clone(), cx);
                            },
                            cx,
                        ))
                        .child(browser_heading("sample preview")),
                );
                return preview;
            }
            preview.child(browser_heading(if entry.kind == EntryKind::Directory {
                "folder"
            } else {
                "project"
            }))
        } else {
            preview = preview.child(
                browser_caption(self.browser_selected_label(cx))
                    .px_2()
                    .overflow_hidden()
                    .text_ellipsis(),
            );
            preview.child(browser_heading(
                if self.browser.section == BrowserSection::Instruments {
                    "device preview · tags pending"
                } else {
                    "select an item to preview"
                },
            ))
        }
    }
}
