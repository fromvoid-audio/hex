//! Compact, bounded left-side media and project browser.
use super::{EditorMode, EditorView, browser_input::BrowserSearch};
use crate::theme::{self, Palette};
use gpui::{
    App, Context, Div, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PathPromptOptions, Window, canvas, div, prelude::*, px, rgb, uniform_list,
};
use hex_library::{DirectoryListing, EntryKind, LibraryEntry, ScanError, scan_directory};
use hex_project::Command;
use hex_session::service::Request;
use std::path::PathBuf;

const DEFAULT_WIDTH: f32 = 280.0;
const MIN_WIDTH: f32 = 220.0;
const MIN_WORKSPACE_WIDTH: f32 = 560.0;
const COLLAPSED_WIDTH: f32 = 28.0;
const PLACE_CAPACITY: usize = 16;
const ROW_HEIGHT: f32 = 22.0;

fn clamped_panel_width(requested: f32, window_width: f32) -> f32 {
    requested.clamp(
        MIN_WIDTH,
        (window_width - MIN_WORKSPACE_WIDTH - 4.0).max(MIN_WIDTH),
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ScanRequest {
    generation: u64,
    directory: PathBuf,
}

#[derive(Default)]
struct ScanCoordinator {
    generation: u64,
    in_flight: bool,
    pending: Option<ScanRequest>,
}

impl ScanCoordinator {
    fn request(&mut self, directory: PathBuf) -> Option<ScanRequest> {
        self.generation = self.generation.saturating_add(1);
        let request = ScanRequest {
            generation: self.generation,
            directory,
        };
        if self.in_flight {
            self.pending = Some(request);
            None
        } else {
            self.in_flight = true;
            Some(request)
        }
    }

    fn complete(&mut self, generation: u64) -> (bool, Option<ScanRequest>) {
        self.in_flight = false;
        let current = generation == self.generation;
        let next = self.pending.take();
        if next.is_some() {
            self.in_flight = true;
        }
        (current, next)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum BrowserSection {
    #[default]
    Samples,
    Instruments,
    Projects,
}

pub(super) struct BrowserState {
    pub open: bool,
    pub width: f32,
    pub section: BrowserSection,
    pub directory: PathBuf,
    home: PathBuf,
    music: PathBuf,
    places: Vec<PathBuf>,
    listing: Option<DirectoryListing>,
    loading: bool,
    scans: ScanCoordinator,
    error: Option<String>,
    resizing: bool,
    pub search: gpui::Entity<BrowserSearch>,
    results_focused: bool,
    selected_index: usize,
    scroll: gpui::UniformListScrollHandle,
    _search_observer: gpui::Subscription,
}

impl BrowserState {
    pub fn new(cx: &mut Context<EditorView>) -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        let music = home.join("Music");
        let search = cx.new(BrowserSearch::new);
        let observer = cx.observe(&search, |view, _, cx| {
            view.browser.selected_index = 0;
            view.browser.results_focused = false;
            cx.notify();
        });
        let places = if music != home {
            vec![music.clone(), home.clone()]
        } else {
            vec![home.clone()]
        };
        Self {
            open: true,
            width: DEFAULT_WIDTH,
            section: BrowserSection::Samples,
            directory: music.clone(),
            home,
            music,
            places,
            listing: None,
            loading: false,
            scans: ScanCoordinator::default(),
            error: None,
            resizing: false,
            search,
            results_focused: false,
            selected_index: 0,
            scroll: gpui::UniformListScrollHandle::new(),
            _search_observer: observer,
        }
    }
}

impl EditorView {
    pub(super) fn browser_scan_initial(&mut self, cx: &mut Context<Self>) {
        self.request_browser_scan(self.browser.directory.clone(), cx);
    }

    fn request_browser_scan(&mut self, directory: PathBuf, cx: &mut Context<Self>) {
        let start = self.browser.scans.request(directory.clone());
        self.browser.directory = directory.clone();
        self.browser.error = None;
        self.browser.loading = true;
        if let Some(request) = start {
            self.start_browser_scan(request, cx);
        }
    }

    fn start_browser_scan(&mut self, request: ScanRequest, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let path_for_scan = request.directory.clone();
            let result = cx
                .background_executor()
                .spawn(async move { scan_directory(path_for_scan) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.browser_scan_finished(request, result, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn browser_scan_finished(
        &mut self,
        request: ScanRequest,
        result: Result<DirectoryListing, ScanError>,
        cx: &mut Context<Self>,
    ) {
        let (is_current, pending) = self.browser.scans.complete(request.generation);
        let mut next = pending;
        if is_current {
            match result {
                Ok(listing) => {
                    self.browser.listing = Some(listing);
                    self.browser.error = None;
                    self.browser.loading = false;
                }
                Err(_)
                    if request.directory == self.browser.music
                        && self.browser.music != self.browser.home
                        && next.is_none() =>
                {
                    let fallback = self.browser.home.clone();
                    self.browser.directory = fallback.clone();
                    self.browser.loading = true;
                    next = self.browser.scans.request(fallback);
                }
                Err(error) => {
                    self.browser.listing = None;
                    self.browser.error = Some(error.to_string().to_lowercase());
                    self.browser.loading = false;
                }
            }
        }
        if let Some(request) = next {
            self.start_browser_scan(request, cx);
        }
    }

    pub(super) fn browser_content_width(&self, window: &Window) -> f32 {
        if !self.browser.open {
            COLLAPSED_WIDTH
        } else {
            clamped_panel_width(self.browser.width, f32::from(window.bounds().size.width)) + 4.0
        }
    }

    pub(super) fn browser_key_context(&self, window: &Window, cx: &App) -> &'static str {
        if self
            .browser
            .search
            .read(cx)
            .focus_handle()
            .is_focused(window)
        {
            if self.browser.results_focused {
                "hex_browser"
            } else {
                "hex_browser_search"
            }
        } else {
            "hex_editor"
        }
    }

    pub(super) fn toggle_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.browser.open = !self.browser.open;
        if !self.browser.open {
            self.browser.results_focused = false;
            self.focus.focus(window);
        }
        cx.notify();
    }

    pub(super) fn browser_previous(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.browser.selected_index = self.browser.selected_index.saturating_sub(1);
        self.browser
            .scroll
            .scroll_to_item(self.browser.selected_index, gpui::ScrollStrategy::Top);
        cx.notify();
    }

    pub(super) fn browser_next(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let count = self.browser_visible_entries(cx).len();
        if count > 0 {
            self.browser.selected_index =
                self.browser.selected_index.saturating_add(1).min(count - 1);
            self.browser
                .scroll
                .scroll_to_item(self.browser.selected_index, gpui::ScrollStrategy::Top);
        }
        cx.notify();
    }

    pub(super) fn browser_activate(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.browser.section == BrowserSection::Instruments {
            let count = self
                .state
                .as_ref()
                .map_or(0, |state| state.project.tracks().len());
            self.enqueue(Request::Edit(Command::AddMidiTrack {
                name: format!("track {}", count + 1),
            }));
            cx.notify();
            return;
        }
        let Some(entry) = self
            .browser_visible_entries(cx)
            .get(self.browser.selected_index)
            .cloned()
        else {
            return;
        };
        match entry.kind {
            EntryKind::Directory => self.request_browser_scan(entry.path, cx),
            EntryKind::WaveAudio => self.import_browser_audio(entry.path),
            EntryKind::Project => self.open_browser_project(entry.path, cx),
        }
        cx.notify();
    }

    pub(super) fn browser_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .browser
            .search
            .read(cx)
            .focus_handle()
            .is_focused(window)
        {
            self.browser.search.update(cx, |input, cx| {
                input.clear(cx);
            });
        } else {
            self.browser.results_focused = true;
        }
        cx.notify();
    }

    fn browser_visible_entries(&self, cx: &App) -> Vec<LibraryEntry> {
        let query = self.browser.search.read(cx).content.to_lowercase();
        self.browser
            .listing
            .as_ref()
            .filter(|listing| listing.directory == self.browser.directory)
            .map(|listing| {
                listing
                    .entries
                    .iter()
                    .filter(|entry| match self.browser.section {
                        BrowserSection::Samples => entry.kind != EntryKind::Project,
                        BrowserSection::Projects => entry.kind != EntryKind::WaveAudio,
                        BrowserSection::Instruments => false,
                    })
                    .filter(|entry| query.is_empty() || entry.name.to_lowercase().contains(&query))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn browser_view(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        if !self.browser.open {
            let rail = div()
                .w(px(COLLAPSED_WIDTH))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .items_center()
                .pt_1()
                .gap_1()
                .bg(rgb(Palette::SURFACE))
                .border_r_1()
                .border_color(rgb(Palette::BORDER))
                .child(self.browser_icon(
                    "▤",
                    "open browser · cmd+alt+b",
                    |view, _, cx| {
                        view.browser.open = true;
                        cx.notify();
                    },
                    cx,
                ));
            return rail;
        }

        let path = self.browser.directory.clone();
        let folder_dialog = self.browser_folder_button(cx);
        let section = self.browser.section;
        let query = self.browser.search.read(cx).content.to_lowercase();
        let entries: Vec<LibraryEntry> = self
            .browser
            .listing
            .as_ref()
            .filter(|listing| listing.directory == path)
            .map(|listing| {
                listing
                    .entries
                    .iter()
                    .filter(|entry| match section {
                        BrowserSection::Samples => entry.kind != EntryKind::Project,
                        BrowserSection::Projects => entry.kind != EntryKind::WaveAudio,
                        BrowserSection::Instruments => false,
                    })
                    .filter(|entry| query.is_empty() || entry.name.to_lowercase().contains(&query))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let row_count = entries.len();
        let rows = uniform_list(
            "library-browser-entries",
            row_count,
            cx.processor(move |view, range: std::ops::Range<usize>, _window, cx| {
                range
                    .filter_map(|index| {
                        entries
                            .get(index)
                            .cloned()
                            .map(|entry| view.browser_row(index, entry, cx))
                    })
                    .collect()
            }),
        )
        .h_full()
        .min_h_0()
        .w_full()
        .track_scroll(self.browser.scroll.clone());

        let bounds = window.bounds();
        let pane_width = clamped_panel_width(self.browser.width, f32::from(bounds.size.width));
        let mut panel = div()
            .w(px(pane_width))
            .flex_shrink_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(Palette::SURFACE))
            .border_r_1()
            .border_color(rgb(Palette::BORDER));
        panel = panel.child(
            div()
                .h(px(25.0))
                .flex_shrink_0()
                .px_1()
                .flex()
                .items_center()
                .gap_1()
                .border_b_1()
                .border_color(rgb(Palette::BORDER))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .text_color(rgb(Palette::TEXT))
                        .child(theme::text(match section {
                            BrowserSection::Samples => "browser · samples",
                            BrowserSection::Instruments => "browser · instruments",
                            BrowserSection::Projects => "browser · projects",
                        })),
                )
                .child(self.browser_icon(
                    "⌕",
                    "choose a folder",
                    move |view, _, cx| view.open_browser_folder(cx),
                    cx,
                ))
                .child(self.browser_icon(
                    "×",
                    "close browser · cmd+alt+b",
                    |view, window, cx| view.toggle_browser(window, cx),
                    cx,
                )),
        );
        panel = panel.child(self.browser_sections(cx));
        if section == BrowserSection::Samples || section == BrowserSection::Projects {
            panel = panel.child(self.browser_places(cx));
            panel = panel.child(
                div()
                    .px_1()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(self.browser_icon(
                        "↑",
                        "parent folder",
                        |view, _, cx| {
                            if let Some(parent) = view.browser.directory.parent().map(PathBuf::from)
                            {
                                view.request_browser_scan(parent, cx);
                            }
                        },
                        cx,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_xs()
                            .text_color(rgb(Palette::MUTED))
                            .child(theme::text(
                                path.file_name()
                                    .and_then(|name| name.to_str())
                                    .unwrap_or("folder"),
                            )),
                    )
                    .child(folder_dialog),
            );
            panel = panel.child(div().px_1().pb_1().child(self.browser.search.clone()));
        }
        match section {
            BrowserSection::Instruments => {
                panel = panel.child(self.browser_instrument_row(cx));
                panel = panel.child(div().flex_1().min_h_0());
            }
            _ if self.browser.loading
                && self
                    .browser
                    .listing
                    .as_ref()
                    .is_none_or(|listing| listing.directory != path) =>
            {
                panel = panel.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .px_2()
                        .pt_2()
                        .text_xs()
                        .text_color(rgb(Palette::MUTED))
                        .child(theme::text("scanning folder…")),
                );
            }
            _ if self.browser.error.is_some() => {
                let error = self.browser.error.clone().unwrap_or_default();
                panel = panel.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .px_2()
                        .pt_2()
                        .text_xs()
                        .text_color(rgb(Palette::CORAL))
                        .child(theme::text(error)),
                );
            }
            _ if row_count == 0 => {
                let empty = if self
                    .browser
                    .listing
                    .as_ref()
                    .is_some_and(|listing| listing.truncated)
                {
                    "no matching items in first 2048 entries"
                } else {
                    "no matching folders or supported files"
                };
                panel = panel.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .px_2()
                        .pt_2()
                        .text_xs()
                        .text_color(rgb(Palette::MUTED))
                        .child(theme::text(empty)),
                );
            }
            _ => {
                panel = panel.child(rows.flex_1());
                if self
                    .browser
                    .listing
                    .as_ref()
                    .is_some_and(|listing| listing.truncated)
                {
                    panel = panel.child(
                        div()
                            .h(px(18.0))
                            .flex_shrink_0()
                            .px_1()
                            .text_xs()
                            .text_color(rgb(Palette::MUTED))
                            .child(theme::text("listing capped · open a smaller folder")),
                    );
                }
            }
        }

        let entity = cx.entity();
        let mut resize = div()
            .id("browser-resize-handle")
            .h_full()
            .w(px(4.0))
            .flex_shrink_0()
            .cursor_col_resize()
            .bg(rgb(Palette::BORDER))
            .hover(|style| style.bg(rgb(Palette::CYAN)));
        resize = resize.child(
            canvas(
                |_, _, _| (),
                move |handle_bounds, _, window, _| {
                    let down_entity = entity.clone();
                    // Mouse event positions are window-local; the browser starts at x = 0.
                    let start_x = 0.0_f32;
                    window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
                        if !phase.bubble()
                            || event.button != MouseButton::Left
                            || !handle_bounds.contains(&event.position)
                        {
                            return;
                        }
                        down_entity.update(cx, |view, cx| {
                            view.browser.resizing = true;
                            view.browser.width = pane_width;
                            cx.notify();
                        });
                    });
                    let move_entity = entity.clone();
                    let available = f32::from(bounds.size.width);
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                        if !phase.bubble() || !event.dragging() {
                            return;
                        }
                        move_entity.update(cx, |view, cx| {
                            if view.browser.resizing {
                                view.browser.width = clamped_panel_width(
                                    f32::from(event.position.x) - start_x,
                                    available,
                                );
                                cx.notify();
                            }
                        });
                    });
                    let up_entity = entity.clone();
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                        if !phase.bubble() || event.button != MouseButton::Left {
                            return;
                        }
                        up_entity.update(cx, |view, cx| {
                            if view.browser.resizing {
                                view.browser.resizing = false;
                                cx.notify();
                            }
                        });
                    });
                },
            )
            .size_full(),
        );
        div().flex().min_h_0().flex_shrink_0().child(panel).child(
            div()
                .w(px(4.0))
                .h_full()
                .flex_shrink_0()
                .min_h_0()
                .child(resize),
        )
    }

    fn browser_row(
        &self,
        index: usize,
        entry: LibraryEntry,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let icon = match entry.kind {
            EntryKind::Directory => "▸",
            EntryKind::WaveAudio => "♫",
            EntryKind::Project => "▤",
        };
        let path = entry.path.clone();
        let selected = self.browser.results_focused && self.browser.selected_index == index;
        div()
            .id(("library-entry", index))
            .h(px(ROW_HEIGHT))
            .px_1()
            .flex()
            .items_center()
            .gap_1()
            .cursor_pointer()
            .bg(rgb(if selected {
                Palette::HOVER
            } else {
                Palette::SURFACE
            }))
            .hover(|style| style.bg(rgb(Palette::HOVER)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    view.browser.selected_index = index;
                    view.browser.results_focused = true;
                    view.browser
                        .search
                        .update(cx, |input, _| input.focus(window));
                    cx.notify();
                    if event.click_count >= 2 {
                        match entry.kind {
                            EntryKind::Directory => view.request_browser_scan(path.clone(), cx),
                            EntryKind::WaveAudio => view.import_browser_audio(path.clone()),
                            EntryKind::Project => view.open_browser_project(path.clone(), cx),
                        }
                    }
                }),
            )
            .child(
                div()
                    .w(px(16.0))
                    .text_xs()
                    .text_color(rgb(Palette::MUTED))
                    .child(theme::text(icon)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .line_clamp(1)
                    .text_ellipsis()
                    .text_xs()
                    .text_color(rgb(Palette::TEXT))
                    .child(theme::text(entry.name)),
            )
    }

    fn browser_sections(&self, cx: &mut Context<Self>) -> Div {
        let mut row = div()
            .h(px(25.0))
            .flex_shrink_0()
            .flex()
            .border_b_1()
            .border_color(rgb(Palette::BORDER));
        for (index, (section, label, icon)) in [
            (BrowserSection::Samples, "samples", "♫"),
            (BrowserSection::Instruments, "instruments", "⌘"),
            (BrowserSection::Projects, "projects", "▤"),
        ]
        .into_iter()
        .enumerate()
        {
            let active = self.browser.section == section;
            row = row.child(
                div()
                    .id(("browser-section", index))
                    .flex_1()
                    .px_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .cursor_pointer()
                    .bg(rgb(if active {
                        Palette::HOVER
                    } else {
                        Palette::SURFACE
                    }))
                    .text_xs()
                    .text_color(rgb(if active {
                        Palette::CYAN
                    } else {
                        Palette::MUTED
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _, cx| {
                            view.browser.section = section;
                            cx.notify();
                        }),
                    )
                    .child(theme::text(icon))
                    .child(theme::text(label)),
            );
        }
        row
    }

    fn browser_places(&self, cx: &mut Context<Self>) -> Div {
        let mut row = div()
            .h(px(23.0))
            .flex_shrink_0()
            .px_1()
            .flex()
            .items_center()
            .gap_1()
            .border_b_1()
            .border_color(rgb(Palette::BORDER));
        for (index, place) in self.browser.places.iter().take(PLACE_CAPACITY).enumerate() {
            let path = place.clone();
            let label = if *place == self.browser.music {
                "music"
            } else if *place == self.browser.home {
                "home"
            } else {
                place
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("folder")
            };
            let selected = self.browser.directory == *place;
            row = row.child(
                div()
                    .id(("browser-place", index))
                    .px_1()
                    .text_xs()
                    .text_color(rgb(if selected {
                        Palette::CYAN
                    } else {
                        Palette::MUTED
                    }))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _, cx| {
                            view.request_browser_scan(path.clone(), cx)
                        }),
                    )
                    .child(theme::text(label)),
            );
        }
        if self.browser.places.len() < PLACE_CAPACITY {
            row = row.child(self.browser_icon(
                "+",
                "add folder place",
                |view, _, cx| view.open_browser_folder(cx),
                cx,
            ));
        }
        row
    }

    fn browser_instrument_row(&self, cx: &mut Context<Self>) -> Div {
        div()
            .h(px(30.0))
            .flex_shrink_0()
            .mx_1()
            .mt_1()
            .px_1()
            .flex()
            .items_center()
            .gap_1()
            .bg(rgb(Palette::RAISED))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(Palette::HOVER)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| {
                    let count = view
                        .state
                        .as_ref()
                        .map_or(0, |state| state.project.tracks().len());
                    view.enqueue(Request::Edit(Command::AddMidiTrack {
                        name: format!("track {}", count + 1),
                    }));
                    cx.notify();
                }),
            )
            .child(
                div()
                    .w(px(18.0))
                    .text_color(rgb(Palette::CYAN))
                    .child(theme::text("⌘")),
            )
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .text_color(rgb(Palette::TEXT))
                    .child(theme::text("midi track · sine")),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(Palette::MUTED))
                    .child(theme::text("+")),
            )
    }

    fn browser_icon(
        &self,
        label: &'static str,
        tooltip: &'static str,
        action: impl Fn(&mut EditorView, &mut Window, &mut Context<EditorView>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let description = theme::text(tooltip);
        div()
            .id(label)
            .w(px(22.0))
            .h(px(22.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_xs()
            .text_color(rgb(Palette::MUTED))
            .hover(|style| style.bg(rgb(Palette::HOVER)))
            .tooltip(move |_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    description: description.clone(),
                })
                .into()
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, _, window, cx| {
                    view.browser.results_focused = false;
                    action(view, window, cx);
                }),
            )
            .child(theme::text(label))
    }

    fn browser_folder_button(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let description = theme::text("choose a folder");
        div()
            .id("browser-folder-picker")
            .w(px(22.0))
            .h(px(22.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_xs()
            .text_color(rgb(Palette::MUTED))
            .tooltip(move |_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    description: description.clone(),
                })
                .into()
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| view.open_browser_folder(cx)),
            )
            .child(theme::text("⌂"))
    }

    fn open_browser_folder(&mut self, cx: &mut Context<Self>) {
        let response = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("add browser folder".into()),
        });
        cx.spawn(async move |this, cx| {
            let picked = response.await;
            let _ = this.update(cx, |view, cx| {
                match picked {
                    Ok(Ok(Some(mut paths))) if !paths.is_empty() => {
                        if let Some(path) = paths.pop() {
                            if !view.browser.places.contains(&path)
                                && view.browser.places.len() < PLACE_CAPACITY
                            {
                                view.browser.places.push(path.clone());
                            }
                            view.request_browser_scan(path, cx);
                        }
                    }
                    Ok(Err(error)) => view.error = Some(error.to_string().to_lowercase()),
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn import_browser_audio(&mut self, path: PathBuf) {
        if self.mode == EditorMode::Session {
            let target = self.selected_cell.filter(|(track, scene)| {
                self.state.as_ref().is_some_and(|state| {
                    state.project.track(*track).is_some()
                        && state
                            .project
                            .scenes()
                            .iter()
                            .any(|item| item.id() == *scene)
                        && state.project.session_cell(*track, *scene).is_none()
                })
            });
            if let Some((track, scene)) = target {
                self.enqueue(Request::ImportToSession { path, track, scene });
                self.status = "importing sample into selected session cell".into();
                return;
            }
            self.set_mode(EditorMode::Arrangement);
            self.error = Some(
                "no empty audio session cell selected · imported sample shown in arrangement"
                    .into(),
            );
        }
        let target = self.selected_track.filter(|track| {
            self.state
                .as_ref()
                .is_some_and(|state| state.project.track(*track).is_some())
        });
        if let Some(track) = target {
            self.enqueue(Request::ImportToTrack {
                path,
                track,
                start: self.cursor_position(),
            });
            self.status = "importing sample to selected track".into();
        } else {
            self.enqueue(Request::Import(path));
        }
    }

    fn open_browser_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.choose_project(path);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{MIN_WIDTH, PathBuf, ScanCoordinator, clamped_panel_width};

    #[test]
    fn browser_width_preserves_minimum_workspace_and_panel() {
        assert_eq!(clamped_panel_width(280.0, 1_240.0), 280.0);
        assert_eq!(clamped_panel_width(500.0, 880.0), 316.0);
        assert_eq!(clamped_panel_width(100.0, 880.0), MIN_WIDTH);
    }

    #[test]
    fn stale_directory_scan_results_are_not_current() {
        let mut scans = ScanCoordinator::default();
        let first = scans
            .request(PathBuf::from("/one"))
            .expect("first scan starts");
        assert!(scans.request(PathBuf::from("/two")).is_none());
        assert!(scans.request(PathBuf::from("/three")).is_none());
        let (first_is_current, next) = scans.complete(first.generation);
        assert!(!first_is_current);
        assert_eq!(
            next.expect("latest scan starts next").directory,
            PathBuf::from("/three")
        );
        assert!(scans.in_flight);
        assert!(scans.pending.is_none());
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "browser_ui_test.rs"]
mod ui_test;
