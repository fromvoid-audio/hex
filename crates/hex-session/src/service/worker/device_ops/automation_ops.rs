//! Device gestures, parameter automation, modulation and native recording arming worker operations.
use super::super::*;
use std::ops::ControlFlow;

pub(super) fn handle(ctx: &mut WorkerDispatch<'_>, request: Request) -> ControlFlow<()> {
    match request {
        Request::DeviceGesture(gesture) => {
            let gesture = *gesture;
            let independent_plugin_eq = ctx
                .worker
                .device_gesture
                .snapshot()
                .filter(|active| active.key == gesture.key())
                .and_then(|active| {
                    ctx.worker
                        .pending_plugin_effect
                        .as_ref()?
                        .independent_eq_gesture(ctx.worker.session.project(), active.target)
                });
            ctx.outcome.gesture_request = true;
            ctx.outcome.gesture_protocol_ack = true;
            let mut deferred = false;
            let outcome = if ctx.worker.tuning_gesture.snapshot().is_some()
                && matches!(
                    gesture,
                    GestureRequest::Begin { .. }
                        | GestureRequest::BeginTrackGains { .. }
                        | GestureRequest::BeginPlugin { .. }
                        | GestureRequest::BeginSnapshot { .. }
                        | GestureRequest::BeginEqCreation { .. }
                ) {
                crate::device_gesture::GestureOutcome {
                    ack: GestureAck {
                        preview_retained: false,
                        source_lost: false,
                        key: gesture.key(),
                        phase: gesture.phase(),
                        sequence: gesture.sequence(),
                        accepted: false,
                        reason: Some("project tuning gesture is active".into()),
                    },
                    playback: None,
                    preview_project: None,
                    canonical_changed: false,
                }
            } else if ctx.worker.events.resident_publication_enabled()
                && ctx.worker.session.audio_mode() == DocumentAudioMode::Resident
            {
                let admission = ctx.worker.export_lane.admission_with_resident_owners(
                    ctx.worker.compiled.as_ref().map(|(_, prepared)| {
                        hex_engine::native_device_resources::ControlSamplerSelection {
                            sources: ctx.worker.current_sources.as_ref(),
                            prepared: Some(prepared),
                        }
                    }),
                    ctx.worker.resident_seek.as_ref(),
                    None,
                );
                let mut outcome = ctx.worker.device_gesture.handle_prepared(
                    &mut ctx.worker.session,
                    gesture,
                    ctx.worker.compiled.as_ref().map(|(_, artifact)| artifact),
                    ctx.worker.project_generation,
                    |session, operation| {
                        let sink = ctx
                            .worker
                            .events
                            .resident_audio()
                            .ok_or(SessionError::Invalid("native preview output unavailable"))?;
                        let mut recording = plugin_recording::Preparation {
                            state: &mut ctx.worker.plugin_recording,
                            lane: &mut ctx.worker.native_automation_take,
                            document_base: ctx.worker.media_base.as_deref(),
                            recovery: ctx.worker.recovery.as_deref(),
                            may_defer: matches!(
                                gesture.phase(),
                                crate::device_gesture::GesturePhase::Commit
                                    | crate::device_gesture::GesturePhase::Cancel
                            ),
                            deferred: false,
                        };
                        if let crate::device_gesture::GesturePreparation::Plugin(operation) =
                            operation
                        {
                            let result = resident_preview::plugin_parameter::operation_using(
                                session,
                                operation,
                                hex_engine::AudioSourceContext {
                                    project_generation: ctx.worker.project_generation,
                                    editor_revision: ctx.worker.revision,
                                },
                                &mut ctx.worker.resident_snapshot,
                                &mut ctx.worker.resident_preview,
                                sink,
                                admission,
                                Some(&mut recording),
                            );
                            deferred |= recording.deferred;
                            return result;
                        }
                        resident_preview::device_operation(
                            session,
                            operation,
                            hex_engine::AudioSourceContext {
                                project_generation: ctx.worker.project_generation,
                                editor_revision: ctx.worker.revision,
                            },
                            &mut ctx.worker.resident_snapshot,
                            &mut ctx.worker.resident_preview,
                            sink,
                            admission,
                        )
                    },
                );
                if !deferred
                    && ctx
                        .worker
                        .resident_preview
                        .as_ref()
                        .and_then(resident_preview::Preview::device_identity)
                        .is_none_or(|preview| preview.key != outcome.ack.key)
                {
                    ctx.worker
                        .device_gesture
                        .retire_unpublished_native_terminal(&mut outcome.ack);
                }
                outcome
            } else if ctx.worker.session.audio_mode() == DocumentAudioMode::VerifiedStreaming
                && audio_sink::requires_plugin_profile(ctx.worker.session.project())
                && ctx.worker.events.resident_audio().is_some()
            {
                let previous = ctx.worker.compiled.as_ref().map(|(_, artifact)| artifact);
                ctx.worker.device_gesture.handle_verified(
                    &mut ctx.worker.session,
                    gesture,
                    previous,
                    ctx.worker.project_generation,
                    |session, operation| {
                        let sink = ctx
                            .worker
                            .events
                            .resident_audio()
                            .ok_or(SessionError::Invalid("native preview output unavailable"))?;
                        session.verified_device_gesture_operation(
                            operation,
                            previous,
                            hex_engine::AudioSourceContext {
                                project_generation: ctx.worker.project_generation,
                                editor_revision: ctx.worker.revision,
                            },
                            sink,
                        )
                    },
                )
            } else {
                ctx.worker.device_gesture.handle(
                    &mut ctx.worker.session,
                    gesture,
                    ctx.worker.compiled.as_ref().map(|(_, artifact)| artifact),
                    ctx.worker.project_generation,
                )
            };
            // Begin may renew a stopped native epoch before metadata
            // admission. Retain the new canonical resources even if
            // the subsequent target validation refuses this gesture.
            if matches!(gesture, GestureRequest::BeginPlugin { .. })
                && ctx.worker.resident_preview.is_none()
                && let Some(snapshot) = ctx.worker.resident_snapshot.as_ref()
            {
                ctx.worker.compiled = Some((ctx.worker.revision, Arc::clone(&snapshot.prepared)));
            }
            let native_terminal_accepted = ctx.worker.events.resident_publication_enabled()
                && ctx.worker.session.audio_mode() == DocumentAudioMode::Resident
                && outcome.ack.accepted
                && matches!(
                    gesture.phase(),
                    crate::device_gesture::GesturePhase::Commit
                        | crate::device_gesture::GesturePhase::Cancel
                );
            if outcome.ack.accepted
                && outcome.canonical_changed
                && let Some(device) = independent_plugin_eq
                && let Some(pending) = ctx.worker.pending_plugin_effect.as_mut()
            {
                pending.rebase_after_accepted_eq_write(
                    hex_engine::AudioSourceContext {
                        project_generation: ctx.worker.project_generation,
                        editor_revision: ctx.worker.revision,
                    },
                    ctx.worker.session.project(),
                    device,
                );
            }
            if deferred {
                ctx.outcome.request_complete = false;
                ctx.worker.plugin_recording.defer(gesture);
                ctx.outcome.gesture_protocol_ack = false;
                ctx.outcome.gesture_request = false;
            } else {
                ctx.worker.plugin_recording.attempt(gesture);
                ctx.outcome.gesture_ack = Some(outcome.ack);
            }
            if let Some(prepared) = outcome.playback {
                if native_terminal_accepted && !outcome.canonical_changed {
                    ctx.worker.compiled = Some((ctx.worker.revision, Arc::clone(&prepared)));
                }
                if outcome.canonical_changed {
                    ctx.worker.unsaved = true;
                    ctx.outcome.replacement_revision = Some(prepared);
                } else if matches!(
                    gesture,
                    GestureRequest::Change { .. }
                        | GestureRequest::ChangePlugin { .. }
                        | GestureRequest::ChangePair { .. }
                        | GestureRequest::ChangeSnapshot { .. }
                        | GestureRequest::ChangeEqCreation { .. }
                ) {
                    ctx.outcome.preview_prepared = Some(prepared);
                    ctx.outcome.preview_project = outcome.preview_project;
                } else {
                    ctx.outcome.restore_prepared = Some(prepared);
                }
            }
        }

        Request::ReplaceDeviceModulation { .. } => {
            ctx.outcome.error = Some("modulation target was not resolved".into());
        }

        Request::DeviceAutomation { .. } | Request::DeviceAutomationControl { .. } => {
            ctx.outcome.error = Some("automation target was not resolved".into());
        }

        Request::SetDeviceParameter {
            device,
            parameter,
            value,
        } => {
            let patched = ctx
                .worker
                .session
                .native_device_class(device)
                .map_err(|reason| {
                    hex_storage::text::lowercase_outside_verbatim(&reason.to_string())
                })
                .and_then(|class| {
                    let entry = ctx
                        .worker
                        .session
                        .project()
                        .device(device)
                        .ok_or_else(|| "device is missing".to_owned())?;
                    hex_engine::patch_native_device_parameter_for_class(
                        class,
                        entry.state(),
                        parameter,
                        value,
                    )
                    .map_err(|reason| {
                        hex_storage::text::lowercase_outside_verbatim(&reason.to_string())
                    })
                });
            match patched {
                Ok(state) => {
                    let before_project = ctx.worker.session.project().clone();
                    let command = crate::device_gesture::manual_device_edit(
                        &ctx.worker.session,
                        device,
                        state,
                        [parameter],
                        true,
                    );
                    match resident_audio::execute_command(
                        &mut ctx.worker.session,
                        command,
                        ctx.worker
                            .compiled
                            .as_ref()
                            .map(|(_, artifact)| artifact.as_ref()),
                        hex_engine::AudioSourceContext {
                            project_generation: ctx.worker.project_generation,
                            editor_revision: ctx.worker.revision.saturating_add(1),
                        },
                        &mut ctx.worker.events,
                        &mut ctx.worker.resident_snapshot,
                        ctx.worker.export_lane.admission_with_resident_owners(
                            ctx.worker.compiled.as_ref().map(|(_, prepared)| {
                                hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: ctx.worker.current_sources.as_ref(),
                                    prepared: Some(prepared),
                                }
                            }),
                            ctx.worker.resident_seek.as_ref(),
                            ctx.worker.resident_preview.as_ref(),
                        ),
                    ) {
                        Ok((_, prepared)) if ctx.worker.session.project() != &before_project => {
                            if let Some(pending) = ctx.worker.pending_plugin_effect.as_mut() {
                                pending.rebase_after_accepted_eq_write(
                                    hex_engine::AudioSourceContext {
                                        project_generation: ctx.worker.project_generation,
                                        editor_revision: ctx.worker.revision,
                                    },
                                    ctx.worker.session.project(),
                                    device,
                                );
                            }
                            ctx.worker.unsaved = true;
                            ctx.outcome.replacement_revision = Some(prepared);
                            ctx.outcome.status = Some(
                                if ctx.worker.events.resident_publication_enabled()
                                    && ctx.worker.session.audio_mode()
                                        == DocumentAudioMode::Resident
                                {
                                    "resident edit accepted · waiting for audio"
                                } else {
                                    "device parameter changed"
                                }
                                .into(),
                            );
                        }
                        Ok(_) => ctx.outcome.status = Some("no change".into()),
                        Err(reason) => {
                            ctx.outcome.error = Some(hex_storage::text::lowercase_outside_verbatim(
                                &reason.to_string(),
                            ))
                        }
                    }
                }
                Err(reason) => ctx.outcome.error = Some(reason),
            }
        }

        Request::DeviceAutomationTake { .. }
        | Request::StartPluginEditorRecording { .. }
        | Request::StopPluginEditorRecording { .. }
        | Request::ContinuePluginEditorRecording { .. }
        | Request::QueryFailedPluginAutomation { .. }
        | Request::InspectSealedAutomationRecovery { .. }
        | Request::QueryDeviceAutomationCloseSafety { .. }
        | Request::PreparePluginEditorClose { .. }
        | Request::RecoverDeviceAutomationTake { .. } => {}

        Request::ArmNativeParameterRecording {
            project_generation: expected_generation,
            revision: expected_revision,
            target,
        } => {
            let result = (|| {
                if expected_generation != ctx.worker.project_generation
                    || expected_revision != ctx.worker.revision
                {
                    return Err(SessionError::Invalid(
                        "automation arm refused · project changed",
                    ));
                }
                if ctx.worker.native_automation_take.busy() {
                    return Err(SessionError::Invalid(
                        "finish the current automation take before changing its arm",
                    ));
                }
                if ctx.worker.device_gesture.snapshot().is_some() {
                    return Err(SessionError::Invalid(
                        "automation arm refused · device edit is still applying",
                    ));
                }
                if !ctx.worker.events.resident_publication_enabled()
                    || ctx.worker.session.audio_mode() != DocumentAudioMode::Resident
                {
                    return Err(SessionError::Invalid(
                        "device automation recording requires ordinary resident playback",
                    ));
                }
                let current =
                    ctx.worker
                        .resident_snapshot
                        .as_ref()
                        .ok_or(SessionError::Invalid(
                            "native automation source unavailable",
                        ))?;
                let sink = ctx
                    .worker
                    .events
                    .resident_audio()
                    .ok_or(SessionError::Invalid(
                        "native automation output unavailable",
                    ))?;
                let input = target
                    .map(|(device, target)| {
                        hex_engine::native_parameter_input_target(
                            ctx.worker.session.project(),
                            device,
                            target,
                        )
                    })
                    .transpose()?;
                ctx.worker
                    .session
                    .prepare_resident_native_parameter_recording(
                        input,
                        hex_engine::AudioSourceContext {
                            project_generation: ctx.worker.project_generation,
                            editor_revision: ctx.worker.revision,
                        },
                        sink,
                        ctx.worker.export_lane.admission_with_resident_owners(
                            ctx.worker.compiled.as_ref().map(|(_, prepared)| {
                                hex_engine::native_device_resources::ControlSamplerSelection {
                                    sources: ctx.worker.current_sources.as_ref(),
                                    prepared: Some(prepared),
                                }
                            }),
                            ctx.worker.resident_seek.as_ref(),
                            ctx.worker.resident_preview.as_ref(),
                        ),
                        current,
                    )
            })();
            match result {
                Ok(snapshot) => {
                    ctx.worker.compiled =
                        Some((ctx.worker.revision, Arc::clone(&snapshot.prepared)));
                    ctx.worker.resident_snapshot = Some(snapshot);
                    ctx.outcome.status = Some(if target.is_some() {
                        "device automation target prepared".into()
                    } else {
                        "device automation recording disarmed".into()
                    });
                }
                Err(reason) => {
                    ctx.outcome.error = Some(hex_storage::text::lowercase_outside_verbatim(
                        &reason.to_string(),
                    ))
                }
            }
        }

        _ => {
            ctx.outcome.error = Some("unexpected worker dispatch route".into());
        }
    }
    ControlFlow::Continue(())
}

#[cfg(test)]
mod tests {
    use super::super::native_device_name;

    #[test]
    fn native_device_name_returns_registered_name_for_known_effect() {
        let catalog =
            hex_engine::native_device_catalog_for_class(hex_engine::NativeDeviceClass::Effect)
                .unwrap();
        let first = catalog.first().unwrap();
        assert_eq!(
            native_device_name(hex_engine::NativeDeviceClass::Effect, first.type_id),
            Ok(first.name.clone())
        );
    }
}
