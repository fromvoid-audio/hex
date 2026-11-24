//! Compact, bounded left-side media and project browser.
//!
//! This UI owner composes existing file, native-device and plugin projections;
//! library/catalog owners retain matching and within-kind ordering. Nonempty All
//! search places native devices then plugins before files. Other sources and empty
//! browsing retain file-first order. Row ranges are shared by virtualization,
//! stable-key selection/history, activation, folder navigation and audition.
mod audition_geometry;
mod random_rank;

mod audition;
mod folder_places;
mod history;
pub(in crate::editor) mod insertion;
mod place_icons;
mod plugins;
mod random;
mod resize;
mod samples;
mod scrollbar;
mod tree;

mod navigation;
mod places_io;
mod playback;
mod preview_cache;
mod view;
use super::{EditorMode, EditorView, browser_input, drag};
use crate::theme;
use gpui::{
    App, Context, Div, MouseButton, MouseDownEvent, PathPromptOptions, Window, canvas, div,
    prelude::*, px, relative, rgb, uniform_list,
};
use hex_audio::sample_preview::Phase;
use hex_library::{DirectoryListing, EntryKind, LibraryEntry, ScanError, scan_directory};
use hex_preferences::{BrowserBuiltin, BrowserPlaces};
use hex_session::service::{
    AudioPreview, BROWSER_PREVIEW_WINDOW_FRAMES, PreviewFailure, PreviewSources,
    PreviewWindowSource, Request,
};
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
    source: BrowserPlaybackSource,
    sample_rate: u32,
}

#[derive(Clone)]
enum BrowserPlaybackSource {
    File(PathBuf),
    ProjectSample(BrowserItemKey, hex_engine::Audio),
}

enum WindowDelivery {
    Accepted,
    Full(hex_engine::Audio),
    Stale,
    Failed(String),
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

pub(super) fn preferences_path_for_home(home: Option<&std::path::Path>) -> Option<PathBuf> {
    home.filter(|path| !path.as_os_str().is_empty())
        .map(hex_preferences::browser_places_path)
}

#[cfg(not(test))]
pub(super) fn default_preferences_path() -> Option<PathBuf> {
    let home = super::user_home_directory();
    preferences_path_for_home(home.as_deref())
}

impl EditorView {
    fn clamped_browser_width(&self, requested: f32, window_width: f32) -> f32 {
        geometry::clamped_panel_width(
            requested,
            window_width,
            super::workspace_view::PANEL_GUTTER + self.timeline_leading_buffer_width(),
        )
    }
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
    ProjectSamples,
    Place(PathBuf),
    Collection(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum BrowserItemKey {
    File(PathBuf),
    Plugin(PathBuf, Option<[u8; 16]>),
    Device(hex_engine::NativeDeviceClass, hex_engine::DeviceTypeId),
    ProjectSample(u64, u64, hex_project::AssetId),
}

#[derive(Clone)]
struct ProjectSample {
    asset: hex_project::AssetId,
    name: String,
}

struct ProjectSampleCache {
    generation: u64,
    revision: u64,
    query: String,
    rows: Arc<Vec<ProjectSample>>,
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
    preferences_path: Option<PathBuf>,
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
    project_samples: RefCell<Option<ProjectSampleCache>>,
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
    project_sample_selection: Option<BrowserItemKey>,
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
    playback_project_sample: Option<BrowserItemKey>,
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
        preferences_path: Option<PathBuf>,
        #[cfg(test)] test_home: Option<PathBuf>,
    ) -> Self {
        #[cfg(not(test))]
        let home = super::user_home_directory()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
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
            project_samples: RefCell::new(None),
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
            project_sample_selection: None,
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
            playback_project_sample: None,
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
            .as_deref()
            .and_then(|path| path.parent())
            .filter(|path| !path.as_os_str().is_empty())
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn test_preferences_path(&self) -> &std::path::Path {
        self.preferences_path
            .as_deref()
            .unwrap_or(std::path::Path::new(""))
    }

    /// Redirects later saves; an in-flight save keeps its captured path.
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn set_test_preferences_path(&mut self, path: PathBuf) {
        self.preferences_path = Some(path);
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
        self.project_sample_selection = None;
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
            header_height: super::clip_name::CLIP_NAME_HEIGHT,
            header_gradient: None,
            notch: super::clip_boundary::Notch::new(notched),
        },
        true,
        move || format!("{selector}-trailing"),
    )
    .opacity(0.9)
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

#[cfg(all(test, not(feature = "ui-test-support")))]
mod unit_tests {
    use gpui::{point, px};

    #[test]
    fn physical_wheel_delta_normalizes_pixels_and_lines_without_mutating_raw_delta() {
        let cases = [
            (
                gpui::ScrollDelta::Pixels(point(px(12.0), px(24.0))),
                false,
                (12.0, 24.0),
            ),
            (
                gpui::ScrollDelta::Pixels(point(px(12.0), px(24.0))),
                true,
                (-12.0, -24.0),
            ),
            (
                gpui::ScrollDelta::Lines(point(2.0, -3.0)),
                false,
                (2.0, -3.0),
            ),
            (
                gpui::ScrollDelta::Lines(point(2.0, -3.0)),
                true,
                (-2.0, 3.0),
            ),
        ];
        for (raw, inverted, (expected_x, expected_y)) in cases {
            let event = gpui::ScrollWheelEvent {
                delta: raw,
                direction_inverted_from_device: inverted,
                ..Default::default()
            };
            match event.physical_delta() {
                gpui::ScrollDelta::Pixels(delta) => {
                    assert_eq!(f32::from(delta.x), expected_x);
                    assert_eq!(f32::from(delta.y), expected_y);
                }
                gpui::ScrollDelta::Lines(delta) => {
                    assert_eq!(delta.x, expected_x);
                    assert_eq!(delta.y, expected_y);
                }
            }
            match (raw, event.delta) {
                (gpui::ScrollDelta::Pixels(before), gpui::ScrollDelta::Pixels(after)) => {
                    assert_eq!(before, after);
                }
                (gpui::ScrollDelta::Lines(before), gpui::ScrollDelta::Lines(after)) => {
                    assert_eq!(before.x, after.x);
                    assert_eq!(before.y, after.y);
                }
                _ => panic!("raw scroll delta variant changed"),
            }
        }
    }
}

#[cfg(test)]
mod path_tests {
    use super::preferences_path_for_home;
    use std::path::Path;

    #[test]
    fn unset_or_empty_home_returns_none_instead_of_system_library() {
        assert_eq!(preferences_path_for_home(None), None);
        assert_eq!(preferences_path_for_home(Some(Path::new(""))), None);
        let resolved = preferences_path_for_home(Some(Path::new("/Users/tester"))).unwrap();
        assert!(resolved.starts_with("/Users/tester"));
        assert!(!resolved.starts_with("/Library/"));
    }
}
