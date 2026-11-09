//! Noninteractive native GPUI quit-handshake regression fixture.
//! Run through `crates/hex-ui/tests/native_quit_test.py` on macOS.

#[cfg(target_os = "macos")]
fn main() {
    use std::{cell::Cell, rc::Rc, time::Duration};

    let cancel_first = std::env::args().any(|arg| arg == "cancel-first");
    gpui::Application::new().run(move |cx: &mut gpui::App| {
        let requests = Rc::new(Cell::new(0));
        cx.on_should_quit(move |cx| {
            let count = requests.get() + 1;
            requests.set(count);
            eprintln!("quit request {count}");
            cx.reply_to_quit(!cancel_first || count > 1);
        });
        if cancel_first {
            let timer = cx.background_executor().timer(Duration::from_millis(100));
            cx.spawn(async move |cx| {
                timer.await;
                if let Err(error) = cx.update(|cx| cx.quit()) {
                    eprintln!("retry failed: {error}");
                }
            })
            .detach();
        }
        cx.quit();
    });
}

#[cfg(not(target_os = "macos"))]
fn main() {}
