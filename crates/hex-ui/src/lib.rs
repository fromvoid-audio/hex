//! # Purpose
//! Own native GPUI display, focus/selection and validated editor intent; desktop injects device
//! output.
//!
//! # Dependencies
//! Uses hex-session/project/engine/audio contracts, hex-widgets, GPUI and the bounded library,
//! preferences, tuning, MIDI, MTS, diagnostics and plugin-catalog services.
//! fastrand supplies transient browser result ranks on the UI thread.
//! Test-only storage access verifies complete control-side EQ upgrade round trips.
//!
//! # Does not own
//! Does not own a second editable project, construct DSP or access files/audio devices from
//! rendering. Render app-owned text through theme::text with bundled Ioskeley Mono; preserve
//! original user data and path casing.
pub mod assets;
mod update;
pub use update::{
    UpdateAction, UpdateApproval, UpdateClient, UpdateCommand, UpdateReady, UpdateStatus,
    UpdateWorker, channel as update_channel, install_update_client,
};
mod mts;
pub use mts::install_mts_publisher;
mod app;
mod build_info;
mod context_menu;
mod controller_input;
mod device_composition;
mod diagnostics;
pub use controller_input::install_controller_input;
pub use diagnostics::{install_action_diagnostics, install_audio_diagnostics, install_session_log};
mod editor;
mod editor_backend;
#[cfg(feature = "ui-test-support")]
mod test_wait;
pub use editor_backend::{EditorAudio, EditorAudioEvent, EditorBackend, EditorPublicationState};
mod layout;
mod log_viewer;
mod section_appearance;
mod section_hit;
mod section_input;
mod section_reorder;
mod section_resize;
mod section_selection;
mod section_surface;
mod startup;
pub use startup::open_or_report;
pub mod output_meter;
mod theme;
mod tooltips;
use gpui::{App, AppContext, Bounds, KeyBinding, WindowBounds, WindowOptions, px, size};
use hex_audio::AudioOutput;

/// Open the editor with a worker-owned native audio backend.
/// The host must install [`assets::HexAssets`] with `Application::with_assets`.
pub fn open_editor_backend(cx: &mut App, backend: EditorBackend) -> Result<(), String> {
    assets::validate(cx)?;
    editor::open_backend(cx, backend)
}

/// Compose the GPUI window around the backend created by the desktop entry point.
/// The host must install [`assets::HexAssets`] with `Application::with_assets`.
pub fn open(cx: &mut App, output: Result<AudioOutput, String>) -> Result<(), String> {
    assets::validate(cx)?;
    theme::load_fonts(cx)?;
    cx.bind_keys([
        KeyBinding::new("space", app::TogglePlay, Some("daw")),
        KeyBinding::new("escape", app::Stop, Some("daw")),
        KeyBinding::new("cmd-q", app::Quit, None),
    ]);
    cx.on_window_closed(|cx| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
    let bounds = Bounds::centered(None, size(px(1180.0), px(780.0)), cx);
    cx.open_window(
        WindowOptions {
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("hex".into()),
                appears_transparent: true,
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(960.0), px(600.0))),
            ..Default::default()
        },
        move |window, cx| cx.new(|cx| app::Daw::new(window, cx, output)),
    )
    .map_err(|error| format!("could not open hex: {error}"))?;
    Ok(())
}

/// Open a disposable offline native timing window with settings under the fixture directory.
/// The caller publishes the bounded final report before requesting application quit.
pub fn open_time_selection_profile(
    cx: &mut App,
    directory: std::path::PathBuf,
    settings: std::path::PathBuf,
    result: std::sync::mpsc::SyncSender<Result<String, String>>,
) -> Result<(), String> {
    assets::validate(cx)?;
    editor::time_selection_profile::open(cx, directory, settings, result)
}

/// Headless-only owning fixture entry. All settings/discovery stay beneath the
/// caller-owned temporary root; it opens the actual view with the supplied backend.
#[cfg(feature = "ui-test-support")]
pub fn open_sampler_load_test_backend(
    cx: &mut gpui::TestAppContext,
    backend: EditorBackend,
    root: std::path::PathBuf,
    directory: std::path::PathBuf,
) -> Result<gpui::AnyWindowHandle, String> {
    editor::sampler_load_fixture(cx, backend, root, directory)
}

