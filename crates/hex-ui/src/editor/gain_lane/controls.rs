use super::*;
use crate::theme;
use gpui::{Div, div, prelude::*, px};
use hex_widgets::{
    NumericControlConfig, NumericControlEvent, NumericControlKind, NumericControlPresentation,
    NumericEventPhase, NumericScale, NumericSpec,
};

/// Gain is a shared numeric control; exact beats move through the existing time viewport/grid.
pub(super) fn numeric(view: &mut EditorView, cx: &mut Context<EditorView>) -> Div {
    let Some(point) = view.gain_point() else {
        return div();
    };
    let Some(track) = view.gain_lane.track else {
        return div();
    };
    let generation = view.project_generation;
    let revision = view.state.as_ref().map_or(0, |state| state.revision);
    let key = (track, point.beat, generation, revision);
    let slot = &mut view.gain_lane.gain;
    if slot.as_ref().is_some_and(|binding| binding.key != key) {
        *slot = None;
    }
    if slot.is_none() {
        let value = f64::from(point.gain) * 100.0;
        let config = NumericSpec::new(0.0, 200.0, value, NumericScale::Linear, 2, Some("%"))
            .map(|spec| {
                spec.with_codec(
                    |value| format!("{value}%"),
                    |text| text.trim().trim_end_matches('%').trim().parse::<f64>().ok(),
                )
            })
            .and_then(|spec| {
                NumericControlConfig::new(
                    "gain-point-value",
                    theme::text("gain"),
                    NumericControlKind::HorizontalSlider,
                    spec,
                    value,
                    theme::control_style(),
                )
            });
        let config = match config {
            Ok(config) => config.with_presentation(NumericControlPresentation::InlineValue),
            Err(reason) => {
                view.gain_lane.error = Some(reason.to_string());
                return div();
            }
        };
        let entity = cx.new(|cx| NumericControl::from_config(cx, config));
        let edit_state = entity.read(cx).edit_state();
        let subscription =
            cx.subscribe(&entity, move |view, _, event: &NumericControlEvent, cx| {
                if view.project_generation != generation
                    || view
                        .state
                        .as_ref()
                        .is_none_or(|state| state.revision != revision)
                    || view.gain_lane.track != Some(track)
                {
                    view.gain_lane.draft = None;
                    view.gain_lane.error = Some("gain draft refused · project changed".into());
                    cx.notify();
                    return;
                }
                match event.phase {
                    NumericEventPhase::Begin => {
                        if view.gain_lane.selected == Some(point.beat) {
                            view.begin_gain_draft(Some(point.beat), point);
                        }
                    }
                    NumericEventPhase::Change | NumericEventPhase::Commit => {
                        if let Some(draft) = view.gain_lane.draft.as_mut() {
                            draft.point.gain = (event.value / 100.0) as f32;
                        }
                        if event.phase == NumericEventPhase::Commit {
                            view.finish_gain_draft();
                        }
                    }
                    NumericEventPhase::Cancel => {
                        view.gain_lane.draft = None;
                        view.gain_lane.error = None;
                    }
                }
                cx.notify();
            });
        *slot = Some(NumericBinding {
            entity,
            edit_state,
            _subscription: subscription,
            key,
        });
    }
    let Some(binding) = slot else { return div() };
    let enabled = !view.opening_project
        && !view.close_prompt_open
        && view.worker_requests_pending == 0
        && view.pending_requests.is_empty();
    binding
        .entity
        .update(cx, |numeric, cx| numeric.set_enabled(enabled, cx));
    let entity = &binding.entity;
    div()
        .w(px(76.0))
        .h(px(20.0))
        .flex_shrink_0()
        .child(entity.clone())
}
