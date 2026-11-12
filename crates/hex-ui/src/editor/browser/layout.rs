//! Browser composition and transient visual review state. Tag storage stays outside the UI.
use super::*;

const SECTIONS: [(BrowserSection, &str); 5] = [
    (BrowserSection::Samples, "samples"),
    (BrowserSection::Instruments, "devices"),
    (BrowserSection::Projects, "projects"),
    (BrowserSection::All, "all"),
    (BrowserSection::Plugins, "vst3"),
];
// Retain the 22px control-column width plus the sidebar's one-pixel border.
// Compact row height is independent of that existing resize minimum.
const PICKER_CONTROL_SIZE: f32 = 22.0;
const PICKER_ROW_HEIGHT: f32 = 18.0;
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

const ICONS: [&str; 7] = [
    "icons/browser-place/samples.svg",
    "icons/browser-place/devices.svg",
    "icons/browser-place/projects.svg",
    "icons/browser-place/collection.svg",
    "icons/browser-place/waveform.svg",
    "icons/browser-place/folder.svg",
    "icons/browser-place/search.svg",
];
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
    visible: [bool; 5],
    icons: [usize; 5],
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
            visible: [true; 5],
            icons: [0, 1, 2, 6, 3],
        }
    }
}

impl BrowserLayout {
    pub(super) fn begin_sidebar_resize(&mut self, x: f32, initial: f32) {
        self.sidebar_drag = Some((x, initial, initial == PICKER_MIN_WIDTH));
    }
    pub(super) fn update_sidebar_resize(&mut self, x: f32, width: f32) {
        if let Some((start, initial, locked)) = self.sidebar_drag {
            let (next, locked) = picker_resize_width(
                initial + x - start,
                (width - 124.0).max(PICKER_MIN_WIDTH),
                locked,
            );
            self.sidebar_width = next;
            self.sidebar_drag = Some((start, initial, locked));
        }
    }
    pub(super) fn history_source_available(&self, source: &BrowserSection) -> bool {
        SECTIONS
            .iter()
            .position(|(section, _)| section == source)
            .is_none_or(|index| self.visible[index])
    }

    pub(super) fn samples_visible(&self) -> bool {
        self.visible[0]
    }
    pub(super) fn set_samples_visible(&mut self, visible: bool) {
        self.visible[0] = visible;
    }
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
        kind: crate::tooltips::Kind::Identification,
        description: theme::text(label),
    })
    .into()
}

fn browser_caption(value: impl AsRef<str>) -> Div {
    div().text_xs().child(theme::text(value))
}

fn browser_vector_icon(path: &'static str) -> Div {
    browser_vector_icon_colored(path, theme::color(theme::PaletteRole::Text))
}

fn browser_vector_icon_colored(path: &'static str, ink: u32) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .child(gpui::svg().path(path).size(px(14.0)).text_color(rgb(ink)))
}

fn picker_icon(icon: Div) -> Div {
    let icon = icon.whitespace_nowrap();
    div()
        .w(px(PICKER_CONTROL_SIZE))
        .h(px(PICKER_ROW_HEIGHT))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .child(icon)
}

// Width/minimum-resize custody stays independent of compact row height.
fn picker_ink(active: bool, missing: bool) -> u32 {
    selection_ink(
        active,
        if missing {
            theme::PaletteRole::Muted
        } else {
            theme::PaletteRole::Text
        },
    )
}

pub(super) fn selection_ink(active: bool, idle: theme::PaletteRole) -> u32 {
    if active {
        theme::readable_text(
            theme::color(theme::PaletteRole::Secondary),
            theme::color(theme::PaletteRole::Background),
        )
    } else {
        theme::color(idle)
    }
}

/// One visual row contract for files, native devices, plugins and virtual groups.
pub(super) fn result_row(selected: bool) -> Div {
    div()
        .h(px(ROW_HEIGHT))
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .px_1()
        .flex()
        .items_center()
        .gap_1()
        .cursor_pointer()
        .bg(rgb(theme::color(if selected {
            theme::PaletteRole::Secondary
        } else {
            theme::PaletteRole::Surface
        })))
        .when(!selected, |row| {
            row.hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Hover))))
        })
        .text_size(px(10.0))
        .line_height(px(ROW_HEIGHT))
        .text_color(rgb(selection_ink(selected, theme::PaletteRole::Text)))
}

