//! Selected-clip fade controls and bounded control-side admission.
use super::{
    EditorView, ViewState,
    waveform::{ClipTooltip, button},
};
use crate::theme;
use gpui::{Context, Div, div, prelude::*, px, rgb};
use hex_project::{ClipFade, Command};
use hex_session::service::Request;

const FADE_STEP_MILLISECONDS: u64 = 5;

pub(super) fn fade_step_frames(sample_rate: u32) -> u64 {
    ((u64::from(sample_rate) * FADE_STEP_MILLISECONDS + 500) / 1_000).max(1)
}

fn fade_ms(frames: u64, sample_rate: u32) -> u64 {
    if sample_rate == 0 {
        return 0;
    }
    ((u128::from(frames) * 1_000 + u128::from(sample_rate) / 2) / u128::from(sample_rate)) as u64
}

fn fade_edit_admitted(
    opening_project: bool,
    close_prompt_open: bool,
    worker_requests_pending: usize,
    pending_requests: usize,
) -> bool {
    !opening_project && !close_prompt_open && worker_requests_pending == 0 && pending_requests == 0
}

fn fade_is_inherited(fade: ClipFade, duration: u64) -> bool {
    fade.offset() != 0 || fade.span() != duration
}

fn compact_fade_time(milliseconds: u64) -> String {
    if milliseconds < 1_000 {
        format!("{milliseconds}ms")
    } else if milliseconds < 10_000 {
        let tenths = milliseconds.saturating_add(50) / 100;
        format!("{}.{:01}s", tenths / 10, tenths % 10)
    } else if milliseconds < 1_000_000 {
        format!("{}s", milliseconds / 1_000)
    } else {
        let tenths = milliseconds.saturating_add(50_000) / 100_000;
        format!("{}.{:01}ks", tenths / 10, tenths % 10)
    }
}

fn fade_edit_block_reason(
    opening_project: bool,
    close_prompt_open: bool,
    worker_requests_pending: usize,
    pending_requests: usize,
) -> Option<&'static str> {
    if opening_project {
        Some("fade edit waits for project loading")
    } else if close_prompt_open {
        Some("fade edit waits for the close prompt")
    } else if worker_requests_pending != 0 || pending_requests != 0 {
        Some("fade edit waits for pending project work")
    } else {
        None
    }
}

fn adjusted_fades(
    current: u64,
    opposite: u64,
    duration: u64,
    step: u64,
    increase: bool,
) -> Option<(u64, u64)> {
    if duration == 0 {
        return None;
    }
    let next = if increase {
        current.saturating_add(step).min(duration)
    } else {
        current.saturating_sub(step).min(duration)
    };
    Some((next, opposite.min(duration)))
}

