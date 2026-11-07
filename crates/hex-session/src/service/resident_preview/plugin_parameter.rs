//! Host parameter input stays at the actual sink; canonical opaque commit stays
//! at the ordinary Editor transaction. No transient project or DSP clone is
//! needed to preview a point on the original hosted instance.
use super::*;
use crate::device_gesture::{GestureTarget, PluginGesturePreparation};
use hex_lattice_contract::{DeviceAutomationTarget, DeviceTypeId};
use resident_audio::PluginParameterCommit;

#[allow(clippy::too_many_arguments)] // Same actual canonical/preview/publication owners as device_operation.
pub(super) fn operation(
    session: &mut Session,
    operation: PluginGesturePreparation,
    context: hex_engine::AudioSourceContext,
    current: &mut Option<resident_audio::Snapshot>,
    preview: &mut Option<Preview>,
    sink: &mut dyn WorkerAudioSink,
    admission: sampler_export::Admission<'_>,
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
    let GestureTarget::PluginParameter { device, parameter } = snapshot.target else {
        return Err(SessionError::Invalid("plugin parameter target changed"));
    };
    let terminal = matches!(operation, PluginGesturePreparation::Commit(_));
    let result = (|| {
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
        let source = session
            .project()
            .device(device)
            .filter(|device| device.state().type_id() == DeviceTypeId::from_static("hex.vst3"))
            .ok_or_else(|| {
                SessionError::PluginSourceLost("plugin parameter device changed".into())
            })?;
        let state = source.state();
        let target = DeviceAutomationTarget::Vst3 { parameter };
        let reading_effect_class = source
            .automation_target(target)
            .filter(|lane| lane.is_reading())
            .and_then(|lane| lane.vst3_class)
            .filter(|class| {
                super::super::device_automation_control::accepts_lane_target(
                    session,
                    device,
                    target,
                    Some(*class),
                )
            });
        let result = sink
            .prepare_plugin_parameter_gesture(PluginParameterGestureInput {
                context,
                operation,
                canonical_state: state,
                reading_effect_class,
            })
            .map_err(|error| match error {
                PluginParameterGestureError::Refused(reason) => SessionError::NativeAudio(reason),
                PluginParameterGestureError::SourceLost(reason) => {
                    SessionError::PluginSourceLost(reason)
                }
            })?;
        let commit = match (operation, result) {
            (
                PluginGesturePreparation::Commit(_),
                PluginParameterGestureResult::Captured(mut capture),
            ) => {
                if reading_effect_class.is_some() {
                    return Err(SessionError::Invalid(
                        "reading vst3 parameter requires an isolated override",
                    ));
                }
                let bind = |captured: NativeDeviceState| bind_capture(state, captured);
                capture.before = bind(capture.before.clone())?;
                capture.after = bind(capture.after.clone())?;
                PluginParameterCommit::Captured { device, capture }
            }
            (
                PluginGesturePreparation::Commit(_),
                PluginParameterGestureResult::OverrideCaptured(capture),
            ) => PluginParameterCommit::Override(Box::new(override_command(
                session, context, device, parameter, &capture,
            )?)),
            (PluginGesturePreparation::Commit(_), PluginParameterGestureResult::Applied) => {
                return Err(SessionError::Invalid(
                    "plugin commit requires an actual capture",
                ));
            }
            (_, PluginParameterGestureResult::Applied) => return Ok(None),
            (_, _) => {
                return Err(SessionError::Invalid(
                    "plugin preview cannot replace canonical state",
                ));
            }
        };
        let next = context
            .editor_revision
            .checked_add(1)
            .ok_or(SessionError::Invalid("preview revision exhausted"))?;
        let snapshot = session.plugin_parameter_resident_snapshot(
            commit,
            hex_engine::AudioSourceContext {
                editor_revision: next,
                ..context
            },
            sink,
            with_preview(admission, preview.as_ref()),
            current.as_ref(),
        )?;
        let prepared = Arc::clone(&snapshot.prepared);
        *current = Some(snapshot);
        *preview = None;
        Ok(Some((Arc::new(session.project().clone()), prepared)))
    })();
    if terminal {
        sink.finish_plugin_parameter_commit(
            context,
            snapshot.key,
            snapshot.sequence,
            result.is_ok(),
        );
    }
    result
}

fn bind_capture(
    state: &NativeDeviceState,
    captured: NativeDeviceState,
) -> Result<NativeDeviceState, SessionError> {
    super::super::plugin_state_save::validate_capture(state, &captured)
        .map_err(SessionError::Invalid)?;
    captured
        .with_host_metadata_from(state)
        .map_err(|_| SessionError::Invalid("plugin capture changed provider"))
}

