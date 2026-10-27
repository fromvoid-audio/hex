//! Host parameter input stays at the actual sink; canonical opaque commit stays
//! at the ordinary Editor transaction. No transient project or DSP clone is
//! needed to preview a point on the original hosted instance.
use super::*;
use crate::device_gesture::{GestureTarget, PluginGesturePreparation};
use hex_lattice_contract::DeviceTypeId;

#[allow(clippy::too_many_arguments)] // Same actual canonical/preview/publication owners as device_operation.
pub(super) fn operation(
    session: &mut Session,
    operation: PluginGesturePreparation,
    context: hex_engine::AudioSourceContext,
    current: &mut Option<resident_audio::Snapshot>,
    preview: &mut Option<Preview>,
    sink: &mut dyn WorkerAudioSink,
    admission: sampler_export::Admission<'_>,
    pending: &mut Option<resident_audio::PendingPluginPublication>,
    stopped_recovery: &mut Option<resident_audio::PendingPluginRecovery>,
) -> crate::device_gesture::PreparedGesture {
    let snapshot = match operation {
        PluginGesturePreparation::Begin { origin, snapshot } => {
            if origin.context != context {
                return Err(SessionError::Invalid("plugin parameter context changed"));
            }
            snapshot
        }
        PluginGesturePreparation::Preview(snapshot)
        | PluginGesturePreparation::Commit(snapshot)
        | PluginGesturePreparation::Restore { snapshot, .. } => snapshot,
    };
    let GestureTarget::PluginParameter { device, .. } = snapshot.target else {
        return Err(SessionError::Invalid("plugin parameter target changed"));
    };
    let terminal = matches!(operation, PluginGesturePreparation::Commit(_));
    let result = (|| {
        let mut renewed = None;
        if let Some(mut recovery) = stopped_recovery.take() {
            if recovery.retrying_reopen {
                if sink.plugin_state_restore_ready() == Some(context)
                    && let Ok(mut next) = session.stage_plugin_state_recovery(
                        context,
                        sink,
                        with_preview(admission, preview.as_ref()),
                        current.as_ref(),
                    )
                {
                    if let Some(old_output) = recovery.unsubmitted_old_output {
                        next.unsubmitted_old_output = Some(old_output);
                        *stopped_recovery = Some(next);
                    } else {
                        recovery.replacement = Some(Box::new(next));
                        recovery.retrying_reopen = false;
                        *stopped_recovery = Some(recovery);
                    }
                    return Err(SessionError::PluginPending);
                }
                *stopped_recovery = Some(recovery);
                return Err(SessionError::PluginPending);
            }
            if let Some(old_output) = recovery.unsubmitted_old_output {
                match recovery.terminal(sink) {
                    ResidentPublicationTerminal::Applied => {
                        let Some(receipt) = recovery.receipt else {
                            recovery.hold();
                            *stopped_recovery = Some(recovery);
                            return Err(SessionError::PluginPending);
                        };
                        let completion = sink.retire_unsubmitted_stopped_plugin_gesture(
                            context,
                            snapshot.key,
                            snapshot.sequence,
                            old_output,
                            receipt,
                        );
                        if completion != StoppedGestureCompletion::Completed {
                            if completion == StoppedGestureCompletion::Conflict {
                                recovery.hold();
                            }
                            *stopped_recovery = Some(recovery);
                            return Err(SessionError::PluginPending);
                        }
                        sink.finish_plugin_state_restore(context, true);
                        recovery
                            .snapshot
                            .inputs
                            .cancel_retired_from(&recovery.previous_inputs);
                        *current = Some(recovery.snapshot);
                        *preview = None;
                        return Err(SessionError::PluginSourceLost(
                            "vst3 output closed before the stopped edit was submitted".into(),
                        ));
                    }
                    ResidentPublicationTerminal::Refused(_) => {
                        let Some(receipt) = recovery.receipt else {
                            recovery.hold();
                            *stopped_recovery = Some(recovery);
                            return Err(SessionError::PluginPending);
                        };
                        match sink.retry_refused_plugin_state_restore(context, receipt) {
                            StoppedGestureCompletion::Completed => recovery.retrying_reopen = true,
                            StoppedGestureCompletion::Pending => {}
                            StoppedGestureCompletion::Conflict => recovery.hold(),
                        }
                    }
                    ResidentPublicationTerminal::OutputClosedApplied
                    | ResidentPublicationTerminal::OutputClosedUnapplied
                    | ResidentPublicationTerminal::OutputClosedRefused(_) => {
                        // The new exclusive output proves this exact failed
                        // replacement's callback has retired. Keep its owners
                        // until the next canonical graph is actually admitted;
                        // the original unsubmitted gesture's lease proof stays
                        // in the desktop owner across every replacement.
                        if sink.plugin_state_restore_ready() == Some(context)
                            && let Ok(mut next) = session.stage_plugin_state_recovery(
                                context,
                                sink,
                                with_preview(admission, preview.as_ref()),
                                current.as_ref(),
                            )
                        {
                            next.unsubmitted_old_output = Some(old_output);
                            *stopped_recovery = Some(next);
                            return Err(SessionError::PluginPending);
                        }
                    }
                    _ => {}
                }
                *stopped_recovery = Some(recovery);
                return Err(SessionError::PluginPending);
            }
            if let Some(replacement) = recovery.replacement.as_ref() {
                match replacement.terminal(sink) {
                    ResidentPublicationTerminal::Applied => {
                        let (Some(old), Some(fresh)) = (recovery.receipt, replacement.receipt)
                        else {
                            recovery.hold();
                            *stopped_recovery = Some(recovery);
                            return Err(SessionError::PluginPending);
                        };
                        let completion = sink.retire_closed_stopped_plugin_gesture(
                            context,
                            snapshot.key,
                            snapshot.sequence,
                            old,
                            fresh,
                        );
                        if completion != StoppedGestureCompletion::Completed {
                            if completion == StoppedGestureCompletion::Conflict {
                                recovery.hold();
                            }
                            *stopped_recovery = Some(recovery);
                            return Err(SessionError::PluginPending);
                        }
                        let Some(replacement) = recovery.replacement.take() else {
                            *stopped_recovery = Some(recovery);
                            return Err(SessionError::PluginPending);
                        };
                        sink.finish_plugin_state_restore(context, true);
                        replacement
                            .snapshot
                            .inputs
                            .cancel_retired_from(&replacement.previous_inputs);
                        *current = Some(replacement.snapshot);
                        *preview = None;
                        return Err(SessionError::PluginSourceLost(
                            "vst3 output closed before the stopped edit completed".into(),
                        ));
                    }
                    ResidentPublicationTerminal::Refused(_) => {
                        let Some(receipt) = replacement.receipt else {
                            recovery.hold();
                            *stopped_recovery = Some(recovery);
                            return Err(SessionError::PluginPending);
                        };
                        match sink.retry_refused_plugin_state_restore(context, receipt) {
                            StoppedGestureCompletion::Completed => recovery.retrying_reopen = true,
                            StoppedGestureCompletion::Pending => {}
                            StoppedGestureCompletion::Conflict => recovery.hold(),
                        }
                        *stopped_recovery = Some(recovery);
                        return Err(SessionError::PluginPending);
                    }
                    ResidentPublicationTerminal::OutputClosedApplied
                    | ResidentPublicationTerminal::OutputClosedUnapplied
                    | ResidentPublicationTerminal::OutputClosedRefused(_) => {
                        if sink.plugin_state_restore_ready() == Some(context)
                            && let Ok(next) = session.stage_plugin_state_recovery(
                                context,
                                sink,
                                with_preview(admission, preview.as_ref()),
                                current.as_ref(),
                            )
                        {
                            // The fresh lease proves the failed replacement is
                            // retired; keep it until the next graph is admitted.
                            recovery.replacement = Some(Box::new(next));
                        }
                        *stopped_recovery = Some(recovery);
                        return Err(SessionError::PluginPending);
                    }
                    _ => {
                        *stopped_recovery = Some(recovery);
                        return Err(SessionError::PluginPending);
                    }
                }
            }
            let result = recovery.terminal(sink);
            let receipt = recovery.receipt;
            match result {
                ResidentPublicationTerminal::Applied => {
                    let Some(receipt) = receipt else {
                        recovery.hold();
                        *stopped_recovery = Some(recovery);
                        return Err(SessionError::PluginPending);
                    };
                    let completion = sink.finish_stopped_plugin_gesture(
                        context,
                        snapshot.key,
                        snapshot.sequence,
                        receipt,
                        true,
                    );
                    if completion != StoppedGestureCompletion::Completed {
                        if completion == StoppedGestureCompletion::Conflict {
                            recovery.hold();
                        }
                        *stopped_recovery = Some(recovery);
                        return Err(SessionError::PluginPending);
                    }
                    recovery
                        .snapshot
                        .inputs
                        .cancel_retired_from(&recovery.previous_inputs);
                    renewed = Some(Arc::clone(&recovery.snapshot.prepared));
                    *current = Some(recovery.snapshot);
                }
                ResidentPublicationTerminal::Refused(_) => {
                    let Some(receipt) = receipt else {
                        recovery.hold();
                        *stopped_recovery = Some(recovery);
                        return Err(SessionError::PluginPending);
                    };
                    let completion = sink.finish_stopped_plugin_gesture(
                        context,
                        snapshot.key,
                        snapshot.sequence,
                        receipt,
                        false,
                    );
                    if completion != StoppedGestureCompletion::Completed {
                        if completion == StoppedGestureCompletion::Conflict {
                            recovery.hold();
                        }
                        *stopped_recovery = Some(recovery);
                        return Err(SessionError::PluginPending);
                    }
                    // The refused graph did not undo the already flushed SDK
                    // point. The sink retains this gesture and restores the
                    // original point before any refusal can be acknowledged.
                    return Err(SessionError::PluginPending);
                }
                ResidentPublicationTerminal::OutputClosedApplied
                | ResidentPublicationTerminal::OutputClosedUnapplied
                | ResidentPublicationTerminal::OutputClosedRefused(_) => {
                    let Some(receipt) = receipt else {
                        recovery.hold();
                        *stopped_recovery = Some(recovery);
                        return Err(SessionError::PluginPending);
                    };
                    let proof = sink.preserve_closed_stopped_plugin_gesture(
                        context,
                        snapshot.key,
                        snapshot.sequence,
                        receipt,
                    );
                    if proof != StoppedGestureCompletion::Completed {
                        if proof == StoppedGestureCompletion::Conflict {
                            recovery.hold();
                        }
                        *stopped_recovery = Some(recovery);
                        return Err(SessionError::PluginPending);
                    }
                    if sink.plugin_state_restore_ready() == Some(context)
                        && let Ok(replacement) = session.stage_plugin_state_recovery(
                            context,
                            sink,
                            with_preview(admission, preview.as_ref()),
                            current.as_ref(),
                        )
                    {
                        recovery.replacement = Some(Box::new(replacement));
                    }
                    *stopped_recovery = Some(recovery);
                    return Err(SessionError::PluginPending);
                }
                _ => {
                    *stopped_recovery = Some(recovery);
                    return Err(SessionError::PluginPending);
                }
            }
        }
        if terminal && let Some(mut publication) = pending.take() {
            if publication.terminal(sink) == ResidentPublicationTerminal::OutputClosedApplied {
                if !publication.retired_applied_started {
                    let started = publication.retired_identity().is_some_and(|identity| {
                        let candidate = Arc::new(PluginRecoveryCandidate {
                            project: Arc::new(publication.token.project().clone()),
                            context: publication.context,
                        });
                        sink.begin_retired_applied_plugin_recovery(identity, candidate)
                            .is_ok()
                    });
                    if started {
                        publication.retired_applied_started = true;
                    } else {
                        publication.quarantine();
                    }
                    *pending = Some(publication);
                    return Err(SessionError::PluginPending);
                }
                if publication.recovery.is_none() {
                    match session.stage_plugin_candidate_recovery(
                        publication.token.project(),
                        publication.context,
                        sink,
                        with_preview(admission, preview.as_ref()),
                        current.as_ref(),
                    ) {
                        Ok(recovery) => publication.recovery = Some(recovery),
                        Err(_) => publication.quarantine(),
                    }
                    *pending = Some(publication);
                    return Err(SessionError::PluginPending);
                }
                if publication.recovery.as_ref().is_some_and(|recovery| {
                    matches!(
                        recovery.terminal(sink),
                        ResidentPublicationTerminal::Refused(_)
                    )
                }) {
                    sink.finish_plugin_state_restore(publication.context, false);
                    publication.quarantine();
                    *pending = Some(publication);
                    return Err(SessionError::PluginPending);
                }
                return match publication.finish_retired_applied(
                    session,
                    sink,
                    context,
                    snapshot.key,
                    snapshot.sequence,
                ) {
                    Ok(restored) => {
                        let prepared = Arc::clone(&restored.prepared);
                        *current = Some(restored);
                        *preview = None;
                        Ok(Some((Arc::new(session.project().clone()), prepared)))
                    }
                    Err(publication) => {
                        *pending = Some(publication);
                        Err(SessionError::PluginPending)
                    }
                };
            }
            if matches!(
                publication.terminal(sink),
                ResidentPublicationTerminal::OutputClosedUnapplied
                    | ResidentPublicationTerminal::OutputClosedRefused(_)
            ) {
                if publication.recovery.is_none() {
                    match session.stage_plugin_state_recovery(
                        context,
                        sink,
                        with_preview(admission, preview.as_ref()),
                        current.as_ref(),
                    ) {
                        Ok(recovery) => publication.recovery = Some(recovery),
                        Err(_) => publication.quarantine(),
                    }
                    *pending = Some(publication);
                    return Err(SessionError::PluginPending);
                }
                if publication.recovery.as_ref().is_some_and(|recovery| {
                    matches!(
                        recovery.terminal(sink),
                        ResidentPublicationTerminal::Refused(_)
                    )
                }) {
                    sink.finish_plugin_state_restore(context, false);
                    publication.quarantine();
                    *pending = Some(publication);
                    return Err(SessionError::PluginPending);
                }
                return match publication.finish_retired_unapplied(
                    session,
                    sink,
                    context,
                    snapshot.key,
                    snapshot.sequence,
                ) {
                    Ok(restored) => {
                        *current = Some(restored);
                        *preview = None;
                        Err(SessionError::PluginSourceLost(
                            "vst3 output closed before the edit was applied".into(),
                        ))
                    }
                    Err(publication) => {
                        *pending = Some(publication);
                        Err(SessionError::PluginPending)
                    }
                };
            }
            return match publication.resolve(session, sink) {
                resident_audio::PluginPublicationResolution::Waiting(publication)
                | resident_audio::PluginPublicationResolution::Indeterminate(publication)
                | resident_audio::PluginPublicationResolution::Conflict(publication) => {
                    *pending = Some(publication);
                    Err(SessionError::PluginPending)
                }
                resident_audio::PluginPublicationResolution::Applied(snapshot) => {
                    let prepared = Arc::clone(&snapshot.prepared);
                    *current = Some(snapshot);
                    *preview = None;
                    Ok(Some((Arc::new(session.project().clone()), prepared)))
                }
                resident_audio::PluginPublicationResolution::Refused(reason) => Err(
                    SessionError::NativeAudio(format!("vst3 publication refused: {reason:?}")),
                ),
            };
        }
        if current.is_none() {
            // Missing canonical preparation does not prove loss of the native
            // instance. Preserve admitted preview custody until its host reports
            // an actual loss or completes restoration/publication.
            return Err(SessionError::Invalid(
                "canonical plugin playback is unavailable",
            ));
        }
        if preview.is_some() {
            return Err(SessionError::Invalid(
                "canonical plugin playback is unavailable",
            ));
        }
        let state = session
            .project()
            .device(device)
            .filter(|device| device.state().type_id() == DeviceTypeId::from_static("hex.vst3"))
            .ok_or_else(|| {
                SessionError::PluginSourceLost("plugin parameter device changed".into())
            })?
            .state();
        let result = sink
            .prepare_plugin_parameter_gesture(PluginParameterGestureInput {
                context,
                operation,
                canonical_state: state,
            })
            .map_err(|error| match error {
                PluginParameterGestureError::Refused(reason) => SessionError::NativeAudio(reason),
                PluginParameterGestureError::SourceLost(reason) => {
                    SessionError::PluginSourceLost(reason)
                }
            })?;
        match (operation, result) {
            (_, PluginParameterGestureResult::Pending) => Err(SessionError::PluginPending),
            (_, PluginParameterGestureResult::StoppedOutputClosed { old_output }) => {
                if let Ok(mut recovery) = session.stage_plugin_state_recovery(
                    context,
                    sink,
                    with_preview(admission, preview.as_ref()),
                    current.as_ref(),
                ) {
                    recovery.unsubmitted_old_output = Some(old_output);
                    *stopped_recovery = Some(recovery);
                }
                Err(SessionError::PluginPending)
            }
            (_, PluginParameterGestureResult::StoppedGraphReady) => {
                if let Ok(recovery) = session.stage_stopped_plugin_recovery(
                    context,
                    sink,
                    with_preview(admission, preview.as_ref()),
                    current.as_ref(),
                ) {
                    *stopped_recovery = Some(recovery);
                }
                // Failure to prepare or admit a successor does not release
                // the sink's stopped-source fence or complete the gesture.
                Err(SessionError::PluginPending)
            }
            (
                PluginGesturePreparation::Commit(_),
                PluginParameterGestureResult::Captured(state),
            ) => {
                if state.type_id() != DeviceTypeId::from_static("hex.vst3") {
                    return Err(SessionError::Invalid("plugin capture changed provider"));
                }
                let token = session.editor.prepare_plugin_state(device, state)?;
                let publication = session.stage_plugin_publication(
                    token,
                    context,
                    sink,
                    with_preview(admission, preview.as_ref()),
                    current.as_ref(),
                )?;
                *pending = Some(publication);
                Err(SessionError::PluginPending)
            }
            (PluginGesturePreparation::Commit(_), _) => Err(SessionError::Invalid(
                "plugin commit requires an actual capture",
            )),
            (_, PluginParameterGestureResult::Applied) => {
                Ok(renewed.map(|prepared| (Arc::new(session.project().clone()), prepared)))
            }
            (_, PluginParameterGestureResult::Captured(_)) => Err(SessionError::Invalid(
                "plugin preview cannot replace canonical state",
            )),
        }
    })();
    if terminal && !matches!(result, Err(SessionError::PluginPending)) {
        sink.finish_plugin_parameter_commit(
            context,
            snapshot.key,
            snapshot.sequence,
            result.is_ok(),
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_gesture::{
        GestureKey, GestureSnapshot, ParameterValue, PluginParameterValue,
    };

    #[derive(Default)]
    struct MissingOutput {
        finished: Vec<(hex_engine::AudioSourceContext, GestureKey, u64, bool)>,
    }
    impl WorkerAudioSink for MissingOutput {
        fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
            Err("no device in this control test".into())
        }
        fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {
            panic!("refused commit cannot publish canonical state");
        }
        fn poll(&mut self) {}
        fn close(&mut self) {}
        fn prepare_plugin_parameter_gesture(
            &mut self,
            _: PluginParameterGestureInput<'_>,
        ) -> Result<PluginParameterGestureResult, PluginParameterGestureError> {
            panic!("missing canonical playback cannot admit sdk work");
        }
        fn finish_plugin_parameter_commit(
            &mut self,
            context: hex_engine::AudioSourceContext,
            key: GestureKey,
            sequence: u64,
            accepted: bool,
        ) {
            self.finished.push((context, key, sequence, accepted));
        }
    }

    #[test]
    fn missing_canonical_owner_refuses_and_delivers_exact_commit_terminal() {
        let mut session = Session::new(48_000, 8, 0).unwrap();
        let hex_project::Created::Track(track) = session
            .editor
            .execute(Command::AddTrack {
                name: "control fixture".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let hex_project::Created::Device(device) = session
            .editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: NativeDeviceState::new(DeviceTypeId::from_static("hex.vst3"), 1, vec![1])
                    .unwrap(),
            })
            .unwrap()
        else {
            panic!("device")
        };
        let before = session.project().clone();
        let context = hex_engine::AudioSourceContext {
            project_generation: 9,
            editor_revision: 3,
        };
        let key = GestureKey {
            generation: 9,
            token: 4,
        };
        let draft = GestureSnapshot {
            key,
            target: GestureTarget::PluginParameter {
                device,
                parameter: 0,
            },
            sequence: 7,
            value: ParameterValue::Float(0.0),
            secondary_value: None,
            parameters: None,
            creation: None,
            plugin_value: Some(PluginParameterValue::new(0.75).unwrap()),
        };
        let mut sink = MissingOutput::default();
        let result = operation(
            &mut session,
            PluginGesturePreparation::Commit(draft),
            context,
            &mut None,
            &mut None,
            &mut sink,
            sampler_export::Admission::default(),
            &mut None,
            &mut None,
        );
        assert!(matches!(result, Err(SessionError::Invalid(_))));
        assert_eq!(session.project(), &before);
        assert_eq!(sink.finished, [(context, key, 7, false)]);
    }

    #[test]
    fn pending_capture_does_not_emit_commit_terminal() {
        struct PendingOutput {
            finishes: usize,
        }
        impl WorkerAudioSink for PendingOutput {
            fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
                Err("control fixture has no device".into())
            }
            fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {
                panic!("pending capture cannot publish canonical state");
            }
            fn poll(&mut self) {}
            fn close(&mut self) {}
            fn prepare_plugin_parameter_gesture(
                &mut self,
                _: PluginParameterGestureInput<'_>,
            ) -> Result<PluginParameterGestureResult, PluginParameterGestureError> {
                Ok(PluginParameterGestureResult::Pending)
            }
            fn finish_plugin_parameter_commit(
                &mut self,
                _: hex_engine::AudioSourceContext,
                _: GestureKey,
                _: u64,
                _: bool,
            ) {
                self.finishes += 1;
            }
        }
        let mut session = Session::new(48_000, 8, 0).unwrap();
        let (hex_project::Created::Track(track), prepared) = session
            .command_prepared_revision(
                Command::AddTrack {
                    name: "pending".into(),
                },
                None,
            )
            .unwrap()
        else {
            panic!("track")
        };
        let resources =
            hex_engine::native_device_resources::PreparedDeviceResources::prepare_on_control(
                &hex_engine::native_device_resources::ResidentDeviceAssets::default(),
                session.project(),
                1_000_000,
                &AtomicBool::new(false),
            )
            .unwrap();
        let hex_project::Created::Device(device) = session
            .editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: NativeDeviceState::new(DeviceTypeId::from_static("hex.vst3"), 1, vec![1])
                    .unwrap(),
            })
            .unwrap()
        else {
            panic!("device")
        };
        let before = session.project().clone();
        let context = hex_engine::AudioSourceContext {
            project_generation: 9,
            editor_revision: 3,
        };
        let draft = GestureSnapshot {
            key: GestureKey {
                generation: 9,
                token: 4,
            },
            target: GestureTarget::PluginParameter {
                device,
                parameter: 0,
            },
            sequence: 7,
            value: ParameterValue::Float(0.0),
            secondary_value: None,
            parameters: None,
            creation: None,
            plugin_value: Some(PluginParameterValue::new(0.75).unwrap()),
        };
        let mut current = Some(resident_audio::Snapshot {
            prepared,
            resources,
            inputs: resident_audio::RuntimeInputs::default(),
            file_sources: None,
        });
        let mut sink = PendingOutput { finishes: 0 };
        let result = operation(
            &mut session,
            PluginGesturePreparation::Commit(draft),
            context,
            &mut current,
            &mut None,
            &mut sink,
            sampler_export::Admission::default(),
            &mut None,
            &mut None,
        );
        assert!(matches!(result, Err(SessionError::PluginPending)));
        assert_eq!(sink.finishes, 0);
        assert_eq!(session.project(), &before);
    }
}
