//! Native, disposable layout catalog. No audio, project or preferences I/O.
// The gallery samples shared compositions; the live windowed controller also
// uses helpers which this disposable fixture does not mount.
#[path = "../src/device_composition.rs"]
#[allow(dead_code)]
mod device_composition;
#[path = "layout_review/gallery.rs"]
mod gallery;
#[path = "../src/theme.rs"]
#[allow(dead_code)]
mod theme;

#[path = "layout_review/sandbox.rs"]
mod sandbox;

use gpui::{App, AppContext, Application, Bounds, WindowBounds, WindowOptions, px, size};

fn main() {
    Application::new().run(|cx: &mut App| {
        hex_widgets::bind_numeric_control_keys(cx);
        hex_widgets::bind_response_graph_keys(cx);
        if let Err(error) = theme::load_fonts(cx) {
            eprintln!("{error}");
            cx.quit();
            return;
        }
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let result = cx.open_window(
            WindowOptions {
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some(theme::text("hex · native widget sandbox")),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1040.0), px(720.0)),
                    cx,
                ))),
                window_min_size: Some(size(px(760.0), px(480.0))),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(sandbox::Sandbox::new);
                view.update(cx, |view, cx| view.focus_first(window, cx));
                view
            },
        );
        if let Err(error) = result {
            eprintln!("could not open layout review: {error}");
            cx.quit();
        }
        cx.activate(true);
    });
}