fn override_command(
    session: &Session,
    context: hex_engine::AudioSourceContext,
    device: hex_project::DeviceId,
    parameter: u32,
    capture: &PluginParameterOverride,
) -> Result<Command, SessionError> {
    let target = DeviceAutomationTarget::Vst3 { parameter };
    let class = Some(capture.class);
    if capture.source != context
        || capture.parameter != parameter
        || !super::super::device_automation_control::accepts_lane_target(
            session, device, target, class,
        )
    {
        return Err(SessionError::Invalid("plugin override target changed"));
    }
    let source = session
        .project()
        .device(device)
        .ok_or(SessionError::Invalid("plugin override target changed"))?;
    if source
        .automation_target(target)
        .is_none_or(|lane| !lane.is_reading() || lane.vst3_class != class)
    {
        return Err(SessionError::Invalid(
            "plugin override requires the exact reading lane",
        ));
    }
    Ok(Command::ManualDeviceEdit {
        device,
        state: bind_capture(source.state(), capture.base.clone())?,
        targets: vec![target],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_gesture::{
        GestureKey, GestureSnapshot, ParameterValue, PluginParameterValue,
    };

    fn plugin_state(class: [u8; 16], hash: [u8; 32], payload: &[u8]) -> NativeDeviceState {
        hex_plugin_state::encode(hex_plugin_state::State {
            identity: hex_plugin_state::Identity {
                class: hex_plugin_state::ClassId::new(class).unwrap(),
                binary: hex_plugin_state::BinaryIdentity {
                    executable_sha256: hash,
                    info_sha256: [3; 32],
                },
            },
            component: Some(payload),
            controller: Some(b"editor"),
        })
        .unwrap()
    }

    #[test]
    fn isolated_override_commits_only_the_exact_lane_and_keeps_the_canonical_undo() {
        use hex_project::{
            AutomationInterpolation, AutomationPoint, AutomationReadSelection as Read,
            AutomationValue, BeatTime,
        };
        let class = [1; 16];
        let original = plugin_state(class, [2; 32], b"old base")
            .with_input_gain_millidb(-3000)
            .unwrap();
        let mut session = Session::new(48_000, 8, 0).unwrap();
        let hex_project::Created::Device(device) = session
            .editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: original.clone(),
            })
            .unwrap()
        else {
            panic!("effect")
        };
        let target = DeviceAutomationTarget::Vst3 { parameter: 0 };
        let other = DeviceAutomationTarget::Vst3 { parameter: 7 };
        for (target, value) in [(target, 0.75), (other, 0.5)] {
            session
                .editor
                .execute(Command::ReplaceDeviceAutomationTarget {
                    device,
                    target,
                    points: vec![AutomationPoint {
                        beat: BeatTime::ZERO,
                        value: AutomationValue::new(value).unwrap(),
                        outgoing: AutomationInterpolation::Step,
                    }],
                })
                .unwrap();
        }
        let before = session.project().clone();
        let context = hex_engine::AudioSourceContext {
            project_generation: 1,
            editor_revision: 3,
        };
        let capture = |source, class, parameter, hash| PluginParameterOverride {
            source,
            class,
            parameter,
            base: plugin_state([1; 16], hash, b"isolated manual base"),
        };
        assert!(
            override_command(
                &session,
                context,
                device,
                0,
                &capture(context, [4; 16], 0, [2; 32])
            )
            .is_err()
        );
        assert!(
            override_command(
                &session,
                context,
                device,
                0,
                &capture(context, class, 7, [2; 32])
            )
            .is_err()
        );
        assert!(
            override_command(
                &session,
                context,
                device,
                0,
                &capture(context, class, 0, [4; 32])
            )
            .is_err()
        );
        let stale = hex_engine::AudioSourceContext {
            editor_revision: 2,
            ..context
        };
        assert!(
            override_command(
                &session,
                context,
                device,
                0,
                &capture(stale, class, 0, [2; 32])
            )
            .is_err()
        );
        assert_eq!(session.project(), &before);

        let command = override_command(
            &session,
            context,
            device,
            0,
            &capture(context, class, 0, [2; 32]),
        )
        .unwrap();
        let expected_base = plugin_state(class, [2; 32], b"isolated manual base")
            .with_input_gain_millidb(-3000)
            .unwrap();
        let inspect = |source: &Project, candidate: &Project| {
            assert_eq!(source, &before);
            let previous = source.device(device).unwrap();
            let candidate = candidate.device(device).unwrap();
            assert_eq!(candidate.state(), &expected_base);
            let mut lane = previous.automation_target(target).unwrap().clone();
            lane.read_selection = Read::Base;
            assert_eq!(candidate.automation_target(target), Some(&lane));
            assert_eq!(
                candidate.automation_target(other),
                previous.automation_target(other)
            );
        };
        assert!(
            session
                .editor
                .execute_prepared_transition(command.clone(), |source, candidate| {
                    inspect(source, candidate);
                    Err::<(), _>(hex_project::Invalid("refused publication"))
                })
                .is_err()
        );
        assert_eq!(session.project(), &before);
        session
            .editor
            .execute_prepared_transition(command, |source, candidate| {
                inspect(source, candidate);
                Ok::<_, hex_project::Invalid>(())
            })
            .unwrap();
        let overridden = session.project().clone();
        assert!(session.editor.undo());
        assert_eq!(session.project(), &before);
        assert!(session.editor.redo());
        assert_eq!(session.project(), &overridden);

        let directory =
            std::env::temp_dir().join(format!("hex-vst3-override-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let cleanup = Cleanup(directory);
        let path = cleanup.0.join("override.hex");
        hex_storage::save_new(&path, &overridden).unwrap();
        assert_eq!(hex_storage::load(&path).unwrap(), overridden);
    }

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
            manual_edit: false,
            touched_parameters: Default::default(),
            track_gains: None,
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
        );
        assert!(matches!(result, Err(SessionError::Invalid(_))));
        assert_eq!(session.project(), &before);
        assert_eq!(sink.finished, [(context, key, 7, false)]);
    }
}
