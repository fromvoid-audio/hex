//! Reusable numeric controls and typed gesture routing for return sends.
use super::{
    EditorView, SendControl, SendControlKey, commit_owned_numeric_edit, control_style,
    dispatch_numeric_gesture,
};
use crate::theme;
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px};
use hex_lattice_contract::ParameterValue;
use hex_project::TrackId;
use hex_session::device_gesture::GestureTarget;
use hex_widgets::{
    NumericControl, NumericControlConfig, NumericControlEvent, NumericControlKind,
    NumericEventPhase, NumericScale, NumericSpec,
};
use std::cell::Cell;

fn canonical_gain(view: &EditorView, key: SendControlKey) -> Option<f32> {
    let project = &view.state.as_ref()?.project;
    project
        .return_tracks()
        .iter()
        .find(|item| item.id() == key.return_track)?;
    Some(
        project
            .track(key.track)?
            .sends()
            .iter()
            .find(|send| send.return_track() == key.return_track)
            .map_or(0.0, hex_project::SendLevel::gain),
    )
}

pub fn return_send_numeric_control(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    track: TrackId,
    return_track: hex_project::ReturnId,
    gain: f32,
) -> Stateful<Div> {
    let key = SendControlKey {
        track,
        return_track,
    };
    let generation = view.project_generation;
    if let std::collections::hash_map::Entry::Vacant(entry) =
        view.device_panel.send_controls.entry(key)
    {
        let target = GestureTarget::ReturnSend {
            track,
            return_track,
        };
        let id = format!("return-send-{}-{}", track.value(), return_track.value());
        let spec = match NumericSpec::new(0.0, 2.0, 0.0, NumericScale::Linear, 3, None::<String>) {
            Ok(spec) => spec,
            Err(_) => return div().id("invalid-return-send-control"),
        };
        let config = match NumericControlConfig::new(
            id,
            theme::text("send"),
            NumericControlKind::HorizontalSlider,
            spec,
            f64::from(gain),
            control_style(),
        ) {
            Ok(config) => config,
            Err(_) => return div().id("invalid-return-send-control"),
        };
        let entity = cx.new(|control_cx| NumericControl::from_config(control_cx, config));
        let atomic_admitted = Cell::new(false);
        let subscription = cx.subscribe(
            &entity,
            move |view, control, event: &NumericControlEvent, cx| {
                if generation != view.project_generation || canonical_gain(view, key).is_none() {
                    return;
                }
                if event.phase == NumericEventPhase::Begin {
                    atomic_admitted.set(false);
                }
                let verified = view.document_audio_mode != hex_session::DocumentAudioMode::Resident;
                if verified {
                    let explicit = event.gesture.is_none();
                    let rejected = match event.phase {
                        NumericEventPhase::Begin => {
                            if explicit && !view.edit_pending() {
                                atomic_admitted.set(true);
                                false
                            } else {
                                true
                            }
                        }
                        NumericEventPhase::Commit if atomic_admitted.replace(false) => {
                            view.edit_pending()
                                || !view.enqueue_device_barrier(
                                    hex_session::service::Request::Edit(
                                        hex_project::Command::SetReturnSend {
                                            track,
                                            return_track,
                                            gain: event.value as f32,
                                        },
                                    ),
                                )
                        }
                        NumericEventPhase::Cancel => {
                            atomic_admitted.set(false);
                            false
                        }
                        _ => false,
                    };
                    if rejected {
                        view.error = Some(
                            if explicit {
                                "finish the current edit before changing the send"
                            } else {
                                "held send controls require resident audio"
                            }
                            .into(),
                        );
                        if let Some(gain) = canonical_gain(view, key) {
                            control.update(cx, |control, cx| {
                                control.cancel(cx);
                                let _ = control.set_external_value(f64::from(gain), cx);
                            });
                        }
                    }
                    cx.notify();
                    return;
                }
                let value = ParameterValue::Float(event.value as f32);
                if let Err(error) = dispatch_numeric_gesture(view, target, event.phase, value)
                    && event.phase != NumericEventPhase::Cancel
                {
                    view.error = Some(error.to_owned().into());
                }
                cx.notify();
            },
        );
        entry.insert(SendControl {
            entity,
            _subscription: subscription,
        });
    }

    let Some(control) = view.device_panel.send_controls.get(&key) else {
        return div().id("missing-return-send-control");
    };
    let target = GestureTarget::ReturnSend {
        track,
        return_track,
    };
    let active = view
        .device_gesture
        .snapshot()
        .filter(|snapshot| snapshot.target == target)
        .and_then(|snapshot| match snapshot.value {
            ParameterValue::Float(value) => Some(f64::from(value)),
            _ => None,
        });
    control.entity.update(cx, |numeric, cx| {
        numeric.set_enabled(
            !view.gain_recorder.blocks_edits()
                && !view.midi_recorder.blocks_edits()
                && !view.opening_project
                && !view.close_prompt_open,
            cx,
        );
        let _ = numeric.set_external_style(control_style(), cx);
        // The worker owns an admitted explicit edit until acknowledgement. Avoid
        // flashing the old canonical value during that handoff.
        if view.document_audio_mode == hex_session::DocumentAudioMode::Resident
            || !view.edit_pending()
        {
            let _ = numeric.set_external_value(active.unwrap_or(f64::from(gain)), cx);
        }
    });
    // NumericControl renders its own slider and value field. Render the entity
    // once so hit regions, focus and value-field IDs have one owner.
    div()
        .id(ElementId::Name(
            format!(
                "return-send-{}-{}-control",
                track.value(),
                return_track.value()
            )
            .into(),
        ))
        .w(px(136.0))
        .h(px(24.0))
        .child(control.entity.clone())
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests;

pub fn finalize_send_controls_for_track(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
    track: TrackId,
) -> bool {
    let active_target = view
        .device_gesture
        .snapshot()
        .map(|snapshot| snapshot.target);
    let mut keys: Vec<_> = view
        .device_panel
        .send_controls
        .keys()
        .filter(|key| key.track == track)
        .copied()
        .collect();
    keys.sort_by_key(|key| {
        u8::from(
            active_target
                != Some(GestureTarget::ReturnSend {
                    track: key.track,
                    return_track: key.return_track,
                }),
        )
    });

    for key in keys {
        let target = GestureTarget::ReturnSend {
            track: key.track,
            return_track: key.return_track,
        };
        let Some(control) = view.device_panel.send_controls.get(&key) else {
            continue;
        };
        let entity = control.entity.clone();
        let prepared = entity.update(cx, |control, cx| control.prepare_finish_edit_for_owner(cx));
        let finalized = match prepared {
            Ok(finalized) => finalized,
            Err(error) => {
                entity.update(cx, |control, cx| control.cancel(cx));
                if view.device_gesture.snapshot().is_some_and(|snapshot| {
                    snapshot.target == target && !view.device_gesture.has_pending_terminal()
                }) {
                    let _ = view.cancel_device_gesture();
                }
                view.error = Some(format!(
                    "invalid send value canceled because its track left the visible area: {error}"
                ).to_lowercase().into());
                continue;
            }
        };
        let Some(finalized) = finalized else {
            continue;
        };
        let active_matches = view
            .device_gesture
            .snapshot()
            .is_some_and(|snapshot| snapshot.target == target)
            && !view.device_gesture.has_pending_terminal();
        if (finalized.previous != finalized.value || active_matches)
            && let Err(error) = commit_owned_numeric_edit(view, target, &finalized)
        {
            view.error = Some(
                format!("could not finish send edit: {error}")
                    .to_lowercase()
                    .into(),
            );
            return false;
        }
        entity.update(cx, |control, cx| {
            control.accept_finish_edit_for_owner(finalized, cx)
        });
    }
    true
}
