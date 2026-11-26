//! Browser composition and transient visual review state. Tag storage stays outside the UI.
use super::*;

const SECTIONS: [(BrowserSection, &str); 5] = [
    (BrowserSection::Samples, "samples"),
    (BrowserSection::Instruments, "devices"),
    (BrowserSection::Projects, "projects"),
    (BrowserSection::All, "all"),
    (BrowserSection::Plugins, "vst3"),
];
const SECTION_PREFS: [hex_preferences::BrowserSidebarSection; 5] = [
    hex_preferences::BrowserSidebarSection::Samples,
    hex_preferences::BrowserSidebarSection::Devices,
    hex_preferences::BrowserSidebarSection::Projects,
    hex_preferences::BrowserSidebarSection::All,
    hex_preferences::BrowserSidebarSection::Plugins,
];
const DEFAULT_SECTION_ORDER: [usize; 5] = [3, 0, 1, 2, 4];
const DEFAULT_SECTION_ICONS: [usize; 5] = [0, 1, 2, 6, 3];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CollectionItem {
    pub(super) name: String,
    pub(super) color: hex_project::ElementColor,
}

fn default_collections() -> Vec<CollectionItem> {
    vec![
        CollectionItem {
            name: "favorites".into(),
            color: hex_project::ElementColor::Red,
        },
        CollectionItem {
            name: "inspiration".into(),
            color: hex_project::ElementColor::Cyan,
        },
        CollectionItem {
            name: "recent finds".into(),
            color: hex_project::ElementColor::Rose,
        },
    ]
}

fn element_color_name(color: hex_project::ElementColor) -> &'static str {
    match color {
        hex_project::ElementColor::Gray => "gray",
        hex_project::ElementColor::Red => "red",
        hex_project::ElementColor::Orange => "orange",
        hex_project::ElementColor::Amber => "amber",
        hex_project::ElementColor::Olive => "olive",
        hex_project::ElementColor::Green => "green",
        hex_project::ElementColor::Cyan => "cyan",
        hex_project::ElementColor::Blue => "blue",
        hex_project::ElementColor::Violet => "violet",
        hex_project::ElementColor::Rose => "rose",
    }
}

fn element_color_from_name(name: &str) -> hex_project::ElementColor {
    match name {
        "gray" => hex_project::ElementColor::Gray,
        "red" => hex_project::ElementColor::Red,
        "orange" => hex_project::ElementColor::Orange,
        "amber" => hex_project::ElementColor::Amber,
        "olive" => hex_project::ElementColor::Olive,
        "green" => hex_project::ElementColor::Green,
        "cyan" => hex_project::ElementColor::Cyan,
        "blue" => hex_project::ElementColor::Blue,
        "violet" => hex_project::ElementColor::Violet,
        "rose" => hex_project::ElementColor::Rose,
        _ => hex_project::ElementColor::Gray,
    }
}
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

const MINI_TAGS_DEFAULT_HEIGHT: f32 = 26.0;
const MINI_TAGS_MAX_HEIGHT: f32 = 140.0;

pub(super) struct BrowserLayout {
    sidebar_width: f32,
    pub(super) sidebar_width_customized: bool,
    pub(super) sidebar_drag: Option<(f32, f32, bool)>,
    pub(super) mini_tags_height: f32,
    pub(super) mini_tags_drag: Option<(f32, f32)>,
    tags_visible: bool,
    pub(super) tags_visible_customized: bool,
    editor_visible: bool,
    settings_visible: bool,
    group_open: [bool; 6],
    editor_group_open: [bool; 6],
    mini_tag_input: Option<gpui::Entity<TextInput>>,
    mini_tag_error: Option<String>,
    pub(super) collection_add_input: Option<gpui::Entity<TextInput>>,
    pub(super) collection_rename_input: Option<(usize, gpui::Entity<TextInput>)>,
    visible: [bool; 5],
    icons: [usize; 5],
    section_icon_ids: [Option<String>; 5],
    section_order: [usize; 5],
    pub(super) collections: Vec<CollectionItem>,
}

