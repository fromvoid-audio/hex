//! Native, disposable layout catalog. No audio, project or preferences I/O.
#[path = "layout_review/gallery.rs"]
mod gallery;
#[path = "../src/theme.rs"]
#[allow(dead_code)]
mod theme;

use gpui::{App, AppContext, Application, Bounds, WindowBounds, WindowOptions, px, size};

fn main() {
    Application::new().run(|cx: &mut App| {
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
                    title: Some(theme::text("hex · layout review")),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1040.0), px(720.0)),
                    cx,
                ))),
                window_min_size: Some(size(px(760.0), px(660.0))),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(gallery::Gallery::new);
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
