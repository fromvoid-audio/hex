//! One bounded sample transport row; cached peaks never decode during painting.
use super::audition_geometry::hit_fraction;
use super::*;
use gpui::{Bounds, CursorStyle, MouseMoveEvent, MouseUpEvent, fill, point, size};

const AUDITION_ROW_HEIGHT: f32 = 22.0;

impl EditorView {
    fn audition_control(
        &self,
        id: &'static str,
        label: &'static str,
        tooltip: &'static str,
        active: bool,
        action: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        div()
            .id(id)
            .debug_selector(move || id.to_owned())
            .w(px(22.0))
            .h(px(22.0))
            .flex_shrink_0()
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .whitespace_nowrap()
            .cursor_pointer()
            .text_color(rgb(theme::color(if active {
                theme::PaletteRole::Primary
            } else {
                theme::PaletteRole::Muted
            })))
            .when(active, |style| {
                style.bg(rgb(theme::color(theme::PaletteRole::Hover)))
            })
            .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Hover))))
            .tooltip(move |_, cx| super::layout::browser_label_tooltip(tooltip, cx))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, _, window, cx| {
                    view.browser.results_focus.focus(window);
                    window.prevent_default();
                    cx.stop_propagation();
                    action(view, cx);
                    cx.notify();
                }),
            )
            .child(theme::text(label))
    }

    fn audition_volume(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let entity = cx.entity();
        let owner = entity.entity_id();
        let handle_top = 2.0 + (1.0 - self.browser.audition_gain) * 16.0;
        div()
            .id("browser-audition-volume")
            .debug_selector(|| "browser-audition-volume".to_owned())
            .relative()
            .w(px(16.0))
            .h(px(AUDITION_ROW_HEIGHT))
            .flex_shrink_0()
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .hover(|style| style.bg(rgb(theme::color(theme::PaletteRole::Hover))))
            .cursor_pointer()
            .tooltip(|_, cx| super::layout::browser_label_tooltip("sample audition volume", cx))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(Self::begin_audition_volume_drag),
            )
            .child(
                div()
                    .absolute()
                    .left(px(7.0))
                    .top(px(2.0))
                    .w(px(2.0))
                    .h(px(18.0))
                    .bg(rgb(theme::color(theme::PaletteRole::Border))),
            )
            .child(
                div()
                    .absolute()
                    .left(px(3.0))
                    .top(px(handle_top))
                    .w(px(10.0))
                    .h(px(2.0))
                    .bg(rgb(theme::color(theme::PaletteRole::Primary))),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, cx| {
                        let view = entity.read(cx);
                        if view.browser.audition_gain_dragging
                            && view.browser.results_focus.is_focused(window)
                        {
                            window.renew_pointer_capture(owner, CursorStyle::ResizeRow);
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
    }

    fn begin_audition_volume_drag(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let down = f32::from(event.position.y);
        if !down.is_finite() || self.browser.preview_selected.is_none() {
            return;
        }
        let initial = self.browser.audition_gain;
        let owner = cx.entity().downgrade();
        let cancelled = owner.clone();
        if !window.capture_pointer(
            cx.entity_id(),
            MouseButton::Left,
            CursorStyle::ResizeRow,
            move |event, window, cx| {
                let released = event
                    .downcast_ref::<MouseUpEvent>()
                    .filter(|event| event.button == MouseButton::Left);
                let position = event
                    .downcast_ref::<MouseMoveEvent>()
                    .map(|event| event.position)
                    .or_else(|| released.map(|event| event.position));
                let Some(y) = position.map(|position| f32::from(position.y)) else {
                    return;
                };
                let _ = owner.update(cx, |view, cx| {
                    if !view.browser.audition_gain_dragging {
                        return;
                    }
                    if view.browser.open
                        && view.browser.preview_selected.is_some()
                        && view.browser.results_focus.is_focused(window)
                        && y.is_finite()
                    {
                        let gain = (initial + (down - y) / AUDITION_ROW_HEIGHT).clamp(0.0, 1.0);
                        view.browser.audition_gain = gain;
                        if let Some(audio) = &mut view.audio {
                            let _ = audio.control.preview_set_gain(gain);
                        }
                    }
                    if released.is_some() {
                        view.browser.audition_gain_dragging = false;
                    }
                    cx.notify();
                });
            },
            move |_, cx| {
                let _ = cancelled.update(cx, |view, cx| {
                    view.browser.audition_gain_dragging = false;
                    cx.notify();
                });
            },
            cx,
        ) {
            return;
        }
        self.browser.results_focus.focus(window);
        self.browser.audition_gain_dragging = true;
        cx.notify();
    }

    pub(super) fn browser_audition_row(
        &mut self,
        entry: Option<LibraryEntry>,
        cx: &mut Context<Self>,
    ) -> Div {
        let fallback = entry.as_ref().map_or_else(
            || self.browser_selected_label(cx).to_owned(),
            |entry| entry.name.clone(),
        );
        let sample = entry.filter(|entry| entry.kind == EntryKind::WaveAudio);
        let volume = sample.as_ref().map(|_| self.audition_volume(cx));
        let active = matches!(
            self.browser.playback_phase,
            Phase::Preparing | Phase::Playing
        );
        let preparing = self.browser.playback_phase == Phase::Preparing;
        let audition = self.audition_control(
            "browser-audition-auto",
            "a",
            if self.browser.auto_audition {
                "automatic sample audition on"
            } else {
                "automatic sample audition off"
            },
            self.browser.auto_audition,
            |view, _| view.browser.auto_audition = !view.browser.auto_audition,
            cx,
        );
        let path = sample.as_ref().map(|entry| entry.path.clone());
        let control = self.audition_control(
            "browser-audition-transport",
            if active { "■" } else { "▶" },
            if preparing {
                "cancel sample preparation"
            } else if active {
                "stop sample audition"
            } else {
                "play selected sample"
            },
            self.browser.playback_phase == Phase::Playing,
            move |view, cx| {
                if matches!(
                    view.browser.playback_phase,
                    Phase::Preparing | Phase::Playing
                ) {
                    view.cancel_browser_sample_playback();
                    view.status = "sample preview stopped".into();
                } else if let Some(path) = &path {
                    view.play_browser_sample(path.clone(), cx);
                }
            },
            cx,
        );
        let click_path = sample.as_ref().map(|entry| entry.path.clone());
        let bounds = Rc::clone(&self.browser.waveform_bounds);
        let measured_bounds = Rc::clone(&bounds);
        let fraction = if sample
            .as_ref()
            .is_some_and(|entry| self.browser.playback_path.as_ref() == Some(&entry.path))
        {
            self.browser.playback_fraction
        } else {
            0.0
        };
        let mut waveform = div()
            .id("browser-audition-waveform")
            .relative()
            .flex_1()
            .min_w_0()
            .h(px(18.0))
            .overflow_hidden()
            .debug_selector(|| "browser-audition-waveform".to_owned())
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    view.browser.results_focus.focus(window);
                    window.prevent_default();
                    cx.stop_propagation();
                    if let (Some(path), Some(bounds)) = (&click_path, bounds.get())
                        && let Some(fraction) = hit_fraction(
                            f32::from(event.position.x),
                            f32::from(bounds.left()),
                            f32::from(bounds.size.width),
                        )
                    {
                        view.play_browser_sample_from(path.clone(), fraction, cx);
                        cx.notify();
                    }
                }),
            );
        self.browser.preview_selected = None;
        if let Some(entry) = sample {
            let rate = self
                .state
                .as_ref()
                .map_or(48_000, |state| state.project.sample_rate());
            self.browser.preview_selected = Some(PreviewKey {
                path: entry.path.clone(),
                sample_rate: rate,
            });
            let slot = self.browser_preview_cell(entry.path.clone(), rate);
            if slot.metadata.borrow().is_none() && slot.error.borrow().is_none() {
                self.queue_browser_audio_preview(
                    PreviewKey {
                        path: entry.path,
                        sample_rate: rate,
                    },
                    slot.clone(),
                    cx,
                );
            }
            let metadata = slot.metadata.borrow().clone();
            if let Some(metadata) = metadata {
                waveform = waveform.child(super::super::overview_waveform::render(
                    super::super::overview_waveform::Source::Browser(Arc::clone(
                        &metadata.waveform,
                    )),
                    Rc::clone(&self.overview_waveform_cache),
                    None,
                ));
            } else {
                let label = if slot.error.borrow().is_some() {
                    "unavailable"
                } else {
                    "preparing…"
                };
                waveform = waveform.child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_xs()
                        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                        .child(theme::text(label)),
                );
            }
            let label = entry.name;
            waveform =
                waveform.tooltip(move |_, cx| super::layout::browser_label_tooltip(&label, cx));
        } else {
            waveform = waveform.child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text(fallback)),
            );
        }
        waveform = waveform.child(
            canvas(
                move |bounds, _, _| {
                    measured_bounds.set(Some(bounds));
                },
                move |bounds, _, window, _| {
                    let width = f32::from(bounds.size.width);
                    if width > 0.0 {
                        let x = (fraction.clamp(0.0, 1.0) * width)
                            .floor()
                            .min((width - 1.0).max(0.0));
                        window.paint_quad(fill(
                            Bounds::new(
                                bounds.origin + point(px(x), px(0.0)),
                                size(px(1.0), bounds.size.height),
                            ),
                            rgb(theme::color(theme::PaletteRole::Primary)),
                        ));
                    }
                },
            )
            .absolute()
            .inset_0(),
        );
        div()
            .h(px(AUDITION_ROW_HEIGHT))
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .overflow_hidden()
            .flex()
            .items_center()
            .gap(px(2.0))
            .debug_selector(|| "browser-audition-row".to_owned())
            .child(audition)
            .child(control)
            .child(waveform)
            .children(volume)
    }
}
