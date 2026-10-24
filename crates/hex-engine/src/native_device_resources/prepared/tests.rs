use super::*;
use crate::{
    Audio, AudioStorageLedgerReservation, PreparedRevision, SessionPlayback,
    SessionPlaybackCommand, SineTable,
};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference};
use hex_project::{BeatTime, Command, Created, Editor};
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn registered_sampler_mixes_original_pcm_with_vector_and_same_asset_project_rate_audio() {
    // Match the existing bounded production seek-worker stack, including debug-build temporaries.
    let run = registered_sampler_mixes_original_pcm_with_vector_and_same_asset_project_rate_audio_on_control;
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(run)
        .unwrap()
        .join()
        .unwrap();
}
fn registered_sampler_mixes_original_pcm_with_vector_and_same_asset_project_rate_audio_on_control()
{
    let cancel = AtomicBool::new(false);
    let mut editor = Editor::new(Project::new(16000).unwrap(), 16).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            path: "source.wav".into(),
            sample_rate: 8000,
            frames: 4,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let registry = hex_devices::native_instrument_registry().unwrap();
    let vector_state = registry
        .default_state(hex_devices::fm4::device::FM4_TYPE_ID)
        .unwrap();
    let mut sampler_state =
        hex_devices::sampler::state::SamplerState::for_source_frames(4).unwrap();
    sampler_state.mode = hex_devices::sampler::state::AuthoredMode::OneShot {
        gate: hex_devices::sampler::GateMode::Trigger,
    };
    sampler_state.envelope = hex_devices::sampler::Envelope {
        attack_seconds: 0.0,
        decay_seconds: 0.0,
        sustain: 1.0,
        release_seconds: 0.0,
    };
    let sampler_state = sampler_state
        .to_native()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(asset.value()).unwrap(),
        }])
        .unwrap();
    for (name, state) in [("sampler", sampler_state), ("vector", vector_state.clone())] {
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack { name: name.into() })
            .unwrap()
        else {
            panic!("midi track")
        };
        editor
            .execute(Command::InsertInstrument { track, state })
            .unwrap();
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("midi clip")
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
    let Created::Track(audio_track) = editor
        .execute(Command::AddTrack {
            name: "audio".into(),
        })
        .unwrap()
    else {
        panic!("audio track")
    };
    editor
        .execute(Command::PlaceClip {
            track: audio_track,
            asset,
            start: 0,
            duration: 8,
            source_offset: 0,
            source_length: 8,
            looping: false,
        })
        .unwrap();
    let limits = AudioStorageLedgerReservation {
        data_rows: 256,
        handle_rows: 64,
        root_rows: 32,
        max_retained_bytes: 20_000_000,
        max_construction_peak_bytes: 20_000_000,
    };
    let original = vec![[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0, 0.0]];
    let mut store = ResidentDeviceAssets::default();
    store
        .insert_on_control(
            (
                asset,
                crate::OwnedAudioSource::try_resident(8000, original)
                    .ok()
                    .unwrap(),
            ),
            0,
            limits,
            4,
            &cancel,
            || Ok::<_, ResourcePreparationError>(()),
        )
        .unwrap();
    let resources =
        PreparedDeviceResources::prepare_on_control(&store, editor.project(), 20_000_000, &cancel)
            .unwrap();
    let derivative = [
        [0.125, -0.25],
        [0.3125, -0.125],
        [0.5, 0.0],
        [0.1875, 0.125],
        [-0.125, 0.25],
        [-0.0625, 0.125],
        [0.0, 0.0],
        [0.0, 0.0],
    ];
    let media = BTreeMap::from([(asset, Audio::new(16000, derivative.to_vec()).unwrap())]);
    let table = Arc::new(SineTable::new());
    let transforms = crate::audio_transform::AudioTransformPreparation::default();
    let prepare = |scope: &PreparedDeviceResources, work| {
        PreparedRevision::prepare_with_device_resources(
            editor.project(),
            &media,
            Arc::clone(&table),
            None,
            &transforms,
            scope,
            work,
            &cancel,
        )
    };
    assert!(
        prepare(&resources, 23).is_err(),
        "three actual two-scan factories require 24 source frames"
    );
    assert_eq!(
        resources
            .compiler_scan_work_on_control(editor.project())
            .unwrap(),
        24
    );
    let revision = Arc::new(prepare(&resources, 24).unwrap());
    assert!(
        SessionPlayback::new(Arc::clone(&revision)).is_err(),
        "resource-free constructor cannot omit custody"
    );
    let foreign =
        PreparedDeviceResources::prepare_on_control(&store, editor.project(), 20_000_000, &cancel)
            .unwrap();
    assert!(SessionPlayback::new_with_device_resources(Arc::clone(&revision), &foreign).is_err());
    assert!(
        PreparedDeviceResources::prepare_on_control(
            &store,
            editor.project(),
            resources.metadata_bytes().unwrap() - 1,
            &cancel
        )
        .is_err()
    );
    {
        // Current/staged configuration handles, active runtime anchors and prospective anchors
        // share one actual original bank. Separate clip PCM remains a conservative allowance.
        let track = editor.project().tracks()[0].id();
        let factory = revision
            .session()
            .mutable_instrument_factory(track)
            .unwrap();
        let bound = resources.factory_inline_bound(track).unwrap();
        assert_eq!(
            inline_resource_factory_bytes(factory, bound).unwrap(),
            std::mem::size_of_val(factory)
        );
        for declared in [
            None,
            Some(std::mem::size_of::<FactoryDeclaration<'_>>() + 1),
        ] {
            let bad = FactoryDeclaration {
                original: factory,
                declared,
            };
            assert_eq!(
                inline_resource_factory_bytes(&bad, usize::MAX),
                Err(AudioStorageError::UnknownStorage)
            );
        }
        assert!(inline_resource_factory_bytes(factory, bound - 1).is_err());
        let mut book = DeviceResourcePublication::reserve_on_control(limits, 8).unwrap();
        let first = crate::playback::Playback::try_from_prepared_with_device_resources(
            Arc::clone(&revision),
            &resources,
        )
        .unwrap();
        let first = match book.publish_on_control(first, 8, Ok::<_, ()>) {
            Ok(first) => first,
            Err(DeviceResourcePublishError::Resources { reason, .. }) => {
                panic!("initial publication: {reason:?}")
            }
            Err(DeviceResourcePublishError::Publication(())) => panic!("unexpected queue refusal"),
        };
        let next_revision = Arc::new(prepare(&foreign, 24).unwrap());
        let next = crate::playback::Playback::try_from_prepared_with_device_resources(
            Arc::clone(&next_revision),
            &foreign,
        )
        .unwrap();
        let refused = book.publish_admitted_on_control(next, 16, |candidate, prospective| {
            let sets = [&resources, &foreign];
            let revisions = [revision.as_ref(), next_revision.as_ref()];
            let registered = [
                DeviceResourceAdmission::new(&sets, &[]).with_prepared_revisions(&revisions),
                prospective,
            ];
            let stores = [&store];
            let phase = DeviceResourcePhase {
                registered: &registered,
                stores: &stores,
                selections: &[],
                outbound: &[],
                legacy: &[],
                checked_runtime_bytes: 0,
                construction_bytes: 0,
            };
            let inventory = inspect_device_resource_union(&phase, &phase, limits).unwrap();
            assert_eq!(inventory.retained_frames, 4 + 16);
            assert_eq!(inventory.construction_frames, 4 + 16);
            let too_small = AudioStorageLedgerReservation {
                max_retained_bytes: inventory.retained.retained_bytes - 1,
                ..limits
            };
            assert!(inspect_device_resource_union(&phase, &phase, too_small).is_err());
            Err::<(), _>(candidate)
        });
        let DeviceResourcePublishError::Publication(next) = refused.err().unwrap() else {
            panic!("resource preparation failed before full union admission")
        };
        assert!(Arc::ptr_eq(
            next.prepared_revision().unwrap(),
            &next_revision
        ));
        assert_eq!(
            book.outstanding_leases(),
            1,
            "refusal issues no prospective obligation"
        );
        // An unavailable-output/seek worker owns its exact unpublished candidate and a captured
        // resource set, until replacement/publication or actual worker-side retirement.
        let captured = foreign.capture_on_control(20_000_000).unwrap();
        let sets = [&resources, &foreign];
        let revisions = [revision.as_ref()];
        let registered = [
            DeviceResourceAdmission::new(&sets, &[]).with_prepared_revisions(&revisions),
            book.admission_on_control(8),
            DeviceResourceAdmission::unpublished_on_control(&next, Some(&captured)),
        ];
        let stores = [&store];
        let phase = DeviceResourcePhase {
            registered: &registered,
            stores: &stores,
            selections: &[],
            outbound: &[],
            legacy: &[],
            checked_runtime_bytes: 0,
            construction_bytes: 0,
        };
        assert_eq!(
            inspect_device_resource_union(&phase, &phase, limits)
                .unwrap()
                .retained_frames,
            4 + 16
        );
        drop(next);
        book.retire_on_control(Box::new(first));
        assert_eq!(book.outstanding_leases(), 0);
    }
    let source_weak = Arc::downgrade(store.assets[0].1.owner.arc());
    drop(foreign);
    {
        let runtime =
            SessionPlayback::prepare_interactive_arrangement_preroll_with_device_resources(
                Arc::clone(&revision),
                &resources,
                3,
                3,
                true,
                &cancel,
            )
            .unwrap();
        let mut sought =
            crate::playback::Playback::from_interactive_arrangement_preroll(runtime).unwrap();
        assert_eq!(sought.position(), 3);
        let mut sought_pcm = [[0.0; 2]; 3];
        sought.render(&mut sought_pcm).unwrap();
        let vector = registry
            .prepare_instrument(
                &vector_state,
                &DevicePreparationContext {
                    sample_rate: 16000,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap()
            .prepare_note(60, 127)
            .unwrap();
        for (offset, actual) in sought_pcm.into_iter().enumerate() {
            let age = offset + 3;
            let sample = derivative[age];
            let vector = vector.sample(age as u64, 8000, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            for lane in 0..2 {
                assert!((actual[lane] - (sample[lane] + vector[lane] + sample[lane])).abs() < 1e-6);
            }
        }
        assert!(
            SessionPlayback::prepare_interactive_arrangement_preroll_with_device_resources(
                Arc::clone(&revision),
                &resources,
                3,
                2,
                true,
                &cancel
            )
            .is_err()
        );
    }
    let mut playback =
        SessionPlayback::new_with_device_resources(Arc::clone(&revision), &resources).unwrap();
    {
        let mut keyboard = crate::playback::Playback::try_from_prepared_with_device_resources(
            Arc::clone(&revision),
            &resources,
        )
        .unwrap();
        let input = Arc::new(crate::computer_keyboard::ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 127, 1))).unwrap();
        keyboard
            .prepare_computer_keyboard(editor.project(), editor.project().tracks()[0].id(), input)
            .unwrap();
        let mut monitored = [[0.0; 2]; 8];
        keyboard.render(&mut monitored).unwrap();
        assert_eq!(
            monitored, derivative,
            "stopped keyboard borrows the same original-rate source, without Arrangement clips"
        );
    }
    let vector = registry
        .prepare_instrument(
            &vector_state,
            &DevicePreparationContext {
                sample_rate: 16000,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
            },
        )
        .unwrap()
        .prepare_note(60, 127)
        .unwrap();
    drop(resources);
    drop(store);
    assert!(
        source_weak.upgrade().is_some(),
        "actual mutable runtime retains its original bank"
    );
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 12];
    playback.render(&mut output).unwrap();
    for (frame, actual) in output.iter().enumerate() {
        let sample = derivative.get(frame).copied().unwrap_or([0.0; 2]);
        let vector = vector.sample(frame as u64, 8000, &hex_lattice::EMPTY_DEVICE_RESOURCES);
        for lane in 0..2 {
            assert!(
                (actual[lane] - (sample[lane] + vector[lane] + sample[lane])).abs() < 1e-6,
                "frame {frame} lane {lane}: {}",
                actual[lane]
            );
        }
    }
    drop(playback);
    assert!(
        source_weak.upgrade().is_none(),
        "last runtime source owner is destroyed on control"
    );
    cancel.store(true, Ordering::Relaxed);
    assert!(
        PreparedDeviceResources::prepare_on_control(
            &ResidentDeviceAssets::default(),
            editor.project(),
            20_000_000,
            &cancel
        )
        .is_err()
    );
}

/// Preserve a real registered factory's runtime law while injecting an unknown/mismatched
/// immutable-storage declaration at the actual resource-admission validation boundary.
struct FactoryDeclaration<'a> {
    original: &'a dyn hex_lattice::PreparedMutableInstrumentRuntime,
    declared: Option<usize>,
}
impl hex_lattice::PreparedMutableInstrumentRuntime for FactoryDeclaration<'_> {
    fn resource_factory_inline_bytes(&self) -> Option<usize> {
        self.declared
    }
    fn runtime_version(&self) -> u32 {
        self.original.runtime_version()
    }
    fn declared_twin_bytes(&self) -> usize {
        self.original.declared_twin_bytes()
    }
    fn max_events_per_block(&self) -> usize {
        self.original.max_events_per_block()
    }
    fn max_active_occurrences(&self) -> usize {
        self.original.max_active_occurrences()
    }
    fn max_release_tail_frames(&self) -> u64 {
        self.original.max_release_tail_frames()
    }
    fn create_runtime(&self) -> Box<dyn hex_lattice::MutableInstrumentRuntime> {
        self.original.create_runtime()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self.original.as_any()
    }
}
