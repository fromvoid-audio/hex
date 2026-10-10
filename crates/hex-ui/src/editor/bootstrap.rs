//! An immediately visible window with a weak task waiting for injected output.
use super::{
    EditorView, MAX_RESIDENT_FRAMES, Quit, editor_window_options, header, install_close_guard,
};
use crate::theme;
use gpui::{App, AppContext, Context, FocusHandle, Window, div, prelude::*, rgb};
use hex_audio::{PreparedOutput, RecoveryOpen};
use hex_session::service::Service;
use std::time::Duration;

pub(super) fn open(cx: &mut App, request: Result<RecoveryOpen, String>) -> Result<(), String> {
    cx.open_window(editor_window_options(cx), move |window, cx| {
        cx.new(|cx| Bootstrap::new(window, cx, request))
    })
    .map_err(|error| format!("could not open hex: {error}"))?;
    Ok(())
}

struct Bootstrap {
    request: Option<Result<RecoveryOpen, String>>,
    message: String,
    focus: FocusHandle,
}

impl Bootstrap {
    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        request: Result<RecoveryOpen, String>,
    ) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window);
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let output = match this.update(cx, |view, _| view.poll()) {
                    Ok(Some(output)) => output,
                    Ok(None) => continue,
                    Err(_) => break,
                };
                // The handle does not retain a closed window. If update fails,
                // the captured output drops through off-thread stream retirement.
                let _ = cx.update_window(window_handle, move |_, window, cx| {
                    let rate = startup_rate(output.as_ref().ok().map(|audio| audio.sample_rate));
                    let service = match Service::new(rate, 32, MAX_RESIDENT_FRAMES) {
                        Ok(service) => service,
                        Err(error) => {
                            let _ = this.update(cx, |view, cx| {
                                view.message = format!("could not start editor: {error}");
                                cx.notify();
                            });
                            return;
                        }
                    };
                    let editor = window.replace_root(cx, move |window, cx| {
                        EditorView::new(window, cx, service, output)
                    });
                    install_close_guard(window, cx, editor.downgrade());
                });
                break;
            }
        })
        .detach();
        Self {
            request: Some(request),
            message: "preparing audio output".into(),
            focus,
        }
    }

    fn poll(&mut self) -> Option<Result<PreparedOutput, String>> {
        let ready = match self.request.as_mut()? {
            Ok(handle) => match handle.try_take() {
                Ok(ready) => ready,
                Err(error) => Some(Err(error.to_string())),
            },
            Err(error) => Some(Err(error.clone())),
        };
        if ready.is_some() {
            self.request = None;
        }
        ready
    }
}

impl Render for Bootstrap {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(theme::color(theme::PaletteRole::Background)))
            .text_color(rgb(theme::color(theme::PaletteRole::Text)))
            .font_family(theme::FONT_FAMILY)
            .track_focus(&self.focus)
            .key_context("hex_bootstrap")
            .on_action(cx.listener(|_, _: &Quit, _, cx| cx.quit()))
            .child(header::title_bar("hex", |control| {
                Box::new(move |event, window, _| match control {
                    header::TitlebarControl::Close => window.remove_window(),
                    header::TitlebarControl::Minimize => window.minimize_window(),
                    header::TitlebarControl::Fullscreen if event.modifiers().alt => {
                        window.zoom_window()
                    }
                    header::TitlebarControl::Fullscreen => window.toggle_fullscreen(),
                })
            }))
            .child(header::loading_toolbar(&self.message))
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod ui_tests {
    use super::Bootstrap;
    use gpui::{
        AppContext, Empty, MouseButton, MouseDownEvent, MouseUpEvent, TestAppContext,
        VisualTestContext, point, px, size,
    };

    #[gpui::test]
    fn bootstrap_stacks_shared_title_and_toolbar_rows(cx: &mut TestAppContext) {
        if !cfg!(target_os = "macos") {
            return;
        }
        let bootstrap = cx.add_window(|_, _| Empty);
        let handle = *bootstrap;
        cx.update_window(handle, |_, window, cx| {
            window.replace_root(cx, |_, cx| Bootstrap {
                request: None,
                message: "preparing audio output".into(),
                focus: cx.focus_handle(),
            });
        })
        .expect("bootstrap window");

        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(size(px(880.0), px(540.0)));
        visual.run_until_parked();
        let title = visual
            .debug_bounds("project-title-bar")
            .expect("startup title row");
        let toolbar = visual
            .debug_bounds("editor-toolbar")
            .expect("startup toolbar row");
        let close = visual
            .debug_bounds("window-control-close")
            .expect("startup close control");
        let fullscreen = visual
            .debug_bounds("window-control-fullscreen")
            .expect("startup fullscreen control");
        let title_bottom = f32::from(title.origin.y + title.size.height);
        assert!((f32::from(toolbar.origin.y) - title_bottom).abs() < 1.0);
        assert!(f32::from(close.origin.x + close.size.width) < f32::from(fullscreen.origin.x));

        let position = point(
            close.origin.x + close.size.width / 2.0,
            close.origin.y + close.size.height / 2.0,
        );
        visual.simulate_event(MouseDownEvent {
            position,
            modifiers: Default::default(),
            button: MouseButton::Left,
            click_count: 1,
            first_mouse: false,
        });
        visual.simulate_event(MouseUpEvent {
            position,
            modifiers: Default::default(),
            button: MouseButton::Left,
            click_count: 1,
        });
        assert!(cx.windows().is_empty());
    }
}

fn startup_rate(opened_rate: Option<u32>) -> u32 {
    opened_rate.unwrap_or(48_000)
}

#[cfg(test)]
mod tests {
    use super::startup_rate;
    #[test]
    fn negotiated_rate_is_authoritative_and_offline_fallback_requires_open_failure() {
        assert_eq!(startup_rate(Some(44_100)), 44_100);
        assert_eq!(startup_rate(Some(96_000)), 96_000);
        assert_eq!(startup_rate(None), 48_000);
        // Canonical validation must reject unsupported rates rather than silently
        // relabel already-opened output at the fallback rate.
        assert!(hex_project::Project::new(startup_rate(Some(384_000))).is_err());
    }
}
