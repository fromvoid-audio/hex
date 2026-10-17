//! Reusable numeric controls and typed gesture routing for return sends.
use super::{EditorView, SendControl, SendControlKey, control_style};
use crate::theme;
use gpui::{Context, Div, ElementId, Stateful, div, prelude::*, px};
use hex_lattice_contract::ParameterValue;
use hex_project::{Command, TrackId};
use hex_session::device_gesture::GestureTarget;
use hex_session::service::Request;
use hex_widgets::{
    NumericControl, NumericControlConfig, NumericControlEvent, NumericControlFinalValue,
    NumericControlKind, NumericEventPhase, NumericScale, NumericSpec,
};

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
        let subscription =
            cx.subscribe(&entity, move |view, _, event: &NumericControlEvent, cx| {
                let value = ParameterValue::Float(event.value as f32);
                if let Err(error) = dispatch_numeric_gesture(view, target, event.phase, value)
                    && event.phase != NumericEventPhase::Cancel
                {
                    view.error = Some(error.to_owned().into());
                }
                cx.notify();
            });
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
        let _ = numeric.set_external_style(control_style(), cx);
        let _ = numeric.set_external_value(active.unwrap_or(f64::from(gain)), cx);
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
        if finalized.previous != finalized.value || active_matches {
            if let Err(error) = commit_owned_numeric_edit(view, target, &finalized) {
                view.error = Some(
                    format!("could not finish send edit: {error}")
                        .to_lowercase()
                        .into(),
                );
                return false;
            }
        }
        entity.update(cx, |control, cx| {
            control.accept_finish_edit_for_owner(finalized, cx)
        });
    }
    true
}

pub fn dispatch_numeric_gesture(
    view: &mut EditorView,
    target: GestureTarget,
    phase: NumericEventPhase,
    value: ParameterValue,
) -> Result<(), &'static str> {
    match phase {
        NumericEventPhase::Begin => view.begin_device_gesture(target, value),
        NumericEventPhase::Change => view.change_device_gesture(value),
        NumericEventPhase::Commit => view.finish_device_gesture(value),
        NumericEventPhase::Cancel => view.cancel_device_gesture(),
    }
}

pub fn commit_owned_numeric_edit(
    view: &mut EditorView,
    target: GestureTarget,
    finalized: &NumericControlFinalValue,
) -> Result<(), &'static str> {
    let previous = ParameterValue::Float(finalized.previous as f32);
    let value = ParameterValue::Float(finalized.value as f32);
    let active = view.device_gesture.snapshot();
    if active.is_some_and(|snapshot| snapshot.target != target)
        || view.device_gesture.has_pending_terminal()
    {
        let command = match target {
            GestureTarget::DeviceParameter { device, parameter } => Request::SetDeviceParameter {
                device,
                parameter,
                value,
            },
            GestureTarget::DeviceParameterPair { .. } => {
                return Err("paired parameter edits use the graph gesture");
            }
            GestureTarget::DeviceParameterSnapshot { .. } => {
                return Err("snapshot parameter edits use the harmonic gesture");
            }
            GestureTarget::ReturnSend {
                track,
                return_track,
            } => Request::Edit(Command::SetReturnSend {
                track,
                return_track,
                gain: match value {
                    ParameterValue::Float(gain) => gain,
                    _ => return Err("return send requires a float value"),
                },
            }),
        };
        if view.enqueue_device_barrier(command) {
            return Ok(());
        }
        return Err("device edit queue is full");
    }
    if active.is_some() {
        view.change_device_gesture(value)?;
        view.finish_device_gesture(value)?;
        return Ok(());
    }
    view.begin_device_gesture(target, previous)?;
    if finalized.value != finalized.previous {
        view.change_device_gesture(value)?;
    }
    view.finish_device_gesture(value)
}
