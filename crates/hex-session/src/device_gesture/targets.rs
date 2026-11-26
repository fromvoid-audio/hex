//! Target preview/commit helpers and static parameter value queries for device gestures.
use super::*;

pub(super) fn terminal_request(active: GestureSnapshot) -> GestureRequest {
    if let Some(value) = active.plugin_value {
        GestureRequest::CommitPlugin {
            key: active.key,
            sequence: active.sequence.max(1),
            value,
        }
    } else if let Some(snapshot) = active.creation {
        GestureRequest::CommitEqCreation {
            key: active.key,
            sequence: active.sequence.max(1),
            snapshot,
        }
    } else if let Some(snapshot) = active.parameters {
        GestureRequest::CommitSnapshot {
            touched_parameters: active.touched_parameters,
            key: active.key,
            sequence: active.sequence.max(1),
            snapshot,
        }
    } else if let Some(second) = active.secondary_value {
        GestureRequest::CommitPair {
            manual_edit: active.manual_edit,
            key: active.key,
            sequence: active.sequence.max(1),
            value: ParameterPairValue {
                first: active.value,
                second,
            },
        }
    } else {
        GestureRequest::Commit {
            manual_edit: active.manual_edit,
            key: active.key,
            sequence: active.sequence.max(1),
            value: active.value,
        }
    }
}
pub(super) fn default_preparation(
    session: &mut Session,
    operation: GesturePreparation,
    previous: Option<&Arc<PreparedRevision>>,
    same_non_resident_owner: bool,
) -> PreparedGesture {
    match operation {
        GesturePreparation::Plugin(_) => Err(SessionError::Invalid(
            "plugin gestures require native host preparation",
        )),
        GesturePreparation::UnchangedPairPreview(_) if same_non_resident_owner => Ok(None),
        GesturePreparation::Preview(active) | GesturePreparation::UnchangedPairPreview(active) => {
            let prepared = if active.track_gains.is_some()
                || active.creation.is_some()
                || active.manual_edit
                    && matches!(
                        active.target,
                        GestureTarget::DeviceParameter { .. }
                            | GestureTarget::DeviceParameterPair { .. }
                            | GestureTarget::DeviceParameterSnapshot { .. }
                    ) {
                session.preview_mix_command(
                    command_for_snapshot(session, active)?,
                    previous.map(Arc::as_ref),
                )
            } else if let Some(parameters) = active.parameters {
                let GestureTarget::DeviceParameterSnapshot { device } = active.target else {
                    return Err(SessionError::Invalid("snapshot target changed"));
                };
                session.preview_device_snapshot(device, &parameters, previous.map(Arc::as_ref))
            } else if let Some(second) = active.secondary_value {
                preview_pair_target(
                    session,
                    active.target,
                    ParameterPairValue {
                        first: active.value,
                        second,
                    },
                    previous.map(Arc::as_ref),
                )
            } else {
                preview_target(
                    session,
                    active.target,
                    active.value,
                    previous.map(Arc::as_ref),
                )
            }?;
            Ok(Some(prepared))
        }
        GesturePreparation::Commit(active) => {
            let prepared = if active.track_gains.is_some()
                || active.creation.is_some()
                || active.manual_edit
                    && matches!(
                        active.target,
                        GestureTarget::DeviceParameter { .. }
                            | GestureTarget::DeviceParameterPair { .. }
                            | GestureTarget::DeviceParameterSnapshot { .. }
                    ) {
                session
                    .command_prepared_revision(
                        command_for_snapshot(session, active)?,
                        previous.map(Arc::as_ref),
                    )
                    .map(|(_, prepared)| prepared)
            } else if let Some(parameters) = active.parameters {
                let GestureTarget::DeviceParameterSnapshot { device } = active.target else {
                    return Err(SessionError::Invalid("snapshot target changed"));
                };
                session.commit_device_snapshot(device, &parameters, previous.map(Arc::as_ref))
            } else if let Some(second) = active.secondary_value {
                commit_pair_target(
                    session,
                    active.target,
                    ParameterPairValue {
                        first: active.value,
                        second,
                    },
                    previous.map(Arc::as_ref),
                )
            } else {
                commit_target(
                    session,
                    active.target,
                    active.value,
                    previous.map(Arc::as_ref),
                )
            }?;
            Ok(Some((Arc::new(session.project().clone()), prepared)))
        }
        GesturePreparation::Restore => Ok(
            previous.map(|prepared| (Arc::new(session.project().clone()), Arc::clone(prepared)))
        ),
    }
}

pub(super) fn preview_target(
    session: &Session,
    target: GestureTarget,
    value: ParameterValue,
    previous: Option<&PreparedRevision>,
) -> Result<(Arc<Project>, Arc<PreparedRevision>), SessionError> {
    match target {
        GestureTarget::MainGain => session.preview_mix_command(
            hex_project::Command::MasterGain {
                gain: mix_value(value, 0.0, 2.0)?,
            },
            previous,
        ),
        GestureTarget::TrackBalance { track } => {
            session.preview_track_balance(track, mix_value(value, -1.0, 1.0)?, previous)
        }
        GestureTarget::ReturnGain { return_track } => {
            session.preview_return_gain(return_track, mix_value(value, 0.0, 2.0)?, previous)
        }
        GestureTarget::TrackGain { track } => {
            session.preview_track_gain(track, track_gain_value(value)?, previous)
        }
        GestureTarget::DeviceParameter { device, parameter } => {
            session.preview_device_parameter(device, parameter, value, previous)
        }
        GestureTarget::TrackGains { .. }
        | GestureTarget::DeviceParameterPair { .. }
        | GestureTarget::PluginParameter { .. }
        | GestureTarget::DeviceParameterSnapshot { .. }
        | GestureTarget::EqNodeCreation { .. } => Err(SessionError::Invalid(
            "paired device gesture requires two values",
        )),
        GestureTarget::ReturnSend {
            track,
            return_track,
        } => session.preview_return_send(track, return_track, send_gain(value)?, previous),
    }
}