impl EditorView {
    pub(super) fn selected_fade_controls(
        &self,
        state: &ViewState,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let (_, clip_id, _, duration, ..) = self.detail_audio_clip_data()?;
        let clip = state.project.clip(clip_id)?;
        let fade = clip.retained_fade();
        let retained = clip.audio_edit().is_some();
        let inherited = retained || fade_is_inherited(fade, duration);
        let admitted = fade_edit_admitted(
            self.opening_project,
            self.close_prompt_open,
            self.worker_requests_pending,
            self.pending_requests.len(),
        );
        let fade_in_ms = fade_ms(fade.fade_in(), state.project.sample_rate());
        let fade_out_ms = fade_ms(fade.fade_out(), state.project.sample_rate());
        let mut controls =
            div()
                .flex()
                .items_center()
                .gap_1()
                .opacity(if admitted { 1.0 } else { 0.45 });
        controls = controls
            .child(
                button(
                    "−",
                    "shorten fade in by 5 ms",
                    ("fade-in-down", clip_id.value()),
                )
                .on_click(cx.listener(|view, _, _, cx| {
                    view.adjust_selected_fade(true, false);
                    cx.notify();
                })),
            )
            .child(
                button(
                    "+",
                    "lengthen fade in by 5 ms",
                    ("fade-in-up", clip_id.value()),
                )
                .debug_selector(|| "fade-in-up".into())
                .on_click(cx.listener(|view, _, _, cx| {
                    view.adjust_selected_fade(true, true);
                    cx.notify();
                })),
            )
            .child(
                div()
                    .id(("fade-in-readout", clip_id.value()))
                    .w(px(56.0))
                    .min_w_0()
                    .flex_shrink_0()
                    .truncate()
                    .text_color(rgb(theme::color(theme::PaletteRole::Text)))
                    .tooltip(move |_, cx| {
                        cx.new(|_| ClipTooltip {
                            kind: crate::tooltips::Kind::Identification,
                            name: format!(
                                "{}fade in · {fade_in_ms} ms · {} reference frames",
                                if retained { "retained " } else { "" },
                                fade.fade_in()
                            ),
                        })
                        .into()
                    })
                    .child(theme::text(format!("↗ {}", compact_fade_time(fade_in_ms)))),
            )
            .child(
                button(
                    "−",
                    "shorten fade out by 5 ms",
                    ("fade-out-down", clip_id.value()),
                )
                .on_click(cx.listener(|view, _, _, cx| {
                    view.adjust_selected_fade(false, false);
                    cx.notify();
                })),
            )
            .child(
                button(
                    "+",
                    "lengthen fade out by 5 ms",
                    ("fade-out-up", clip_id.value()),
                )
                .on_click(cx.listener(|view, _, _, cx| {
                    view.adjust_selected_fade(false, true);
                    cx.notify();
                })),
            )
            .child(
                div()
                    .id(("fade-out-readout", clip_id.value()))
                    .w(px(56.0))
                    .min_w_0()
                    .flex_shrink_0()
                    .truncate()
                    .text_color(rgb(theme::color(theme::PaletteRole::Text)))
                    .tooltip(move |_, cx| {
                        cx.new(|_| ClipTooltip {
                            kind: crate::tooltips::Kind::Identification,
                            name: format!(
                                "{}fade out · {fade_out_ms} ms · {} reference frames",
                                if retained { "retained " } else { "" },
                                fade.fade_out()
                            ),
                        })
                        .into()
                    })
                    .child(theme::text(format!("↘ {}", compact_fade_time(fade_out_ms)))),
            )
            .child(
                button("×", "reset clip fades", ("fade-reset", clip_id.value()))
                    .debug_selector(|| "fade-reset".into())
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.reset_selected_fades();
                        cx.notify();
                    })),
            );
        if inherited {
            controls = controls.child(
                div()
                    .id(("fade-inherited", clip_id.value()))
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .debug_selector(||"retained-fade-disclosure".into())
                    .tooltip(|_, cx| {
                        cx.new(|_| ClipTooltip { kind: crate::tooltips::Kind::control("retained fades"),
                            name: "retained fades show authored reference-frame durations; the curve follows its frozen musical projection; editing reanchors and clamps both fades to this clip".into(),
                        })
                        .into()
                    })
                    .child(theme::text(if retained{"retained"}else{"split"})),
            );
        }
        Some(controls)
    }

    fn adjust_selected_fade(&mut self, fade_in: bool, increase: bool) {
        if !fade_edit_admitted(
            self.opening_project,
            self.close_prompt_open,
            self.worker_requests_pending,
            self.pending_requests.len(),
        ) {
            self.report_fade_edit_blocked();
            return;
        }
        let Some((_, clip_id, _, duration, ..)) = self.detail_audio_clip_data() else {
            return;
        };
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let Some(clip) = state.project.clip(clip_id) else {
            return;
        };
        let fade = clip.retained_fade();
        let step = fade_step_frames(state.project.sample_rate());
        let (current, opposite) = if fade_in {
            (fade.fade_in(), fade.fade_out())
        } else {
            (fade.fade_out(), fade.fade_in())
        };
        let Some((next, opposite)) = adjusted_fades(current, opposite, duration, step, increase)
        else {
            return;
        };
        let (fade_in, fade_out) = if fade_in {
            (next, opposite)
        } else {
            (opposite, next)
        };
        if fade.offset() == 0
            && fade.span() == duration
            && fade.fade_in() == fade_in
            && fade.fade_out() == fade_out
        {
            return;
        }
        if fade.has_shape() {
            let Ok(local) = fade.reanchor(duration) else {
                return;
            };
            let Ok(next) = ClipFade::new(
                duration,
                0,
                fade_in.min(duration - local.in_start()),
                fade_out.min(duration - local.out_end_offset()),
            )
            .and_then(|next| {
                next.with_shape(
                    local.in_start(),
                    local.out_end_offset(),
                    local.in_curve(),
                    local.out_curve(),
                )?
                .with_curve_positions(local.in_position(), local.out_position())
            }) else {
                return;
            };
            if next != fade {
                self.enqueue(Request::Edit(Command::SetClipFadeEnvelope {
                    clip: clip_id,
                    fade: next,
                }));
            }
            return;
        }
        self.enqueue(Request::Edit(Command::SetClipFades {
            clip: clip_id,
            fade_in,
            fade_out,
        }));
    }

    fn reset_selected_fades(&mut self) {
        if !fade_edit_admitted(
            self.opening_project,
            self.close_prompt_open,
            self.worker_requests_pending,
            self.pending_requests.len(),
        ) {
            self.report_fade_edit_blocked();
            return;
        }
        let Some((_, clip_id, _, duration, ..)) = self.detail_audio_clip_data() else {
            return;
        };
        let Some(state) = self.state.as_ref() else {
            return;
        };
        if state.project.clip(clip_id).is_none_or(|clip| {
            let fade = clip.retained_fade();
            fade.span() == duration
                && fade.offset() == 0
                && fade.fade_in() == 0
                && fade.fade_out() == 0
                && !fade.has_shape()
        }) {
            return;
        }
        self.enqueue(Request::Edit(Command::SetClipFades {
            clip: clip_id,
            fade_in: 0,
            fade_out: 0,
        }));
    }

    fn report_fade_edit_blocked(&mut self) {
        if let Some(reason) = fade_edit_block_reason(
            self.opening_project,
            self.close_prompt_open,
            self.worker_requests_pending,
            self.pending_requests.len(),
        ) {
            self.status = reason.into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        adjusted_fades, compact_fade_time, fade_edit_admitted, fade_edit_block_reason,
        fade_is_inherited, fade_ms, fade_step_frames,
    };

    #[test]
    fn fade_step_and_readouts_use_project_sample_rate() {
        assert_eq!(fade_step_frames(48_000), 240);
        assert_eq!(fade_step_frames(44_100), 221);
        assert_eq!(fade_step_frames(1), 1);
        assert_eq!(fade_ms(240, 48_000), 5);
        assert_eq!(fade_ms(u64::MAX, 0), 0);
    }

    #[test]
    fn fade_edit_rejects_open_close_and_queued_or_inflight_work() {
        assert!(fade_edit_admitted(false, false, 0, 0));
        assert!(!fade_edit_admitted(true, false, 0, 0));
        assert!(!fade_edit_admitted(false, true, 0, 0));
        assert!(!fade_edit_admitted(false, false, 1, 0));
        assert!(!fade_edit_admitted(false, false, 0, 1));
        assert_eq!(
            fade_edit_block_reason(false, false, 1, 0),
            Some("fade edit waits for pending project work")
        );
    }

    #[test]
    fn readouts_stay_compact_with_full_precision_available_elsewhere() {
        assert_eq!(compact_fade_time(100), "100ms");
        assert_eq!(compact_fade_time(1_250), "1.3s");
        assert_eq!(compact_fade_time(12_000), "12s");
        assert_eq!(compact_fade_time(1_250_000), "1.3ks");
    }

    #[test]
    fn fade_adjustments_clamp_both_lengths_to_the_local_clip() {
        assert_eq!(
            adjusted_fades(900, 800, 1_000, 240, true),
            Some((1_000, 800))
        );
        assert_eq!(
            adjusted_fades(1_500, 1_200, 1_000, 240, false),
            Some((1_000, 1_000))
        );
        assert_eq!(adjusted_fades(0, 200, 1_000, 240, false), Some((0, 200)));
    }

    #[test]
    fn split_envelope_domain_is_marked_in_the_readout() {
        let inherited = hex_project::ClipFade::new(1_000, 400, 600, 500).unwrap();
        let reanchored = hex_project::ClipFade::new(600, 0, 600, 500).unwrap();
        assert!(fade_is_inherited(inherited, 600));
        assert!(!fade_is_inherited(reanchored, 600));
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod musical_tests;
