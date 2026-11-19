//! Real worker/source/history checks; no native admission is inferred.
use super::{event, owner};
use crate::service::{Event, Request, Service, WorkerAudioSink};
use crate::{DocumentAudioMode, SourcePlanReservation};
use hex_engine::NativeDeviceClass;
use hex_project::{Command, DeviceComparisonAction as Action, DeviceVariant as Slot};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

fn run(service: &Service, request: Request) -> Event {
    service.submit(request).unwrap();
    let result = event(service);
    assert!(result.error.is_none(), "{:?}", result.error);
    result
}
fn reservation() -> SourcePlanReservation {
    SourcePlanReservation {
        reader: hex_media::VerifiedWavReaderLimits {
            reader: hex_media::WavReaderLimits {
                max_file_bytes: 64 * 1024 * 1024,
                max_source_frames: 8_000_000,
                max_header_count: 32,
                max_window_frames: 4096,
            },
            max_manifest_bytes: 1024 * 1024,
        },
        max_retained_sources: 64,
        max_path_bytes: 1024 * 1024,
        max_manifest_bytes: 4 * 1024 * 1024,
    }
}
#[test]
fn comparison_metadata_both_modes_save_status_history_keep_exact_preparation_source() {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    for mode in [
        DocumentAudioMode::Resident,
        DocumentAudioMode::VerifiedStreaming,
    ] {
        let folder = std::env::temp_dir().join(format!(
            "hex-ab-source-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&folder).unwrap();
        let service = Service::new(48_000, 32, 16_384).unwrap();
        event(&service);
        run(
            &service,
            Request::NewWithAudioMode {
                mode,
                reservation: (mode == DocumentAudioMode::VerifiedStreaming).then(reservation),
            },
        );
        let wav = folder.join("actual.wav");
        let mut bytes = Vec::from(*b"RIFF");
        bytes.extend_from_slice(&(36u32 + 256).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&96_000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&256u32.to_le_bytes());
        for _ in 0..128 {
            bytes.extend_from_slice(&4000i16.to_le_bytes());
        }
        std::fs::write(&wav, bytes).unwrap();
        run(
            &service,
            Request::ImportToNewTrack {
                path: wav,
                before_track: None,
                start: 0,
            },
        );
        let initial = run(
            &service,
            Request::Edit(Command::InsertTrackWithDevice {
                name: "notes".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            }),
        );
        let device = initial
            .state
            .project
            .tracks()
            .last()
            .unwrap()
            .instrument()
            .unwrap()
            .id();
        let source = initial
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(source.sources().len(), 1, "real prepared media binding");
        let before_comparison_save = run(&service, Request::SaveNew(folder.join("before.hex")));
        assert!(
            Arc::ptr_eq(
                before_comparison_save
                    .state
                    .canonical_audio_source
                    .as_ref()
                    .unwrap(),
                &source
            ),
            "a save before comparison must retain the actual preparation source too"
        );
        let target = crate::service::preset::Owner {
            project_generation: 1,
            ..owner(&initial, device, NativeDeviceClass::Instrument)
        };
        let changed = run(
            &service,
            Request::CompareDevice {
                owner: target,
                expected: Slot::A,
                action: Action::Switch,
            },
        );
        assert!(changed.playback.is_none());
        assert!(changed.state.comparison_audio.as_ref().unwrap().follows(
            &initial.state,
            &changed.state,
            1
        ));
        assert!(!changed.state.comparison_audio.as_ref().unwrap().follows(
            &initial.state,
            &changed.state,
            0
        ));
        let saved = run(&service, Request::SaveNew(folder.join("native.hex")));
        assert!(!saved.state.unsaved);
        let saved_again = run(&service, Request::SaveCurrent);
        let undo = run(&service, Request::Undo);
        assert_eq!(undo.state.project, initial.state.project);
        let redo = run(&service, Request::Redo);
        assert_eq!(redo.state.project, changed.state.project);
        assert!(undo.playback.is_none() && redo.playback.is_none());
        for state in [
            &changed.state,
            &saved.state,
            &saved_again.state,
            &undo.state,
            &redo.state,
        ] {
            assert!(Arc::ptr_eq(
                state.canonical_audio_source.as_ref().unwrap(),
                &source
            ));
            assert_eq!(
                state.audio_authoring_revision(source.context()),
                Some(state.revision)
            );
            let current = hex_engine::AudioSourceContext {
                editor_revision: state.revision,
                ..source.context()
            };
            assert_eq!(
                state.audio_authoring_revision(current),
                Some(state.revision)
            );
            assert_eq!(
                state.audio_authoring_revision(hex_engine::AudioSourceContext {
                    project_generation: current.project_generation + 1,
                    ..current
                }),
                None
            );
            assert_eq!(
                state.audio_authoring_revision(hex_engine::AudioSourceContext {
                    editor_revision: source.context().editor_revision - 1,
                    ..current
                }),
                None
            );
            for intermediate in source.context().editor_revision + 1..state.revision {
                assert_eq!(
                    state.audio_authoring_revision(hex_engine::AudioSourceContext {
                        editor_revision: intermediate,
                        ..current
                    }),
                    None
                );
            }
        }
        // An audible edit must break the equivalence and produce a new source.
        let audio = run(
            &service,
            Request::Edit(Command::Mute {
                track: initial.state.project.tracks().last().unwrap().id(),
                muted: true,
            }),
        );
        assert!(audio.state.comparison_audio.is_none());
        assert!(!Arc::ptr_eq(
            audio.state.canonical_audio_source.as_ref().unwrap(),
            &source
        ));
        assert_eq!(audio.state.audio_authoring_revision(source.context()), None);
        assert!(!changed.state.comparison_audio.as_ref().unwrap().follows(
            &initial.state,
            &audio.state,
            1
        ));
        drop(service);
        std::fs::remove_dir_all(folder).unwrap();
    }
}

struct Sink {
    sources: Arc<Mutex<Vec<Arc<crate::service::CanonicalAudioSourceSnapshot>>>>,
    polls: Arc<AtomicUsize>,
}
impl WorkerAudioSink for Sink {
    fn initialize(&mut self, _: &AtomicBool) -> Result<u32, String> {
        Ok(48_000)
    }
    fn canonical_changed(&mut self, source: Arc<crate::service::CanonicalAudioSourceSnapshot>) {
        self.sources.lock().unwrap().push(source);
    }
    fn poll(&mut self) {
        self.polls.fetch_add(1, Ordering::Relaxed);
    }
    fn close(&mut self) {}
}
#[test]
fn comparison_metadata_event_backpressure_keeps_native_source_and_order_without_extra_preparation()
{
    let sources = Arc::new(Mutex::new(Vec::new()));
    let polls = Arc::new(AtomicUsize::new(0));
    let service = Service::with_audio_sink(
        32,
        0,
        None,
        Box::new(Sink {
            sources: sources.clone(),
            polls: polls.clone(),
        }),
    )
    .unwrap();
    event(&service);
    let initial = run(
        &service,
        Request::Edit(Command::InsertTrackWithDevice {
            name: "held".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        }),
    );
    let device = initial.state.project.tracks()[0].instrument().unwrap().id();
    let count = sources.lock().unwrap().len();
    service
        .submit(Request::CompareDevice {
            owner: owner(&initial, device, NativeDeviceClass::Instrument),
            expected: Slot::A,
            action: Action::Switch,
        })
        .unwrap();
    // Two metadata history events fill the existing queue. No extra queue or
    // source notification is introduced; the blocked event stays owned there.
    service.submit(Request::Undo).unwrap();
    service.submit(Request::Redo).unwrap();
    let before = polls.load(Ordering::Relaxed);
    let deadline = std::time::Instant::now() + crate::test_wait::HANG_BOUND;
    while polls.load(Ordering::Relaxed) <= before + 3 {
        assert!(std::time::Instant::now() < deadline);
        crate::test_wait::pause();
    }
    assert_eq!(sources.lock().unwrap().len(), count);
    let mut previous = initial.state;
    for _ in 0..3 {
        let current = event(&service);
        assert!(current.error.is_none());
        assert!(current.playback.is_none());
        assert!(current.state.comparison_audio.as_ref().unwrap().follows(
            &previous,
            &current.state,
            0
        ));
        previous = current.state;
    }
    assert_eq!(sources.lock().unwrap().len(), count);
}
