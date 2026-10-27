//! Thin typed bindings for Arrangement musical audio editing.
use super::EditorView;
use crate::theme;
use gpui::{Context, Div, div, prelude::*, px, rgb};
use hex_project::{BeatTime, BeatsPreserve, BeatsTail, ClipId, WarpMode};
mod actions;
pub(crate) mod sample_bpm;
use actions::{Action, command, mode_name, modes};
#[cfg(test)]
use hex_project::Command;
const PAGE: usize = 8;
#[derive(Clone, Copy)]
struct Capture {
    clip: ClipId,
    revision: u64,
    generation: u64,
}
pub(super) fn warp_command(
    view: &EditorView,
    clip: ClipId,
) -> Result<hex_project::Command, hex_project::Invalid> {
    let state = view
        .state
        .as_ref()
        .ok_or(hex_project::Invalid("project unavailable"))?;
    if view.selected_clip_ids().len() > 1 {
        actions::selection_command(&state.project, clip, Action::Warp)
    } else {
        command(&state.project, clip, Action::Warp)
    }
}
/// A numeric pitch draft preserves the former relative selected-clip semantics.
/// Project/session retain canonical validation and audio preparation; this UI
/// constructs their existing commands without owning a separate pitch model.
pub(super) fn pitch_command(
    project: &hex_project::Project,
    clip: ClipId,
    value: f64,
    multiple: bool,
) -> Result<hex_project::Command, hex_project::Invalid> {
    if !value.is_finite() || value.fract() != 0. || !(-4800.0..=4800.0).contains(&value) {
        return Err(hex_project::Invalid(
            "pitch must be whole cents between -4800 and 4800",
        ));
    }
    let source = project
        .clip(clip)
        .ok_or(hex_project::Invalid("audio clip no longer exists"))?;
    let current = source.audio_edit().map_or(0, |e| e.domain.pitch_cents);
    let delta = (value as i16)
        .checked_sub(current)
        .ok_or(hex_project::Invalid(
            "pitch change is outside the supported range",
        ))?;
    if multiple {
        actions::selection_command(project, clip, Action::Pitch(delta))
    } else {
        command(project, clip, Action::Pitch(delta))
    }
}
/// Selected-clip stretch controls stay above the scrollable source fields.
pub(super) fn warp_header(
    view: &mut EditorView,
    clip_id: ClipId,
    cx: &mut Context<EditorView>,
) -> Div {
    let mut controls = div().flex().flex_col().gap(px(2.0)).flex_shrink_0();
    let Some(state) = view.state.as_ref() else {
        return controls;
    };
    let Some(clip) = state.project.clip(clip_id) else {
        return controls;
    };
    let capture = Capture {
        clip: clip_id,
        revision: state.revision,
        generation: view.project_generation,
    };
    let domain = clip.audio_edit().map(|edit| edit.domain.as_ref());
    let active = domain.is_some_and(|domain| domain.warp_enabled);
    let mode = domain
        .and_then(|domain| domain.warp.as_ref())
        .map_or(modes()[0], |warp| warp.mode);
    controls = controls.child(sample_bpm::field(view, clip_id, cx));
    if active {
        if let WarpMode::Beats {
            preserve,
            tail,
            decay,
        } = mode
        {
            controls = controls.child(beats_controls(capture, preserve, tail, decay, false, cx));
        } else {
            controls = controls.child(musical_control_label(&format!(
                "algorithm · {}",
                mode_name(mode)
            )));
        }
        // Direct choices expose only algorithms backed by the existing worker.
        for (row_index, choices) in modes().chunks(2).enumerate() {
            let mut row = div().flex().gap(px(2.0));
            for (column, choice) in choices.iter().enumerate() {
                let index = (row_index * 2 + column) as u8;
                let selected = std::mem::discriminant(choice) == std::mem::discriminant(&mode);
                row = row.child(
                    button(
                        capture,
                        mode_name(*choice),
                        Action::SelectMode(index),
                        false,
                        cx,
                    )
                    .flex_1()
                    .border_1()
                    .border_color(rgb(theme::color(if selected {
                        theme::PaletteRole::Text
                    } else {
                        theme::PaletteRole::Border
                    })))
                    .debug_selector(move || format!("audio-clip-warp-mode-{index}")),
                );
            }
            controls = controls.child(row);
        }
    }
    controls
}
pub(super) fn view(view: &EditorView, clip_id: ClipId, cx: &mut Context<EditorView>) -> Div {
    let mut controls = div().flex().flex_col().gap(px(2.0));
    let Some(state) = view.state.as_ref() else {
        return controls;
    };
    let Some(clip) = state.project.clip(clip_id) else {
        return controls;
    };
    let capture = Capture {
        clip: clip_id,
        revision: state.revision,
        generation: view.project_generation,
    };
    let edit = clip.audio_edit();
    let domain = edit.map(|e| e.domain.as_ref());
    let active = domain.is_some_and(|d| d.warp_enabled);
    let mode = domain
        .and_then(|d| d.warp.as_ref())
        .map_or(modes()[0], |w| w.mode);
    controls = controls.child(musical_control_label("musical audio"));
    controls = controls.child(button(
        capture,
        if edit.is_some_and(|e| e.live_musical_start().is_some()) {
            "anchor · beats"
        } else {
            "anchor · frames"
        },
        Action::Anchor,
        false,
        cx,
    ));
    if active {
        match mode {
            WarpMode::RePitch => {
                controls =
                    controls.child(musical_control_label("transpose stored; tempo sets pitch"))
            }
            WarpMode::Beats { .. } => {}
            WarpMode::Tones { grain_ms } => {
                controls = controls.child(musical_control_row(
                    capture,
                    format!("grain {grain_ms} ms"),
                    Action::Grain(-5),
                    Action::Grain(5),
                    false,
                    cx,
                ))
            }
            WarpMode::Texture {
                grain_ms,
                fluctuation,
                seed,
            } => {
                controls = controls.child(musical_control_row(
                    capture,
                    format!("grain {grain_ms} ms"),
                    Action::Grain(-5),
                    Action::Grain(5),
                    false,
                    cx,
                ));
                controls = controls.child(musical_control_row(
                    capture,
                    format!("jitter {fluctuation}%"),
                    Action::Fluctuation(-10),
                    Action::Fluctuation(10),
                    false,
                    cx,
                ));
                controls = controls.child(button(
                    capture,
                    &format!("seed {seed} · next"),
                    Action::Seed,
                    false,
                    cx,
                ));
            }
            WarpMode::Complex => {}
            WarpMode::ComplexPro { formants, envelope } => {
                controls = controls.child(musical_control_row(
                    capture,
                    format!("formants {formants}%"),
                    Action::Formants(-10),
                    Action::Formants(10),
                    false,
                    cx,
                ));
                controls = controls.child(musical_control_row(
                    capture,
                    format!("envelope {envelope}"),
                    Action::Resolution(-32),
                    Action::Resolution(32),
                    false,
                    cx,
                ));
            }
        }
    }
    let point_count = domain.map_or(0, |d| {
        d.source_gain.len().max(if active {
            d.warp.as_ref().map_or(0, |w| w.markers.len())
        } else {
            0
        })
    });
    let page = clamped_page(view.audio_point_page, point_count);
    let first = page.saturating_mul(PAGE);
    controls = controls.child(musical_control_row(
        capture,
        format!("points · page {}", page + 1),
        Action::Page(-1),
        Action::Page(1),
        false,
        cx,
    ));
    if active && let Some(warp) = domain.and_then(|d| d.warp.as_ref()) {
        controls = controls.child(button(
            capture,
            "add warp marker",
            Action::MarkerAdd,
            false,
            cx,
        ));
        for (index, marker) in warp.markers.iter().enumerate().skip(first).take(PAGE) {
            controls = controls.child(musical_control_label(&format!(
                "marker {index} · source {}",
                marker.source_frame
            )));
            if index > 0 && index + 1 < warp.markers.len() {
                controls = controls.child(musical_control_row(
                    capture,
                    "source frame".into(),
                    Action::MarkerFrame(index, -1),
                    Action::MarkerFrame(index, 1),
                    false,
                    cx,
                ));
            }
            if index > 0 {
                controls = controls.child(musical_control_row(
                    capture,
                    format!(
                        "beat {:.4}",
                        marker.beat.raw() as f64 / BeatTime::ONE.raw() as f64
                    ),
                    Action::MarkerBeat(index, -1),
                    Action::MarkerBeat(index, 1),
                    false,
                    cx,
                ));
            }
            if index > 0 && index + 1 < warp.markers.len() {
                controls = controls.child(button(
                    capture,
                    "remove marker",
                    Action::MarkerRemove(index),
                    false,
                    cx,
                ));
            }
        }
    }
    controls = controls.child(musical_control_label("source gain envelope"));
    controls = controls.child(button(
        capture,
        "add source gain point",
        Action::EnvelopeAdd,
        false,
        cx,
    ));
    if let Some(domain) = domain {
        for (index, point) in domain.source_gain.iter().enumerate().skip(first).take(PAGE) {
            controls = controls.child(musical_control_row(
                capture,
                format!("src {}", point.frame),
                Action::EnvelopeFrame(index, -1),
                Action::EnvelopeFrame(index, 1),
                false,
                cx,
            ));
            controls = controls.child(musical_control_row(
                capture,
                format!("gain {:.2}×", f64::from(point.gain_q16) / 65_536.0),
                Action::EnvelopeGain(index, -1),
                Action::EnvelopeGain(index, 1),
                false,
                cx,
            ));
            controls = controls.child(button(
                capture,
                "remove gain point",
                Action::EnvelopeRemove(index),
                false,
                cx,
            ));
        }
    }
    controls
}
/// The existing canonical Beats bindings live beside the visible mode picker,
/// above source/marker fields; no second state or DSP path.
fn beats_controls(
    capture: Capture,
    preserve: BeatsPreserve,
    selected: BeatsTail,
    envelope: u8,
    disabled: bool,
    cx: &mut Context<EditorView>,
) -> Div {
    let preserve = match preserve {
        BeatsPreserve::Transients => "transients".into(),
        BeatsPreserve::Division(beat) => format!(
            "{:.3} beats",
            beat.raw() as f64 / BeatTime::ONE.raw() as f64
        ),
    };
    let mut choices = div().flex().gap(px(2.0));
    for (tail, label, selector) in [
        (BeatsTail::Off, "stop", "audio-clip-beats-stop"),
        (BeatsTail::Forward, "forward", "audio-clip-beats-forward"),
        (
            BeatsTail::Alternating,
            "back / forth",
            "audio-clip-beats-alternating",
        ),
    ] {
        choices = choices.child(
            button(capture, label, Action::SelectTail(tail), disabled, cx)
                .flex_grow()
                .flex_shrink_0()
                .border_1()
                .border_color(rgb(theme::color(if tail == selected {
                    theme::PaletteRole::Text
                } else {
                    theme::PaletteRole::Border
                })))
                .debug_selector(move || selector.into()),
        );
    }
    div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .flex_shrink_0()
        .child(
            button(
                capture,
                &format!("preserve · {preserve}"),
                Action::Preserve,
                disabled,
                cx,
            )
            .debug_selector(|| "audio-clip-beats-preserve".into()),
        )
        .child(choices)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(2.0))
                .child(
                    button(capture, "−", Action::Decay(-10), disabled, cx)
                        .debug_selector(|| "audio-clip-beats-envelope-less".into()),
                )
                .child(musical_control_label(&format!("envelope {envelope}")).flex_1())
                .child(
                    button(capture, "+", Action::Decay(10), disabled, cx)
                        .debug_selector(|| "audio-clip-beats-envelope-more".into()),
                )
                .debug_selector(|| "audio-clip-beats-envelope".into()),
        )
}

