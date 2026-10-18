//! GPUI UI service; device output is supplied by the desktop composition root.
mod mts;
pub use mts::install_mts_publisher;
mod app;
mod build_info;
mod context_menu;
mod controller_input;
mod device_composition;
mod diagnostics;
pub use controller_input::install_controller_input;
pub use diagnostics::install_session_log;
mod editor;
mod editor_backend;
pub use editor_backend::{EditorAudio, EditorAudioEvent, EditorBackend, EditorPublicationState};
mod layout;
pub mod output_meter;
mod theme;
use gpui::{App, AppContext, Bounds, KeyBinding, WindowBounds, WindowOptions, px, size};
use hex_audio::AudioOutput;

/// Open the editor with a worker-owned native audio backend.
pub fn open_editor_backend(cx: &mut App, backend: EditorBackend) -> Result<(), String> {
    editor::open_backend(cx, backend)
}

/// Compose the GPUI window around the backend created by the desktop entry point.
pub fn open(cx: &mut App, output: Result<AudioOutput, String>) -> Result<(), String> {
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
    editor::time_selection_profile::open(cx, directory, settings, result)
}
