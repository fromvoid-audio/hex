//! Desktop composition only: create backend, open the GPUI shell, run lifecycle.
use gpui::{App, Application};

fn main() {
    Application::new().run(|cx: &mut App| {
        let demo = std::env::args_os()
            .skip(1)
            .any(|argument| argument == "--demo");
        let result = if demo {
            hex_ui::open(cx, hex_audio::AudioOutput::open())
        } else {
            hex_ui::open_editor_pending(
                cx,
                hex_audio::request_initial_open().map_err(|error| error.to_string()),
            )
        };
        if let Err(error) = result {
            eprintln!("{}", error.to_lowercase());
            cx.quit();
            return;
        }
        cx.activate(true);
    });
}
