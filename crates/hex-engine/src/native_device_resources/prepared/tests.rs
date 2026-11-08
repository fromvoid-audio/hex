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
#[test]
fn registered_sampler_mixes_streamed_clips_without_losing_ordinary_resource_scope() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| registered_sampler_fixture(true, false))
        .unwrap()
        .join()
        .unwrap();
}
#[test]
fn shared_sampler_has_one_receiver_resource_configuration_and_ordinary_fallback() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| registered_sampler_fixture(false, true))
        .unwrap()
        .join()
        .unwrap();
}
fn registered_sampler_mixes_original_pcm_with_vector_and_same_asset_project_rate_audio_on_control()
{
    registered_sampler_fixture(false, false);
}
fn registered_sampler_fixture(streamed: bool, shared: bool) {
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
    let shared_contributor = shared.then(|| {
        let receiver = editor.project().tracks()[0].id();
        let source = editor
            .project()
            .track(receiver)
            .unwrap()
            .source_id()
            .unwrap();
        let Created::Track(contributor) = editor
            .execute(Command::AddMidiTrack {
                name: "shared contributor".into(),
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
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track: contributor,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("contributor clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 64,
                channel: 0,
                velocity: 127,
            })
            .unwrap();
        contributor
    });
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
    if let Some(contributor) = shared_contributor {
        let receiver = editor.project().tracks()[0].id();
        let source = editor
            .project()
            .track(receiver)
            .unwrap()
            .source_id()
            .unwrap();
        assert_eq!(resources.configurations.len(), 1);
        assert_eq!(resources.configurations[0].track, receiver);
        assert!(resources.lease_on_control(receiver).unwrap().is_some());
        assert!(resources.lease_on_control(contributor).unwrap().is_none());
        assert!(revision.source_routing().has_shared_sources());
        assert_eq!(
            revision.source_routing().receiver(contributor),
            Some(receiver)
        );
        let playback =
            SessionPlayback::new_with_device_resources(Arc::clone(&revision), &resources).unwrap();
        assert_eq!(playback.device_resource_leases().count(), 1);
        let private = ControlSamplerSources::prepare_on_control(
            &store,
            editor.project(),
            20_000_000,
            8,
            &cancel,
        )
        .unwrap();
        assert!(private.has_track(receiver));
        assert!(!private.has_track(contributor));
        // The same singleton sampler qualifies for CURRENT; binding the extra
        // contributor declines only that closed constructor, while the ordinary
        // resource-backed revision above has already succeeded.
        let mut closed = Editor::new(editor.project().clone(), 16).unwrap();
        let unrelated = closed
            .project()
            .tracks()
            .iter()
            .filter(|track| track.id() != receiver && track.id() != contributor)
            .map(|track| track.id())
            .collect::<Vec<_>>();
        for track in unrelated {
            closed.execute(Command::RemoveTrack(track)).unwrap();
        }
        assert!(matches!(
            super::super::current_sampler::validate_project(closed.project()),
            Err(AudioStorageError::UnsupportedGraph)
        ));
        closed.execute(Command::RemoveTrack(contributor)).unwrap();
        assert_eq!(
            super::super::current_sampler::validate_project(closed.project()),
            Ok(true)
        );
        editor
            .execute(Command::SetNativeSourceReceiver {
                source,
                track: Some(contributor),
            })
            .unwrap();
        assert!(resources.validate_project(editor.project()).is_err());
        let moved = PreparedDeviceResources::prepare_on_control(
            &store,
            editor.project(),
            20_000_000,
            &cancel,
        )
        .unwrap();
        assert_eq!(moved.configurations.len(), 1);
        assert_eq!(moved.configurations[0].track, contributor);
        assert!(moved.lease_on_control(receiver).unwrap().is_none());
        assert!(moved.lease_on_control(contributor).unwrap().is_some());
        editor
            .execute(Command::SetNativeSourceReceiver {
                source,
                track: None,
            })
            .unwrap();
        let silent = PreparedDeviceResources::prepare_on_control(
            &store,
            editor.project(),
            20_000_000,
            &cancel,
        )
        .unwrap();
        assert!(silent.configurations.is_empty());
        return;
    }
    assert!(
        SessionPlayback::new(Arc::clone(&revision)).is_err(),
        "resource-free constructor cannot omit custody"
    );
    let foreign =
        PreparedDeviceResources::prepare_on_control(&store, editor.project(), 20_000_000, &cancel)
            .unwrap();
    assert!(SessionPlayback::new_with_device_resources(Arc::clone(&revision), &foreign).is_err());
    if streamed {
        let generation = 17;
        let key = crate::StreamSourceKey::new(generation, asset);
        let canonical = editor.project().asset(asset).unwrap();
        let metadata = Arc::new(
            crate::StreamedSourceMetadata::new(
                key,
                canonical.source_sample_rate(),
                editor.project().sample_rate(),
                canonical.source_frames(),
                canonical.playback_frames(),
                canonical.conversion_version(),
                [0x17; 32],
            )
            .unwrap(),
        );
        let mixed = BTreeMap::from([(
            asset,
            crate::compile::PreparedAudioSource::Streamed(metadata),
        )]);
        let prepare = |generation, scan| {
            PreparedRevision::prepare_with_prepared_audio_sources_and_device_resources(
                editor.project(),
                &mixed,
                generation,
                Arc::clone(&table),
                None,
                &transforms,
                &resources,
                scan,
                &cancel,
            )
        };
        assert!(
            prepare(0, 24).is_err(),
            "zero generation refuses even before source lookup"
        );
        assert!(
            prepare(generation + 1, 24).is_err(),
            "foreign stream generation refuses"
        );
        assert!(
            prepare(generation, 23).is_err(),
            "ordinary factory work bound still applies"
        );
        let mixed = Arc::new(prepare(generation, 24).unwrap());
        assert!(mixed.contains_streamed_sources());
        assert!(mixed.requires_device_resources());
        assert!(SessionPlayback::new(Arc::clone(&mixed)).is_err());
        assert!(SessionPlayback::new_with_device_resources(Arc::clone(&mixed), &foreign).is_err());
        struct Pages<'a> {
            key: crate::StreamSourceKey,
            frames: &'a [crate::Frame],
        }
        impl crate::MediaPages for Pages<'_> {
            fn page(
                &mut self,
                key: crate::StreamSourceKey,
                index: u64,
            ) -> Option<crate::BorrowedMediaPage<'_>> {
                (key == self.key && index == 0).then_some(crate::BorrowedMediaPage {
                    page_index: index,
                    frames: self.frames,
                })
            }
            fn request_page(&mut self, _: crate::StreamSourceKey, _: u64) {
                panic!("fixture page is ready")
            }
        }
        let mut pages = Pages {
            key,
            frames: &derivative,
        };
        let mut expected = crate::playback::Playback::try_from_prepared_with_device_resources(
            Arc::clone(&revision),
            &resources,
        )
        .unwrap();
        let mut actual =
            crate::playback::Playback::try_from_prepared_with_device_resources(mixed, &resources)
                .unwrap();
        let context = crate::AudioSourceContext {
            project_generation: 11,
            editor_revision: 29,
        };
        let action = crate::PlaybackAction::Transport(crate::PlaybackCommand::PlayFrom(0));
        let prepare_action = |runtime: &mut crate::playback::Playback| {
            runtime
                .stream_playback(context)
                .prepare_stream_action(action)
                .unwrap()
        };
        let wrong_context = crate::AudioSourceContext {
            editor_revision: 30,
            ..context
        };
        let prepared = prepare_action(&mut actual);
        let mut wrong = actual.stream_playback(wrong_context);
        assert!(
            wrong
                .visit_prepared_stream_action(&prepared, 8, &mut |_| Ok(()))
                .is_err()
        );
        assert!(wrong.commit_stream_action(prepared).is_err());
        assert!(!actual.playing());
        let prepared = prepare_action(&mut actual);
        assert!(
            expected
                .stream_playback(context)
                .commit_stream_action(prepared)
                .is_err()
        );
        assert!(!actual.playing());
        assert!(!expected.playing());
        // Demand preflight borrows the real ordinary predecessor and full native
        // successor. It does not inherit routes, move graph custody or play.
        let window = actual
            .stream_playback(context)
            .preview_stream_handoff(
                &mut expected.stream_playback(context),
                8,
                Some(true),
                &mut |extent| {
                    assert_eq!(extent.source, key.into());
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(window.frames, 8);
        assert!(!actual.playing());
        assert!(!expected.playing());
        let prepared = prepare_action(&mut actual);
        let mut extents = Vec::new();
        let window = actual
            .stream_playback(context)
            .visit_prepared_stream_action(&prepared, 8, &mut |extent| {
                extents.push(extent);
                Ok(())
            })
            .unwrap();
        assert_eq!(window.frames, 8);
        assert!(!extents.is_empty());
        assert!(extents.iter().all(|extent| extent.source == key.into()));
        actual
            .stream_playback(context)
            .commit_stream_action(prepared)
            .unwrap();
        assert!(actual.playing());
        for runtime in [&mut expected, &mut actual] {
            runtime
                .try_apply_action(crate::PlaybackAction::Transport(
                    crate::PlaybackCommand::Play,
                ))
                .unwrap();
        }
        for frames in [1, 3, 4, 17] {
            let mut resident = vec![[0.0; 2]; frames];
            let mut streamed = vec![[0.0; 2]; frames];
            expected.render(&mut resident).unwrap();
            actual
                .stream_playback(context)
                .render_with_pages_and_signal_observers(&mut streamed, &mut pages, None, None)
                .unwrap();
            assert_eq!(
                streamed, resident,
                "original-rate sampler, vector and converted clip share the ordinary render kernel"
            );
        }
        // Interactive held-note starts use the same original-rate sampler
        // resources and page admission. No private scope or prefix replay.
        for destination in [3, 8_000] {
            let action =
                crate::PlaybackAction::Transport(crate::PlaybackCommand::PlayFrom(destination));
            let plan = actual
                .stream_playback(context)
                .prepare_stream_action(action)
                .unwrap();
            actual
                .stream_playback(context)
                .visit_prepared_stream_action(&plan, 8, &mut |_| Ok(()))
                .unwrap();
            actual
                .stream_playback(context)
                .commit_stream_action(plan)
                .unwrap();
            expected.try_apply_action(action).unwrap();
            let mut resident = [[0.0; 2]; 8];
            let mut streamed = resident;
            expected.render(&mut resident).unwrap();
            actual
                .stream_playback(context)
                .render_with_pages_and_signal_observers(&mut streamed, &mut pages, None, None)
                .unwrap();
            assert_eq!(streamed, resident);
            assert_eq!(actual.position(), destination + 8);
        }
        // Resource custody is independent of page readiness. The ordinary book must
        // admit/retire the scoped graph without treating file extent as resident PCM.
        let mut book = DeviceResourcePublication::reserve_on_control(limits, 8, 8).unwrap();
        let actual = match book.publish_on_control(actual, Ok::<_, ()>) {
            Ok(runtime) => runtime,
            Err(DeviceResourcePublishError::Resources { reason, .. }) => {
                panic!("mixed resource admission: {reason:?}")
            }
            Err(DeviceResourcePublishError::Publication(())) => panic!("unexpected queue refusal"),
        };
        assert_eq!(book.outstanding_leases(), 1);
        let registered = [book.admission_on_control()];
        let phase = DeviceResourcePhase {
            registered: &registered,
            stores: &[],
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
            4,
            "the registered mixed graph retains only the four original sampler PCM frames"
        );
        assert!(book.retire_on_control(Box::new(actual)).is_ok());
        assert_eq!(book.outstanding_leases(), 0);
        return;
    }

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
        let proof = revision.session().mutable_history_retarget(track).unwrap();
        let proof_bytes = std::mem::size_of_val(proof);
        assert_eq!(
            resources.compiler_construction_allowance_bytes().unwrap(),
            std::mem::size_of_val(factory) * 2 + proof_bytes,
            "both factory copies and the actual proof overlap before compilation finishes"
        );
        assert_eq!(
            revision.session().resource_factory_inline_bytes().unwrap(),
            std::mem::size_of_val(factory) + proof_bytes
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
        let mut book = DeviceResourcePublication::reserve_on_control(limits, 8, 8).unwrap();
        let first = crate::playback::Playback::try_from_prepared_with_device_resources(
            Arc::clone(&revision),
            &resources,
        )
        .unwrap();
        let first = match book.publish_on_control(first, Ok::<_, ()>) {
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
        let refused = book.publish_admitted_on_control(next, |candidate, prospective| {
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
            // Two graphs share the same eight-frame derivative; the original is distinct.
            assert_eq!(inventory.retained_frames, 4 + 8);
            assert_eq!(inventory.construction_frames, 4 + 8);
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
            book.admission_on_control(),
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
            4 + 8
        );
        drop(next);
        assert!(book.retire_on_control(Box::new(first)).is_ok());
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
        // Interactive Play-from starts both mutable sampler and FM4 voices
        // empty at the destination. The frame-anchored audio clip keeps its
        // authored position; no instrument prefix history is replayed.
        for (age, actual) in sought_pcm.into_iter().enumerate() {
            let sampler = derivative[age];
            let vector = vector.sample(age as u64, 8000, &hex_lattice::EMPTY_DEVICE_RESOURCES);
            let audio = derivative[age + 3];
            for lane in 0..2 {
                assert!((actual[lane] - (sampler[lane] + vector[lane] + audio[lane])).abs() < 1e-6);
            }
        }
        // No prefix replays, so the replay work budget does not bind.
        let beyond_budget =
            SessionPlayback::prepare_interactive_arrangement_preroll_with_device_resources(
                Arc::clone(&revision),
                &resources,
                3,
                2,
                true,
                &cancel,
            )
            .unwrap();
        assert_eq!(beyond_budget.arrangement_frame(), 3);
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

fn pcm_playback(audio: Audio) -> crate::playback::Playback {
    let clip = crate::Clip::new(audio, 0, 4, false).unwrap();
    let track = crate::Track::new(vec![clip], 1.0, false).unwrap();
    crate::playback::Playback::new(crate::Project::new(48_000, vec![track]).unwrap())
}
fn pcm_limits() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 256,
        handle_rows: 256,
        root_rows: 64,
        max_retained_bytes: 2_000_000,
        max_construction_peak_bytes: 2_000_000,
    }
}
fn pcm_inventory(registered: &[DeviceResourceAdmission<'_>], originals: &[&Audio]) -> u64 {
    let phase = DeviceResourcePhase {
        stores: &[],
        registered,
        selections: &[],
        outbound: &[],
        legacy: originals,
        checked_runtime_bytes: 0,
        construction_bytes: 0,
    };
    let inventory = inspect_device_resource_union(&phase, &phase, pcm_limits()).unwrap();
    assert_eq!(inventory.retained_frames, inventory.construction_frames);
    inventory.retained_frames
}
#[test]
fn pcm_union_counts_capacity_once_across_original_and_unpublished_aliases() {
    let mut frames = Vec::with_capacity(37);
    frames.resize(4, [0.25; 2]);
    let audio = Audio::new(48_000, frames).unwrap();
    let capacity = audio.pcm_capacity_frames() as u64;
    let candidate = pcm_playback(audio.clone());
    let views = [DeviceResourceAdmission::unpublished_on_control(
        &candidate, None,
    )];
    assert_eq!(pcm_inventory(&views, &[&audio, &audio]), capacity);
    // Content equality never deduplicates an independently allocated buffer.
    let distinct = Audio::new(48_000, audio.frames().to_vec()).unwrap();
    assert_eq!(
        pcm_inventory(&views, &[&audio, &distinct]),
        capacity + distinct.pcm_capacity_frames() as u64
    );
}
#[test]
fn pcm_publication_refusal_retirement_and_output_custody_keep_exact_backing_union() {
    let audio = Audio::new(48_000, vec![[0.25; 2]; 4]).unwrap();
    let other = Audio::new(48_000, vec![[0.5; 2]; 4]).unwrap();
    let mut book = DeviceResourcePublication::reserve_on_control(pcm_limits(), 8, 2).unwrap();
    let accept = |result| match result {
        Ok(playback) => playback,
        Err(_) => panic!("publication refused"),
    };
    let first = accept(book.publish_on_control(pcm_playback(audio.clone()), Ok::<_, ()>));
    let second = accept(book.publish_on_control(pcm_playback(audio.clone()), Ok::<_, ()>));
    assert_eq!(pcm_inventory(&[book.admission_on_control()], &[&audio]), 4);
    // Full graph-record capacity refuses before invoking the enqueue closure.
    assert!(matches!(
        book.publish_on_control(pcm_playback(other.clone()), |_| -> Result<(), ()> {
            panic!("enqueue called at capacity")
        }),
        Err(DeviceResourcePublishError::Resources { .. })
    ));
    assert!(
        book.retire_on_control(Box::new(pcm_playback(other.clone())))
            .is_err()
    );
    assert_eq!(book.unreturned_graphs_on_control(), 2);
    assert!(book.retire_on_control(Box::new(first)).is_ok());
    assert_eq!(pcm_inventory(&[book.admission_on_control()], &[]), 4);
    let refused = book.publish_on_control(pcm_playback(other.clone()), Err::<(), _>);
    let Err(DeviceResourcePublishError::Publication(returned)) = refused else {
        panic!("queue refusal")
    };
    assert_eq!(book.unreturned_graphs_on_control(), 1);
    assert_eq!(pcm_inventory(&[book.admission_on_control()], &[]), 4);
    // Retry the exact returned owner; the refused capture committed no record.
    let third = accept(book.publish_on_control(returned, Ok::<_, ()>));
    assert_eq!(pcm_inventory(&[book.admission_on_control()], &[]), 8);
    assert!(book.retire_on_control(Box::new(second)).is_ok());
    assert_eq!(pcm_inventory(&[book.admission_on_control()], &[]), 4);
    assert!(book.retire_on_control(Box::new(third)).is_ok());
    assert_eq!(pcm_inventory(&[book.admission_on_control()], &[]), 0);
}

#[test]
fn pcm_retired_output_book_retains_backing_until_control_releases_custody() {
    let audio = Audio::new(48_000, vec![[0.25; 2]; 4]).unwrap();
    let weak = Arc::downgrade(&audio.frames);
    let mut book = DeviceResourcePublication::reserve_on_control(pcm_limits(), 8, 2).unwrap();
    let graph = match book.publish_on_control(pcm_playback(audio), Ok::<_, ()>) {
        Ok(graph) => graph,
        Err(_) => panic!("publication"),
    };
    // Shutdown can destroy the output graph before exclusive fresh-output custody.
    // Its detached book still accounts for and retains the old PCM on control.
    drop(graph);
    assert!(weak.upgrade().is_some());
    assert_eq!(pcm_inventory(&[book.admission_on_control()], &[]), 4);
    drop(book);
    assert!(weak.upgrade().is_none());
}
