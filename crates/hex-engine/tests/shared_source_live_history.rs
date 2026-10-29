//! Shared source handoff across refusal and live undo/redo, using public playback contracts.
use hex_engine::audio_transform::AudioTransformPreparation;
use hex_engine::native_device_resources::{
    PreparedDeviceResources, ResidentDeviceAssets, ResourcePreparationError,
};
use hex_engine::session_playback::{SessionPlayback, SessionPlaybackCommand, SessionPlaybackError};
use hex_engine::{
    AudioStorageLedgerReservation, Frame, OwnedAudioSource, PreparedRevision, SineTable,
};
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, NativeDeviceMetadata,
};
use hex_project::{BeatTime, Command, Created, Editor, TrackId};
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::AtomicBool},
};
struct Fixture {
    editor: Editor,
    store: ResidentDeviceAssets,
    receiver: TrackId,
    contributor: TrackId,
    source: hex_project::SourceId,
}
const PCM: [Frame; 4] = [[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.25, 0.125]];
fn fixture() -> Fixture {
    let mut editor = Editor::new(hex_project::Project::new(8000).unwrap(), 64).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            path: "held.wav".into(),
            sample_rate: 8000,
            frames: 4,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Track(receiver) = editor
        .execute(Command::AddMidiTrack {
            name: "receiver".into(),
        })
        .unwrap()
    else {
        panic!("receiver")
    };
    let mut state = hex_devices::sampler::state::SamplerState::for_source_frames(4).unwrap();
    state.mode = hex_devices::sampler::state::AuthoredMode::Classic {
        loop_enabled: true,
        loop_region: state.region,
    };
    state.envelope = hex_devices::sampler::Envelope {
        attack_seconds: 0.0,
        decay_seconds: 0.0,
        sustain: 1.0,
        release_seconds: 0.0,
    };
    let state = state
        .to_native()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(asset.value()).unwrap(),
        }])
        .unwrap();
    editor
        .execute(Command::InsertInstrument {
            track: receiver,
            state,
        })
        .unwrap();
    let source = editor
        .project()
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let Created::Track(contributor) = editor
        .execute(Command::AddMidiTrack {
            name: "contributor".into(),
        })
        .unwrap()
    else {
        panic!("contributor")
    };
    editor
        .execute(Command::BindTrackSource {
            track: contributor,
            source: Some(source),
        })
        .unwrap();
    for track in [receiver, contributor] {
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 127,
            })
            .unwrap();
    }
    editor
        .execute(Command::Gain {
            track: receiver,
            gain: 0.25,
        })
        .unwrap();
    editor
        .execute(Command::Gain {
            track: contributor,
            gain: 0.75,
        })
        .unwrap();
    let mut store = ResidentDeviceAssets::default();
    store
        .insert_on_control(
            (
                asset,
                OwnedAudioSource::try_resident(8000, PCM.to_vec())
                    .ok()
                    .unwrap(),
            ),
            0,
            AudioStorageLedgerReservation {
                data_rows: 256,
                handle_rows: 64,
                root_rows: 32,
                max_retained_bytes: 20_000_000,
                max_construction_peak_bytes: 20_000_000,
            },
            4,
            &AtomicBool::new(false),
            || Ok::<_, ResourcePreparationError>(()),
        )
        .unwrap();
    Fixture {
        editor,
        store,
        receiver,
        contributor,
        source,
    }
}

fn playback(f: &Fixture) -> SessionPlayback {
    let cancel = AtomicBool::new(false);
    let resources = PreparedDeviceResources::prepare_on_control(
        &f.store,
        f.editor.project(),
        20_000_000,
        &cancel,
    )
    .unwrap();
    let revision = PreparedRevision::prepare_with_device_resources(
        f.editor.project(),
        &BTreeMap::new(),
        Arc::new(SineTable::new()),
        None,
        &AudioTransformPreparation::default(),
        &resources,
        resources
            .compiler_scan_work_on_control(f.editor.project())
            .unwrap(),
        &cancel,
    )
    .unwrap();
    SessionPlayback::new_with_device_resources(Arc::new(revision), &resources).unwrap()
}
fn check_pcm(playback: &mut SessionPlayback, start: usize, count: usize, gain: f32) {
    let mut output = vec![[0.0; 2]; count];
    for chunk in output.chunks_mut(3) {
        playback.render(chunk).unwrap();
    }
    for (index, frame) in output.iter().enumerate() {
        for channel in 0..2 {
            assert!(
                (frame[channel] - PCM[(start + index) % 4][channel] * gain).abs() < 0.000_002,
                "start={start} index={index} frame={frame:?} gain={gain}"
            );
        }
    }
}
fn started(f: &Fixture) -> SessionPlayback {
    let mut old = playback(f);
    old.apply(SessionPlaybackCommand::Play).unwrap();
    check_pcm(&mut old, 0, 7, 0.5);
    old
}
fn on_control(run: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(run)
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn shared_provider_refusal_then_live_undo_redo_preserves_donor_pcm() {
    on_control(|| {
        let mut f = fixture();
        let mut donor = started(&f);
        let device = f
            .editor
            .project()
            .native_source(f.source)
            .unwrap()
            .instrument()
            .id();
        let replacement = hex_devices::operator_state::OPERATOR_METADATA
            .default_state()
            .unwrap();
        f.editor
            .execute(Command::ReplaceDeviceState {
                device,
                state: replacement,
            })
            .unwrap();
        let mut refused = playback(&f);
        assert_eq!(
            refused.try_inherit_state_from(&mut donor),
            Err(SessionPlaybackError::SourceHistoryUnavailable)
        );
        assert_eq!(donor.performance_frame(), 7);
        check_pcm(&mut donor, 7, 19, 0.5);

        assert!(f.editor.undo());
        let mut restored = playback(&f);
        restored.try_inherit_state_from(&mut donor).unwrap();
        assert_eq!(restored.performance_frame(), 26);
        assert_eq!(
            f.editor
                .project()
                .native_source(f.source)
                .unwrap()
                .instrument()
                .id(),
            device
        );
        assert_eq!(
            f.editor
                .project()
                .source_contributors(f.source)
                .map(|track| track.id())
                .collect::<Vec<_>>(),
            [f.receiver, f.contributor]
        );
        check_pcm(&mut restored, 26, 11, 0.5);

        assert!(f.editor.redo());
        // Redo restores the same unsupported candidate; retrying it also
        // checks that the first refused preparation left it reusable.
        assert_eq!(
            refused.try_inherit_state_from(&mut restored),
            Err(SessionPlaybackError::SourceHistoryUnavailable)
        );
        assert_eq!(restored.performance_frame(), 37);
        assert_eq!(
            f.editor
                .project()
                .native_source(f.source)
                .unwrap()
                .instrument()
                .id(),
            device
        );
        assert_eq!(
            f.editor
                .project()
                .source_contributors(f.source)
                .map(|track| track.id())
                .collect::<Vec<_>>(),
            [f.receiver, f.contributor]
        );
        check_pcm(&mut restored, 37, 17, 0.5);
    });
}