pub(super) fn commit_target(
    session: &mut Session,
    target: GestureTarget,
    value: ParameterValue,
    previous: Option<&PreparedRevision>,
) -> Result<Arc<PreparedRevision>, SessionError> {
    match target {
        GestureTarget::MainGain => session
            .command_prepared_revision(
                hex_project::Command::MasterGain {
                    gain: mix_value(value, 0.0, 2.0)?,
                },
                previous,
            )
            .map(|(_, prepared)| prepared),
        GestureTarget::TrackBalance { track } => {
            session.commit_track_balance(track, mix_value(value, -1.0, 1.0)?, previous)
        }
        GestureTarget::ReturnGain { return_track } => {
            session.commit_return_gain(return_track, mix_value(value, 0.0, 2.0)?, previous)
        }
        GestureTarget::TrackGain { track } => {
            session.commit_track_gain(track, track_gain_value(value)?, previous)
        }
        GestureTarget::DeviceParameter { device, parameter } => {
            session.commit_device_parameter(device, parameter, value, previous)
        }
        GestureTarget::TrackGains { .. }
        | GestureTarget::DeviceParameterPair { .. }
        | GestureTarget::PluginParameter { .. }
        | GestureTarget::DeviceParameterSnapshot { .. }
        | GestureTarget::EqNodeCreation { .. } => Err(SessionError::Invalid(
            "paired device gesture requires two values",
        )),
        GestureTarget::ReturnSend {
            track,
            return_track,
        } => session.commit_return_send(track, return_track, send_gain(value)?, previous),
    }
}

pub(super) fn preview_pair_target(
    session: &Session,
    target: GestureTarget,
    value: ParameterPairValue,
    previous: Option<&PreparedRevision>,
) -> Result<(Arc<Project>, Arc<PreparedRevision>), SessionError> {
    match target {
        GestureTarget::DeviceParameterPair {
            device,
            first,
            second,
        } => session.preview_device_parameter_pair(
            device,
            (first, value.first),
            (second, value.second),
            previous,
        ),
        _ => Err(SessionError::Invalid(
            "paired values require a paired device gesture",
        )),
    }
}

pub(super) fn commit_pair_target(
    session: &mut Session,
    target: GestureTarget,
    value: ParameterPairValue,
    previous: Option<&PreparedRevision>,
) -> Result<Arc<PreparedRevision>, SessionError> {
    match target {
        GestureTarget::DeviceParameterPair {
            device,
            first,
            second,
        } => session.commit_device_parameter_pair(
            device,
            (first, value.first),
            (second, value.second),
            previous,
        ),
        _ => Err(SessionError::Invalid(
            "paired values require a paired device gesture",
        )),
    }
}

pub(super) fn static_track_balance(session: &Session, track: TrackId) -> Result<f32, SessionError> {
    require_resident_mix(session)?;
    session
        .project()
        .track(track)
        .map(hex_project::Track::balance)
        .ok_or(SessionError::Invalid(
            "track balance target no longer exists",
        ))
}

pub(super) fn static_return_gain(session: &Session, id: ReturnId) -> Result<f32, SessionError> {
    require_resident_mix(session)?;
    session
        .project()
        .return_tracks()
        .iter()
        .find(|track| track.id() == id)
        .map(hex_project::ReturnTrack::gain)
        .ok_or(SessionError::Invalid("return gain target no longer exists"))
}

pub(super) fn require_resident_mix(session: &Session) -> Result<(), SessionError> {
    if session.audio_mode() != crate::DocumentAudioMode::Resident {
        return Err(SessionError::Invalid(
            "held mixer controls require resident audio mode",
        ));
    }
    Ok(())
}

pub(super) fn mix_value(value: ParameterValue, min: f32, max: f32) -> Result<f32, SessionError> {
    match value {
        ParameterValue::Float(value) if value.is_finite() && (min..=max).contains(&value) => {
            Ok(value)
        }
        _ => Err(SessionError::Invalid(
            "mixer value must be finite and within its range",
        )),
    }
}

pub(super) fn static_track_gain(session: &Session, track: TrackId) -> Result<f32, SessionError> {
    let source = session
        .project()
        .track(track)
        .ok_or(SessionError::Invalid("track gain target no longer exists"))?;
    if !source.gain_envelope().is_empty() || !source.gain_lane().spans().is_empty() {
        return Err(SessionError::Invalid("track gain is owned by automation"));
    }
    let gain = source.gain();
    if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
        return Err(SessionError::Invalid("gain must be finite and within 0..2"));
    }
    Ok(gain)
}

pub(super) fn track_gain_value(value: ParameterValue) -> Result<f32, SessionError> {
    match value {
        ParameterValue::Float(gain) => Ok(gain),
        _ => Err(SessionError::Invalid("track gain must be numeric")),
    }
}

pub(super) fn send_gain(value: ParameterValue) -> Result<f32, SessionError> {
    match value {
        ParameterValue::Float(gain) if gain.is_finite() && (0.0..=2.0).contains(&gain) => Ok(gain),
        ParameterValue::Float(_) => Err(SessionError::Invalid(
            "return send gain must be finite and between zero and two",
        )),
        _ => Err(SessionError::Invalid("return send gain must be numeric")),
    }
}
