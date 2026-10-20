//! One bounded sample transport row; cached peaks never decode during painting.
use super::*;
use gpui::{Bounds, fill, point, size};
use hex_session::CachedPeak;

/// Normalize hit geometry without trusting stale or zero-width bounds.
fn hit_fraction(x: f32, left: f32, width: f32) -> Option<f64> {
    if !x.is_finite() || !left.is_finite() || !width.is_finite() || width <= 0.0 {
        return None;
    }
    Some(((f64::from(x) - f64::from(left)) / f64::from(width)).clamp(0.0, 1.0))
}

/// Resolve against the actual decoded PCM, independently of cached display rate.
pub(super) fn frame_at_fraction(frames: usize, fraction: f64) -> usize {
    if !fraction.is_finite() || fraction <= 0.0 {
        return 0;
    }
    if fraction >= 1.0 {
        return frames;
    }
    ((frames as f64 * fraction).floor() as usize).min(frames)
}

fn combined_peak(channels: &[CachedPeak; 2]) -> CachedPeak {
    CachedPeak {
        min: channels[0].min.min(channels[1].min),
        max: channels[0].max.max(channels[1].max),
    }
}

fn mini_waveform(waveform: hex_session::SharedWaveform) -> Div {
    div().absolute().inset_0().child(
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let width = f32::from(bounds.size.width);
                let height = f32::from(bounds.size.height);
                if width <= 0.0 || height <= 0.0 || waveform.bins.is_empty() {
                    return;
                }
                let step = width / waveform.bins.len() as f32;
                let half = (height - 2.0).max(0.0) * 0.5;
                let color = rgb(theme::color(theme::PaletteRole::Muted));
                for (index, channels) in waveform.bins.iter().enumerate() {
                    let peak = combined_peak(channels);
                    let x = index as f32 * step;
                    let top = (half - peak.max.clamp(-1.0, 1.0) * half) + 1.0;
                    let bottom = (half - peak.min.clamp(-1.0, 1.0) * half) + 1.0;
                    let bar_width = step.max(1.0).min((width - x).max(0.0));
                    window.paint_quad(fill(
                        Bounds::new(
                            bounds.origin + point(px(x), px(top.min(height))),
                            size(
                                px(bar_width),
                                px((bottom - top).max(1.0).min((height - top).max(0.0))),
                            ),
                        ),
                        color,
                    ));
                }
            },
        )
        .absolute()
        .inset_0(),
    )
}

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
                    window.prevent_default();
                    cx.stop_propagation();
                    action(view, cx);
                    cx.notify();
                }),
            )
            .child(theme::text(label))
    }

    pub(super) fn browser_audition_row(
        &mut self,
        entry: Option<LibraryEntry>,
        cx: &mut Context<Self>,
    ) -> Div {
        let sample_rate = match self.state.as_ref() {
            Some(state) => match state.read_with(|snapshot| snapshot.sample_rate()) {
                Ok(rate) => rate,
                Err(_) => return div().child(theme::text("sample preview read unavailable")),
            },
            None => 48_000,
        };
        let fallback = entry.as_ref().map_or_else(
            || self.browser_selected_label(cx).to_owned(),
            |entry| entry.name.clone(),
        );
        let sample = entry.filter(|entry| entry.kind == EntryKind::WaveAudio);
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
            let rate = sample_rate;
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
                waveform = waveform.child(mini_waveform(metadata.waveform.clone()));
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
            .h(px(22.0))
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn waveform_hit_positions_map_to_bounded_decoded_frames() {
        for (x, expected) in [(-5.0, 0), (10.0, 0), (60.0, 50), (110.0, 100), (200.0, 100)] {
            assert_eq!(
                frame_at_fraction(100, hit_fraction(x, 10.0, 100.0).unwrap()),
                expected
            );
        }
        assert_eq!(
            frame_at_fraction(7, hit_fraction(0.25, 0.0, 0.5).unwrap()),
            3
        );
        assert_eq!(frame_at_fraction(0, 1.0), 0);
        assert_eq!(frame_at_fraction(100, f64::NAN), 0);
        for width in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(hit_fraction(1.0, 0.0, width).is_none());
        }
        assert!(hit_fraction(f32::NAN, 0.0, 10.0).is_none());
    }

    #[test]
    fn combined_waveform_keeps_opposite_phase_channel_peaks() {
        assert_eq!(
            combined_peak(&[
                CachedPeak { min: 0.8, max: 0.8 },
                CachedPeak {
                    min: -0.8,
                    max: -0.8
                },
            ]),
            CachedPeak {
                min: -0.8,
                max: 0.8
            }
        );
    }
}
