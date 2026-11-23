use super::super::audio_admission::{AdmissionState, AudioAdmissionStatus, ProjectRevision};
use super::*;
#[test]
fn comparison_exact_source_maps_verified_observations_without_status_oscillation_or_old_ack_promotion()
 {
    // Actual worker-minted binding; AppliedAudioSource below is a nonowning
    // protocol fixture, not proof that a native constructor/callback ran.
    let service = hex_session::service::Service::new(48_000, 16, 0).unwrap();
    let deadline = crate::test_wait::hang_deadline();
    let receive = || loop {
        assert!(Instant::now() < deadline, "bounded comparison worker");
        match service.try_event() {
            Ok(event) => break event,
            Err(std::sync::mpsc::TryRecvError::Empty) => std::thread::yield_now(),
            Err(error) => panic!("worker {error}"),
        }
    };
    receive();
    service
        .submit(Request::Edit(hex_project::Command::InsertTrackWithDevice {
            name: "source".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        }))
        .unwrap();
    let initial = receive();
    let device = initial.state.project.tracks()[0].instrument().unwrap();
    service
        .submit(Request::CompareDevice {
            owner: hex_session::service::preset::Owner {
                project_generation: 0,
                revision: initial.state.revision,
                device: device.id(),
                class: hex_engine::NativeDeviceClass::Instrument,
                type_id: device.state().type_id(),
            },
            expected: hex_project::DeviceVariant::A,
            action: hex_project::DeviceComparisonAction::Switch,
        })
        .unwrap();
    let metadata = receive();
    let source = metadata.state.canonical_audio_source.as_ref().unwrap();
    let actual = hex_audio::prepared::AppliedAudioSource {
        context: source.context(),
        audio_revision: 31,
        output_identity: 41,
    };
    let mode = hex_session::DocumentAudioMode::VerifiedStreaming;
    let mut status = AudioAdmissionStatus::default();
    status.canonical_revision(ProjectRevision {
        generation: 0,
        revision: metadata.state.revision,
    });
    assert_eq!(
        reconcile_verified_source_binding(
            &mut status,
            mode,
            Some(41),
            0,
            Some(&metadata.state),
            None
        ),
        (false, None)
    );
    assert_eq!(status.current().unwrap().state, AdmissionState::Pending);
    let (changed, current) = reconcile_verified_source_binding(
        &mut status,
        mode,
        Some(41),
        0,
        Some(&metadata.state),
        Some(actual),
    );
    assert!(changed);
    assert_eq!(current, Some(actual));
    assert_eq!(
        status.current().unwrap().project.revision,
        metadata.state.revision
    );
    assert_eq!(status.current().unwrap().state, AdmissionState::Applied);
    assert_eq!(
        reconcile_verified_source_binding(
            &mut status,
            mode,
            Some(41),
            0,
            Some(&metadata.state),
            Some(actual)
        ),
        (false, Some(actual))
    );
    assert_eq!(
        reconcile_verified_source_binding(
            &mut status,
            mode,
            Some(42),
            0,
            Some(&metadata.state),
            Some(actual)
        ),
        (false, None)
    );
    assert_eq!(
        reconcile_verified_source_binding(
            &mut status,
            mode,
            Some(41),
            1,
            Some(&metadata.state),
            Some(actual)
        ),
        (false, None)
    );
    service
        .submit(Request::Edit(hex_project::Command::Mute {
            track: initial.state.project.tracks()[0].id(),
            muted: true,
        }))
        .unwrap();
    let audio = receive();
    status.canonical_revision(ProjectRevision {
        generation: 0,
        revision: audio.state.revision,
    });
    status.unpublished_refusal(
        ProjectRevision {
            generation: 0,
            revision: audio.state.revision,
        },
        "refused",
    );
    let (_, current) = reconcile_verified_source_binding(
        &mut status,
        mode,
        Some(41),
        0,
        Some(&audio.state),
        Some(actual),
    );
    assert!(current.is_none());
    assert_eq!(
        status.current().unwrap().state,
        AdmissionState::Rejected("refused")
    );
    assert_eq!(
        status.current().unwrap().project.revision,
        audio.state.revision
    );
    let audible = status
        .last_audible()
        .expect("the historical source remains audible");
    assert_eq!(
        audible.project,
        ProjectRevision {
            generation: actual.context.project_generation,
            revision: actual.context.editor_revision,
        }
    );
    assert_eq!(audible.audio_revision, actual.audio_revision);
}