pub(super) fn result_label(
    value: impl AsRef<str>,
    selected: bool,
    idle: theme::PaletteRole,
) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .line_clamp(1)
        .text_ellipsis()
        .h(px(ROW_HEIGHT))
        .text_size(px(10.0))
        .line_height(px(ROW_HEIGHT))
        .text_color(rgb(selection_ink(selected, idle)))
        .child(theme::text(value))
}

fn picker_row(active: bool, missing: bool) -> Div {
    div()
        .h(px(PICKER_ROW_HEIGHT))
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .overflow_hidden()
        .flex()
        .items_center()
        .gap_1()
        .bg(rgb(theme::color(if active {
            theme::PaletteRole::Secondary
        } else {
            theme::PaletteRole::Surface
        })))
        .text_color(rgb(picker_ink(active, missing)))
}

// Keep the full capture gutter independent of its visible separator. The leading
// line adjoins the existing 1px panel border without consuming layout width.
pub(super) fn resize_divider_line(
    group: &'static str,
    selector: &'static str,
    active: bool,
) -> Div {
    div()
        .absolute()
        .left(px(0.0))
        .top(px(0.0))
        .w(px(1.0))
        .h_full()
        .bg(rgb(theme::color(if active {
            theme::PaletteRole::Secondary
        } else {
            theme::PaletteRole::Border
        })))
        .group_hover(group, |style| {
            style.bg(rgb(theme::color(theme::PaletteRole::Secondary)))
        })
        .debug_selector(move || selector.to_owned())
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
        .text_size(px(10.0))
        .line_height(px(PICKER_ROW_HEIGHT))
        .h(px(PICKER_ROW_HEIGHT))
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
    fn browser_history_arrow(&self, forward: bool, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let enabled = if forward {
            self.browser.history.can_forward()
        } else {
            self.browser.history.can_back()
        };
        let selector = if forward {
            "browser-history-forward"
        } else {
            "browser-history-back"
        };
        hex_widgets::action(
            theme::text(if forward { "›" } else { "‹" }),
            theme::action_style(),
        )
        .id(selector)
        .debug_selector(move || selector.to_owned())
        .size(px(18.0))
        .text_xs()
        .when(!enabled, |button| button.cursor_default().opacity(0.4))
        .when(enabled, |button| {
            button
                .cursor_pointer()
                .text_color(rgb(theme::color(theme::PaletteRole::Text)))
        })
        .tooltip(move |_, cx| {
            cx.new(|_| super::super::transport::MiniTooltip {
                kind: crate::tooltips::Kind::Identification,
                description: theme::text(if forward { "forward" } else { "back" }),
            })
            .into()
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |view, _, window, cx| {
                if enabled {
                    view.move_browser_history(forward, cx);
                } else {
                    // A disabled arrow is inert, including the ancestor's
                    // ordinary non-text focus transfer and text checkpoint.
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }),
        )
    }

    pub(super) fn browser_toolbar(&self, cx: &mut Context<Self>) -> Div {
        browser_divider()
            .h(px(24.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .px_1()
            .gap(px(2.0))
            .debug_selector(|| "browser-toolbar".to_owned())
            .child(self.browser_history_arrow(false, cx))
            .child(self.browser_history_arrow(true, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .capture_any_mouse_down(cx.listener(|view, event: &MouseDownEvent, _, cx| {
                        if event.button == MouseButton::Left {
                            view.checkpoint_browser_history(cx);
                            view.browser.results_focused = false;
                            cx.notify();
                        }
                    }))
                    .child(self.browser.search.clone()),
            )
            .child(self.browser_icon(
                "↻",
                "refresh browser source",
                |view, _, cx| view.refresh_browser_tree(cx),
                cx,
            ).size(px(18.0)))
            .child(self.browser_reveal_button(cx).size(px(18.0)))
            .child(
                self.browser_icon(
                    "⌗",
                    "toggle input quantization · shared with midi recording · enter inserts on selected grid",
                    |view, _, cx| {
                        view.toggle_input_quantization();
                        cx.notify();
                    },
                    cx,
                ).size(px(18.0)).rounded(px(hex_widgets::TOGGLE_CORNER_RADIUS))
                .debug_selector(|| "browser-insert-grid".to_owned())
                .when(
                    self.input_quantization != hex_project::InputQuantization::Off,
                    |button| button.text_color(rgb(theme::color(theme::PaletteRole::Primary))),
                ),
            )
            .child(self.browser_icon(
                "≡",
                "show or hide tag filters",
                |view, _, cx| {
                    view.browser.layout.tags_visible = !view.browser.layout.tags_visible;
                    cx.notify();
                },
                cx,
            ).size(px(18.0)).rounded(px(hex_widgets::TOGGLE_CORNER_RADIUS)))
            .child(self.browser_icon(
                "⋮",
                "show or hide tag editor",
                |view, _, cx| {
                    view.browser.layout.editor_visible = !view.browser.layout.editor_visible;
                    cx.notify();
                },
                cx,
            ).size(px(18.0)).rounded(px(hex_widgets::TOGGLE_CORNER_RADIUS)))
            .child(self.browser_icon(
                "×",
                "close browser · cmd+alt+b",
                |view, window, cx| {
                    view.toggle_browser(window, cx);
                },
                cx,
            ).size(px(18.0)))
    }

    fn browser_reveal_button(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let target = self.browser_selected_sample(cx);
        let enabled = target.is_some();
        hex_widgets::action(theme::text("◎"), theme::action_style())
            .id("browser-reveal-sample")
            .debug_selector(|| "browser-reveal-sample".to_owned())
            .w(px(22.0))
            .h(px(22.0))
            .text_xs()
            .when(!enabled, |button| button.cursor_default().opacity(0.4))
            .tooltip(move |_, cx| {
                browser_label_tooltip(
                    if enabled {
                        "show selected sample"
                    } else {
                        "no sample selected in current results"
                    },
                    cx,
                )
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, _, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    if let Some(path) = &target {
                        view.reveal_browser_sample(path, window, cx);
                    }
                }),
            )
    }

    fn browser_collections(&self, compact: bool, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let mut collections = div()
            .id("browser-collections")
            .debug_selector(|| "browser-collections".into())
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .child(picker_group_heading("collections", compact));
        for (index, name) in ["favorites", "inspiration", "recent finds"]
            .into_iter()
            .enumerate()
        {
            let active = self.browser.section == BrowserSection::Collection(index);
            let color = [
                hex_project::ElementColor::Red,
                hex_project::ElementColor::Cyan,
                hex_project::ElementColor::Rose,
            ][index];
            let icon = picker_icon(
                div()
                    .size(px(8.0))
                    .bg(rgb(theme::element_colors(color).accent))
                    .debug_selector(move || format!("browser-picker-glyph-collection-{index}")),
            );
            collections = collections.child(
                picker_row(active, false)
                    .id(("browser-collection", index))
                    .debug_selector(move || format!("browser-collection-{index}"))
                    .tooltip(move |_, cx| {
                        browser_label_tooltip(format!("{name} · collections pending"), cx)
                    })
                    .cursor_pointer()
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
        collections
    }

    pub(super) fn browser_sidebar(
        &self,
        width: f32,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let width = self.browser.layout.sidebar_width(width);
        let compact = width == PICKER_MIN_WIDTH;
        let mut sidebar = div()
            .id("browser-sidebar-sources")
            .debug_selector(|| "browser-sidebar-sources".into())
            .w_full()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .child(picker_group_heading("library", compact));
        for index in [3, 0, 1, 2, 4] {
            let (section, name) = SECTIONS[index].clone();
            if !self.browser.layout.visible[index] {
                continue;
            }
            let active = self.browser.section == section;
            let icon = picker_icon(
                browser_vector_icon_colored(
                    ICONS[self.browser.layout.icons[index]],
                    picker_ink(active, false),
                )
                .debug_selector(move || format!("browser-picker-glyph-section-{index}")),
            );
            sidebar =
                sidebar.child(
                    picker_row(active, false)
                        .id(("browser-section", index))
                        .debug_selector(move || format!("browser-section-{name}"))
                        .tooltip(move |_, cx| browser_label_tooltip(name, cx))
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, _, window, cx| {
                                view.select_browser_source(section.clone(), cx);
                                view.browser.results_focus.focus(window);
                                window.prevent_default();
                                cx.notify();
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                                if name == "samples" {
                                    view.open_browser_samples_menu(event.position, window, cx);
                                    cx.stop_propagation();
                                    window.prevent_default();
                                }
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
            "configure browser sources",
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
            sidebar = sidebar.children([self.plugin_scan_settings(cx)]);
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
                                .debug_selector(move || {
                                    format!("browser-section-visibility-{index}")
                                })
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .cursor_pointer()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |view, _, _, cx| {
                                        if index == 0 {
                                            view.set_browser_source_visible(
                                                BrowserSection::Samples,
                                                !view.browser.layout.visible[index],
                                                cx,
                                            );
                                            return;
                                        }
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
                                .child(browser_vector_icon(
                                    ICONS[self.browser.layout.icons[index]],
                                )),
                        ),
                );
            }
        }
        if self.browser.layout.settings_visible {
            for (index, place) in self.browser.places.iter().take(PLACE_CAPACITY).enumerate() {
                let path = place.clone();
                let visible = self.browser_place_visible(place);
                let name = if *place == self.browser.music {
                    "music".to_owned()
                } else if *place == self.browser.home {
                    "home".to_owned()
                } else {
                    place
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                };
                let tooltip = name.clone();
                sidebar = sidebar.child(
                    div()
                        .id(("browser-place-visibility", index))
                        .debug_selector(move || format!("browser-place-visibility-{index}"))
                        .h(px(PICKER_CONTROL_SIZE))
                        .w_full()
                        .min_w_0()
                        .flex_shrink_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .cursor_pointer()
                        .tooltip(move |_, cx| browser_label_tooltip(&tooltip, cx))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, _, window, cx| {
                                view.set_browser_source_visible(
                                    BrowserSection::Place(path.clone()),
                                    !visible,
                                    cx,
                                );
                                view.browser.results_focus.focus(window);
                                window.prevent_default();
                            }),
                        )
                        .child(picker_label(format!(
                            "{} {name}",
                            if visible { "☑" } else { "□" }
                        ))),
                );
            }
        }
        let active = self.browser.section == BrowserSection::ProjectSamples;
        let sources = sidebar
            .child(self.browser_places(compact, cx))
            .child(picker_group_heading("project", compact))
            .child(
                picker_row(active, false)
                    .id("browser-project-samples")
                    .debug_selector(|| "browser-project-samples".to_owned())
                    .tooltip(|_, cx| browser_label_tooltip("project samples", cx))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|view, _, window, cx| {
                            view.select_browser_source(BrowserSection::ProjectSamples, cx);
                            view.browser.results_focus.focus(window);
                            window.prevent_default();
                            cx.notify();
                        }),
                    )
                    .child(picker_icon(browser_vector_icon_colored(
                        ICONS[0],
                        picker_ink(active, false),
                    )))
                    .child(picker_label("samples")),
            );
        div()
            .id("browser-sidebar")
            .debug_selector(|| "browser-sidebar".to_owned())
            .w(px(width))
            .flex_shrink_0()
            .min_h_0()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(rgb(theme::color(theme::PaletteRole::Border)))
            .child(sources)
            .child(self.browser_collections(compact, cx))
    }

    fn browser_places(&self, compact: bool, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let drop_entity = cx.entity();
        let mut places = div()
            .id("browser-places-drop")
            .debug_selector(|| "browser-places-drop".into())
            .w_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .can_drop(move |payload, _, cx| {
                payload
                    .downcast_ref::<folder_places::FolderDrag>()
                    .is_some_and(|drag| {
                        let view = drop_entity.read(cx);
                        view.folder_drag_current(drag, cx)
                            && view.browser.preferences_writable
                            && !view.browser.folder_bookmark_pending
                            && (view.browser.places.contains(&drag.path)
                                || view.browser.places.len() < PLACE_CAPACITY)
                    })
            })
            .drag_over::<folder_places::FolderDrag>(|style, _, _, _| {
                style
                    .bg(rgb(theme::color(theme::PaletteRole::Hover)))
                    .border_color(rgb(theme::color(theme::PaletteRole::Secondary)))
            })
            .on_drop(
                cx.listener(|view, drag: &folder_places::FolderDrag, _, cx| {
                    view.bookmark_browser_folder(drag, cx);
                }),
            )
            .child(picker_group_heading("places", compact));
        for (index, place) in self.browser.places.iter().take(PLACE_CAPACITY).enumerate() {
            if !self.browser_place_visible(place) {
                continue;
            }
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
            let missing = self.browser_place_missing(place);
            let active = self.browser.section == BrowserSection::Place(place.clone());
            let label = if missing {
                format!("{name} · missing")
            } else {
                name.to_owned()
            };
            let tooltip = if missing {
                format!("{name} · folder missing · right-click to relocate")
            } else {
                name.to_owned()
            };
            let icon = picker_icon(
                browser_vector_icon_colored(
                    self.browser_place_icon_path(place),
                    picker_ink(active, missing),
                )
                .debug_selector(move || format!("browser-picker-glyph-place-{index}")),
            );
            places = places.child(
                picker_row(active, missing)
                    .relative()
                    .id(("browser-place", index))
                    .debug_selector(move || format!("browser-place-{index}"))
                    .tooltip(move |_, cx| browser_label_tooltip(&tooltip, cx))
                    .cursor_pointer()
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
                        picker_label(label)
                            .debug_selector(move || format!("browser-picker-label-place-{index}")),
                    ),
            );
        }
        if self.browser.places.len() < PLACE_CAPACITY {
            places = places.child(self.browser_icon(
                "+",
                "add folder place",
                |view, _, cx| view.open_browser_folder(cx),
                cx,
            ));
        }
        if let Some(status) = &self.browser.places_status {
            let tooltip = status.clone();
            places = places.child(
                picker_heading(status)
                    .id("browser-places-status")
                    .debug_selector(|| "browser-places-status".into())
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .tooltip(move |_, cx| browser_label_tooltip(&tooltip, cx)),
            );
        }
        places
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
            .relative()
            .group("browser-sidebar-resize")
            .child(resize_divider_line(
                "browser-sidebar-resize",
                "browser-sidebar-resize-line",
                self.browser.layout.sidebar_drag.is_some(),
            ))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event, window, cx| {
                    view.begin_browser_resize(
                        super::resize::Divider::Sidebar,
                        event,
                        sidebar_width,
                        window,
                        cx,
                    );
                }),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, cx| {
                        entity.read(cx).renew_browser_resize(
                            super::resize::Divider::Sidebar,
                            window,
                            entity.entity_id(),
                        );
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
        match self.browser_item_at(self.browser.selected_index, cx) {
            Some(key @ BrowserItemKey::ProjectSample(..)) => self
                .browser_history_index(&key, cx)
                .and_then(|index| {
                    self.browser_project_samples(cx)
                        .get(index)
                        .map(|row| row.name.clone())
                })
                .unwrap_or_else(|| "no item selected".into()),
            Some(BrowserItemKey::Plugin(path, id)) => self
                .browser_visible_plugins(cx)
                .iter()
                .find(|row| row.module == path && row.cid == id)
                .map_or_else(|| "no item selected".into(), |row| row.name.clone()),
            Some(BrowserItemKey::File(path)) => path.file_name().map_or_else(
                || "no item selected".into(),
                |name| name.to_string_lossy().into_owned(),
            ),
            Some(BrowserItemKey::Device(class, id)) => self
                .browser_visible_devices(cx)
                .iter()
                .find(|(candidate, entry)| *candidate == class && entry.type_id == id)
                .map_or_else(
                    || "no item selected".into(),
                    |(_, entry)| entry.name.to_string(),
                ),
            None => "no item selected".into(),
        }
    }

    pub(super) fn browser_selection_preview(&mut self, cx: &mut Context<Self>) -> Div {
        let entry = if self.browser.section == BrowserSection::Instruments {
            None
        } else {
            self.browser_selected_entry(cx)
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
                        "show or hide tag editor",
                        |view, _, cx| {
                            view.browser.layout.editor_visible =
                                !view.browser.layout.editor_visible;
                            cx.notify();
                        },
                        cx,
                    )),
            )
            .child(self.browser_audition_row(entry, cx))
    }
}
