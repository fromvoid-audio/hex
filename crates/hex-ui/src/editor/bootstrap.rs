//! An immediately visible window with a weak task waiting for injected output.
use super::{EditorView, MAX_RESIDENT_FRAMES, Quit, editor_window_options, install_close_guard};
use crate::theme::{self, Palette};
use gpui::{App, AppContext, Context, FocusHandle, Window, div, prelude::*, px, rgb};
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
            .bg(rgb(Palette::BACKGROUND))
            .text_color(rgb(Palette::TEXT))
            .font_family(theme::FONT_FAMILY)
            .track_focus(&self.focus)
            .key_context("hex_bootstrap")
            .on_action(cx.listener(|_, _: &Quit, _, cx| cx.quit()))
            .child(
                div()
                    .h(px(38.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .pl(px(theme::TITLEBAR_LEADING_INSET))
                    .pr_2()
                    .border_b_1()
                    .border_color(rgb(Palette::BORDER))
                    .bg(rgb(Palette::SURFACE))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(Palette::CORAL))
                            .child(theme::text("hex")),
                    )
                    .child(div().text_xs().child(theme::text(&self.message))),
            )
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
