//! Compact, bounded left-side media and project browser.
//!
//! This UI owner composes existing file, native-device and plugin projections;
//! library/catalog owners retain matching and within-kind ordering. Nonempty All
//! search places native devices then plugins before files. Other sources and empty
//! browsing retain file-first order. Row ranges are shared by virtualization,
//! stable-key selection/history, activation, folder navigation and audition.
mod audition_geometry;
mod random_rank;
mod scrollbar_geometry;

mod audition;
mod folder_places;
mod history;
mod insertion;
mod place_icons;
mod plugins;
mod random;
mod resize;
mod samples;
mod scrollbar;
mod tree;

use super::{EditorMode, EditorView, browser_input, drag};
use crate::theme;
use gpui::{
    App, Context, Div, MouseButton, MouseDownEvent, PathPromptOptions, Window, canvas, div,
    prelude::*, px, relative, rgb, uniform_list,
};
use hex_audio::sample_preview::Phase;
use hex_library::{DirectoryListing, EntryKind, LibraryEntry, ScanError, scan_directory};
use hex_preferences::{BrowserBuiltin, BrowserPlaces};
use hex_session::service::{AudioPreview, PreviewFailure, PreviewSources, Request};
use hex_widgets::TextInput;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

mod layout;
use layout::{result_label, result_row, selection_ink};

// Preference diagnostics remain useful without unbounded field/path text in UI tooltips.
fn places_error(prefix: &str, error: &hex_preferences::PreferencesError) -> String {
    let reason = hex_session::service::text::lowercase_outside_verbatim(&error.to_string());
    let mut message = format!(
        "{prefix} · {}",
        reason.chars().take(160).collect::<String>()
    );
    if reason.chars().count() > 160 {
        message.push('…');
    }
    message
}

pub(super) const DEFAULT_WIDTH: f32 = 480.0;
const COLLAPSED_WIDTH: f32 = 28.0;
const PLACE_CAPACITY: usize = 16;
const DEFAULT_PLACE_COUNT: usize = 2;
/// Uniform virtual height, including plugin groups and history scroll restoration.
const ROW_HEIGHT: f32 = 18.0;
const PREVIEW_CACHE_CAPACITY: usize = 16;
const GAIN_SEARCH_ERROR: &str = "finish or cancel the gain value edit before search";

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct PreviewKey {
    path: PathBuf,
    sample_rate: u32,
}

#[derive(Clone)]
struct PlaybackPreviewRequest {
    generation: u64,
    output_generation: u64,
    output_identity: u64,
    start_fraction: f64,
    path: PathBuf,
    sample_rate: u32,
}

#[derive(Clone)]
struct PreviewRequest {
    key: PreviewKey,
    slot: PreviewSlot,
    /// Set when a different waveform request supersedes this one in flight.
    cancel: Arc<AtomicBool>,
}

#[derive(Clone, Default)]
struct PreviewSlot {
    metadata: Rc<RefCell<Option<Rc<AudioPreview>>>>,
    error: Rc<RefCell<Option<String>>>,
}

#[cfg(not(test))]
pub(super) fn default_preferences_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    hex_preferences::browser_places_path(&home)
}

fn clamped_panel_width(requested: f32, window_width: f32) -> f32 {
    geometry::clamped_panel_width(requested, window_width, super::workspace_view::PANEL_GUTTER)
}

#[path = "browser/geometry.rs"]
mod geometry;
pub(super) use geometry::MIN_WORKSPACE_WIDTH;
use geometry::browser_audio_drag_ghost_position;

#[path = "browser/scan.rs"]
mod scan;
use scan::{ScanCoordinator, ScanRequest};