impl Default for BrowserLayout {
    fn default() -> Self {
        Self {
            sidebar_width: 124.0,
            sidebar_width_customized: false,
            sidebar_drag: None,
            mini_tags_height: MINI_TAGS_DEFAULT_HEIGHT,
            mini_tags_drag: None,
            tags_visible: true,
            tags_visible_customized: false,
            editor_visible: false,
            settings_visible: false,
            group_open: [true, true, false, false, false, false],
            editor_group_open: [true; 6],
            mini_tag_input: None,
            mini_tag_error: None,
            collection_add_input: None,
            collection_rename_input: None,
            visible: [true; 5],
            icons: DEFAULT_SECTION_ICONS,
            section_icon_ids: Default::default(),
            section_order: DEFAULT_SECTION_ORDER,
            collections: default_collections(),
        }
    }
}

impl BrowserLayout {
    pub(super) fn begin_mini_tags_resize(&mut self, y: f32, initial: f32) {
        self.mini_tags_drag = Some((y, initial));
    }

    pub(super) fn update_mini_tags_resize(&mut self, y: f32, window_height: f32) {
        if let Some((start_y, initial)) = self.mini_tags_drag {
            let max_height = if window_height.is_finite() && window_height > 260.0 {
                MINI_TAGS_MAX_HEIGHT.min(window_height * 0.35)
            } else {
                MINI_TAGS_MAX_HEIGHT
            }
            .max(MINI_TAGS_DEFAULT_HEIGHT);
            // Dragging upward (smaller y) grows the mini tag strip upward into results.
            let next = (initial + (start_y - y)).clamp(MINI_TAGS_DEFAULT_HEIGHT, max_height);
            self.mini_tags_height = next;
        }
    }

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
    pub(super) fn can_hide_section(&self, index: usize) -> bool {
        index < self.visible.len()
            && (!self.visible[index] || self.visible.iter().filter(|v| **v).count() > 1)
    }
    pub(super) fn first_visible_section(&self) -> BrowserSection {
        for index in self.section_order {
            if self.visible[index] {
                return SECTIONS[index].0.clone();
            }
        }
        BrowserSection::All
    }
    pub(super) fn collection_name(&self, index: usize) -> Option<&str> {
        self.collections.get(index).map(|item| item.name.as_str())
    }
    pub(super) fn section_icon_id(&self, index: usize) -> Option<&str> {
        self.section_icon_ids.get(index)?.as_deref()
    }
    pub(super) fn section_icon_path(&self, index: usize) -> &'static str {
        if let Some(id) = self.section_icon_id(index) {
            place_icons::path_for_id(id).unwrap_or(ICONS[5])
        } else {
            ICONS[self.icons[index]]
        }
    }
    pub(super) fn set_section_icon_id(&mut self, index: usize, id: Option<&'static str>) {
        if index >= self.icons.len() {
            return;
        }
        self.section_icon_ids[index] = id.map(str::to_owned);
        if let Some(path) = id.and_then(place_icons::path_for_id)
            && let Some(pos) = ICONS.iter().position(|candidate| *candidate == path)
        {
            self.icons[index] = pos;
        } else {
            self.icons[index] = DEFAULT_SECTION_ICONS[index];
        }
    }
    pub(super) fn move_section_in_order(&mut self, order_pos: usize, delta: isize) -> bool {
        let target = order_pos as isize + delta;
        if order_pos >= self.section_order.len()
            || target < 0
            || (target as usize) >= self.section_order.len()
        {
            return false;
        }
        self.section_order.swap(order_pos, target as usize);
        true
    }
    pub(super) fn apply_preferences(&mut self, prefs: &BrowserPlaces) {
        for (index, section) in SECTION_PREFS.into_iter().enumerate() {
            self.visible[index] = !prefs.sidebar_section_hidden(section);
            let saved_icon = prefs.sidebar_section_icon(section).map(str::to_owned);
            self.section_icon_ids[index] = saved_icon.clone();
            if let Some(id) = saved_icon.as_deref() {
                if let Some(path) = place_icons::path_for_id(id)
                    && let Some(pos) = ICONS.iter().position(|candidate| *candidate == path)
                {
                    self.icons[index] = pos;
                } else {
                    self.icons[index] = 5;
                }
            } else {
                self.icons[index] = DEFAULT_SECTION_ICONS[index];
            }
        }
        if !self.visible.iter().any(|v| *v) {
            self.visible[3] = true;
        }
        let saved_order = prefs.sidebar_order();
        if saved_order.len() == SECTION_PREFS.len() {
            let mut next = DEFAULT_SECTION_ORDER;
            let mut ok = true;
            for (slot, section) in saved_order.iter().enumerate() {
                if let Some(idx) = SECTION_PREFS.iter().position(|s| s == section) {
                    next[slot] = idx;
                } else {
                    ok = false;
                }
            }
            if ok {
                self.section_order = next;
            }
        }
        if let Some(saved_collections) = prefs.collections() {
            self.collections = saved_collections
                .iter()
                .map(|item| CollectionItem {
                    name: item.name.clone(),
                    color: element_color_from_name(&item.color),
                })
                .collect();
        }
        if let Some(width) = prefs.sidebar_width() {
            self.sidebar_width = width as f32;
            self.sidebar_width_customized = true;
        }
        if let Some(tags_visible) = prefs.tags_visible() {
            self.tags_visible = tags_visible;
            self.tags_visible_customized = true;
        }
    }
    pub(super) fn write_preferences(
        &self,
        places: &mut BrowserPlaces,
    ) -> Result<(), hex_preferences::PreferencesError> {
        // First clear hidden flags so re-enabling sections never hits the
        // last-visible-section check mid-update, then apply hidden flags.
        for (index, section) in SECTION_PREFS.into_iter().enumerate() {
            if self.visible[index] {
                places.set_sidebar_section_hidden(section, false)?;
            }
        }
        for (index, section) in SECTION_PREFS.into_iter().enumerate() {
            if !self.visible[index] {
                places.set_sidebar_section_hidden(section, true)?;
            }
            places.set_sidebar_section_icon(section, self.section_icon_ids[index].as_deref())?;
        }
        if self.section_order != DEFAULT_SECTION_ORDER {
            let order = self
                .section_order
                .into_iter()
                .map(|idx| SECTION_PREFS[idx])
                .collect();
            places.set_sidebar_order(order)?;
        } else {
            places.set_sidebar_order(Vec::new())?;
        }
        let defaults = default_collections();
        if self.collections != defaults {
            let stored = self
                .collections
                .iter()
                .map(|item| hex_preferences::BrowserCollectionPreference {
                    name: item.name.clone(),
                    color: element_color_name(item.color).to_owned(),
                })
                .collect();
            places.set_collections(Some(stored))?;
        } else {
            places.set_collections(None)?;
        }
        if self.sidebar_width_customized {
            let clamped = (self.sidebar_width.round() as u32).clamp(
                hex_preferences::MIN_SIDEBAR_WIDTH,
                hex_preferences::MAX_SIDEBAR_WIDTH,
            );
            places.set_sidebar_width(Some(clamped))?;
        }
        if self.tags_visible_customized {
            places.set_tags_visible(Some(self.tags_visible));
        }
        Ok(())
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
                    view.browser.layout.tags_visible_customized = true;
                    view.sync_sidebar_preferences_and_save(cx);
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
        for (index, item) in self.browser.layout.collections.iter().enumerate() {
            let active = self.browser.section == BrowserSection::Collection(index);
            let color = item.color;
            let name = item.name.clone();
            let (available, missing) = self.browser.samples.collection_counts(&name);
            let tooltip = if missing > 0 {
                format!("{name} · {available} items · {missing} missing")
            } else if available == 0 {
                format!("{name} · empty")
            } else if available == 1 {
                format!("{name} · 1 item")
            } else {
                format!("{name} · {available} items")
            };
            let icon = picker_icon(
                div()
                    .size(px(8.0))
                    .bg(rgb(theme::element_colors(color).accent))
                    .debug_selector(move || format!("browser-picker-glyph-collection-{index}")),
            );
            if let Some((renaming_idx, input)) = self.browser.layout.collection_rename_input.clone()
                && renaming_idx == index
            {
                let input_for_keys = input.clone();
                collections = collections.child(
                    picker_row(active, false)
                        .id(("browser-collection", index))
                        .debug_selector(move || format!("browser-collection-{index}"))
                        .child(icon)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .on_key_down(cx.listener(
                                    move |view, event: &gpui::KeyDownEvent, _, cx| {
                                        match event.keystroke.key.as_str() {
                                            "enter" => {
                                                let raw = input_for_keys.read(cx).text().to_owned();
                                                if let Err(err) =
                                                    view.rename_browser_collection(index, &raw, cx)
                                                {
                                                    view.browser.places_status = Some(err.into());
                                                } else {
                                                    view.browser.layout.collection_rename_input =
                                                        None;
                                                }
                                                cx.stop_propagation();
                                                cx.notify();
                                            }
                                            "escape" => {
                                                view.browser.layout.collection_rename_input = None;
                                                cx.stop_propagation();
                                                cx.notify();
                                            }
                                            _ => {}
                                        }
                                    },
                                ))
                                .child(input),
                        ),
                );
                continue;
            }
            collections = collections.child(
                picker_row(active, false)
                    .id(("browser-collection", index))
                    .debug_selector(move || format!("browser-collection-{index}"))
                    .tooltip(move |_, cx| browser_label_tooltip(&tooltip, cx))
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
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                            view.open_browser_collection_menu(index, event.position, window, cx);
                            window.prevent_default();
                            cx.stop_propagation();
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
        for index in self.browser.layout.section_order {
            let (section, name) = SECTIONS[index].clone();
            if !self.browser.layout.visible[index] {
                continue;
            }
            let active = self.browser.section == section;
            let icon = picker_icon(
                browser_vector_icon_colored(
                    self.browser.layout.section_icon_path(index),
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
            sidebar = sidebar.child(picker_group_heading("sections", compact));
            let section_order = self.browser.layout.section_order;
            for (order_pos, index) in section_order.into_iter().enumerate() {
                let (_, name) = SECTIONS[index];
                let visible = self.browser.layout.visible[index];
                let can_up = order_pos > 0;
                let can_down = order_pos + 1 < section_order.len();
                sidebar = sidebar.child(
                    div()
                        .h(px(PICKER_CONTROL_SIZE))
                        .flex_shrink_0()
                        .min_w_0()
                        .overflow_hidden()
                        .flex()
                        .items_center()
                        .gap(px(2.0))
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
                                        if view.browser.layout.visible[index]
                                            && !view.browser.layout.can_hide_section(index)
                                        {
                                            view.browser.places_status = Some(
                                                "at least one browser section must stay visible"
                                                    .into(),
                                            );
                                            cx.notify();
                                            return;
                                        }
                                        view.browser.layout.visible[index] =
                                            !view.browser.layout.visible[index];
                                        view.reconcile_browser_source_visibility(cx);
                                        view.sync_sidebar_preferences_and_save(cx);
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
                                .id(("browser-section-move-up", order_pos))
                                .debug_selector(move || format!("browser-section-move-up-{index}"))
                                .w(px(14.0))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_xs()
                                .when(can_up, |el| el.cursor_pointer())
                                .when(!can_up, |el| el.opacity(0.3))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |view, _, window, cx| {
                                        window.prevent_default();
                                        cx.stop_propagation();
                                        if view.browser.layout.move_section_in_order(order_pos, -1)
                                        {
                                            view.sync_sidebar_preferences_and_save(cx);
                                            cx.notify();
                                        }
                                    }),
                                )
                                .child(theme::text("↑")),
                        )
                        .child(
                            div()
                                .id(("browser-section-move-down", order_pos))
                                .debug_selector(move || {
                                    format!("browser-section-move-down-{index}")
                                })
                                .w(px(14.0))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_xs()
                                .when(can_down, |el| el.cursor_pointer())
                                .when(!can_down, |el| el.opacity(0.3))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |view, _, window, cx| {
                                        window.prevent_default();
                                        cx.stop_propagation();
                                        if view.browser.layout.move_section_in_order(order_pos, 1) {
                                            view.sync_sidebar_preferences_and_save(cx);
                                            cx.notify();
                                        }
                                    }),
                                )
                                .child(theme::text("↓")),
                        )
                        .child(
                            div()
                                .id(("browser-section-icon", index))
                                .debug_selector(move || format!("browser-section-icon-{index}"))
                                .w(px(PICKER_CONTROL_SIZE))
                                .flex_shrink_0()
                                .cursor_pointer()
                                .tooltip(|_, cx| browser_label_tooltip("choose section icon", cx))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                                        view.open_browser_section_icons(
                                            index,
                                            event.position,
                                            window,
                                            cx,
                                        );
                                        window.prevent_default();
                                        cx.stop_propagation();
                                    }),
                                )
                                .child(browser_vector_icon(
                                    self.browser.layout.section_icon_path(index),
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
        let icon = picker_icon(browser_vector_icon_colored(
            ICONS[0],
            picker_ink(active, false),
        ));
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
                    .child(icon)
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
        let active_count = self.browser.samples.active_tag_count();
        if active_count > 0 {
            let matching = self.browser_visible_entries(cx).len();
            tags = tags.child(
                div()
                    .px_2()
                    .py(px(2.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_1()
                    .border_b_1()
                    .border_color(rgb(theme::color(theme::PaletteRole::Border)))
                    .child(
                        browser_caption(format!("{active_count} active · {matching} matching"))
                            .text_color(rgb(theme::color(theme::PaletteRole::Muted))),
                    )
                    .child(
                        div()
                            .id("browser-tag-clear")
                            .debug_selector(|| "browser-tag-clear".to_owned())
                            .cursor_pointer()
                            .px_1()
                            .rounded(px(2.0))
                            .bg(rgb(theme::color(theme::PaletteRole::Raised)))
                            .hover(|s| s.bg(rgb(theme::color(theme::PaletteRole::Hover))))
                            .text_xs()
                            .text_color(rgb(theme::color(theme::PaletteRole::Text)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|view, _, window, cx| {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                    view.browser.samples.clear_active_tags();
                                    view.browser.selected_index = 0;
                                    cx.notify();
                                }),
                            )
                            .child(theme::text("clear")),
                    ),
            );
        }
        for (index, (name, values)) in TAG_GROUPS.into_iter().enumerate() {
            let open = self.browser.layout.group_open[index];
            let selected_in_group = self.browser.samples.active_tag_groups[index].len();
            let heading = if selected_in_group > 0 {
                format!(
                    "{name} ({selected_in_group}) {}",
                    if open { "▾" } else { "▸" }
                )
            } else {
                format!("{name} {}", if open { "▾" } else { "▸" })
            };
            let mut group = browser_divider().child(
                div()
                    .id(("browser-tag-group", index))
                    .debug_selector(move || format!("browser-tag-group-{index}"))
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
                    .child(browser_caption(heading)),
            );
            if open {
                group = group.child(div().px_2().pb_1().flex().flex_wrap().gap_1().children(
                    values.iter().enumerate().map(|(value_idx, &value)| {
                        let active = self.browser.samples.is_group_tag_active(index, value);
                        let chip_bg = if active {
                            theme::color(theme::PaletteRole::Secondary)
                        } else {
                            theme::color(theme::PaletteRole::Background)
                        };
                        let chip_ink = if active {
                            theme::readable_text(
                                chip_bg,
                                theme::color(theme::PaletteRole::Background),
                            )
                        } else {
                            theme::color(theme::PaletteRole::Text)
                        };
                        div()
                            .id(("browser-tag-chip", index * 32 + value_idx))
                            .debug_selector(move || format!("browser-tag-chip-{index}-{value}"))
                            .cursor_pointer()
                            .px_1()
                            .py(px(1.0))
                            .rounded(px(2.0))
                            .border_1()
                            .border_color(rgb(theme::color(if active {
                                theme::PaletteRole::Secondary
                            } else {
                                theme::PaletteRole::Border
                            })))
                            .bg(rgb(chip_bg))
                            .text_xs()
                            .text_color(rgb(chip_ink))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |view, _, window, cx| {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                    view.browser.samples.toggle_group_tag(index, value);
                                    view.browser.selected_index = 0;
                                    cx.notify();
                                }),
                            )
                            .child(theme::text(value))
                    }),
                ));
            }
            tags = tags.child(group);
        }
        div().child(tags)
    }

    fn browser_tag_target_sample(&self, cx: &App) -> Option<PathBuf> {
        if self.browser.section == BrowserSection::Instruments {
            return None;
        }
        self.browser_selected_entry(cx)
            .filter(|entry| entry.kind == EntryKind::WaveAudio)
            .map(|entry| entry.path)
    }

    pub(super) fn browser_tag_editor(&self, width: f32, cx: &mut Context<Self>) -> Option<Div> {
        if !self.browser.layout.editor_visible {
            return None;
        }
        let target_path = self.browser_tag_target_sample(cx);
        let assigned_tags: std::collections::BTreeSet<String> = target_path
            .as_deref()
            .and_then(|path| self.browser.samples.catalog.tags_for(path))
            .cloned()
            .unwrap_or_default();
        let header_status = if target_path.is_some() {
            format!("{} assigned", assigned_tags.len())
        } else {
            "select a sample to edit tags".into()
        };
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
            .child(browser_heading(&header_status))
            .child(
                browser_caption(self.browser_selected_label(cx))
                    .px_2()
                    .overflow_hidden()
                    .text_ellipsis(),
            );
        let mut groups = div()
            .id("browser-tag-editor-groups")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        if !self.browser.layout.collections.is_empty() {
            let mut col_section = browser_divider().child(browser_heading("collections"));
            for (idx, item) in self.browser.layout.collections.iter().enumerate() {
                let col_name = item.name.clone();
                let col_color = item.color;
                let assigned = target_path.as_deref().is_some_and(|path| {
                    self.browser
                        .samples
                        .catalog
                        .collection_members(&col_name)
                        .is_some_and(|members| members.contains(path))
                });
                let path_for_click = target_path.clone();
                let enabled = path_for_click.is_some();
                let label = format!("{} {col_name}", if assigned { "☑" } else { "☐" });
                col_section = col_section.child(
                    div()
                        .id(("browser-collection-assign", idx))
                        .debug_selector(move || format!("browser-collection-assign-{idx}"))
                        .px_2()
                        .py(px(2.0))
                        .flex()
                        .items_center()
                        .gap_1()
                        .when(enabled, |row| {
                            row.cursor_pointer()
                                .hover(|s| s.bg(rgb(theme::color(theme::PaletteRole::Hover))))
                        })
                        .text_xs()
                        .text_color(rgb(theme::color(if assigned {
                            theme::PaletteRole::Secondary
                        } else if enabled {
                            theme::PaletteRole::Text
                        } else {
                            theme::PaletteRole::Muted
                        })))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, _, window, cx| {
                                window.prevent_default();
                                cx.stop_propagation();
                                if let Some(ref path) = path_for_click
                                    && let Err(err) =
                                        view.toggle_sample_collection(path, &col_name, cx)
                                {
                                    view.browser.layout.mini_tag_error = Some(err.into());
                                }
                                cx.notify();
                            }),
                        )
                        .child(
                            div()
                                .size(px(6.0))
                                .flex_shrink_0()
                                .bg(rgb(theme::element_colors(col_color).accent)),
                        )
                        .child(theme::text(label)),
                );
            }
            if let Some(input) = self.browser.layout.collection_add_input.clone() {
                let input_for_keys = input.clone();
                col_section = col_section.child(
                    div()
                        .id("browser-collection-add-input")
                        .debug_selector(|| "browser-collection-add-input".to_owned())
                        .px_2()
                        .py(px(2.0))
                        .on_key_down(cx.listener(move |view, event: &gpui::KeyDownEvent, _, cx| {
                            match event.keystroke.key.as_str() {
                                "enter" => {
                                    let raw = input_for_keys.read(cx).text().to_owned();
                                    match view.create_browser_collection(
                                        &raw,
                                        hex_project::ElementColor::Amber,
                                        cx,
                                    ) {
                                        Ok(_) => {
                                            view.browser.layout.collection_add_input = None;
                                        }
                                        Err(err) => {
                                            view.browser.layout.mini_tag_error = Some(err.into());
                                        }
                                    }
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                                "escape" => {
                                    view.browser.layout.collection_add_input = None;
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                                _ => {}
                            }
                        }))
                        .child(input),
                );
            } else {
                col_section = col_section.child(
                    div()
                        .id("browser-collection-add")
                        .debug_selector(|| "browser-collection-add".to_owned())
                        .px_2()
                        .py(px(2.0))
                        .cursor_pointer()
                        .text_xs()
                        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|view, _, window, cx| {
                                window.prevent_default();
                                cx.stop_propagation();
                                let input = cx.new(browser_input::tag_input);
                                let focus = input.read(cx).focus_handle().clone();
                                view.browser.layout.collection_add_input = Some(input);
                                focus.focus(window);
                                cx.notify();
                            }),
                        )
                        .child(theme::text("+ new collection…")),
                );
            }
            groups = groups.child(col_section);
        }
        for (group_idx, (name, values)) in TAG_GROUPS.into_iter().enumerate() {
            let open = self.browser.layout.editor_group_open[group_idx];
            let mut section = browser_divider().child(
                div()
                    .id(("browser-tag-editor-group", group_idx))
                    .cursor_pointer()
                    .px_2()
                    .py(px(2.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _, cx| {
                            view.browser.layout.editor_group_open[group_idx] =
                                !view.browser.layout.editor_group_open[group_idx];
                            cx.notify();
                        }),
                    )
                    .child(browser_caption(format!(
                        "{name} {}",
                        if open { "▾" } else { "▸" }
                    ))),
            );
            if open {
                for (val_idx, &value) in values.iter().enumerate() {
                    let checked = assigned_tags.contains(value);
                    let path_for_click = target_path.clone();
                    let label = format!("{} {value}", if checked { "☑" } else { "☐" });
                    let enabled = path_for_click.is_some();
                    section = section.child(
                        div()
                            .id(("browser-tag-editor-row", group_idx * 32 + val_idx))
                            .debug_selector(move || {
                                format!("browser-tag-editor-toggle-{group_idx}-{value}")
                            })
                            .px_2()
                            .py(px(2.0))
                            .when(enabled, |row| {
                                row.cursor_pointer()
                                    .hover(|s| s.bg(rgb(theme::color(theme::PaletteRole::Hover))))
                            })
                            .text_xs()
                            .text_color(rgb(theme::color(if checked {
                                theme::PaletteRole::Secondary
                            } else if enabled {
                                theme::PaletteRole::Text
                            } else {
                                theme::PaletteRole::Muted
                            })))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |view, _, window, cx| {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                    if let Some(ref path) = path_for_click
                                        && let Err(err) = view.toggle_sample_tag(path, value, cx)
                                    {
                                        view.browser.layout.mini_tag_error = Some(err.into());
                                    }
                                    cx.notify();
                                }),
                            )
                            .child(theme::text(label)),
                    );
                }
            }
            groups = groups.child(section);
        }
        // Custom tags not covered by preset groups.
        let preset: std::collections::BTreeSet<&str> = TAG_GROUPS
            .iter()
            .flat_map(|(_, vals)| vals.iter().copied())
            .collect();
        let custom_tags: Vec<String> = self
            .browser
            .samples
            .catalog
            .all_tags()
            .into_iter()
            .filter(|tag| !preset.contains(tag.as_str()))
            .collect();
        if !custom_tags.is_empty() {
            let mut custom = browser_divider().child(browser_heading("custom"));
            for (idx, tag) in custom_tags.into_iter().enumerate() {
                let checked = assigned_tags.contains(&tag);
                let path_for_click = target_path.clone();
                let label = format!("{} {tag}", if checked { "☑" } else { "☐" });
                let tag_click = tag.clone();
                custom = custom.child(
                    div()
                        .id(("browser-tag-editor-custom", idx))
                        .debug_selector(move || format!("browser-tag-editor-custom-{tag}"))
                        .px_2()
                        .py(px(2.0))
                        .cursor_pointer()
                        .text_xs()
                        .text_color(rgb(theme::color(if checked {
                            theme::PaletteRole::Secondary
                        } else {
                            theme::PaletteRole::Text
                        })))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |view, _, window, cx| {
                                window.prevent_default();
                                cx.stop_propagation();
                                if let Some(ref path) = path_for_click
                                    && let Err(err) = view.toggle_sample_tag(path, &tag_click, cx)
                                {
                                    view.browser.layout.mini_tag_error = Some(err.into());
                                }
                                cx.notify();
                            }),
                        )
                        .child(theme::text(label)),
                );
            }
            groups = groups.child(custom);
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
        let target_path = self.browser_tag_target_sample(cx);
        let assigned_tags: Vec<String> = target_path
            .as_deref()
            .and_then(|path| self.browser.samples.catalog.tags_for(path))
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default();
        let mini_height = self.browser.layout.mini_tags_height;
        let resize_entity = cx.entity();
        let resize_handle = div()
            .id("browser-mini-tags-resize")
            .debug_selector(|| "browser-mini-tags-resize".to_owned())
            .absolute()
            .top(px(-2.0))
            .left_0()
            .right_0()
            .h(px(4.0))
            .cursor_row_resize()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    let initial = view.browser.layout.mini_tags_height;
                    view.begin_browser_resize(
                        super::resize::Divider::MiniTags,
                        event,
                        initial,
                        window,
                        cx,
                    );
                    cx.stop_propagation();
                }),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, (), window, cx| {
                        resize_entity.read(cx).renew_browser_resize(
                            super::resize::Divider::MiniTags,
                            window,
                            resize_entity.entity_id(),
                        );
                    },
                )
                .size_full(),
            );

        let mut chips = div()
            .id("browser-mini-tags-chips")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1();
        for (idx, tag) in assigned_tags.into_iter().enumerate() {
            let path_for_remove = target_path.clone();
            let tag_remove = tag.clone();
            let selector_tag = tag.clone();
            chips = chips.child(
                div()
                    .id(("browser-mini-tag", idx))
                    .debug_selector(move || format!("browser-mini-tag-remove-{selector_tag}"))
                    .cursor_pointer()
                    .px_1()
                    .rounded(px(2.0))
                    .bg(rgb(theme::color(theme::PaletteRole::Raised)))
                    .border_1()
                    .border_color(rgb(theme::color(theme::PaletteRole::Border)))
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Text)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            if let Some(ref path) = path_for_remove {
                                view.remove_sample_tag(path, &tag_remove, cx);
                            }
                            cx.notify();
                        }),
                    )
                    .child(theme::text(format!("{tag} ×"))),
            );
        }
        if let Some(input) = self.browser.layout.mini_tag_input.clone() {
            let path_for_commit = target_path.clone();
            let input_for_keys = input.clone();
            chips = chips.child(
                div()
                    .id("browser-mini-tag-input")
                    .debug_selector(|| "browser-mini-tag-input".to_owned())
                    .w(px(72.0))
                    .on_key_down(cx.listener(move |view, event: &gpui::KeyDownEvent, _, cx| {
                        match event.keystroke.key.as_str() {
                            "enter" => {
                                let raw = input_for_keys.read(cx).text().to_owned();
                                if let Some(ref path) = path_for_commit {
                                    match view.add_sample_tag(path, &raw, cx) {
                                        Ok(_) => {
                                            view.browser.layout.mini_tag_error = None;
                                            view.browser.layout.mini_tag_input = None;
                                        }
                                        Err(err) => {
                                            view.browser.layout.mini_tag_error = Some(err.into());
                                        }
                                    }
                                } else {
                                    view.browser.layout.mini_tag_input = None;
                                }
                                cx.stop_propagation();
                                cx.notify();
                            }
                            "escape" => {
                                view.browser.layout.mini_tag_input = None;
                                view.browser.layout.mini_tag_error = None;
                                cx.stop_propagation();
                                cx.notify();
                            }
                            _ => {}
                        }
                    }))
                    .child(input),
            );
        } else {
            let can_add = target_path.is_some();
            chips = chips.child(
                div()
                    .id("browser-mini-tag-add")
                    .debug_selector(|| "browser-mini-tag-add".to_owned())
                    .whitespace_nowrap()
                    .flex_shrink_0()
                    .px_1()
                    .rounded(px(2.0))
                    .bg(rgb(theme::color(theme::PaletteRole::Background)))
                    .border_1()
                    .border_color(rgb(theme::color(theme::PaletteRole::Border)))
                    .text_xs()
                    .text_color(rgb(theme::color(if can_add {
                        theme::PaletteRole::Text
                    } else {
                        theme::PaletteRole::Muted
                    })))
                    .when(can_add, |btn| btn.cursor_pointer())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                            if !can_add {
                                return;
                            }
                            let input = cx.new(browser_input::tag_input);
                            let focus = input.read(cx).focus_handle().clone();
                            view.browser.layout.mini_tag_input = Some(input);
                            view.browser.layout.mini_tag_error = None;
                            focus.focus(window);
                            cx.notify();
                        }),
                    )
                    .child(theme::text("add…")),
            );
        }
        if let Some(ref err) = self.browser.layout.mini_tag_error {
            chips = chips.child(
                browser_caption(err).text_color(rgb(theme::color(theme::PaletteRole::Primary))),
            );
        }

        browser_divider()
            .h(px(mini_height + 24.0))
            .flex_shrink_0()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .border_t_1()
            .debug_selector(|| "browser-selection-preview".to_owned())
            .child(
                browser_divider()
                    .relative()
                    .h(px(mini_height))
                    .flex_shrink_0()
                    .min_w_0()
                    .px_1()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(resize_handle)
                    .child(browser_caption("tags:").whitespace_nowrap().flex_shrink_0())
                    .child(chips)
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
