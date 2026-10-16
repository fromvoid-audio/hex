//! GPUI UI service; device output is supplied by the desktop composition root.
mod app;
mod build_info;
mod context_menu;
mod device_composition;
mod diagnostics;
pub use diagnostics::install_session_log;
mod editor;
mod layout;
mod theme;
use gpui::{App, AppContext, Bounds, KeyBinding, WindowBounds, WindowOptions, px, size};
use hex_audio::{AudioOutput, PreparedOutput};

/// Open the canonical project editor around a prepared native output stream.
pub fn open_editor(cx: &mut App, output: Result<PreparedOutput, String>) -> Result<(), String> {
    editor::open(cx, output)
}

/// Present a loading window while the injected initial output opens off-thread.
pub fn open_editor_pending(
    cx: &mut App,
    request: Result<hex_audio::RecoveryOpen, String>,
) -> Result<(), String> {
    editor::open_pending(cx, request)
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