fn clamped_page(page: usize, count: usize) -> usize {
    page.min(count.saturating_sub(1) / PAGE)
}

fn musical_control_label(value: &str) -> Div {
    div()
        .min_h(px(18.0))
        .text_xs()
        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
        .child(theme::text(value))
}
fn musical_control_row(
    capture: Capture,
    value: String,
    minus: Action,
    plus: Action,
    disabled: bool,
    cx: &mut Context<EditorView>,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(2.0))
        .child(button(capture, "−", minus, disabled, cx))
        .child(musical_control_label(&value).flex_1())
        .child(button(capture, "+", plus, disabled, cx))
}
// Busy admission is checked at input time; only semantic unavailability dims.
fn button(
    capture: Capture,
    value: &str,
    action: Action,
    disabled: bool,
    cx: &mut Context<EditorView>,
) -> gpui::Stateful<Div> {
    let surface = if matches!(action, Action::Warp) {
        hex_widgets::toggle_action(theme::text(value), theme::action_style())
    } else {
        hex_widgets::action(theme::text(value), theme::action_style())
    };
    surface
        .id(gpui::ElementId::Name(
            format!("musical-{}-{}", capture.clip.value(), action_key(action)).into(),
        ))
        .min_h(px(20.0))
        .px(px(3.0))
        .text_xs()
        .opacity(if disabled { 0.4 } else { 1.0 })
        .on_click(cx.listener(move |view, _, _, cx| {
            if disabled
                || view.edit_pending()
                || view.project_generation != capture.generation
                || view.detail_clip_id() != Some(capture.clip)
            {
                return;
            }
            let Some(state) = view
                .state
                .as_ref()
                .filter(|state| state.revision == capture.revision)
            else {
                return;
            };
            if let Action::Page(direction) = action {
                let count = state
                    .project
                    .clip(capture.clip)
                    .and_then(|clip| clip.audio_edit())
                    .map_or(0, |edit| {
                        edit.domain
                            .source_gain
                            .len()
                            .max(if edit.domain.warp_enabled {
                                edit.domain
                                    .warp
                                    .as_ref()
                                    .map_or(0, |warp| warp.markers.len())
                            } else {
                                0
                            })
                    });
                let current = clamped_page(view.audio_point_page, count);
                view.audio_point_page = clamped_page(
                    if direction < 0 {
                        current.saturating_sub(1)
                    } else {
                        current.saturating_add(1)
                    },
                    count,
                );
                cx.notify();
                return;
            }
            let intent = if view.selected_clip_ids().len() > 1 {
                actions::selection_command(&state.project, capture.clip, action)
            } else {
                command(&state.project, capture.clip, action)
            };
            match intent {
                Ok(command) => {
                    if matches!(
                        action,
                        Action::MarkerAdd
                            | Action::MarkerFrame(..)
                            | Action::MarkerBeat(..)
                            | Action::MarkerRemove(_)
                            | Action::EnvelopeAdd
                            | Action::EnvelopeFrame(..)
                            | Action::EnvelopeGain(..)
                            | Action::EnvelopeRemove(_)
                    ) {
                        view.enqueue_focused_source_edit(command);
                    } else {
                        view.enqueue(hex_session::service::Request::Edit(command));
                    }
                }
                Err(error) => view.error = Some(error.0.into()),
            };
            cx.notify();
        }))
}
fn action_key(action: Action) -> String {
    match action {
        Action::SelectMode(index) => format!("mode-{index}"),
        Action::SelectTail(tail) => format!(
            "tail-{}",
            match tail {
                BeatsTail::Off => 0,
                BeatsTail::Forward => 1,
                BeatsTail::Alternating => 2,
            }
        ),
        Action::MarkerFrame(i, d) => format!("marker-frame-{i}-{d}"),
        Action::MarkerBeat(i, d) => format!("marker-beat-{i}-{d}"),
        Action::MarkerRemove(i) => format!("marker-remove-{i}"),
        Action::EnvelopeFrame(i, d) => format!("env-frame-{i}-{d}"),
        Action::EnvelopeGain(i, d) => format!("env-gain-{i}-{d}"),
        Action::EnvelopeRemove(i) => format!("env-remove-{i}"),
        _ => format!(
            "{}",
            match action {
                Action::Pitch(v) =>
                    if v < 0 {
                        0
                    } else {
                        1
                    },
                Action::Anchor => 2,
                Action::Warp => 3,
                Action::Grain(v) =>
                    if v < 0 {
                        6
                    } else {
                        7
                    },
                Action::Fluctuation(v) =>
                    if v < 0 {
                        8
                    } else {
                        9
                    },
                Action::Seed => 10,
                Action::Preserve => 11,
                Action::Decay(v) =>
                    if v < 0 {
                        13
                    } else {
                        14
                    },
                Action::Formants(v) =>
                    if v < 0 {
                        15
                    } else {
                        16
                    },
                Action::Resolution(v) =>
                    if v < 0 {
                        17
                    } else {
                        18
                    },
                Action::MarkerAdd => 19,
                Action::EnvelopeAdd => 20,
                Action::Page(v) =>
                    if v < 0 {
                        21
                    } else {
                        22
                    },
                _ => 23,
            }
        ),
    }
}
#[cfg(test)]
mod tests;
#[cfg(all(test, feature = "ui-test-support"))]
mod ui_tests;
