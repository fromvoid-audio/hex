use crate::diagnostics::ActionTraceContext as _;
use crate::theme;
use gpui::{Context, FocusHandle, Window, actions, div, prelude::*, px, rgb};
use hex_audio::AudioOutput;
use hex_engine::live::{Command, Mode, TRACKS};
use std::{sync::atomic::Ordering::Relaxed, time::Duration};

actions!(hex, [TogglePlay, Stop, Quit]);

pub(super) const TRACK_COLORS: [u32; TRACKS] = [0xe95843, 0x779c9a, 0x8588b0, 0xb58a72];
pub(super) const HEADER_HEIGHT: f32 = 38.0;
pub(super) const MIXER_HEIGHT: f32 = 150.0;
pub(super) const INSPECTOR_HEIGHT: f32 = 72.0;

pub(crate) struct Daw {
    pub(super) audio: Option<AudioOutput>,
    pub(super) status: String,
    pub(super) mode: Mode,
    pub(super) bpm: f64,
    pub(super) gains: [f32; TRACKS],
    pub(super) muted: [bool; TRACKS],
    pub(super) selected: usize,
    pub(super) browser_open: bool,
    focus: FocusHandle,
}

impl Daw {
    pub(crate) fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        output: Result<AudioOutput, String>,
    ) -> Self {
        let (audio, status) = match output {
            Ok(audio) => (Some(audio), "demo set ready · space to play".into()),
            Err(error) => (None, format!("audio unavailable: {error}").to_lowercase()),
        };
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        let focus = cx.focus_handle();
        focus.focus(window);
        Self {
            audio,
            status,
            mode: Mode::Arrangement,
            bpm: 120.0,
            gains: [1.0; TRACKS],
            muted: [false; TRACKS],
            selected: 0,
            browser_open: true,
            focus,
        }
    }

    pub(super) fn send(&mut self, command: Command) -> bool {
        let Some(audio) = &mut self.audio else {
            return false;
        };
        if audio.telemetry.device_fault.load(Relaxed) {
            self.status = "audio device failed. stop and restart hex to reconnect.".into();
            return false;
        }
        if audio.commands.push(command).is_err() {
            self.status = "control queue full. try again; stop remains available.".into();
            return false;
        }
        self.status = "demo playback · edits apply at the next audio block".into();
        true
    }

    pub(super) fn playing(&self) -> bool {
        self.audio
            .as_ref()
            .is_some_and(|audio| audio.telemetry.playing.load(Relaxed))
    }

    pub(super) fn toggle(&mut self) {
        self.send(if self.playing() {
            Command::Pause
        } else {
            Command::Play
        });
    }

    pub(super) fn stop(&mut self) {
        if let Some(audio) = &self.audio {
            audio.stop();
        }
        self.status = "stopped · returned to start".into();
    }
}

impl Render for Daw {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let beat = self
            .audio
            .as_ref()
            .map_or(0.0, |audio| audio.telemetry.beat());
        let active_scene =
            self.audio
                .as_ref()
                .map_or(0, |audio| audio.telemetry.scene.load(Relaxed)) as usize;
        let fault = self
            .audio
            .as_ref()
            .is_some_and(|audio| audio.telemetry.device_fault.load(Relaxed));
        let main = if self.mode == Mode::Arrangement {
            self.arrangement(cx, beat)
        } else {
            self.session(cx, active_scene)
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(theme::color(theme::PaletteRole::Background)))
            .text_color(rgb(theme::color(theme::PaletteRole::Text)))
            .font_family(theme::FONT_FAMILY)
            .track_focus(&self.focus)
            .key_context("daw")
            .on_action(cx.trace(|this, _: &TogglePlay, _, cx| {
                this.toggle();
                cx.notify();
            }))
            .on_action(cx.trace(|this, _: &Stop, _, cx| {
                this.stop();
                cx.notify();
            }))
            .on_action(|_: &Quit, _, cx| {
                let _scope = crate::diagnostics::action_scope(cx, "hex::Quit");
                cx.quit();
            })
            .child(self.transport(cx, beat))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.browser(cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .child(
                                div()
                                    .h(px(24.0))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .px_2()
                                    .border_b_1()
                                    .border_color(rgb(theme::color(theme::PaletteRole::Border)))
                                    .bg(rgb(theme::color(theme::PaletteRole::Surface)))
                                    .text_xs()
                                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                                    .child(theme::text(if self.mode == Mode::Arrangement {
                                        "arrangement · 4 tracks · 2 bars"
                                    } else {
                                        "session · 4 tracks · 2 scenes"
                                    }))
                                    .child(theme::text("4/4 · loop 8 beats")),
                            )
                            .child(div().flex_1().min_h_0().flex().flex_col().child(main))
                            .child(self.mixer(cx))
                            .child(self.inspector()),
                    ),
            )
            .child(self.status_bar(fault))
    }
}
