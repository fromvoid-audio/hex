//! History preparation refuses before commit at the same resource/hook boundary.
use super::*;
struct Sink {
    prepared: bool,
}
impl WorkerAudioSink for Sink {
    fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
        Ok(16000)
    }
    fn canonical_changed(&mut self, _: Arc<CanonicalAudioSourceSnapshot>) {}
    fn prepare_sampler_edit(&mut self, input: NativeSamplerInput<'_>) -> Result<(), String> {
        self.prepared = true;
        assert!(
            input
                .project
                .tracks()
                .iter()
                .any(|track| track.instrument().is_some_and(|device| {
                    hex_engine::sampler_parameter_value_on_control(
                        hex_engine::SamplerParameter::Gain,
                        device.state(),
                    ) == Ok(1.0)
                }))
        );
        Err("native preparation refused".into())
    }
    fn poll(&mut self) {}
    fn close(&mut self) {}
}
#[test]
fn sampler_history_tight_admission_and_native_hook_refusal_preserve_project_edge_and_overview() {
    let files = Files::new(true);
    let (mut session, original) = direct_sampler(&files);
    let device = session.project().tracks()[0].instrument().unwrap();
    let id = device.id();
    let state = hex_engine::patch_sampler_parameter_on_control(
        hex_engine::SamplerParameter::Gain,
        device.state(),
        0.5,
    )
    .unwrap();
    let (_, changed) = session
        .command_service_snapshot(
            Command::ReplaceDeviceState { device: id, state },
            Admission {
                current: Some(original.selection()),
                ..Default::default()
            },
        )
        .unwrap();
    drop(original);
    let before = session.project().clone();
    let overview = session.source_overviews();
    let mut sink = Sink { prepared: false };
    let context = hex_engine::AudioSourceContext {
        project_generation: 0,
        editor_revision: 1,
    };
    for tight in [true, false] {
        let admission = Admission {
            current: Some(changed.selection()),
            native: NativeSamplerReservationHold {
                frames: if tight { 1024 } else { 0 },
                ..Default::default()
            },
            ..Default::default()
        };
        let result = session.history_service_snapshot_with_native(
            false,
            admission,
            (context, &mut sink, &AtomicBool::new(false)),
        );
        let error = result.err().unwrap().to_string();
        assert!(
            error.contains(if tight {
                "frame limit"
            } else {
                "native preparation refused"
            }),
            "{error}"
        );
        assert_eq!(sink.prepared, !tight);
        assert_eq!(session.project(), &before);
        assert!(session.editor.can_undo());
        assert!(!session.editor.can_redo());
        assert!(session.source_overviews().shares_storage_with(&overview));
        assert!(
            session
                .editor
                .inspect_history(
                    false,
                    super::super::super::sampler_parameter::validate_history_pair
                )
                .unwrap()
                .is_ok()
        );
    }
}
#[test]
fn sampler_history_empty_is_immediate_without_native_audio_and_cancellation_still_validates() {
    let service = Service::new(16000, 16, 1024).unwrap();
    let before = event(&service);
    let context = before
        .state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .context();
    for cancelled in [false, true] {
        service
            .submit(Request::SamplerEdit(crate::service::SamplerEdit::History(
                SamplerHistoryEdit {
                    request_id: 7,
                    project_generation: context.project_generation,
                    revision: before.state.revision,
                    redo: false,
                    cancel: Arc::new(AtomicBool::new(cancelled)),
                },
            )))
            .unwrap();
        let result = event(&service);
        assert_eq!(result.error.is_some(), cancelled);
        assert_eq!(result.sampler_edit_complete, Some(7));
        assert_eq!(result.state.revision, before.state.revision);
        assert_eq!(result.state.project, before.state.project);
        assert!(!result.state.can_undo && !result.state.can_redo);
    }
}

#[test]
fn sampler_history_refusal_never_finalizes_an_unrelated_active_gesture() {
    use crate::device_gesture::{GestureKey, GestureRequest, GestureTarget, ParameterValue};
    let service = Service::new(16000, 16, 1024).unwrap();
    event(&service);
    let before = edit(
        &service,
        Command::AddMidiTrack {
            name: "other".into(),
        },
    );
    let Some(Created::Track(track)) = before.created else {
        panic!("track")
    };
    let key = GestureKey {
        generation: 0,
        token: 1,
    };
    service
        .submit(Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::TrackGain { track },
        }))
        .unwrap();
    assert!(event(&service).gesture_ack.unwrap().accepted);
    service
        .submit(Request::DeviceGesture(GestureRequest::Change {
            key,
            sequence: 1,
            value: ParameterValue::Float(0.5),
        }))
        .unwrap();
    let preview = event(&service);
    assert!(preview.active_device_gesture.is_some());
    service
        .submit(Request::SamplerEdit(crate::service::SamplerEdit::History(
            SamplerHistoryEdit {
                request_id: 7,
                project_generation: 0,
                revision: before.state.revision,
                redo: false,
                cancel: Arc::new(AtomicBool::new(false)),
            },
        )))
        .unwrap();
    let refused = event(&service);
    assert!(refused.error.as_ref().unwrap().contains("active gesture"));
    assert_eq!(refused.sampler_edit_complete, Some(7));
    assert_eq!(refused.state.revision, before.state.revision);
    assert_eq!(refused.state.project, before.state.project);
    assert!(refused.active_device_gesture.is_some());
    assert!(!refused.state.can_redo);
    service
        .submit(Request::DeviceGesture(GestureRequest::Cancel { key }))
        .unwrap();
    assert_eq!(event(&service).state.project, before.state.project);
}
