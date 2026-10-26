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
            (
                PluginGesturePreparation::Commit(_),
                PluginParameterGestureResult::Captured(state),
            ) => {
                if state.type_id() != DeviceTypeId::from_static("hex.vst3") {
                    return Err(SessionError::Invalid("plugin capture changed provider"));
                }
                commit_preview(
                    session,
                    Command::ReplaceDeviceState { device, state },
                    context,
                    current,
                    preview,
                    sink,
                    admission,
                )
            }
            (PluginGesturePreparation::Commit(_), _) => Err(SessionError::Invalid(
                "plugin commit requires an actual capture",
            )),
            (_, PluginParameterGestureResult::Applied) => Ok(None),
            (_, PluginParameterGestureResult::Captured(_)) => Err(SessionError::Invalid(
                "plugin preview cannot replace canonical state",
            )),
        }
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
        );
        assert!(matches!(result, Err(SessionError::Invalid(_))));
        assert_eq!(session.project(), &before);
        assert_eq!(sink.finished, [(context, key, 7, false)]);
    }
}