/// The one selected source across all picker groups during this session. `directory` is only
/// the current bounded folder location, never another selected source.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum BrowserSection {
    All,
    #[default]
    Samples,
    Instruments,
    Plugins,
    Projects,
    Place(PathBuf),
    Collection(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum BrowserItemKey {
    File(PathBuf),
    Plugin(PathBuf, Option<[u8; 16]>),
    Device(hex_engine::NativeDeviceClass, hex_engine::DeviceTypeId),
}

/// Global row ranges; existing projections retain their local order/identity.
struct BrowserRows {
    files: std::ops::Range<usize>,
    devices: std::ops::Range<usize>,
    plugins: std::ops::Range<usize>,
}

impl BrowserRows {
    fn new(files: usize, devices: usize, plugins: usize, devices_first: bool) -> Self {
        if devices_first {
            Self {
                devices: 0..devices,
                plugins: devices..devices + plugins,
                files: devices + plugins..devices + plugins + files,
            }
        } else {
            Self {
                files: 0..files,
                devices: files..files + devices,
                plugins: files + devices..files + devices + plugins,
            }
        }
    }
}

pub(super) struct BrowserState {
    pub open: bool,
    layout: layout::BrowserLayout,
    pub width: f32,
    pub section: BrowserSection,
    pub directory: PathBuf,
    home: PathBuf,
    music: PathBuf,
    pub(super) places: Vec<PathBuf>,
    place_icons: HashMap<PathBuf, String>,
    source_preferences: BrowserPlaces,
    preferences_path: PathBuf,
    preferences_loading: bool,
    pub(super) preferences_writable: bool,
    preferences_dirty: bool,
    places_save_in_flight: bool,
    folder_bookmark_pending: bool,
    pending_places_save: Option<BrowserPlaces>,
    /// Latest failure from a save that a queued save followed; reported when
    /// the save chain ends so a later success cannot hide it.
    unreported_places_save_error: Option<String>,
    pub(super) places_status: Option<String>,
    samples: samples::SampleBrowser,
    plugins: plugins::Plugins,
    tree: tree::Tree,
    listing: Option<DirectoryListing>,
    loading: bool,
    pending_selection: Option<BrowserItemKey>,
    scans: ScanCoordinator,
    error: Option<String>,
    resizing: bool,
    pub search: gpui::Entity<TextInput>,
    results_focus: gpui::FocusHandle,
    // Selection/highlight activity, independent of actual query/navigation focus.
    results_focused: bool,
    selected_index: usize,
    scroll: gpui::UniformListScrollHandle,
    scrollbar: scrollbar::State,
    _search_observer: gpui::Subscription,
    observed_query: String,
    history: history::History,
    playback_generation: u64,
    /// Cancellation flag of the one in-flight audition preparation; set once
    /// a newer request or a stop supersedes it.
    playback_in_flight: Option<Arc<AtomicBool>>,
    playback_pending: Option<PlaybackPreviewRequest>,
    auto_audition: bool,
    audition_gain: f32,
    audition_gain_dragging: bool,
    playback_phase: Phase,
    playback_output: Option<(u64, u64)>,
    playback_path: Option<PathBuf>,
    playback_fraction: f32,
    waveform_bounds: Rc<Cell<Option<gpui::Bounds<gpui::Pixels>>>>,
    preview_cache: HashMap<PreviewKey, PreviewSlot>,
    /// Decoded sources shared by waveform and audition jobs.
    preview_sources: PreviewSources,
    preview_selected: Option<PreviewKey>,
    #[cfg(test)]
    preview_preparations: usize,
    preview_in_flight: Option<PreviewRequest>,
    preview_pending: Option<PreviewRequest>,
    device_catalog: Option<
        Result<
            Vec<(
                hex_engine::NativeDeviceClass,
                hex_engine::NativeDeviceCatalogEntry,
            )>,
            String,
        >,
    >,
}

impl BrowserState {
    pub fn new(
        cx: &mut Context<EditorView>,
        preferences_path: PathBuf,
        #[cfg(test)] test_home: Option<PathBuf>,
    ) -> Self {
        #[cfg(not(test))]
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        #[cfg(test)]
        let home = test_home.expect("editor test home must be resolved before startup");
        let music = home.join("Music");
        let search = cx.new(browser_input::search);
        let observer = cx.observe(&search, |view, _, cx| {
            view.sync_browser_query(cx);
            cx.notify();
        });
        let places = if music != home {
            vec![music.clone(), home.clone()]
        } else {
            vec![home.clone()]
        };
        Self {
            open: true,
            layout: layout::BrowserLayout::default(),
            width: DEFAULT_WIDTH,
            section: BrowserSection::Samples,
            directory: music.clone(),
            home,
            music,
            places,
            place_icons: HashMap::new(),
            source_preferences: BrowserPlaces::default(),
            preferences_path,
            preferences_loading: true,
            preferences_writable: true,
            preferences_dirty: false,
            places_save_in_flight: false,
            folder_bookmark_pending: false,
            pending_places_save: None,
            unreported_places_save_error: None,
            places_status: None,
            samples: samples::SampleBrowser::default(),
            plugins: plugins::Plugins::default(),
            tree: tree::Tree::default(),
            listing: None,
            loading: false,
            pending_selection: None,
            scans: ScanCoordinator::default(),
            error: None,
            resizing: false,
            search,
            results_focus: cx.focus_handle(),
            results_focused: false,
            selected_index: 0,
            scroll: gpui::UniformListScrollHandle::new(),
            scrollbar: scrollbar::State::default(),
            _search_observer: observer,
            observed_query: String::new(),
            history: history::History::default(),
            playback_generation: 0,
            auto_audition: true,
            audition_gain: 1.0,
            audition_gain_dragging: false,
            playback_phase: Phase::Idle,
            playback_output: None,
            playback_path: None,
            playback_fraction: 0.0,
            waveform_bounds: Rc::new(Cell::new(None)),
            playback_in_flight: None,
            playback_pending: None,
            preview_cache: HashMap::new(),
            preview_sources: PreviewSources::default(),
            preview_selected: None,
            #[cfg(test)]
            preview_preparations: 0,
            preview_in_flight: None,
            preview_pending: None,
            device_catalog: None,
        }
    }

    /// Pure configured application-data location; performs no filesystem work.
    pub(super) fn data_directory(&self) -> Option<&std::path::Path> {
        self.preferences_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn test_preferences_path(&self) -> &std::path::Path {
        &self.preferences_path
    }

    /// Redirects later saves; an in-flight save keeps its captured path.
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn set_test_preferences_path(&mut self, path: PathBuf) {
        self.preferences_path = path;
    }

    #[cfg(feature = "ui-test-support")]
    pub(super) fn set_native_sampler_fixture_paths(&mut self, home: PathBuf, directory: PathBuf) {
        self.home = home.clone();
        self.music = home.join("Music");
        self.places = vec![directory.clone()];
        self.directory = directory.clone();
        self.section = BrowserSection::Place(directory);
        self.auto_audition = false;
    }
    fn reset_results(&mut self) {
        self.scrollbar.invalidate();
        self.selected_index = 0;
        self.results_focused = false;
        self.scroll.scroll_to_item(0, gpui::ScrollStrategy::Top);
    }

    pub(super) fn scan_generation(&self) -> u64 {
        self.scans.generation
    }
}

pub(super) struct BrowserAudioDragGhost;

impl gpui::Render for BrowserAudioDragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl gpui::IntoElement {
        div().w(px(0.0)).h(px(0.0))
    }
}

struct BrowserGhostGeometry {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    scale: f32,
}

struct BrowserGhostDecor {
    selector: &'static str,
    notched: bool,
    grid: Option<(Vec<super::musical_time::GridLine>, u64, u64)>,
    waveform_visual: Option<Div>,
}

fn browser_audio_drag_ghost(
    payload: &drag::BrowserAudioDrag,
    geometry: BrowserGhostGeometry,
    decor: BrowserGhostDecor,
) -> Div {
    let BrowserGhostGeometry {
        left,
        top,
        width,
        height,
        scale,
    } = geometry;
    let BrowserGhostDecor {
        selector,
        notched,
        grid,
        waveform_visual,
    } = decor;
    let metadata = payload.metadata.borrow().clone();
    let status = metadata.as_ref().map_or_else(
        || {
            payload.metadata_error.borrow().clone().map_or_else(
                || "preparing waveform…".to_owned(),
                |_| "preview unavailable".to_owned(),
            )
        },
        |metadata| {
            let seconds = metadata.playback_frames as f64 / f64::from(metadata.sample_rate);
            format!("{seconds:.2} s · {} frames", metadata.playback_frames)
        },
    );
    let stroke = 1.0 / scale.max(1.0);
    let fill = theme::color(theme::PaletteRole::ClipFill);
    let width = width.max(3.0);
    let height = height.max(3.0);
    let mut ghost = div()
        .debug_selector(move || selector.to_owned())
        .absolute()
        .left_0()
        .top_0()
        .w_full()
        .h_full()
        .overflow_hidden()
        .border_1()
        .border_l(px(stroke))
        .border_r(px(0.0))
        .border_color(rgb(theme::color(theme::PaletteRole::ClipOutline)))
        .bg(rgb(theme::color(theme::PaletteRole::ClipFill)));
    if let Some((lines, start, end)) = grid {
        ghost = ghost.children(super::tracks::aligned_clip_grid_layer(
            &lines, start, end, stroke, true,
        ));
    }
    if let Some(waveform) = waveform_visual {
        ghost = ghost.children([super::tracks::clip_waveform_body(waveform)]);
    } else if let Some(metadata) = metadata {
        ghost = ghost.children([super::tracks::clip_waveform_body(
            super::waveform::waveform_summary_view(
                Arc::clone(&metadata.waveform),
                crate::theme::color(crate::theme::PaletteRole::Waveform),
            ),
        )]);
    } else {
        ghost = ghost.child(super::tracks::clip_waveform_body(
            div().child(
                div()
                    .absolute()
                    .top(relative(0.5))
                    .left(px(4.0))
                    .right(px(4.0))
                    .h(px(1.0))
                    .bg(rgb(theme::color(theme::PaletteRole::ClipText))),
            ),
        ));
    }
    let ghost = ghost.child(
        super::clip_name::passive_name_header(&payload.label, fill, true)
            .debug_selector(|| "browser-drag-name".into())
            .gap_1()
            .child(div().flex_shrink_0().child(theme::text(status))),
    );
    super::clip_boundary::pixel_backdrop_shell(
        ghost,
        gpui::Bounds::new(
            gpui::point(px(left), px(top)),
            gpui::size(px(width), px(height)),
        ),
        super::clip_boundary::Backdrop {
            body: fill,
            header: fill,
            outline: theme::color(theme::PaletteRole::ClipOutline),
            hover_outline: theme::color(theme::PaletteRole::ClipOutline),
            stroke,
            bottom_radius: 0.0,
            header_height: super::clip_name::CLIP_NAME_HEIGHT,
            header_gradient: None,
            notch: super::clip_boundary::Notch::new(notched),
        },
        true,
        move || format!("{selector}-trailing"),
    )
    .opacity(0.9)
}

impl EditorView {
    pub(super) fn browser_scan_initial(&mut self, cx: &mut Context<Self>) {
        self.start_browser_places_load(cx);
        self.request_browser_scan(self.browser.directory.clone(), cx);
        self.start_sample_catalog(cx);
        self.start_plugin_catalog(cx);
    }

    pub(super) fn start_browser_places_load(&mut self, cx: &mut Context<Self>) {
        let path = self.browser.preferences_path.clone();
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _test_environment = test_environment;
                    hex_preferences::load(&path)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.browser_places_loaded(result, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn browser_places_loaded(
        &mut self,
        loaded: Result<BrowserPlaces, hex_preferences::PreferencesError>,
        cx: &mut Context<Self>,
    ) {
        self.browser.preferences_loading = false;
        match loaded {
            Err(error) => {
                self.browser.preferences_writable = false;
                self.browser.places_status =
                    Some(places_error("could not load browser places", &error));
            }
            Ok(loaded) => {
                self.browser
                    .layout
                    .set_samples_visible(!loaded.builtin_hidden(BrowserBuiltin::Samples));
                self.browser.source_preferences = loaded.clone();
                self.browser.place_icons = loaded
                    .icon_assignments()
                    .map(|(path, id)| (path.to_owned(), id.to_owned()))
                    .collect();
                if self
                    .browser
                    .place_icons
                    .values()
                    .any(|id| !place_icons::known(id))
                    || [BrowserBuiltin::Home, BrowserBuiltin::Music]
                        .into_iter()
                        .filter_map(|source| loaded.builtin_icon(source))
                        .any(|id| !place_icons::known(id))
                {
                    self.browser.places_status =
                        Some("some place icons unavailable · showing folder".into());
                }
                let mut combined = loaded.additional().to_vec();
                let local_places = self
                    .browser
                    .places
                    .iter()
                    .filter(|path| **path != self.browser.home && **path != self.browser.music)
                    .cloned();
                for path in local_places {
                    if !combined.contains(&path) {
                        combined.push(path);
                    }
                }
                if combined.len() > PLACE_CAPACITY.saturating_sub(DEFAULT_PLACE_COUNT) {
                    self.browser.preferences_writable = false;
                    self.browser.places_status = Some(format!(
                        "browser places exceed the {} folder limit",
                        PLACE_CAPACITY
                    ));
                } else {
                    let additions = combined
                        .into_iter()
                        .filter(|path| !self.browser.places.contains(path))
                        .collect::<Vec<_>>();
                    self.browser.places.extend(additions);
                    if self.browser.preferences_dirty {
                        self.browser.preferences_dirty = false;
                        self.queue_browser_places_save(cx);
                    }
                }
            }
        }
        self.reconcile_browser_source_visibility(cx);
        self.refresh_sample_catalog(cx);
    }

    pub(super) fn queue_browser_places_save(&mut self, cx: &mut Context<Self>) {
        if self.browser.preferences_loading {
            self.browser.preferences_dirty = true;
            self.browser.places_status = Some("saving browser places…".into());
            return;
        }
        if !self.browser.preferences_writable {
            return;
        }
        let additional = self
            .browser
            .places
            .iter()
            .filter(|path| **path != self.browser.home && **path != self.browser.music)
            .cloned();
        let places = BrowserPlaces::new(additional).and_then(|mut places| {
            places.vst3_scan_sources = self.browser.source_preferences.vst3_scan_sources.clone();
            for source in [
                BrowserBuiltin::Music,
                BrowserBuiltin::Home,
                BrowserBuiltin::Samples,
            ] {
                places.set_builtin_icon(
                    source,
                    self.browser.source_preferences.builtin_icon(source),
                )?;
                places.set_builtin_hidden(
                    source,
                    self.browser.source_preferences.builtin_hidden(source),
                );
            }
            for path in places.additional().to_vec() {
                places
                    .set_place_hidden(&path, self.browser.source_preferences.place_hidden(&path))?;
            }
            for (path, id) in &self.browser.place_icons {
                if places.additional().contains(path) {
                    places.set_icon(path, Some(id))?;
                }
            }
            Ok(places)
        });
        match places {
            Ok(places) => {
                self.browser.source_preferences = places.clone();
                self.browser.pending_places_save = Some(places);
                self.browser.places_status = Some("saving browser places…".into());
                self.start_browser_places_save(cx);
            }
            Err(error) => {
                self.browser.places_status =
                    Some(places_error("could not save browser places", &error));
            }
        }
    }

    fn start_browser_places_save(&mut self, cx: &mut Context<Self>) {
        if self.browser.places_save_in_flight || self.browser.preferences_loading {
            return;
        }
        let Some(places) = self.browser.pending_places_save.take() else {
            return;
        };
        self.browser.places_save_in_flight = true;
        let path = self.browser.preferences_path.clone();
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _test_environment = test_environment;
                    hex_preferences::save(&path, &places)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.browser_places_saved(result, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn browser_places_saved(
        &mut self,
        result: Result<(), hex_preferences::PreferencesError>,
        cx: &mut Context<Self>,
    ) {
        self.browser.places_save_in_flight = false;
        let error = result
            .err()
            .map(|error| places_error("could not save browser places", &error));
        if self.browser.pending_places_save.is_some() {
            // A queued save supersedes the data, not the user's notice: show
            // the failure now and keep the latest one until it is reported.
            if let Some(error) = error {
                self.browser.places_status = Some(error.clone());
                self.browser.unreported_places_save_error = Some(error);
            }
            self.start_browser_places_save(cx);
            return;
        }
        let earlier = self.browser.unreported_places_save_error.take();
        self.browser.places_status = Some(match (error, earlier) {
            (Some(error), _) => error,
            (None, Some(earlier)) => format!("{earlier} · a later save succeeded"),
            (None, None) => "browser places saved".into(),
        });
    }

    fn select_browser_source(&mut self, source: BrowserSection, cx: &mut Context<Self>) {
        self.checkpoint_browser_history(cx);
        self.apply_browser_source(source, cx);
        self.record_browser_location(cx);
    }

    fn apply_browser_source(&mut self, source: BrowserSection, cx: &mut Context<Self>) {
        self.browser.scans.generation = self.browser.scans.generation.saturating_add(1);
        self.browser.scans.pending = None;
        self.browser.loading = false;
        self.browser.error = None;
        self.browser.tree.source_changed();
        self.browser.section = source;
        self.reset_browser_results();
        match &self.browser.section {
            BrowserSection::Place(path) => self.request_browser_scan(path.clone(), cx),
            BrowserSection::All | BrowserSection::Projects => {
                if self
                    .browser
                    .listing
                    .as_ref()
                    .is_none_or(|listing| listing.directory != self.browser.directory)
                {
                    self.request_browser_scan(self.browser.directory.clone(), cx);
                }
            }
            _ => {}
        }
    }

    fn reconcile_browser_source(&mut self, cx: &mut Context<Self>) {
        if matches!(&self.browser.section, BrowserSection::Place(path) if !self.browser.places.contains(path))
        {
            let mut catalog = self.browser.samples.catalog.clone();
            catalog.retain_roots(&self.sample_roots());
            self.browser.samples.replace(catalog);
            self.select_browser_source(BrowserSection::Samples, cx);
            self.refresh_sample_catalog(cx);
            self.browser.places_status = Some("selected place removed · showing samples".into());
        }
    }

    fn request_browser_scan(&mut self, directory: PathBuf, cx: &mut Context<Self>) {
        if directory != self.browser.directory {
            self.reset_browser_results();
        }
        if matches!(self.browser.section, BrowserSection::Place(_)) {
            // Retain identity privately while unavailable rows are hidden. Query/source
            // changes clear this snapshot through reset_browser_results.
            if !self.browser.loading {
                self.browser.pending_selection = self.browser_selected_item(cx);
            }
            if self.browser.playback_path.is_some() || self.browser.playback_pending.is_some() {
                self.cancel_browser_sample_playback();
                self.status = "sample preview stopped".into();
            }
        }
        let start = self.browser.scans.request(directory.clone());
        self.browser.directory = directory.clone();
        self.browser.error = None;
        self.browser.loading = true;
        if let Some(request) = start {
            self.start_browser_scan(request, cx);
        }
    }

    fn reset_browser_results(&mut self) {
        self.browser.pending_selection = None;
        if self.browser.playback_output.is_some() {
            self.cancel_browser_sample_playback();
            self.status = "sample preview stopped".into();
        }
        self.browser.reset_results();
    }

    pub(super) fn cancel_browser_sample_playback(&mut self) {
        self.browser.playback_generation = self.browser.playback_generation.saturating_add(1);
        self.cancel_browser_playback_preparation();
        self.browser.playback_pending = None;
        self.browser.playback_phase = Phase::Idle;
        self.browser.playback_output = None;
        self.browser.playback_path = None;
        self.browser.playback_fraction = 0.0;
        if let Some(audio) = self.audio.as_mut() {
            audio.control.preview_cancel();
        }
    }

    /// Stop the in-flight audition preparation within bounded work; its
    /// result is already stale because the playback generation moved on.
    fn cancel_browser_playback_preparation(&self) {
        if let Some(cancel) = &self.browser.playback_in_flight {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    /// A completed sample drop (admitted into a device, track, Arrangement,
    /// Session or new track) ends that browser interaction, so it stops the
    /// active preview through the ordinary cancellation. A cancelled or refused
    /// drop leaves the preview playing, as in Live, until the user moves on.
    pub(super) fn stop_browser_preview_for_drop(&mut self) {
        if self.browser.playback_output.is_some() || self.browser.playback_pending.is_some() {
            self.cancel_browser_sample_playback();
        }
    }

    fn play_browser_sample(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.play_browser_sample_from(path, 0.0, cx);
    }

    fn play_browser_sample_from(&mut self, path: PathBuf, fraction: f64, cx: &mut Context<Self>) {
        // Previously painted controls can retain a path across source validation.
        if self.browser_place_unavailable() {
            return;
        }
        let fraction = if fraction.is_finite() {
            fraction.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if self.browser.playback_output.is_some() {
            self.cancel_browser_sample_playback();
        }
        self.browser.playback_phase = Phase::Unavailable;
        self.browser.playback_fraction = fraction as f32;
        self.status = "sample preview unavailable".into();
        if self.document_audio_mode == hex_session::DocumentAudioMode::VerifiedStreaming {
            self.error = Some("sample preview is unavailable in verified streaming mode".into());
            return;
        }
        let Some(generation) = self.browser.playback_generation.checked_add(1) else {
            self.error = Some("sample preview request counter exhausted".into());
            return;
        };
        if self
            .error
            .as_ref()
            .is_some_and(|error| error.starts_with("sample preview"))
        {
            self.error = None;
        }
        self.browser.playback_generation = generation;
        self.cancel_browser_playback_preparation();
        self.browser.playback_path = Some(path.clone());
        self.browser.playback_pending = None;
        let Some(audio) = self.audio.as_mut() else {
            self.error = Some("sample preview unavailable · no audio output".into());
            return;
        };
        let output_generation = match audio
            .control
            .preview_set_gain(self.browser.audition_gain)
            .and_then(|()| audio.control.preview_request())
        {
            Ok(token) => token,
            Err(error) => {
                self.error = Some(format!("sample preview failed · {error}").into());
                return;
            }
        };
        let Some(output_identity) = audio.control.output_identity() else {
            self.error = Some("sample preview unavailable · audio output closed".into());
            return;
        };
        let request = PlaybackPreviewRequest {
            generation,
            output_generation,
            output_identity,
            path,
            sample_rate: audio.sample_rate,
            start_fraction: fraction,
        };
        self.browser.playback_phase = Phase::Preparing;
        self.browser.playback_output = Some((output_identity, output_generation));
        self.status = "sample preview preparing".into();
        if self.browser.playback_in_flight.is_some() {
            self.browser.playback_pending = Some(request);
        } else {
            self.start_browser_sample_playback(request, cx);
        }
    }

    fn start_browser_sample_playback(
        &mut self,
        request: PlaybackPreviewRequest,
        cx: &mut Context<Self>,
    ) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.browser.playback_in_flight = Some(Arc::clone(&cancel));
        let sources = self.browser.preview_sources.clone();
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let path = request.path.clone();
            let sample_rate = request.sample_rate;
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _test_environment = test_environment;
                    sources
                        .prepare_playback(path, sample_rate, &cancel)
                        .map_err(|failure| failure.to_string())
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.browser.playback_in_flight = None;
                if request.generation == view.browser.playback_generation
                    && let Some(audio) = view.audio.as_mut().filter(|audio| {
                        audio.control.output_identity() == Some(request.output_identity)
                            && audio.sample_rate == request.sample_rate
                    })
                {
                    let admitted = result.and_then(|pcm| {
                        let frame = audition_geometry::frame_at_fraction(
                            pcm.frames().len(),
                            request.start_fraction,
                        );
                        audio
                            .control
                            .preview_publish_from(request.output_generation, pcm, frame)
                            .map_err(|error| error.to_string())
                    });
                    match admitted {
                        Ok(()) => {
                            if view
                                .error
                                .as_ref()
                                .is_some_and(|error| error.starts_with("sample preview"))
                            {
                                view.error = None;
                            }
                            // Callback acknowledgement, not publication, marks playback.
                        }
                        Err(error) => {
                            audio.control.preview_cancel();
                            view.browser.playback_output = None;
                            view.browser.playback_phase = Phase::Unavailable;
                            view.status = "sample preview failed".into();
                            view.error = Some(format!("sample preview failed · {error}").into())
                        }
                    }
                }
                if let Some(pending) = view.browser.playback_pending.take() {
                    view.start_browser_sample_playback(pending, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn browser_preview_selected(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if !self.browser.results_focused || self.browser.section == BrowserSection::Instruments {
            return;
        }
        if let Some(entry) = self.browser_selected_entry(cx) {
            match entry.kind {
                EntryKind::WaveAudio => self.play_browser_sample(entry.path, cx),
                EntryKind::Directory => self.expand_browser_folder(entry.path, cx),
                EntryKind::Project => {}
            }
        }
        cx.notify();
    }

    pub(super) fn poll_browser_sample_playback(&mut self) -> bool {
        let Some((identity, _)) = self.browser.playback_output else {
            return false;
        };
        let snapshot = self
            .audio
            .as_ref()
            .filter(|audio| audio.control.output_identity() == Some(identity))
            .and_then(|audio| audio.control.preview_snapshot());
        self.accept_browser_sample_snapshot(snapshot)
    }

    fn accept_browser_sample_snapshot(
        &mut self,
        snapshot: Option<hex_audio::sample_preview::Snapshot>,
    ) -> bool {
        let Some((_, generation)) = self.browser.playback_output else {
            return false;
        };
        let mut progress_changed = false;
        let phase = snapshot.map_or(Phase::Unavailable, |snapshot| {
            if snapshot.generation == generation {
                if snapshot.frames > 0
                    && matches!(
                        snapshot.phase,
                        Phase::Preparing | Phase::Playing | Phase::Finished
                    )
                {
                    let fraction =
                        (snapshot.frame as f64 / snapshot.frames as f64).clamp(0.0, 1.0) as f32;
                    let width = self
                        .browser
                        .waveform_bounds
                        .get()
                        .map_or(1.0, |bounds| f32::from(bounds.size.width).max(1.0));
                    progress_changed = (fraction * width).floor()
                        != (self.browser.playback_fraction * width).floor();
                    self.browser.playback_fraction = fraction;
                }
                snapshot.phase
            } else if snapshot.generation == 0 && snapshot.phase == Phase::Idle {
                Phase::Idle
            } else {
                Phase::Unavailable
            }
        });
        self.accept_browser_sample_phase(phase) || progress_changed
    }

    fn accept_browser_sample_phase(&mut self, phase: Phase) -> bool {
        if self.browser.playback_phase == phase {
            return false;
        }
        self.browser.playback_phase = phase;
        self.status = match phase {
            Phase::Idle => "sample preview stopped",
            Phase::Preparing => "sample preview preparing",
            Phase::Playing => "sample preview playing",
            Phase::Finished => "sample preview finished",
            Phase::Unavailable => "sample preview unavailable",
        }
        .into();
        if matches!(phase, Phase::Finished | Phase::Idle) {
            self.browser.playback_output = None;
        } else if phase == Phase::Unavailable {
            self.cancel_browser_sample_playback();
            self.browser.playback_phase = Phase::Unavailable;
            self.error = Some("sample preview unavailable · audio output stopped".into());
        }
        true
    }

    fn browser_selection_audition(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.browser.playback_path != path {
            if self.browser.playback_output.is_some() {
                self.cancel_browser_sample_playback();
                self.status = "sample preview stopped".into();
            } else {
                self.browser.playback_path = None;
                self.browser.playback_fraction = 0.0;
                self.browser.playback_phase = Phase::Idle;
            }
        }
        if self.browser.auto_audition
            && let Some(path) = path
        {
            self.play_browser_sample(path, cx);
        }
    }

    fn browser_preview_cell(&mut self, path: PathBuf, sample_rate: u32) -> PreviewSlot {
        let key = PreviewKey { path, sample_rate };
        if let Some(metadata) = self.browser.preview_cache.get(&key) {
            return metadata.clone();
        }
        self.evict_browser_preview_slot();
        let slot = PreviewSlot::default();
        self.browser.preview_cache.insert(key, slot.clone());
        slot
    }

    fn evict_browser_preview_slot(&mut self) {
        if self.browser.preview_cache.len() < PREVIEW_CACHE_CAPACITY {
            return;
        }
        let protected = self
            .browser
            .preview_in_flight
            .iter()
            .chain(self.browser.preview_pending.iter())
            .map(|request| &request.key)
            .chain(self.browser.preview_selected.iter());
        if let Some(evicted) = self
            .browser
            .preview_cache
            .keys()
            .find(|candidate| !protected.clone().any(|key| key == *candidate))
            .cloned()
        {
            self.browser.preview_cache.remove(&evicted);
        }
    }

    fn request_browser_audio_preview(
        &mut self,
        path: PathBuf,
        sample_rate: u32,
        cx: &mut Context<Self>,
    ) -> Rc<RefCell<Option<Rc<AudioPreview>>>> {
        let slot = self.browser_preview_cell(path.clone(), sample_rate);
        if slot.metadata.borrow().is_some() || slot.error.borrow().is_some() {
            return slot.metadata;
        }
        self.queue_browser_audio_preview(PreviewKey { path, sample_rate }, slot.clone(), cx);
        slot.metadata
    }

    fn request_browser_audio_preview_for_drag(
        &mut self,
        drag: &drag::BrowserAudioDrag,
        cx: &mut Context<Self>,
    ) {
        let key = PreviewKey {
            path: drag.path.clone(),
            sample_rate: drag.sample_rate,
        };
        let slot = PreviewSlot {
            metadata: drag.metadata.clone(),
            error: drag.metadata_error.clone(),
        };
        if !self.browser.preview_cache.contains_key(&key) {
            self.evict_browser_preview_slot();
        }
        self.browser.preview_cache.insert(key.clone(), slot.clone());
        *slot.metadata.borrow_mut() = None;
        *slot.error.borrow_mut() = None;
        self.queue_browser_audio_preview(key, slot, cx);
    }

    fn queue_browser_audio_preview(
        &mut self,
        key: PreviewKey,
        slot: PreviewSlot,
        cx: &mut Context<Self>,
    ) {
        if slot.metadata.borrow().is_some() {
            return;
        }
        *slot.error.borrow_mut() = None;
        let request = PreviewRequest {
            key,
            slot: slot.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        if let Some(in_flight) = self
            .browser
            .preview_in_flight
            .as_ref()
            .filter(|in_flight| in_flight.key == request.key)
        {
            if Rc::ptr_eq(&in_flight.slot.metadata, &request.slot.metadata) {
                self.browser.preview_pending = None;
            } else {
                self.browser.preview_pending = Some(request);
            }
            return;
        }
        if self
            .browser
            .preview_pending
            .as_ref()
            .is_some_and(|pending| pending.key == request.key)
        {
            return;
        }
        if self.browser.preview_in_flight.is_some() {
            self.browser.preview_pending = Some(request);
            self.cancel_superseded_browser_audio_preview();
        } else {
            self.start_browser_audio_preview(request, cx);
        }
    }

    /// A newer waveform request replaced the pending one; stop the in-flight
    /// job unless the selected row or an active drag still needs its result.
    fn cancel_superseded_browser_audio_preview(&self) {
        let Some(in_flight) = self.browser.preview_in_flight.as_ref() else {
            return;
        };
        let needed = self.browser.preview_selected.as_ref() == Some(&in_flight.key)
            || self.active_browser_audio_drag.as_ref().is_some_and(|drag| {
                drag.path == in_flight.key.path && drag.sample_rate == in_flight.key.sample_rate
            });
        if !needed {
            in_flight.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn start_browser_audio_preview(&mut self, request: PreviewRequest, cx: &mut Context<Self>) {
        #[cfg(test)]
        {
            self.browser.preview_preparations += 1;
        }
        self.browser.preview_in_flight = Some(request.clone());
        let sources = self.browser.preview_sources.clone();
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let path = request.key.path.clone();
            let sample_rate = request.key.sample_rate;
            let cancel = Arc::clone(&request.cancel);
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _test_environment = test_environment;
                    sources.prepare_waveform(path, sample_rate, &cancel)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.browser_audio_preview_finished(request, result, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn browser_audio_preview_finished(
        &mut self,
        request: PreviewRequest,
        result: Result<AudioPreview, PreviewFailure>,
        cx: &mut Context<Self>,
    ) {
        if self
            .browser
            .preview_in_flight
            .as_ref()
            .is_some_and(|in_flight| in_flight.key == request.key)
        {
            self.browser.preview_in_flight = None;
        }
        match result {
            Ok(preview) => *request.slot.metadata.borrow_mut() = Some(Rc::new(preview)),
            // A superseded job leaves its slot empty, so a later request retries.
            Err(PreviewFailure::Cancelled) => {}
            Err(PreviewFailure::Failed(_)) => {
                *request.slot.error.borrow_mut() =
                    Some("audio preview could not be prepared".into());
            }
        }
        if let Some(active) = self
            .active_browser_audio_drag
            .clone()
            .filter(|drag| drag.path == request.key.path)
        {
            self.refresh_browser_audio_drag(&active);
        }
        if let Some(pending) = self.browser.preview_pending.take() {
            self.start_browser_audio_preview(pending, cx);
        }
    }

    pub(super) fn floating_browser_audio_drag_ghost(&self, scale: f32) -> Option<Div> {
        let payload = self
            .active_browser_audio_drag
            .as_ref()
            .filter(|drag| self.browser_audio_drag_is_current(drag))?;
        if let Some(drag::BrowserDropDestination::NewTrack {
            before_track: None,
            start,
        }) = payload.destination.get()
        {
            let state = self.state.as_ref()?;
            let body = self.arrangement_bounds?;
            let viewport = self.visible_viewport(state.timeline_frames);
            let timeline_width = body.width - self.track_header_width();
            if !timeline_width.is_finite() || timeline_width <= 0.0 || viewport.length == 0 {
                return None;
            }
            let Some(end) = payload
                .metadata
                .borrow()
                .as_ref()
                .filter(|metadata| metadata.sample_rate == state.project.sample_rate())
                .and_then(|metadata| start.checked_add(metadata.playback_frames))
            else {
                let left = body.left
                    + ((start.saturating_sub(viewport.start).min(viewport.length)) as f64
                        / viewport.length as f64
                        * f64::from(timeline_width)) as f32;
                let top = if state.project.tracks().is_empty() {
                    body.top
                } else {
                    payload.insertion_line_y.get().unwrap_or(body.top) + 1.0
                };
                let height = (body.top + body.height - top).clamp(0.0, 44.0);
                if height <= 0.0 {
                    return None;
                }
                return Some(browser_audio_drag_ghost(
                    payload,
                    BrowserGhostGeometry {
                        left,
                        top,
                        width: 3.0,
                        height,
                        scale,
                    },
                    BrowserGhostDecor {
                        selector: "browser-audio-append-preview",
                        notched: false,
                        grid: None,
                        waveform_visual: None,
                    },
                ));
            };
            let visible_start = start.max(viewport.start);
            let visible_end = end.min(viewport.end()?);
            if visible_start >= visible_end {
                return None;
            }
            let left = body.left
                + ((visible_start - viewport.start) as f64 / viewport.length as f64
                    * f64::from(timeline_width)) as f32;
            let width = (visible_end - visible_start) as f64 / viewport.length as f64
                * f64::from(timeline_width);
            let top = if state.project.tracks().is_empty() {
                body.top
            } else {
                payload.insertion_line_y.get().unwrap_or(body.top) + 1.0
            };
            let height = (body.top + body.height - top).clamp(0.0, 44.0);
            if height <= 0.0 {
                return None;
            }
            return Some(browser_audio_drag_ghost(
                payload,
                BrowserGhostGeometry {
                    left,
                    top,
                    width: width as f32,
                    height,
                    scale,
                },
                BrowserGhostDecor {
                    selector: "browser-audio-append-preview",
                    notched: visible_end == end,
                    grid: Some((
                        super::musical_time::grid_lines(
                            &state.project,
                            viewport,
                            timeline_width,
                            self.snap_grid,
                        ),
                        visible_start,
                        visible_end,
                    )),
                    waveform_visual: payload
                        .metadata
                        .borrow()
                        .as_ref()
                        .filter(|metadata| metadata.sample_rate == state.project.sample_rate())
                        .map(|metadata| {
                            super::waveform::waveform_source_segment_view(
                                Arc::clone(&metadata.waveform),
                                start,
                                visible_start,
                                visible_end,
                                crate::theme::color(crate::theme::PaletteRole::Waveform),
                            )
                        }),
                },
            ));
        }
        if payload.lane_hovered.get() {
            return Some(
                div()
                    .debug_selector(|| "browser-audio-floating-preview-hidden".to_owned())
                    .absolute()
                    .left_0()
                    .top_0()
                    .w(px(0.0))
                    .h(px(0.0)),
            );
        }
        let (pointer_x, pointer_y) = payload.last_pointer.get()?;
        let (left, top) = browser_audio_drag_ghost_position((pointer_x, pointer_y));
        Some(browser_audio_drag_ghost(
            payload,
            BrowserGhostGeometry {
                left,
                top,
                width: 184.0,
                height: 36.0,
                scale,
            },
            BrowserGhostDecor {
                selector: "browser-audio-floating-preview",
                notched: true,
                grid: None,
                waveform_visual: None,
            },
        ))
    }

    fn start_browser_scan(&mut self, request: ScanRequest, cx: &mut Context<Self>) {
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let path_for_scan = request.directory.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _test_environment = test_environment;
                    scan_directory(path_for_scan)
                })
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
            let selection = if self.browser.loading
                && matches!(self.browser.section, BrowserSection::Place(_))
            {
                self.browser.pending_selection.take()
            } else {
                self.browser_selected_item(cx)
            };
            self.browser.samples.invalidate_listing();
            match result {
                Ok(listing) => {
                    self.browser.listing = Some(listing);
                    self.browser.error = None;
                    self.browser.loading = false;
                }
                Err(_)
                    if !matches!(self.browser.section, BrowserSection::Place(_))
                        && request.directory == self.browser.music
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
                    self.browser.error = Some(
                        hex_session::service::text::lowercase_outside_verbatim(&error.to_string()),
                    );
                    self.browser.loading = false;
                }
            }
            self.restore_browser_selection(selection, cx);
        }
        if let Some(request) = next {
            self.start_browser_scan(request, cx);
        }
    }

    pub(super) fn browser_content_width(&self, window: &Window) -> f32 {
        let content = if !self.browser.open {
            COLLAPSED_WIDTH
        } else {
            let base =
                clamped_panel_width(self.browser.width, f32::from(window.bounds().size.width));
            base + self.browser.layout.editor_width(base) + 4.0
        };
        content + 2.0 * hex_widgets::PANEL_FRAME_INSET
    }

    pub(super) fn browser_key_context(&self, window: &Window, cx: &App) -> &'static str {
        if self
            .browser
            .search
            .read(cx)
            .focus_handle()
            .is_focused(window)
        {
            "hex_browser_search"
        } else if self.browser.results_focus.is_focused(window) {
            "hex_browser"
        } else {
            "hex_editor"
        }
    }

    pub(super) fn search_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.gain_numeric_editing(cx) {
            self.error = Some(GAIN_SEARCH_ERROR.into());
            cx.notify();
            return;
        }
        if self
            .error
            .as_ref()
            .is_some_and(|error| error.as_ref() == GAIN_SEARCH_ERROR)
        {
            self.error = None;
        }
        if !super::device_panel::finalize_controls(self, cx) {
            cx.notify();
            return;
        }
        if self.clip_name_edit.is_some() {
            self.finish_clip_name(true, window, cx);
        }
        if self.theme_settings.open {
            self.cancel_theme_settings(window, cx);
            if self.theme_settings.open {
                return;
            }
        }
        if self.context_menu.is_some() {
            self.dismiss_context_menu(window, cx);
        }
        if self.controller_input_is_open() {
            self.close_controller_input_menu(window, cx);
        }
        self.browser.open = true;
        self.select_browser_source(BrowserSection::All, cx);
        self.browser.layout.reveal_all();
        self.reset_browser_results();
        self.browser.search.update(cx, |input, cx| {
            input.focus_and_select_all(window, cx);
        });
        cx.notify();
    }

    pub(super) fn toggle_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.browser.open = !self.browser.open;
        if !self.browser.open {
            self.browser.layout.sidebar_drag = None;
            self.browser.resizing = false;
            self.browser.results_focused = false;
            self.focus.focus(window);
        }
        cx.notify();
    }

    pub(super) fn browser_previous(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.move_browser_selection(false, cx);
        if self.browser.results_focused {
            self.browser.results_focus.focus(window);
        }
    }

    pub(super) fn browser_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.move_browser_selection(true, cx);
        if self.browser.results_focused {
            self.browser.results_focus.focus(window);
        }
    }

    fn move_browser_selection(&mut self, next: bool, cx: &mut Context<Self>) {
        self.checkpoint_browser_history(cx);
        let previous = (self.browser.results_focused, self.browser.selected_index);
        let count = self.browser_visible_entries(cx).len()
            + self.browser_visible_devices(cx).len()
            + self.browser_visible_plugins(cx).len();
        self.browser.selected_index = if !self.browser.results_focused || count == 0 {
            0
        } else if next {
            self.browser.selected_index.saturating_add(1).min(count - 1)
        } else {
            self.browser.selected_index.saturating_sub(1).min(count - 1)
        };
        self.browser.results_focused = count > 0;
        if previous != (self.browser.results_focused, self.browser.selected_index) {
            let path = self
                .browser_selected_entry(cx)
                .filter(|entry| entry.kind == EntryKind::WaveAudio)
                .map(|entry| entry.path);
            self.browser_selection_audition(path, cx);
        }
        self.browser.scroll.scroll_to_item(
            self.browser_scroll_row(self.browser.selected_index, cx),
            gpui::ScrollStrategy::Top,
        );
        cx.notify();
    }

    pub(super) fn browser_activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.checkpoint_browser_history(cx);
        if let Some(BrowserItemKey::Plugin(module, cid)) = self.browser_selected_item(cx) {
            if !self
                .browser
                .search
                .read(cx)
                .focus_handle()
                .is_focused(window)
            {
                self.activate_browser_plugin(&module, cid, cx);
            }
            cx.notify();
            return;
        }

        if self.browser.section == BrowserSection::Instruments {
            let Some(BrowserItemKey::Device(hex_engine::NativeDeviceClass::Instrument, type_id)) =
                self.browser_selected_item(cx)
            else {
                self.error =
                    Some("select an available browser instrument before pressing enter".into());
                cx.notify();
                return;
            };
            // Selection is metadata only; native defaults are constructed by Service.
            let registered = hex_engine::native_device_catalog_for_class(
                hex_engine::NativeDeviceClass::Instrument,
            )
            .is_ok_and(|entries| entries.iter().any(|entry| entry.type_id == type_id));
            let Some(state) = self.state.as_ref().filter(|_| registered) else {
                self.error = Some("selected browser instrument is unavailable".into());
                cx.notify();
                return;
            };
            let gain = match self.track_defaults.captured_gain() {
                Ok(gain) => gain,
                Err(error) => {
                    self.error = Some(error.into());
                    cx.notify();
                    return;
                }
            };
            self.enqueue(Request::InsertBrowserInstrumentTrack {
                project_generation: self.project_generation,
                revision: state.revision,
                class: hex_engine::NativeDeviceClass::Instrument,
                type_id,
                new_track_gain: Some(gain),
            });
            cx.notify();
            return;
        }
        let Some(entry) = self.browser_selected_entry(cx) else {
            if matches!(
                self.browser_selected_item(cx),
                Some(BrowserItemKey::Device(..))
            ) {
                self.status = "drag a browser device onto a track or device rail".into();
                cx.notify();
            }
            return;
        };
        match entry.kind {
            EntryKind::Directory => self.toggle_browser_folder(entry.path, false, cx),
            EntryKind::WaveAudio => {
                // Text/IME Enter must not import a sample. Non-file device activation
                // keeps its existing informational action, independent of this guard.
                if !self
                    .browser
                    .search
                    .read(cx)
                    .focus_handle()
                    .is_focused(window)
                {
                    self.insert_browser_audio(entry.path, cx);
                }
            }
            EntryKind::Project => self.open_browser_project(entry.path, cx),
        }
        cx.notify();
    }

    pub(super) fn browser_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_plugin_effect();
        self.cancel_first_sampler_load(None);
        self.checkpoint_browser_history(cx);
        // Audio/device owners handle Escape first. A remaining browser folder
        // drag has no model gesture; retire it before clearing the query.
        if cx.stop_active_drag(window) {
            cx.notify();
            return;
        }
        if self
            .browser
            .search
            .read(cx)
            .focus_handle()
            .is_focused(window)
            || self.browser.results_focus.is_focused(window)
        {
            self.reset_browser_results();
            self.browser.search.update(cx, |input, cx| {
                input.clear(cx);
            });
        } else {
            self.browser.results_focused = true;
        }
        cx.notify();
    }

    fn browser_selected_item(&self, cx: &App) -> Option<BrowserItemKey> {
        if !self.browser.results_focused {
            return None;
        }
        self.browser_item_at(self.browser.selected_index, cx)
    }

    fn browser_selected_sample(&self, cx: &App) -> Option<PathBuf> {
        if !self.browser.results_focused {
            return None;
        }
        self.browser_selected_entry(cx)
            .filter(|entry| entry.kind == EntryKind::WaveAudio)
            .map(|entry| entry.path)
    }

    /// Reveal only the same stable sample painted by the control. A replacement
    /// result or selection cannot turn an old pointer event into a new selection.
    /// This view-only action must not enter the selection/audition request path.
    fn reveal_browser_sample(
        &mut self,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.browser_selected_sample(cx).as_deref() != Some(path) {
            return;
        }
        let index = self.browser.selected_index;
        self.browser
            .scroll
            .scroll_to_item(index, gpui::ScrollStrategy::Top);
        self.browser.results_focus.focus(window);
        cx.notify();
    }

    fn restore_browser_selection(&mut self, selection: Option<BrowserItemKey>, cx: &App) {
        let index = selection
            .as_ref()
            .and_then(|key| self.browser_history_index(key, cx));
        if let Some(index) = index {
            self.browser.selected_index = index;
            self.browser.results_focused = true;
            self.browser.scroll.scroll_to_item(
                self.browser_scroll_row(index, cx),
                gpui::ScrollStrategy::Top,
            );
        } else {
            self.reset_browser_results();
        }
    }

    /// The current sample catalog for relink lookups, and whether a scan is
    /// still running (so results may be incomplete). Callers search a clone on a
    /// background worker.
    pub(super) fn sample_catalog_snapshot(&self) -> (hex_library::catalog::SampleCatalog, bool) {
        (
            self.browser.samples.catalog.clone(),
            self.browser.samples.history_loading(),
        )
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn test_replace_sample_catalog(
        &mut self,
        catalog: hex_library::catalog::SampleCatalog,
    ) -> bool {
        let settled = !self.browser.samples.history_loading();
        self.browser.samples.replace(catalog);
        settled
    }

    fn browser_place_unavailable(&self) -> bool {
        matches!(self.browser.section, BrowserSection::Place(_))
            && (self.browser.loading || self.browser.error.is_some())
    }

    fn browser_devices_first(&self, cx: &App) -> bool {
        self.browser.section == BrowserSection::All
            && !self.browser.search.read(cx).text().is_empty()
    }

    fn browser_rows(&self, cx: &App) -> BrowserRows {
        BrowserRows::new(
            self.browser_visible_entries(cx).len(),
            self.browser_visible_devices(cx).len(),
            self.browser_visible_plugins(cx).len(),
            self.browser_devices_first(cx),
        )
    }

    fn browser_selected_entry(&self, cx: &App) -> Option<LibraryEntry> {
        let local = self
            .browser
            .selected_index
            .checked_sub(self.browser_rows(cx).files.start)?;
        self.browser_visible_entries(cx).get(local).cloned()
    }

    fn browser_visible_entries(&self, cx: &App) -> Arc<Vec<LibraryEntry>> {
        if self.browser_place_unavailable() {
            return Arc::new(Vec::new());
        }
        let query = self.browser.search.read(cx).text().to_lowercase();
        let listing = self
            .browser
            .listing
            .as_ref()
            .filter(|listing| listing.directory == self.browser.directory);
        let base = match &self.browser.section {
            BrowserSection::Samples
                if samples::ResultsMode::for_query(&query) == samples::ResultsMode::Hierarchy =>
            {
                self.browser.tree.roots(self.sample_roots())
            }
            BrowserSection::Samples => self.browser.samples.visible(&query),
            BrowserSection::Place(root) if !query.is_empty() => self
                .browser
                .samples
                .visible_with_listing(&query, Some(root), listing),
            BrowserSection::Place(_) => self.browser.samples.folder(&query, listing, false),
            BrowserSection::Collection(_) => Arc::new(Vec::new()),
            BrowserSection::All if query.is_empty() => {
                let mut roots = self.sample_roots();
                roots.push(self.browser.directory.clone());
                self.browser.tree.roots(roots)
            }
            BrowserSection::All => self
                .browser
                .samples
                .visible_with_listing(&query, None, listing),
            BrowserSection::Projects => self.browser.samples.folder(&query, listing, true),
            BrowserSection::Instruments | BrowserSection::Plugins => Arc::new(Vec::new()),
        };
        self.browser.tree.visible(
            base,
            &query,
            match self.browser.section {
                BrowserSection::Samples => tree::Files::Samples,
                BrowserSection::Projects => tree::Files::Projects,
                _ => tree::Files::All,
            },
        )
    }

    fn browser_visible_devices(
        &self,
        cx: &App,
    ) -> Vec<(
        hex_engine::NativeDeviceClass,
        hex_engine::NativeDeviceCatalogEntry,
    )> {
        if !matches!(
            self.browser.section,
            BrowserSection::All | BrowserSection::Instruments
        ) {
            return Vec::new();
        }
        let query = self.browser.search.read(cx).text().to_lowercase();
        self.browser
            .device_catalog
            .as_ref()
            .and_then(|catalog| catalog.as_ref().ok())
            .into_iter()
            .flatten()
            .filter(|(_, entry)| query.is_empty() || entry.name.to_lowercase().contains(&query))
            .cloned()
            .collect()
    }

    pub(super) fn browser_view(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        let width = self.browser_content_width(window);
        let content = self.browser_content_view(window, cx);
        hex_widgets::panel_frame(theme::workspace_panel_style(), content)
            .w(px(width))
            .flex_shrink_0()
    }

    fn browser_content_view(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        self.browser.scrollbar.begin_render();
        if !self.browser.open {
            let rail = div()
                .w(px(COLLAPSED_WIDTH))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .items_center()
                .pt_1()
                .gap_1()
                .bg(rgb(theme::color(theme::PaletteRole::Surface)))
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

        self.reconcile_browser_source(cx);
        self.reconcile_browser_tree_requests(cx);
        self.restore_browser_history_position(cx);
        let path = self.browser.directory.clone();
        let folder_dialog = self.browser_folder_button(cx);
        let control = self.browser_random_button(cx);
        let folder_dialog = div()
            .flex()
            .items_center()
            .gap_1()
            .child(folder_dialog)
            .child(control);
        let section = self.browser.section.clone();
        if matches!(section, BrowserSection::All | BrowserSection::Instruments)
            && self.browser.device_catalog.is_none()
        {
            let selection = self.browser_selected_item(cx);
            let catalog = (|| {
                let mut entries = Vec::new();
                for class in [
                    hex_engine::NativeDeviceClass::Instrument,
                    hex_engine::NativeDeviceClass::Effect,
                ] {
                    entries.extend(
                        hex_engine::native_device_catalog_for_class(class)
                            .map_err(|error| {
                                hex_session::service::text::lowercase_outside_verbatim(
                                    &error.to_string(),
                                )
                            })?
                            .into_iter()
                            .map(|entry| (class, entry)),
                    );
                }
                Ok(entries)
            })();
            self.browser.device_catalog = Some(catalog);
            if selection.is_some() {
                self.restore_browser_selection(selection, cx);
            }
        }
        let entries = self.browser_visible_entries(cx);
        let devices = if section == BrowserSection::All {
            self.browser_visible_devices(cx)
        } else {
            Vec::new()
        };
        let file_count = entries.len();
        let device_count = devices.len();
        let plugins = self.browser_visible_plugins(cx);
        let plugin_groups =
            (section == BrowserSection::Plugins).then(|| plugins::PluginGroups::new(&plugins));
        let row_count = file_count
            + device_count
            + plugins.len()
            + plugin_groups
                .as_ref()
                .map_or(0, plugins::PluginGroups::heading_count);
        let positions = BrowserRows::new(
            file_count,
            device_count,
            plugins.len(),
            self.browser_devices_first(cx),
        );
        let rows = uniform_list(
            "library-browser-entries",
            row_count,
            cx.processor(move |view, range: std::ops::Range<usize>, _window, cx| {
                range
                    .filter_map(|index| {
                        if let Some(groups) = &plugin_groups {
                            match groups.line(index)? {
                                plugins::PluginLine::Heading(group, _) => {
                                    Some(Self::browser_plugin_heading(group))
                                }
                                plugins::PluginLine::Plugin(logical) => plugins
                                    .get(logical)
                                    .cloned()
                                    .map(|row| view.browser_plugin_row(logical, row, cx)),
                            }
                        } else if positions.files.contains(&index) {
                            entries
                                .get(index - positions.files.start)
                                .cloned()
                                .map(|entry| view.browser_row(index, entry, cx))
                        } else if positions.devices.contains(&index) {
                            devices.get(index - positions.devices.start).cloned().map(
                                |(class, entry)| view.browser_device_row(index, class, entry, cx),
                            )
                        } else {
                            plugins
                                .get(index.checked_sub(positions.plugins.start)?)
                                .cloned()
                                .map(|row| view.browser_plugin_row(index, row, cx))
                        }
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
            .bg(rgb(theme::color(theme::PaletteRole::Surface)));
        let toolbar = self.browser_toolbar(cx);
        if section == BrowserSection::Samples {
            panel = panel.child(self.sample_filters(cx));
        }
        if matches!(
            section,
            BrowserSection::All | BrowserSection::Projects | BrowserSection::Place(_)
        ) {
            panel =
                panel.child(
                    div()
                        .h(px(24.0))
                        .flex_shrink_0()
                        .px_1()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(div().flex_1().min_w_0().overflow_hidden().text_xs().child(
                            theme::text(if section == BrowserSection::All {
                                "all folders"
                            } else {
                                path.file_name()
                                    .and_then(|name| name.to_str())
                                    .unwrap_or("folder")
                            }),
                        ))
                        .child(folder_dialog),
                );
        }
        panel = panel.child(self.browser_tag_selector(cx));
        panel = panel.child(
            div()
                .h(px(22.0))
                .flex_shrink_0()
                .px_2()
                .bg(rgb(theme::color(theme::PaletteRole::Hover)))
                .text_xs()
                .child(theme::text("name")),
        );
        if section == BrowserSection::All {
            let notice = self
                .browser
                .error
                .clone()
                .or_else(|| {
                    self.browser
                        .device_catalog
                        .as_ref()
                        .and_then(|catalog| catalog.as_ref().err())
                        .map(|error| format!("device catalog unavailable · {error}"))
                })
                .or_else(|| self.browser.loading.then(|| "scanning folder…".into()));
            if let Some(notice) = notice {
                panel = panel.child(
                    div()
                        .px_1()
                        .text_xs()
                        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                        .child(theme::text(notice)),
                );
            }
        }
        match section {
            BrowserSection::Plugins => {
                let controls = self.plugin_controls(cx);
                panel = panel
                    .child(controls)
                    .child(self.browser_results(rows, row_count, cx));
            }
            BrowserSection::Collection(_) => {
                panel = panel.child(
                    div()
                        .flex_1()
                        .px_2()
                        .pt_2()
                        .text_xs()
                        .child(theme::text("collection assignments pending")),
                );
            }
            BrowserSection::Instruments => {
                match self
                    .browser
                    .device_catalog
                    .clone()
                    .unwrap_or_else(|| Ok(Vec::new()))
                {
                    Ok(_) => {
                        let entries = self.browser_visible_devices(cx);
                        if entries.is_empty() {
                            panel = panel.child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .px_2()
                                    .pt_2()
                                    .text_xs()
                                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                                    .child(theme::text("no matching devices")),
                            );
                        } else {
                            let count = entries.len();
                            let list = uniform_list(
                                "native-device-browser-entries",
                                count,
                                cx.processor(
                                    move |view, range: std::ops::Range<usize>, _window, cx| {
                                        range
                                            .filter_map(|index| {
                                                entries.get(index).cloned().map(|(class, entry)| {
                                                    view.browser_device_row(index, class, entry, cx)
                                                })
                                            })
                                            .collect()
                                    },
                                ),
                            )
                            .h_full()
                            .min_h_0()
                            .w_full()
                            .track_scroll(self.browser.scroll.clone())
                            .flex_1();
                            panel = panel.child(self.browser_results(list, count, cx));
                        }
                    }
                    Err(error) => {
                        panel = panel.child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .px_2()
                                .pt_2()
                                .text_xs()
                                .text_color(rgb(theme::color(theme::PaletteRole::Primary)))
                                .child(theme::text(format!(
                                    "device catalog unavailable · {error}"
                                ))),
                        );
                    }
                }
            }
            BrowserSection::Samples => {
                if row_count == 0 {
                    panel = panel.child(
                        div()
                            .flex_1()
                            .px_2()
                            .pt_2()
                            .text_xs()
                            .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                            .child(theme::text(
                                "no matching samples · add a folder or change filters",
                            )),
                    );
                } else {
                    panel = panel.child(self.browser_results(rows, row_count, cx));
                }
            }
            _ if row_count == 0
                && self.browser.loading
                && (matches!(self.browser.section, BrowserSection::Place(_))
                    || self
                        .browser
                        .listing
                        .as_ref()
                        .is_none_or(|listing| listing.directory != path)) =>
            {
                panel = panel.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .px_2()
                        .pt_2()
                        .text_xs()
                        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                        .child(theme::text("scanning folder…")),
                );
            }
            _ if row_count == 0 && self.browser.error.is_some() => {
                let error = self.browser.error.clone().unwrap_or_default();
                panel = panel.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .px_2()
                        .pt_2()
                        .text_xs()
                        .text_color(rgb(theme::color(theme::PaletteRole::Primary)))
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
                    "no matching items"
                };
                panel = panel.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .px_2()
                        .pt_2()
                        .text_xs()
                        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                        .child(theme::text(empty)),
                );
            }
            _ => {
                panel = panel.child(self.browser_results(rows, row_count, cx));
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
                            .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                            .child(theme::text("listing capped · open a smaller folder")),
                    );
                }
            }
        }

        if let Some(status) = self.browser.tree.status.clone() {
            panel = panel.child(
                div()
                    .h(px(18.0))
                    .flex_shrink_0()
                    .px_1()
                    .overflow_hidden()
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text(status)),
            );
        }
        let preview = self.browser_selection_preview(cx);
        let sidebar = self.browser_sidebar(pane_width, cx);
        let editor = self.browser_tag_editor(pane_width, cx);
        let editor_width = self.browser.layout.editor_width(pane_width);
        panel = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(panel.w_full().flex_1().min_h_0().min_w_0())
            .child(preview);
        // The rail belongs to the fixed base column. The optional editor is a
        // sibling of that WHOLE column, so neither opening control can move.
        panel = div()
            .w(px(pane_width + editor_width))
            .flex_shrink_0()
            .min_h_0()
            .flex()
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .text_color(rgb(theme::color(theme::PaletteRole::Text)))
            .child(
                div()
                    .w(px(pane_width))
                    .flex_shrink_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(toolbar)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .child(sidebar)
                            .child(self.browser_sidebar_resize(pane_width, cx))
                            .child(panel),
                    ),
            )
            .children(editor);

        let entity = cx.entity();
        let mut resize = div()
            .id("browser-resize-handle")
            .debug_selector(|| "browser-resize-handle".to_owned())
            .h_full()
            .w(px(4.0))
            .flex_shrink_0()
            .cursor_col_resize()
            .relative()
            .group("browser-resize-handle")
            .child(layout::resize_divider_line(
                "browser-resize-handle",
                "browser-resize-line",
                self.browser.resizing,
            ));
        resize = resize
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event, window, cx| {
                    view.begin_browser_resize(
                        resize::Divider::Outer,
                        event,
                        pane_width,
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
                            resize::Divider::Outer,
                            window,
                            entity.entity_id(),
                        );
                    },
                )
                .size_full(),
            );

        div()
            .track_focus(&self.browser.results_focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, window, cx| {
                    // Only explicit text entry keeps editing focus. Descendant text
                    // inputs stop this local bubble; no global pointer interception.
                    if view.browser.open {
                        view.browser.results_focus.focus(window);
                    } else {
                        view.focus.focus(window);
                    }
                    window.prevent_default();
                    cx.notify();
                }),
            )
            .flex()
            .min_h_0()
            .flex_shrink_0()
            .child(panel)
            .child(
                div()
                    .w(px(4.0))
                    .h_full()
                    .flex_shrink_0()
                    .min_h_0()
                    .child(resize),
            )
    }

    fn browser_row(
        &mut self,
        index: usize,
        entry: LibraryEntry,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let (disclosure, icon) = match entry.kind {
            EntryKind::Directory => match self.browser.tree.disclosure(&entry.path) {
                tree::Disclosure::Loading => (Some("…"), None),
                tree::Disclosure::Error => (Some("!"), None),
                tree::Disclosure::Expanded => (None, Some("icons/browser-disclosure/expanded.svg")),
                tree::Disclosure::Collapsed => {
                    (None, Some("icons/browser-disclosure/collapsed.svg"))
                }
            },
            EntryKind::WaveAudio => (None, Some("icons/browser-place/samples.svg")),
            EntryKind::Project => (None, Some("icons/browser-place/projects.svg")),
        };
        let path = entry.path.clone();
        let name = entry.name.clone();
        let drag_payload = if entry.kind == EntryKind::WaveAudio {
            self.state.as_ref().and_then(|state| {
                // Rendering a row reads only the existing metadata cache.
                let slot = self
                    .browser
                    .preview_cache
                    .get(&PreviewKey {
                        path: path.clone(),
                        sample_rate: state.project.sample_rate(),
                    })
                    .cloned()
                    .unwrap_or_default();
                self.audio_file_drag_payload(
                    path.clone(),
                    name.clone(),
                    Some(self.browser.scan_generation()),
                    slot.metadata,
                    slot.error,
                )
            })
        } else {
            None
        };
        let selected = self.browser.results_focused && self.browser.selected_index == index;
        let click_path = path.clone();
        let click_kind = entry.kind;
        let mut row = result_row(selected)
            .id(gpui::SharedString::from(tree::row_id(&path)))
            .debug_selector(|| format!("browser-entry-{index}"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    // Even a stale painted row leaves query editing; availability
                    // still gates every selection/audition/import action below.
                    view.browser.results_focus.focus(window);
                    window.prevent_default();
                    if view.browser_place_unavailable() {
                        cx.notify();
                        cx.stop_propagation();
                        return;
                    }
                    let Some(current) =
                        view.browser_history_index(&BrowserItemKey::File(click_path.clone()), cx)
                    else {
                        return;
                    };
                    view.checkpoint_browser_history(cx);
                    view.browser.selected_index = current;
                    view.browser.results_focused = true;
                    view.browser_selection_audition(
                        (click_kind == EntryKind::WaveAudio).then(|| click_path.clone()),
                        cx,
                    );
                    cx.notify();
                    if event.click_count >= 2 {
                        match click_kind {
                            EntryKind::Directory => view.toggle_browser_folder(
                                click_path.clone(),
                                event.modifiers.platform,
                                cx,
                            ),
                            EntryKind::WaveAudio => {
                                // Sample double-click remains selection/audition.
                                // Import is an explicit Enter or drag/drop action.
                                cx.stop_propagation();
                            }
                            EntryKind::Project => view.open_browser_project(click_path.clone(), cx),
                        }
                    }
                }),
            );
        if entry.kind == EntryKind::WaveAudio {
            let sampler_path = path.clone();
            row = row.on_mouse_down(
                MouseButton::Right,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    if view.browser_place_unavailable() {
                        return;
                    }
                    let Some(current) =
                        view.browser_history_index(&BrowserItemKey::File(sampler_path.clone()), cx)
                    else {
                        return;
                    };
                    view.browser.selected_index = current;
                    view.browser.results_focused = true;
                    view.open_first_sampler_menu(sampler_path.clone(), event.position, window, cx);
                }),
            );
        }
        if let Some(payload) = drag_payload {
            let hover_path = path.clone();
            let sample_rate = self
                .state
                .as_ref()
                .map_or(48_000, |state| state.project.sample_rate());
            let hover_entity = cx.entity();
            row = row.on_hover(move |hovered, _, cx| {
                if *hovered {
                    hover_entity.update(cx, |view, cx| {
                        view.request_browser_audio_preview(hover_path.clone(), sample_rate, cx);
                    });
                }
            });
            let drag_entity = cx.entity();
            row = row.on_drag(payload.clone(), move |payload, _, window, cx| {
                let position = window.mouse_position();
                payload.snap_modifier.set(window.modifiers().platform);
                payload.copy_modifier.set(window.modifiers().alt);
                cx.set_active_drag_cursor_style(
                    drag::copy_cursor(payload.copy_modifier.get()),
                    window,
                );
                payload
                    .last_pointer
                    .set(Some((f32::from(position.x), f32::from(position.y))));
                let (preview, lane_hovered) = drag_entity.update(cx, |view, cx| {
                    view.begin_browser_audio_drag(payload);
                    view.request_browser_audio_preview_for_drag(payload, cx);
                    view.browser_audio_drop_position(
                        payload,
                        f32::from(position.x),
                        f32::from(position.y),
                    )
                });
                payload.preview.set(preview);
                payload.lane_hovered.set(lane_hovered);
                drag_entity.update(cx, |_, cx| cx.notify());
                cx.new(|_| BrowserAudioDragGhost)
            });
            let move_entity = cx.entity();
            row = row.on_drag_move::<drag::BrowserAudioDrag>(move |event, window, cx| {
                let Some(payload) = event
                    .dragged_item()
                    .downcast_ref::<drag::BrowserAudioDrag>()
                else {
                    return;
                };
                let pointer = event.event.position;
                let pointer_xy = (f32::from(pointer.x), f32::from(pointer.y));
                payload.last_pointer.set(Some(pointer_xy));
                payload.snap_modifier.set(event.event.modifiers.platform);
                payload.copy_modifier.set(event.event.modifiers.alt);
                cx.set_active_drag_cursor_style(
                    drag::copy_cursor(payload.copy_modifier.get()),
                    window,
                );
                let (preview, lane_hovered) = move_entity.update(cx, |view, _| {
                    view.browser_audio_drop_position(payload, pointer_xy.0, pointer_xy.1)
                });
                payload.preview.set(preview);
                payload.lane_hovered.set(lane_hovered);
                move_entity.update(cx, |_, cx| cx.notify());
            });
            let release_entity = cx.entity();
            let release_payload = payload.clone();
            row = row.on_mouse_up(MouseButton::Left, move |_, window, cx| {
                cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
                release_entity.update(cx, |view, cx| {
                    view.clear_browser_audio_drag(&release_payload);
                    cx.notify();
                });
            });
        }
        if entry.kind == EntryKind::Directory && !self.browser_place_unavailable() {
            let payload = self.browser_folder_drag(path.clone(), name);
            row = row.on_drag(payload, |payload, _, _, cx| {
                cx.new(|_| folder_places::FolderGhost(payload.label.clone()))
            });
        }
        let depth = self.browser.tree.depth(&path);
        let mut control = div()
            .w(px(16.0))
            .flex_shrink_0()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .text_size(px(10.0))
            .line_height(px(ROW_HEIGHT))
            .text_color(rgb(selection_ink(selected, theme::PaletteRole::Muted)))
            .children(disclosure.map(theme::text))
            .children(icon.map(|path| {
                gpui::svg()
                    .path(path)
                    .size(px(14.0))
                    .text_color(rgb(selection_ink(selected, theme::PaletteRole::Muted)))
            }));
        if icon.is_some() && entry.kind != EntryKind::Directory {
            control = control.debug_selector(move || format!("browser-file-icon-{index}"));
        }
        if entry.kind == EntryKind::Directory {
            let folder = path.clone();
            control = control
                .debug_selector(move || format!("browser-disclosure-{index}"))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                        view.toggle_browser_folder(folder.clone(), event.modifiers.platform, cx);
                        view.browser.results_focus.focus(window);
                        window.prevent_default();
                        cx.stop_propagation();
                    }),
                );
        }
        let tooltip = self.browser.tree.error(&path).map_or_else(
            || path.to_string_lossy().into_owned(),
            |error| {
                format!(
                    "{} · {error}",
                    hex_session::service::text::verbatim(path.display())
                )
            },
        );
        row.tooltip(move |_, cx| layout::browser_label_tooltip(&tooltip, cx))
            .child(
                div()
                    .w(px((depth as f32 * 12.0).min(96.0)))
                    .max_w(relative(0.4))
                    .flex_shrink_0(),
            )
            .child(control)
            .child(
                result_label(entry.name, selected, theme::PaletteRole::Text)
                    .debug_selector(move || format!("browser-entry-label-{index}")),
            )
    }

    fn browser_device_row(
        &mut self,
        index: usize,
        class: hex_engine::NativeDeviceClass,
        entry: hex_engine::NativeDeviceCatalogEntry,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let class_label = match class {
            hex_engine::NativeDeviceClass::Instrument => "instrument",
            hex_engine::NativeDeviceClass::Effect => "effect",
        };
        let type_id = entry.type_id;
        let debug_type = type_id.as_str().unwrap_or("invalid").to_owned();
        let name = entry.name.to_lowercase();
        let payload = self.state.as_ref().map(|state| drag::NativeDeviceDrag {
            project_generation: self.project_generation,
            revision: state.revision,
            source: drag::NativeDeviceDragSource::Browser { class, type_id },
            destination: Rc::new(Cell::new(None)),
            return_destination: Rc::new(Cell::new(None)),
        });
        let label_id = format!("browser-device-label-{class_label}-{debug_type}");
        let selected = self.browser.results_focused && self.browser.selected_index == index;
        let row = result_row(selected)
            .id(gpui::SharedString::from(format!(
                "browser-device-{class_label}-{debug_type}"
            )))
            .debug_selector(move || format!("browser-device-{class_label}-{debug_type}"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    let Some(current) =
                        view.browser_history_index(&BrowserItemKey::Device(class, type_id), cx)
                    else {
                        return;
                    };
                    view.checkpoint_browser_history(cx);
                    view.browser.selected_index = current;
                    view.browser.results_focused = true;
                    view.browser.results_focus.focus(window);
                    window.prevent_default();
                    if event.click_count == 2 {
                        let request = view
                            .selected_track
                            .ok_or("select a track before adding a device")
                            .and_then(|track| {
                                view.browser_device_append_request(track, class, type_id, cx)
                            });
                        match request {
                            Ok(request) => {
                                if view.enqueue_device_barrier(request) {
                                    view.error = None;
                                    view.status = "adding device to track".into();
                                }
                            }
                            Err(reason) => view.error = Some(reason.into()),
                        }
                        cx.stop_propagation();
                    }
                    cx.notify();
                }),
            )
            .child(
                div()
                    .w(px(16.0))
                    .h(px(ROW_HEIGHT))
                    .text_size(px(10.0))
                    .line_height(px(ROW_HEIGHT))
                    .text_color(rgb(selection_ink(
                        selected,
                        theme::PaletteRole::SecondaryText,
                    )))
                    .child(theme::text(
                        if class == hex_engine::NativeDeviceClass::Instrument {
                            "⌘"
                        } else {
                            "◈"
                        },
                    )),
            )
            .child(
                result_label(name, selected, theme::PaletteRole::Text)
                    .debug_selector(move || label_id.clone()),
            );
        Self::with_browser_device_drag(row, payload, cx)
    }

    fn with_browser_device_drag(
        row: gpui::Stateful<Div>,
        payload: Option<drag::NativeDeviceDrag>,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        let Some(payload) = payload else {
            return row;
        };
        let drag_entity = cx.entity();
        let release_entity = drag_entity.clone();
        row.on_drag(payload.clone(), move |payload, _, window, cx| {
            cx.set_active_drag_cursor_style(gpui::CursorStyle::ClosedHand, window);
            drag_entity.update(cx, |view, cx| {
                view.begin_native_device_drag(payload);
                cx.notify();
            });
            cx.new(|_| BrowserAudioDragGhost)
        })
        .on_mouse_up(MouseButton::Left, move |_, window, cx| {
            cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
            release_entity.update(cx, |view, cx| {
                view.clear_native_device_drag(&payload);
                cx.notify();
            });
        })
    }

    fn browser_icon(
        &self,
        label: &'static str,
        tooltip: &'static str,
        action: impl Fn(&mut EditorView, &mut Window, &mut Context<EditorView>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let description = theme::text(tooltip);
        hex_widgets::action(theme::text(label), theme::action_style())
            .id(label)
            .debug_selector(move || format!("browser-control-{label}"))
            .w(px(22.0))
            .h(px(22.0))
            .text_xs()
            .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
            .tooltip(move |_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
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
    }

    fn browser_folder_button(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let description = theme::text("choose a folder");
        hex_widgets::action(theme::text("⌂"), theme::action_style())
            .id("browser-folder-picker")
            .w(px(22.0))
            .h(px(22.0))
            .text_xs()
            .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
            .tooltip(move |_, cx| {
                cx.new(|_| super::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    description: description.clone(),
                })
                .into()
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| view.open_browser_folder(cx)),
            )
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
                            if !view.browser.places.contains(&path) {
                                if view.browser.places.len() < PLACE_CAPACITY {
                                    view.browser.places.push(path.clone());
                                    view.queue_browser_places_save(cx);
                                } else {
                                    view.browser.places_status =
                                        Some("browser folder limit reached".into());
                                }
                            }
                            if view.browser.places.contains(&path)
                                && !view.browser_place_visible(&path)
                            {
                                view.set_browser_source_visible(
                                    BrowserSection::Place(path.clone()),
                                    true,
                                    cx,
                                );
                            }
                            view.refresh_sample_catalog(cx);
                            if view.browser.section != BrowserSection::Samples
                                && view.browser.places.contains(&path)
                                && view.browser_place_visible(&path)
                            {
                                view.select_browser_source(BrowserSection::Place(path), cx);
                            }
                        }
                    }
                    Ok(Err(error)) => {
                        view.error = Some(
                            hex_session::service::text::lowercase_outside_verbatim(
                                &error.to_string(),
                            )
                            .into(),
                        )
                    }
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_browser_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.browser_place_unavailable() {
            return;
        }
        self.choose_project(path, hex_session::DocumentAudioMode::Resident);
        cx.notify();
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "browser_ui_test.rs"]
mod ui_test;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "console_drop_ui_test.rs"]
mod console_drop_ui_test;

#[cfg(all(test, feature = "ui-test-support"))]
#[path = "browser/folder_places_ui_test.rs"]
mod folder_places_ui_test;