/// Exercise the real headless MIDI receipt-pressure and editor custody path.
#[cfg(feature = "ui-test-support")]
pub fn exercise_midi_receipt_pressure(
    cx: &mut gpui::TestAppContext,
    root: std::path::PathBuf,
    reservation: hex_session::SourcePlanReservation,
    output: hex_audio::WorkerPreparedOutput,
    render: impl FnMut(&mut [[f32; 2]], &mut [f32]) -> Result<(), hex_engine::Invalid>,
) {
    editor::midi_pressure_fixture(cx, root, reservation, output, render)
}

/// Read-only browser insertion evidence. Neither the view nor this query owns
/// a native endpoint, and a pending request alone grants no applied source.
#[cfg(feature = "ui-test-support")]
pub struct PluginEffectTestView {
    pub state: Option<std::sync::Arc<hex_session::service::ViewState>>,
    pub pending: bool,
    pub queued_requests: usize,
    /// Source-bound VST3 parameter controls keyed by device and parameter ID.
    pub parameter_controls: Vec<(u64, u32, gpui::EntityId)>,
    /// A controller parameter enumeration/page query is awaiting its terminal.
    pub parameter_query_pending: bool,
    /// (device, parameter, displayed value, independent follow control, arranger lane).
    pub parameter_touches: Vec<(u64, u32, f64, Option<gpui::EntityId>, bool)>,
    /// Normalized plugin intent held by the existing keyed gesture owner.
    pub parameter_gesture: Option<f64>,
    /// Weak request identity only; it retains no request or native owner.
    pub operation: Option<std::sync::Weak<hex_session::service::PluginEffectControl>>,
    pub error: Option<String>,
    pub selected_owner: Option<hex_session::service::PluginEffectDestination>,
    pub status: String,
    pub applied: Option<hex_audio::prepared::AppliedAudioSource>,
    /// Original callback status, copied for native graph recovery tests.
    pub transport: Option<(u64, bool)>,
}

#[cfg(feature = "ui-test-support")]
pub fn plugin_effect_test_view(
    window: gpui::AnyWindowHandle,
    cx: &gpui::App,
) -> Result<PluginEffectTestView, String> {
    editor::plugin_effect_fixture_view(window, cx)
}

/// Seed metadata for the original native-effect integration fixture beneath
/// its owned temporary root. This does not scan, prepare or acknowledge a source.
#[cfg(feature = "ui-test-support")]
pub fn write_plugin_effect_test_catalog(
    root: &std::path::Path,
    selection: &hex_session::service::PluginEffectSelection,
    subcategories: &str,
    factory_flags: u32,
) -> Result<(), String> {
    editor::plugin_effect_fixture_catalog(root, selection, subcategories, factory_flags)
}

/// Read-only headless source metadata and peak-only table; no native owner.
#[cfg(feature = "ui-test-support")]
pub struct SamplerSourceTestView {
    pub device: hex_project::DeviceId,
    pub asset: hex_project::AssetId,
    pub generation: u64,
    pub revision: u64,
    pub source_frames: u64,
    pub playback_frames: u64,
    pub original_source: Option<hex_project::OriginalSourceFacts>,
    pub overviews: hex_session::OriginalSourceOverviews,
}
#[cfg(feature = "ui-test-support")]
pub fn sampler_source_test_view(
    window: gpui::AnyWindowHandle,
    track: hex_project::TrackId,
    cx: &gpui::App,
) -> Result<Option<SamplerSourceTestView>, String> {
    editor::sampler_source_fixture_view(window, track, cx)
}

/// Read-only headless fixture diagnostics; no runtime/source/input owner escape.
#[cfg(feature = "ui-test-support")]
pub fn sampler_keyboard_test_status(
    window: gpui::AnyWindowHandle,
    cx: &gpui::App,
) -> Result<String, String> {
    editor::sampler_keyboard_fixture_status(window, cx)
}

/// Read-only readiness for the next headless editing/playback gesture, including stopped restore.
#[cfg(feature = "ui-test-support")]
pub fn sampler_midi_test_ready(
    window: gpui::AnyWindowHandle,
    cx: &gpui::App,
) -> Result<bool, String> {
    editor::sampler_midi_fixture_ready(window, cx)
}

/// Compile-time source provenance; guarded builds include the frozen tree digest.
pub fn diagnostic_build_identity() -> &'static str {
    concat!(
        env!("HEX_DISPLAY_VERSION"),
        " source=",
        env!("HEX_DIAGNOSTIC_SOURCE")
    )
}
