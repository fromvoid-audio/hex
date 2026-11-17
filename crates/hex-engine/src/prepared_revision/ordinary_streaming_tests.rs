//! Actual registered seven-frame processor, mixed file metadata and resident
//! oracle. Page access is limited to the engine's predicted source keys. Native
//! catalog/endpoint custody is exercised separately by hex-audio's real-file tests.
use super::*;
use crate::playback::Playback;
use crate::{
    BorrowedMediaPage, Frame, LaunchQuantization, MediaPages, PlaybackAction, PlaybackCommand,
    SessionPlaybackCommand, StreamSourceKey, StreamedSourceMetadata,
};
use std::collections::BTreeMap;

#[test]
fn pitch_revision_keeps_session_source_raw_and_refuses_mismatched_pages() {
    use crate::native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets};
    use hex_project::{Command, Created, Editor, Project, WarpResampler};
    use std::sync::atomic::AtomicBool;

    const FRAMES: u64 = 20_000_000;
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { track, clip, asset } = editor
        .execute(Command::ImportAudioTrack {
            name: "long".into(),
            path: "long.wav".into(),
            sample_rate: 48_000,
            frames: FRAMES,
        })
        .unwrap()
    else {
        panic!("audio fixture")
    };
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 700 })
        .unwrap();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene fixture")
    };
    editor
        .execute(Command::CreateSessionAudioClip {
            track,
            scene,
            name: "raw".into(),
            asset,
            source_start: 0,
            source_end: 4096,
        })
        .unwrap();
    let raw = StreamedSourceMetadata::new(
        StreamSourceKey::new(7, asset),
        48_000,
        48_000,
        FRAMES,
        FRAMES,
        hex_project::IDENTITY_CONVERSION,
        [7; 32],
    )
    .unwrap();
    let derived = raw.with_plain_pitch(700, WarpResampler::V2).unwrap();
    let sources = BTreeMap::from([(asset, PreparedAudioSource::Streamed(Arc::new(raw.clone())))]);
    let cancel = AtomicBool::new(false);
    let resources = PreparedDeviceResources::prepare_on_control(
        &ResidentDeviceAssets::default(),
        editor.project(),
        1 << 20,
        &cancel,
    )
    .unwrap();
    let effects = hex_devices::native_registry().unwrap();
    let transforms = crate::audio_transform::AudioTransformPreparation::default();
    let prepare = |pitched: &BTreeMap<_, _>| {
        PreparedRevision::prepare_with_prepared_pitch_sources_device_resources_and_effect_registry(
            editor.project(),
            &sources,
            pitched,
            7,
            Arc::new(SineTable::new()),
            None,
            &transforms,
            &resources,
            0,
            &cancel,
            &effects,
            None,
        )
    };
    assert!(prepare(&BTreeMap::new()).is_err());
    let wrong = StreamedSourceMetadata::new(
        raw.key(),
        48_000,
        48_000,
        FRAMES,
        FRAMES,
        hex_project::IDENTITY_CONVERSION,
        [8; 32],
    )
    .unwrap()
    .with_plain_pitch(700, WarpResampler::V2)
    .unwrap();
    assert!(prepare(&BTreeMap::from([(derived.key(), Arc::new(wrong))])).is_err());
    let revision = prepare(&BTreeMap::from([(
        derived.key(),
        Arc::new(derived.clone()),
    )]))
    .unwrap();
    assert_eq!(
        revision
            .arrangement_stream_sources()
            .map(|s| s.key())
            .collect::<Vec<_>>(),
        vec![derived.key()]
    );
    let PreparedAudioSource::Streamed(session_source) =
        revision.session().audio_clips()[0].source()
    else {
        panic!("streamed session source")
    };
    assert_eq!(session_source.as_ref(), &raw);
    let semantic = |pitched: &BTreeMap<_, _>| {
        PreparedRevision::prepare_with_prepared_pitch_sources(
            editor.project(),
            &sources,
            pitched,
            7,
            Arc::new(SineTable::new()),
            None,
        )
    };
    assert!(semantic(&BTreeMap::new()).is_err());
    let semantic = semantic(&BTreeMap::from([(
        derived.key(),
        Arc::new(derived.clone()),
    )]))
    .unwrap();
    assert_eq!(
        semantic
            .arrangement_stream_sources()
            .map(|source| source.key())
            .collect::<Vec<_>>(),
        vec![derived.key()]
    );
    let PreparedAudioSource::Streamed(session_source) =
        semantic.session().audio_clips()[0].source()
    else {
        panic!("streamed session source")
    };
    assert_eq!(session_source.as_ref(), &raw);
}
struct Pages<'a> {
    samples: &'a [Frame],
    key: StreamSourceKey,
    extents: &'a [crate::StreamResourceExtent],
}

#[test]
fn fenced_retained_restart_preview_matches_committed_stream_demand() {
    use hex_project::{Command, Created, Project, TrackDeviceSlot};

    let mut f = crate::signal_graph::tests::fixture(&[&[]], 1024);
    let Created::Track(receiver) = f
        .editor
        .execute(Command::InsertTrackWithDevice {
            name: "receiver".into(),
            before_track: None,
            slot: TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("receiver")
    };
    let source = f
        .editor
        .project()
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let Created::Track(peer) = f
        .editor
        .execute(Command::AddMidiTrack {
            name: "peer".into(),
        })
        .unwrap()
    else {
        panic!("peer")
    };
    f.editor
        .execute(Command::BindTrackSource {
            track: peer,
            source: Some(source),
        })
        .unwrap();
    let original = f.editor.project().clone();
    let mut snapshot = original.snapshot();
    snapshot.retained_source_paths.push(receiver.value());
    let retained = Project::from_snapshot(snapshot).unwrap();
    let asset = *f.media.keys().next().unwrap();
    let key = StreamSourceKey::new(31, asset);
    let sources = BTreeMap::from([(
        asset,
        PreparedAudioSource::Streamed(Arc::new(
            StreamedSourceMetadata::new(
                key,
                48_000,
                48_000,
                1024,
                1024,
                hex_project::IDENTITY_CONVERSION,
                [31; 32],
            )
            .unwrap(),
        )),
    )]);
    let effects = hex_devices::native_registry().unwrap();
    let instruments = hex_devices::native_instrument_registry().unwrap();
    let prepare = |project: &Project| {
        Arc::new(
            PreparedRevision::prepare_with_source_bank(
                project,
                crate::compile::SourceBank::Mixed(&sources, 31),
                Arc::new(SineTable::new()),
                None,
                &effects,
                &instruments,
                &crate::audio_transform::AudioTransformPreparation::default(),
                None,
                None,
                &[],
            )
            .unwrap(),
        )
    };
    let context = crate::AudioSourceContext {
        project_generation: 9,
        editor_revision: 2,
    };
    let mut old = Playback::from_prepared(prepare(&original));
    old.apply(PlaybackCommand::Play);
    let mut old_extents = Vec::new();
    old.stream_playback(context)
        .visit_next_stream_chunk(32, None, &mut |extent| {
            old_extents.push(extent);
            Ok(())
        })
        .unwrap();
    old.stream_playback(context)
        .render_with_pages_and_signal_observers(
            &mut [[0.0; 2]; 32],
            &mut Pages {
                samples: &f.samples,
                key,
                extents: &old_extents,
            },
            None,
            None,
        )
        .unwrap();
    let mut next = Playback::from_prepared(prepare(&retained));
    assert!(matches!(
        next.stream_playback(context)
            .preview_ordinary_fenced_restart_with_report(
                &mut old.stream_playback(context),
                32,
                None,
                &mut |_| Ok(())
            ),
        Err(crate::PlaybackHandoffError::RuntimeFenceChanged)
    ));
    assert!(next.request_live_restart());
    assert!(
        next.stream_playback(context)
            .preview_stream_handoff(&mut old.stream_playback(context), 32, None, &mut |_| Ok(()))
            .is_err()
    );
    let position = old.position();
    let mut predicted = Vec::new();
    let (window, report) = next
        .stream_playback(context)
        .preview_ordinary_fenced_restart_with_report(
            &mut old.stream_playback(context),
            32,
            None,
            &mut |extent| {
                predicted.push(extent);
                Ok(())
            },
        )
        .unwrap();
    assert!(report.running_audio_restart());
    assert_eq!(old.position(), position);
    assert_eq!(window.frames, 32);
    assert!(!predicted.is_empty());
    next.try_restart_runtime_under_arrangement_fence(&mut old, Some(9), Some(9))
        .unwrap();
    let mut committed = Vec::new();
    next.stream_playback(context)
        .visit_next_stream_chunk(32, None, &mut |extent| {
            committed.push(extent);
            Ok(())
        })
        .unwrap();
    assert_eq!(predicted, committed);
    assert_eq!(next.position(), position);
}

#[test]
fn track_limiter_batches_stream_demand_across_loop_without_changing_pcm() {
    let mut f = crate::signal_graph::tests::fixture(&[&[]], 1024);
    let asset = *f.media.keys().next().unwrap();
    let key = StreamSourceKey::new(31, asset);
    let sources = BTreeMap::from([(
        asset,
        PreparedAudioSource::Streamed(Arc::new(
            StreamedSourceMetadata::new(
                key,
                48_000,
                48_000,
                1024,
                1024,
                hex_project::IDENTITY_CONVERSION,
                [31; 32],
            )
            .unwrap(),
        )),
    )]);
    f.editor
        .execute(hex_project::Command::SetLoopRegion {
            start: 0,
            end: 127,
            enabled: true,
        })
        .unwrap();
    f.editor
        .execute(hex_project::Command::InsertDevice {
            track: f.tracks[0],
            before: None,
            state: crate::default_native_device_state_for_class(
                crate::NativeDeviceClass::Effect,
                crate::DeviceTypeId::from_static("hex.limiter"),
            )
            .unwrap(),
        })
        .unwrap();
    let effects = hex_devices::native_registry().unwrap();
    let instruments = hex_devices::native_instrument_registry().unwrap();
    let streamed = Arc::new(
        PreparedRevision::prepare_with_source_bank(
            f.editor.project(),
            crate::compile::SourceBank::Mixed(&sources, 31),
            Arc::new(SineTable::new()),
            None,
            &effects,
            &instruments,
            &crate::audio_transform::AudioTransformPreparation::default(),
            None,
            None,
            &[],
        )
        .unwrap(),
    );
    let resident = Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &f.media,
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut actual = Playback::from_prepared(streamed);
    let mut oracle = Playback::from_prepared(resident);
    for playback in [&mut actual, &mut oracle] {
        playback.apply(PlaybackCommand::Play);
    }
    let context = crate::AudioSourceContext {
        project_generation: 9,
        editor_revision: 2,
    };
    for count in [64, 63, 17, 257, 512] {
        let before_projection = (actual.position(), actual.performance_frame());
        let mut projected_windows = Vec::new();
        let mut ahead = 0;
        while ahead < count {
            let mut demand = Vec::new();
            let width = actual
                .stream_playback(context)
                .visit_current_stream_quantum_at(ahead as u64, count - ahead, None, &mut |row| {
                    demand.push(row);
                    Ok(())
                })
                .unwrap();
            assert_eq!(width, (count - ahead).min(256));
            projected_windows.push(demand);
            ahead += width;
        }
        assert_eq!(
            (actual.position(), actual.performance_frame()),
            before_projection
        );
        let mut expected = vec![[0.0; 2]; count];
        oracle.render(&mut expected).unwrap();
        let mut observed = vec![[0.0; 2]; count];
        let mut offset = 0;
        let mut projected_end = 0;
        let mut predicted = Vec::new();
        let mut committed = Vec::new();
        let mut projected_index = 0;
        while offset < count {
            if offset == projected_end {
                predicted.clear();
                committed.clear();
                let before = (actual.position(), actual.performance_frame());
                let projected = actual
                    .stream_playback(context)
                    .visit_current_stream_quantum(count - offset, None, &mut |extent| {
                        predicted.push(extent);
                        Ok(())
                    })
                    .unwrap();
                assert_eq!(projected, (count - offset).min(256));
                assert_eq!((actual.position(), actual.performance_frame()), before);
                projected_end = offset + projected;
            }
            let position = actual.position();
            let mut extents = Vec::new();
            let window = actual
                .stream_playback(context)
                .visit_next_stream_chunk(projected_end - offset, None, &mut |extent| {
                    extents.push(extent);
                    Ok(())
                })
                .unwrap();
            assert_eq!(
                window.frames,
                (projected_end - offset).min(127 - position as usize)
            );
            assert_eq!(
                extents,
                [crate::StreamResourceExtent {
                    source: key.into(),
                    page_index: 0,
                    start: position as usize,
                    end: position as usize + window.frames,
                }]
            );
            committed.extend_from_slice(&extents);
            actual
                .stream_playback(context)
                .render_with_pages_and_signal_observers(
                    &mut observed[offset..offset + window.frames],
                    &mut Pages {
                        samples: &f.samples,
                        key,
                        extents: &extents,
                    },
                    None,
                    None,
                )
                .unwrap();
            offset += window.frames;
            if offset == projected_end {
                assert_eq!(
                    predicted, committed,
                    "callback projection differs from committed chunks"
                );
                assert_eq!(projected_windows[projected_index], committed);
                projected_index += 1;
            }
        }
        assert_eq!(observed, expected);
        assert_eq!(
            (actual.position(), actual.performance_frame()),
            (oracle.position(), oracle.performance_frame())
        );
    }
}
impl MediaPages for Pages<'_> {
    fn page(&mut self, key: StreamSourceKey, index: u64) -> Option<BorrowedMediaPage<'_>> {
        assert!(
            self.extents
                .iter()
                .any(|extent| extent.source == key.into() && extent.page_index == index)
        );
        (key == self.key && index == 0).then_some(BorrowedMediaPage {
            page_index: 0,
            frames: self.samples,
        })
    }
    fn request_page(&mut self, _: StreamSourceKey, _: u64) {
        panic!("all demanded source pages are installed")
    }
}

#[test]
fn ordinary_positive_latency_demand_actions_and_handoff_match_resident_without_relaxing_constructor_scope()
 {
    let mut f = crate::signal_graph::tests::fixture(&[&[], &[7]], 1024);
    let asset = *f.media.keys().next().unwrap();
    let hex_project::Created::Scene(scene) = f
        .editor
        .execute(hex_project::Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let hex_project::Created::SessionClip(clip) = f
        .editor
        .execute(hex_project::Command::CreateSessionAudioClip {
            track: f.tracks[0],
            scene,
            name: "source".into(),
            asset,
            source_start: 0,
            source_end: 1024,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    let key = StreamSourceKey::new(31, asset);
    let sources: BTreeMap<_, _> = f
        .media
        .iter()
        .map(|(id, audio)| {
            (
                *id,
                if *id == asset {
                    PreparedAudioSource::Streamed(Arc::new(
                        StreamedSourceMetadata::new(
                            key,
                            48_000,
                            48_000,
                            1024,
                            1024,
                            hex_project::IDENTITY_CONVERSION,
                            [31; 32],
                        )
                        .unwrap(),
                    ))
                } else {
                    PreparedAudioSource::Resident(audio.clone())
                },
            )
        })
        .collect();
    let definition = crate::signal_graph::tests::Definition;
    let definitions: [&dyn hex_lattice::NativeDeviceDefinition; 1] = [&definition];
    let effects = hex_lattice::NativeDeviceRegistry::new(&definitions).unwrap();
    let instruments = hex_devices::native_instrument_registry().unwrap();
    let streamed = Arc::new(
        PreparedRevision::prepare_with_source_bank(
            f.editor.project(),
            crate::compile::SourceBank::Mixed(&sources, 31),
            Arc::new(SineTable::new()),
            None,
            &effects,
            &instruments,
            &crate::audio_transform::AudioTransformPreparation::default(),
            None,
            None,
            &[],
        )
        .unwrap(),
    );
    let resident = crate::signal_graph::tests::prepare(&f).unwrap();
    let mut closed = crate::SessionPlayback::new(Arc::clone(&streamed)).unwrap();
    closed.apply(SessionPlaybackCommand::Play).unwrap();
    assert!(
        closed
            .visit_next_stream_chunk(8, None, &mut |_| Ok(()))
            .is_err(),
        "constructor demand must retain its closed-family refusal"
    );
    let mut offline =
        crate::SessionPlayback::new_finite_arrangement_bounce(Arc::clone(&streamed), None, 512)
            .unwrap();
    let mut offline_oracle =
        crate::SessionPlayback::new_finite_arrangement_bounce(Arc::clone(&resident), None, 512)
            .unwrap();
    offline.apply(SessionPlaybackCommand::Play).unwrap();
    offline_oracle.apply(SessionPlaybackCommand::Play).unwrap();
    for count in [1, 3, 17, 256] {
        let mut extents = Vec::new();
        let window = offline
            .visit_next_stream_chunk(count, None, &mut |extent| {
                extents.push(extent);
                Ok(())
            })
            .unwrap();
        assert_eq!(window.frames, count);
        let mut observed = vec![[0.0; 2]; count];
        let mut expected = observed.clone();
        offline
            .render_with_pages(
                &mut observed,
                &mut Pages {
                    samples: &f.samples,
                    key,
                    extents: &extents,
                },
            )
            .unwrap();
        offline_oracle.render(&mut expected).unwrap();
        assert_eq!(observed, expected);
    }
    let context = crate::AudioSourceContext {
        project_generation: 9,
        editor_revision: 2,
    };
    let mut actual = Playback::from_prepared(Arc::clone(&streamed));
    let mut oracle = Playback::from_prepared(Arc::clone(&resident));
    for runtime in [&mut actual, &mut oracle] {
        runtime
            .try_apply_action(PlaybackAction::Transport(PlaybackCommand::Play))
            .unwrap();
    }
    for action in [
        None,
        Some(PlaybackAction::Session(
            SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            },
        )),
        Some(PlaybackAction::Session(
            SessionPlaybackCommand::BackToArrangement {
                quantization: LaunchQuantization::Immediate,
            },
        )),
    ] {
        if let Some(action) = action {
            let old = (actual.position(), actual.performance_frame());
            let prepared = actual
                .stream_playback(context)
                .prepare_stream_action(action)
                .unwrap();
            let window = actual
                .stream_playback(context)
                .visit_prepared_stream_action(&prepared, 32, &mut |_| Ok(()))
                .unwrap();
            assert_eq!(window.frames, 32);
            assert_eq!((actual.position(), actual.performance_frame()), old);
            actual
                .stream_playback(context)
                .commit_stream_action(prepared)
                .unwrap();
            oracle.try_apply_action(action).unwrap();
        }
        for count in [1, 3, 17, 257] {
            let mut expected = vec![[0.0; 2]; count];
            oracle.render(&mut expected).unwrap();
            let mut observed = vec![[0.0; 2]; count];
            let mut offset = 0;
            while offset < count {
                let clock = (actual.position(), actual.performance_frame());
                let mut extents = Vec::new();
                let window = actual
                    .stream_playback(context)
                    .visit_next_stream_chunk(count - offset, None, &mut |extent| {
                        extents.push(extent);
                        Ok(())
                    })
                    .unwrap();
                assert_eq!(
                    window.frames,
                    (count - offset).min(256),
                    "demand may cover a bounded transaction while routing remains sample-accurate"
                );
                assert_eq!((actual.position(), actual.performance_frame()), clock);
                actual
                    .stream_playback(context)
                    .render_with_pages_and_signal_observers(
                        &mut observed[offset..offset + window.frames],
                        &mut Pages {
                            samples: &f.samples,
                            key,
                            extents: &extents,
                        },
                        None,
                        None,
                    )
                    .unwrap();
                offset += window.frames;
            }
            assert_eq!(observed, expected);
            assert_eq!(
                (actual.position(), actual.performance_frame()),
                (oracle.position(), oracle.performance_frame())
            );
        }
    }
    let mut successor = Playback::from_prepared(streamed);
    let mut resident_successor = Playback::from_prepared(resident);
    let clock = (actual.position(), actual.performance_frame());
    let (window, report) = successor
        .stream_playback(context)
        .preview_stream_handoff_with_report(
            &mut actual.stream_playback(context),
            32,
            None,
            &mut |_| Ok(()),
        )
        .unwrap();
    assert!(!report.running_audio_restart());
    assert_eq!(window.frames, 32);
    assert_eq!((actual.position(), actual.performance_frame()), clock);
    successor.try_inherit_runtime_from(&mut actual).unwrap();
    resident_successor
        .try_inherit_runtime_from(&mut oracle)
        .unwrap();
    let mut expected = [[0.0; 2]; 16];
    resident_successor.render(&mut expected).unwrap();
    let mut observed = expected;
    for offset in 0..observed.len() {
        let mut extents = Vec::new();
        successor
            .stream_playback(context)
            .visit_next_stream_chunk(1, None, &mut |extent| {
                extents.push(extent);
                Ok(())
            })
            .unwrap();
        successor
            .stream_playback(context)
            .render_with_pages_and_signal_observers(
                &mut observed[offset..offset + 1],
                &mut Pages {
                    samples: &f.samples,
                    key,
                    extents: &extents,
                },
                None,
                None,
            )
            .unwrap();
    }
    assert_eq!(observed, expected);
}

#[test]
fn main_only_latency_and_retired_main_history_keep_batched_ordinary_stream_demand() {
    let mut f = crate::signal_graph::tests::fixture(&[&[]], 1024);
    let asset = *f.media.keys().next().unwrap();
    let key = StreamSourceKey::new(31, asset);
    let sources = BTreeMap::from([(
        asset,
        PreparedAudioSource::Streamed(Arc::new(
            StreamedSourceMetadata::new(
                key,
                48_000,
                48_000,
                1024,
                1024,
                hex_project::IDENTITY_CONVERSION,
                [31; 32],
            )
            .unwrap(),
        )),
    )]);
    f.editor
        .execute(hex_project::Command::SetLoopRegion {
            start: 0,
            end: 127,
            enabled: true,
        })
        .unwrap();
    let state = crate::default_native_device_state_for_class(
        crate::NativeDeviceClass::Effect,
        crate::DeviceTypeId::from_static("hex.limiter"),
    )
    .unwrap();
    let hex_project::Created::Device(device) = f
        .editor
        .execute(hex_project::Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("main limiter");
    };
    let effects = hex_devices::native_registry().unwrap();
    let instruments = hex_devices::native_instrument_registry().unwrap();
    let prepare = |project: &hex_project::Project| {
        Arc::new(
            PreparedRevision::prepare_with_source_bank(
                project,
                crate::compile::SourceBank::Mixed(&sources, 31),
                Arc::new(SineTable::new()),
                None,
                &effects,
                &instruments,
                &crate::audio_transform::AudioTransformPreparation::default(),
                None,
                None,
                &[],
            )
            .unwrap(),
        )
    };
    let mut actual = Playback::from_prepared(prepare(f.editor.project()));
    let mut oracle = Playback::from_prepared(Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &f.media,
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    ));
    for runtime in [&mut actual, &mut oracle] {
        runtime.apply(PlaybackCommand::Play);
    }
    let context = crate::AudioSourceContext {
        project_generation: 9,
        editor_revision: 2,
    };
    for removed in [false, true] {
        if removed {
            f.editor
                .execute(hex_project::Command::RemoveDevice(device))
                .unwrap();
            let mut successor = Playback::from_prepared(prepare(f.editor.project()));
            let mut resident = Playback::from_prepared(Arc::new(
                PreparedRevision::prepare(
                    f.editor.project(),
                    &f.media,
                    Arc::new(SineTable::new()),
                    None,
                )
                .unwrap(),
            ));
            successor.try_inherit_runtime_from(&mut actual).unwrap();
            resident.try_inherit_runtime_from(&mut oracle).unwrap();
            actual = successor;
            oracle = resident;
        }
        // Finish with queued Limiter output, so removal must carry its real
        // history and content-origin stamps rather than merely a fresh graph.
        for count in [64, 63, 17, 257] {
            let mut expected = vec![[0.0; 2]; count];
            oracle.render(&mut expected).unwrap();
            let mut observed = vec![[0.0; 2]; count];
            let mut offset = 0;
            while offset < count {
                let clock = (actual.position(), actual.performance_frame());
                let mut extents = Vec::new();
                let window = actual
                    .stream_playback(context)
                    .visit_next_stream_chunk(count - offset, None, &mut |extent| {
                        extents.push(extent);
                        Ok(())
                    })
                    .unwrap();
                assert_eq!(
                    window.frames,
                    (count - offset).min(256).min(127 - clock.0 as usize),
                    "Main-only latency must not begin a whole render transaction per sample; removed={removed}"
                );
                assert_eq!((actual.position(), actual.performance_frame()), clock);
                assert_eq!(
                    extents,
                    [crate::StreamResourceExtent {
                        source: key.into(),
                        page_index: 0,
                        start: clock.0 as usize,
                        end: clock.0 as usize + window.frames,
                    }]
                );
                actual
                    .stream_playback(context)
                    .render_with_pages_and_signal_observers(
                        &mut observed[offset..offset + window.frames],
                        &mut Pages {
                            samples: &f.samples,
                            key,
                            extents: &extents,
                        },
                        None,
                        None,
                    )
                    .unwrap();
                offset += window.frames;
            }
            assert_eq!(observed, expected, "removed={removed}");
            assert_eq!(
                (actual.position(), actual.performance_frame()),
                (oracle.position(), oracle.performance_frame())
            );
        }
    }
}

#[test]
fn closed_group_latency_stream_demand_matches_resident() {
    use hex_project::{Command, Created, TrackSelectionIds};
    for on_group in [false, true] {
        let mut f = crate::signal_graph::tests::fixture(&[&[], &[]], 1024);
        let asset = *f.media.keys().next().unwrap();
        let key = StreamSourceKey::new(41, asset);
        let Created::Track(group) = f
            .editor
            .execute(Command::GroupTracks {
                tracks: TrackSelectionIds::new(&f.tracks).unwrap(),
                name: "group".into(),
            })
            .unwrap()
        else {
            panic!("group")
        };
        let state = crate::default_native_device_state_for_class(
            crate::NativeDeviceClass::Effect,
            crate::DeviceTypeId::from_static("hex.limiter"),
        )
        .unwrap();
        f.editor
            .execute(Command::InsertDevice {
                track: if on_group { group } else { f.tracks[0] },
                before: None,
                state,
            })
            .unwrap();
        f.editor
            .execute(Command::SetLoopRegion {
                start: 0,
                end: 127,
                enabled: true,
            })
            .unwrap();
        let mut sources: BTreeMap<_, _> = f
            .media
            .iter()
            .map(|(id, audio)| (*id, PreparedAudioSource::Resident(audio.clone())))
            .collect();
        sources.insert(
            asset,
            PreparedAudioSource::Streamed(Arc::new(
                StreamedSourceMetadata::new(
                    key,
                    48_000,
                    48_000,
                    1024,
                    1024,
                    hex_project::IDENTITY_CONVERSION,
                    [41; 32],
                )
                .unwrap(),
            )),
        );
        let streamed = Arc::new(
            PreparedRevision::prepare_with_source_bank(
                f.editor.project(),
                crate::compile::SourceBank::Mixed(&sources, 41),
                Arc::new(SineTable::new()),
                None,
                &hex_devices::native_registry().unwrap(),
                &hex_devices::native_instrument_registry().unwrap(),
                &crate::audio_transform::AudioTransformPreparation::default(),
                None,
                None,
                &[],
            )
            .unwrap(),
        );
        let resident = Arc::new(
            PreparedRevision::prepare(
                f.editor.project(),
                &f.media,
                Arc::new(SineTable::new()),
                None,
            )
            .unwrap(),
        );
        for partition in [1, 257] {
            let mut actual = crate::SessionPlayback::new(Arc::clone(&streamed)).unwrap();
            let mut oracle = crate::SessionPlayback::new(Arc::clone(&resident)).unwrap();
            actual.apply(SessionPlaybackCommand::Play).unwrap();
            oracle.apply(SessionPlaybackCommand::Play).unwrap();
            for step in 0..3 {
                if step == 1 {
                    let token = actual
                        .prepare_stream_action(PlaybackAction::Transport(PlaybackCommand::Seek(17)))
                        .unwrap();
                    let before = (actual.performance_frame(), actual.arrangement_frame());
                    let mut demand = Vec::new();
                    actual
                        .visit_prepared_stream_action(&token, 32, &mut |row| {
                            demand.push(row);
                            Ok(())
                        })
                        .unwrap();
                    assert!(!demand.is_empty());
                    assert_eq!(
                        (actual.performance_frame(), actual.arrangement_frame()),
                        before
                    );
                    actual.apply(SessionPlaybackCommand::Seek(17)).unwrap();
                    oracle.apply(SessionPlaybackCommand::Seek(17)).unwrap();
                } else if step == 2 {
                    let mut next = crate::SessionPlayback::new(Arc::clone(&streamed)).unwrap();
                    let mut next_oracle =
                        crate::SessionPlayback::new(Arc::clone(&resident)).unwrap();
                    next.preview_stream_handoff(&mut actual, 32, None, &mut |_| Ok(()))
                        .unwrap();
                    next.try_inherit_state_from(&mut actual).unwrap();
                    next_oracle.try_inherit_state_from(&mut oracle).unwrap();
                    actual = next;
                    oracle = next_oracle;
                }
                let mut expected = vec![[0.0; 2]; 514];
                for block in expected.chunks_mut(partition) {
                    oracle.render(block).unwrap();
                }
                let mut observed = vec![[0.0; 2]; expected.len()];
                let mut offset = 0;
                while offset < observed.len() {
                    let mut extents = Vec::new();
                    let window = actual
                        .visit_next_stream_chunk(
                            (observed.len() - offset).min(partition),
                            None,
                            &mut |row| {
                                extents.push(row);
                                Ok(())
                            },
                        )
                        .unwrap();
                    assert!(window.frames > 0);
                    actual
                        .render_with_pages(
                            &mut observed[offset..offset + window.frames],
                            &mut Pages {
                                samples: &f.samples,
                                key,
                                extents: &extents,
                            },
                        )
                        .unwrap();
                    offset += window.frames;
                }
                assert_eq!(
                    observed, expected,
                    "on_group={on_group} partition={partition} step={step}"
                );
            }
        }
    }
}

#[test]
fn closed_flat_feed_latency_stream_demand_matches_resident() {
    feed_stream_demand_matches_resident(FeedTopology::Flat, true);
}

#[test]
fn closed_flat_feed_zero_latency_stream_demand_matches_resident() {
    feed_stream_demand_matches_resident(FeedTopology::Flat, false);
}

#[derive(Clone, Copy)]
enum FeedTopology {
    Flat,
    Siblings,
    GroupSource,
    GroupDestination,
    ReturnSource,
}

#[test]
fn closed_grouped_sibling_feed_latency_matches_resident() {
    feed_stream_demand_matches_resident(FeedTopology::Siblings, true);
}
#[test]
fn closed_grouped_sibling_feed_zero_latency_matches_resident() {
    feed_stream_demand_matches_resident(FeedTopology::Siblings, false);
}
#[test]
fn closed_group_source_feed_latency_matches_resident() {
    feed_stream_demand_matches_resident(FeedTopology::GroupSource, true);
}
#[test]
fn closed_group_source_feed_zero_latency_matches_resident() {
    feed_stream_demand_matches_resident(FeedTopology::GroupSource, false);
}
#[test]
fn closed_group_destination_feed_latency_matches_resident() {
    feed_stream_demand_matches_resident(FeedTopology::GroupDestination, true);
}
#[test]
fn closed_group_destination_feed_zero_latency_matches_resident() {
    feed_stream_demand_matches_resident(FeedTopology::GroupDestination, false);
}
#[test]
fn closed_return_feed_latency_matches_resident() {
    feed_stream_demand_matches_resident(FeedTopology::ReturnSource, true);
}
#[test]
fn closed_return_feed_zero_latency_matches_resident() {
    feed_stream_demand_matches_resident(FeedTopology::ReturnSource, false);
}

fn feed_stream_demand_matches_resident(topology: FeedTopology, limiter: bool) {
    use hex_project::{Command, Created, TrackSelectionIds};
    for limiter_on_receiver in [false, true] {
        if !limiter && limiter_on_receiver {
            continue;
        }
        for stream_index in 0..2 {
            let mut f = crate::signal_graph::tests::fixture(&[&[], &[]], 1024);
            let asset = *f.media.keys().nth(stream_index).unwrap();
            let key = StreamSourceKey::new(41, asset);
            let source = f.tracks[1];
            let destination = f.tracks[0];
            let group_tracks = match topology {
                FeedTopology::Flat | FeedTopology::ReturnSource => None,
                FeedTopology::Siblings => Some(vec![source, destination]),
                FeedTopology::GroupSource => Some(vec![source]),
                FeedTopology::GroupDestination => Some(vec![destination]),
            };
            let group = group_tracks.map(|tracks| {
                let Created::Track(group) = f
                    .editor
                    .execute(Command::GroupTracks {
                        tracks: TrackSelectionIds::new(&tracks).unwrap(),
                        name: "group".into(),
                    })
                    .unwrap()
                else {
                    panic!("group")
                };
                group
            });
            let source = if matches!(topology, FeedTopology::GroupSource) {
                group.unwrap()
            } else {
                source
            };
            let destination = if matches!(topology, FeedTopology::GroupDestination) {
                group.unwrap()
            } else {
                destination
            };
            let mut return_bus = None;
            let feed_source = if matches!(topology, FeedTopology::ReturnSource) {
                let Created::ReturnTrack(bus) = f
                    .editor
                    .execute(Command::AddReturnTrack {
                        name: "return".into(),
                    })
                    .unwrap()
                else {
                    panic!("return")
                };
                f.editor
                    .execute(Command::SetReturnSend {
                        track: source,
                        return_track: bus,
                        gain: 0.5,
                    })
                    .unwrap();
                return_bus = Some(bus);
                hex_project::RoutingOwner::Return(bus)
            } else {
                hex_project::RoutingOwner::Track(source)
            };
            f.editor
                .execute(Command::SetAudioFeed {
                    source: feed_source,
                    destination,
                    gain: 0.5,
                })
                .unwrap();
            if limiter {
                let state = crate::default_native_device_state_for_class(
                    crate::NativeDeviceClass::Effect,
                    crate::DeviceTypeId::from_static("hex.limiter"),
                )
                .unwrap();
                let command = if let (true, Some(bus)) = (limiter_on_receiver, return_bus) {
                    Command::InsertReturnDevice {
                        return_track: bus,
                        before: None,
                        state,
                    }
                } else {
                    Command::InsertDevice {
                        track: if limiter_on_receiver {
                            f.tracks[0]
                        } else {
                            f.tracks[1]
                        },
                        before: None,
                        state,
                    }
                };
                f.editor.execute(command).unwrap();
            }
            f.editor
                .execute(Command::SetLoopRegion {
                    start: 0,
                    end: 127,
                    enabled: true,
                })
                .unwrap();
            let mut sources: BTreeMap<_, _> = f
                .media
                .iter()
                .map(|(id, audio)| (*id, PreparedAudioSource::Resident(audio.clone())))
                .collect();
            sources.insert(
                asset,
                PreparedAudioSource::Streamed(Arc::new(
                    StreamedSourceMetadata::new(
                        key,
                        48_000,
                        48_000,
                        1024,
                        1024,
                        hex_project::IDENTITY_CONVERSION,
                        [41; 32],
                    )
                    .unwrap(),
                )),
            );
            let streamed = Arc::new(
                PreparedRevision::prepare_with_source_bank(
                    f.editor.project(),
                    crate::compile::SourceBank::Mixed(&sources, 41),
                    Arc::new(SineTable::new()),
                    None,
                    &hex_devices::native_registry().unwrap(),
                    &hex_devices::native_instrument_registry().unwrap(),
                    &crate::audio_transform::AudioTransformPreparation::default(),
                    None,
                    None,
                    &[],
                )
                .unwrap(),
            );
            let resident = Arc::new(
                PreparedRevision::prepare(
                    f.editor.project(),
                    &f.media,
                    Arc::new(SineTable::new()),
                    None,
                )
                .unwrap(),
            );
            for partition in [1, 257] {
                let mut actual = crate::SessionPlayback::new(Arc::clone(&streamed)).unwrap();
                let mut oracle = crate::SessionPlayback::new(Arc::clone(&resident)).unwrap();
                actual.apply(SessionPlaybackCommand::Play).unwrap();
                oracle.apply(SessionPlaybackCommand::Play).unwrap();
                for step in 0..3 {
                    if step == 1 {
                        let token = actual
                            .prepare_stream_action(PlaybackAction::Transport(
                                PlaybackCommand::Seek(17),
                            ))
                            .unwrap();
                        let before = (actual.performance_frame(), actual.arrangement_frame());
                        let mut demand = Vec::new();
                        actual
                            .visit_prepared_stream_action(&token, 32, &mut |row| {
                                demand.push(row);
                                Ok(())
                            })
                            .unwrap();
                        assert!(!demand.is_empty());
                        assert_eq!(
                            (actual.performance_frame(), actual.arrangement_frame()),
                            before
                        );
                        actual.apply(SessionPlaybackCommand::Seek(17)).unwrap();
                        oracle.apply(SessionPlaybackCommand::Seek(17)).unwrap();
                    } else if step == 2 {
                        let mut next = crate::SessionPlayback::new(Arc::clone(&streamed)).unwrap();
                        let mut next_oracle =
                            crate::SessionPlayback::new(Arc::clone(&resident)).unwrap();
                        next.preview_stream_handoff(&mut actual, 32, None, &mut |_| Ok(()))
                            .unwrap();
                        next.try_inherit_state_from(&mut actual).unwrap();
                        next_oracle.try_inherit_state_from(&mut oracle).unwrap();
                        actual = next;
                        oracle = next_oracle;
                    }
                    let mut expected = vec![[0.0; 2]; 514];
                    for block in expected.chunks_mut(partition) {
                        oracle.render(block).unwrap();
                    }
                    let mut observed = vec![[0.0; 2]; expected.len()];
                    let mut offset = 0;
                    while offset < observed.len() {
                        let mut extents = Vec::new();
                        let window = actual
                            .visit_next_stream_chunk(
                                (observed.len() - offset).min(partition),
                                None,
                                &mut |row| {
                                    extents.push(row);
                                    Ok(())
                                },
                            )
                            .unwrap();
                        assert!(window.frames > 0);
                        actual
                            .render_with_pages(
                                &mut observed[offset..offset + window.frames],
                                &mut Pages {
                                    samples: &f.samples,
                                    key,
                                    extents: &extents,
                                },
                            )
                            .unwrap();
                        offset += window.frames;
                    }
                    assert_eq!(
                        observed, expected,
                        "limiter_on_receiver={limiter_on_receiver} stream_index={stream_index} partition={partition} step={step}"
                    );
                }
            }
        }
    }
}

#[test]
fn closed_zero_latency_group_demand_matches_render_through_loop_and_seek() {
    let mut f = crate::signal_graph::tests::fixture(&[&[]], 1024);
    let asset = *f.media.keys().next().unwrap();
    let key = StreamSourceKey::new(39, asset);
    f.editor
        .execute(hex_project::Command::GroupTracks {
            tracks: hex_project::TrackSelectionIds::new(&f.tracks).unwrap(),
            name: "group".into(),
        })
        .unwrap();
    f.editor
        .execute(hex_project::Command::SetLoopRegion {
            start: 0,
            end: 127,
            enabled: true,
        })
        .unwrap();
    let grouped = f.editor.project();
    let mut snapshot = grouped.snapshot();
    for track in &mut snapshot.tracks {
        track.kind = hex_project::TrackKind::Ordinary;
        track.parent_group = None;
    }
    let flat = hex_project::Project::from_snapshot(snapshot).unwrap();
    let sources = BTreeMap::from([(
        asset,
        PreparedAudioSource::Streamed(Arc::new(
            StreamedSourceMetadata::new(
                key,
                48_000,
                48_000,
                1024,
                1024,
                hex_project::IDENTITY_CONVERSION,
                [39; 32],
            )
            .unwrap(),
        )),
    )]);
    let effects = hex_devices::native_registry().unwrap();
    let instruments = hex_devices::native_instrument_registry().unwrap();
    let mut revision = PreparedRevision::prepare_with_source_bank(
        &flat,
        crate::compile::SourceBank::Mixed(&sources, 39),
        Arc::new(SineTable::new()),
        None,
        &effects,
        &instruments,
        &crate::audio_transform::AudioTransformPreparation::default(),
        None,
        None,
        &[],
    )
    .unwrap();
    let mut arrangement = revision.arrangement().as_ref().clone();
    arrangement.signal_graph =
        crate::signal_graph::SignalGraph::prepare(&arrangement, &[], grouped).unwrap();
    for track in &mut arrangement.tracks {
        track.source_mix = crate::PreparedTrackMix::identity();
    }
    revision.arrangement = crate::owned_shared::OwnedShared::new_owned(arrangement);
    let revision = Arc::new(revision);
    let mut actual = crate::SessionPlayback::new(Arc::clone(&revision)).unwrap();
    assert_eq!(
        actual
            .visit_next_stream_chunk(64, None, &mut |_| Ok(()))
            .unwrap()
            .frames,
        0
    );
    actual.apply(SessionPlaybackCommand::Play).unwrap();
    for count in [64, 63, 17, 127, 29] {
        let mut output = vec![[0.0; 2]; count];
        let mut offset = 0;
        while offset < count {
            let mut extents = Vec::new();
            let window = actual
                .visit_next_stream_chunk(count - offset, None, &mut |extent| {
                    extents.push(extent);
                    Ok(())
                })
                .unwrap();
            assert_eq!(
                extents,
                [crate::StreamResourceExtent {
                    source: key.into(),
                    page_index: 0,
                    start: window.arrangement_frame as usize,
                    end: window.arrangement_frame as usize + window.frames,
                }]
            );
            actual
                .render_with_pages(
                    &mut output[offset..offset + window.frames],
                    &mut Pages {
                        samples: &f.samples,
                        key,
                        extents: &extents,
                    },
                )
                .unwrap();
            for (i, frame) in output[offset..offset + window.frames].iter().enumerate() {
                assert_eq!(*frame, f.samples[window.arrangement_frame as usize + i]);
            }
            offset += window.frames;
        }
    }
    let before = (actual.performance_frame(), actual.arrangement_frame());
    let token = actual
        .prepare_stream_action(PlaybackAction::Transport(PlaybackCommand::Seek(17)))
        .unwrap();
    assert!(
        actual
            .visit_prepared_stream_action(&token, 20, &mut |_| Err(crate::Invalid(
                "visitor refused"
            )))
            .is_err()
    );
    let mut projected = Vec::new();
    let window = actual
        .visit_prepared_stream_action(&token, 20, &mut |extent| {
            projected.push(extent);
            Ok(())
        })
        .unwrap();
    assert_eq!((window.arrangement_frame, window.frames), (17, 20));
    assert_eq!(
        (actual.performance_frame(), actual.arrangement_frame()),
        before
    );
    assert_eq!((projected[0].start, projected[0].end), (17, 37));
    actual.apply(SessionPlaybackCommand::Seek(17)).unwrap();
    let mut extents = Vec::new();
    let window = actual
        .visit_next_stream_chunk(20, None, &mut |extent| {
            extents.push(extent);
            Ok(())
        })
        .unwrap();
    assert_eq!((window.arrangement_frame, window.frames), (17, 20));
    assert_eq!((extents[0].start, extents[0].end), (17, 37));
    let before = (actual.performance_frame(), actual.arrangement_frame());
    let mut candidate = crate::SessionPlayback::new(Arc::clone(&revision)).unwrap();
    assert!(
        candidate
            .preview_stream_handoff(&mut actual, 20, None, &mut |_| Err(crate::Invalid(
                "visitor refused"
            )))
            .is_err()
    );
    assert_eq!(
        (actual.performance_frame(), actual.arrangement_frame()),
        before
    );
    let mut planned = Vec::new();
    let window = candidate
        .preview_stream_handoff(&mut actual, 20, None, &mut |extent| {
            planned.push(extent);
            Ok(())
        })
        .unwrap();
    assert_eq!((window.arrangement_frame, window.frames), (17, 20));
    assert_eq!((planned[0].start, planned[0].end), (17, 37));
    assert_eq!(
        (actual.performance_frame(), actual.arrangement_frame()),
        before
    );
    candidate.try_inherit_state_from(&mut actual).unwrap();
    let mut output = [[0.0; 2]; 20];
    candidate
        .render_with_pages(
            &mut output,
            &mut Pages {
                samples: &f.samples,
                key,
                extents: &planned,
            },
        )
        .unwrap();
    assert_eq!(output, f.samples[17..37]);
}

#[test]
fn resident_batch_keeps_streamed_session_lookup_and_miss_semantics() {
    check_streamed_session_lookup_and_miss_semantics(false);
}

#[test]
fn resident_session_batch_keeps_streamed_session_lookup_and_miss_semantics() {
    check_streamed_session_lookup_and_miss_semantics(true);
}

fn check_streamed_session_lookup_and_miss_semantics(with_resident_sessions: bool) {
    struct FirstMiss<'a> {
        key: StreamSourceKey,
        samples: &'a [Frame],
        reads: usize,
        requests: usize,
    }
    impl MediaPages for FirstMiss<'_> {
        fn page(&mut self, key: StreamSourceKey, index: u64) -> Option<BorrowedMediaPage<'_>> {
            assert_eq!((key, index), (self.key, 0));
            self.reads += 1;
            (self.reads > 1).then_some(BorrowedMediaPage {
                page_index: 0,
                frames: self.samples,
            })
        }
        fn request_page(&mut self, key: StreamSourceKey, index: u64) {
            assert_eq!((key, index), (self.key, 0));
            self.requests += 1;
        }
    }
    let delays: &[&[u32]] = if with_resident_sessions {
        &[&[7], &[], &[], &[], &[]]
    } else {
        &[&[7], &[]]
    };
    let mut f = crate::signal_graph::tests::fixture(delays, 32);
    for audio in f.media.values_mut() {
        *audio = crate::Audio::new(48_000, vec![[0.125, -0.25]; 32]).unwrap();
    }
    let hex_project::Created::Asset(asset) = f
        .editor
        .execute(hex_project::Command::AddAsset {
            path: "streamed-session.wav".into(),
            sample_rate: 48_000,
            frames: 32,
        })
        .unwrap()
    else {
        panic!("created streamed asset")
    };
    let hex_project::Created::Scene(scene) = f
        .editor
        .execute(hex_project::Command::AddScene {
            name: "scene".into(),
        })
        .unwrap()
    else {
        panic!("created scene")
    };
    let hex_project::Created::SessionClip(clip) = f
        .editor
        .execute(hex_project::Command::CreateSessionAudioClip {
            track: f.tracks[1],
            scene,
            name: "stream".into(),
            asset,
            source_start: 0,
            source_end: 32,
        })
        .unwrap()
    else {
        panic!("created session clip")
    };
    let mut resident_clips = Vec::new();
    if with_resident_sessions {
        for &track in &f.tracks[2..] {
            let resident_asset = f.editor.project().track(track).unwrap().clips()[0].asset();
            f.media.insert(
                resident_asset,
                crate::Audio::new(48_000, vec![[0.0; 2]; 32]).unwrap(),
            );
            let hex_project::Created::SessionClip(resident_clip) = f
                .editor
                .execute(hex_project::Command::CreateSessionAudioClip {
                    track,
                    scene,
                    name: "resident".into(),
                    asset: resident_asset,
                    source_start: 0,
                    source_end: 32,
                })
                .unwrap()
            else {
                panic!("created resident clip")
            };
            resident_clips.push(resident_clip);
        }
    }
    let key = StreamSourceKey::new(77, asset);
    let mut sources: BTreeMap<_, _> = f
        .media
        .iter()
        .map(|(id, audio)| (*id, PreparedAudioSource::Resident(audio.clone())))
        .collect();
    sources.insert(
        asset,
        PreparedAudioSource::Streamed(Arc::new(
            StreamedSourceMetadata::new(
                key,
                48_000,
                48_000,
                32,
                32,
                hex_project::IDENTITY_CONVERSION,
                [77; 32],
            )
            .unwrap(),
        )),
    );
    let definition = crate::signal_graph::tests::Definition;
    let definitions: [&dyn hex_lattice::NativeDeviceDefinition; 1] = [&definition];
    let effects = hex_lattice::NativeDeviceRegistry::new(&definitions).unwrap();
    let instruments = hex_devices::native_instrument_registry().unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare_with_source_bank(
            f.editor.project(),
            crate::compile::SourceBank::Mixed(&sources, 77),
            Arc::new(SineTable::new()),
            None,
            &effects,
            &instruments,
            &crate::audio_transform::AudioTransformPreparation::default(),
            None,
            None,
            &[],
        )
        .unwrap(),
    );
    assert!(!revision.arrangement().contains_streamed_sources());
    let samples = [[0.25, -0.125]; 32];
    for partition in [1, 7, 17, 40] {
        let mut runtime = crate::SessionPlayback::new(Arc::clone(&revision)).unwrap();
        runtime
            .apply(SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        for &resident_clip in &resident_clips {
            runtime
                .apply(SessionPlaybackCommand::LaunchClip {
                    clip: resident_clip,
                    quantization: LaunchQuantization::Immediate,
                })
                .unwrap();
        }
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        let mut pages = FirstMiss {
            key,
            samples: &samples,
            reads: 0,
            requests: 0,
        };
        let mut output = [[0.0; 2]; 40];
        for chunk in output.chunks_mut(partition) {
            runtime.render_with_pages(chunk, &mut pages).unwrap();
        }
        let mut expected = [[0.0; 2]; 40];
        for (index, frame) in expected.iter_mut().enumerate().skip(7).take(32) {
            *frame = if index == 7 {
                [0.125, -0.25]
            } else {
                [0.375, -0.375]
            };
        }
        assert_eq!(output, expected, "partition {partition}");
        assert_eq!((pages.reads, pages.requests), (32, 1));
    }
}

#[test]
fn closed_main_latency_demand_matches_resident_through_loop_seek_and_handoff() {
    closed_latency_demand_matches_resident(false);
}

#[test]
fn closed_track_limiter_demand_matches_resident_through_loop_seek_and_handoff() {
    closed_latency_demand_matches_resident(true);
}

fn closed_latency_demand_matches_resident(upstream: bool) {
    use hex_project::Command;
    let mut f = if upstream {
        crate::signal_graph::tests::fixture(&[&[], &[]], 1024)
    } else {
        crate::signal_graph::tests::fixture(&[&[]], 1024)
    };
    let asset = *f.media.keys().next().unwrap();
    let key = StreamSourceKey::new(39, asset);
    f.editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 127,
            enabled: true,
        })
        .unwrap();
    let state = crate::default_native_device_state_for_class(
        crate::NativeDeviceClass::Effect,
        crate::DeviceTypeId::from_static(if upstream {
            "hex.limiter"
        } else {
            "hex.saturator"
        }),
    )
    .unwrap();
    let command = if upstream {
        Command::InsertDevice {
            track: f.tracks[0],
            before: None,
            state,
        }
    } else {
        Command::InsertMainDevice {
            before: None,
            state,
        }
    };
    f.editor.execute(command).unwrap();
    let mut sources: BTreeMap<_, _> = f
        .media
        .iter()
        .map(|(id, audio)| (*id, PreparedAudioSource::Resident(audio.clone())))
        .collect();
    sources.insert(
        asset,
        PreparedAudioSource::Streamed(Arc::new(
            StreamedSourceMetadata::new(
                key,
                48_000,
                48_000,
                1024,
                1024,
                hex_project::IDENTITY_CONVERSION,
                [39; 32],
            )
            .unwrap(),
        )),
    );
    let effects = hex_devices::native_registry().unwrap();
    let instruments = hex_devices::native_instrument_registry().unwrap();
    let streamed = Arc::new(
        PreparedRevision::prepare_with_source_bank(
            f.editor.project(),
            crate::compile::SourceBank::Mixed(&sources, 39),
            Arc::new(SineTable::new()),
            None,
            &effects,
            &instruments,
            &crate::audio_transform::AudioTransformPreparation::default(),
            None,
            None,
            &[],
        )
        .unwrap(),
    );
    let resident = Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &f.media,
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    for partition in [1, 257] {
        let mut actual = crate::SessionPlayback::new(Arc::clone(&streamed)).unwrap();
        let mut oracle = crate::SessionPlayback::new(Arc::clone(&resident)).unwrap();
        assert_eq!(
            actual
                .visit_next_stream_chunk(20, None, &mut |_| Ok(()))
                .unwrap()
                .frames,
            0
        );
        actual.apply(SessionPlaybackCommand::Play).unwrap();
        oracle.apply(SessionPlaybackCommand::Play).unwrap();
        for cycle in 0..3 {
            if cycle == 1 {
                let before = (actual.performance_frame(), actual.arrangement_frame());
                let token = actual
                    .prepare_stream_action(PlaybackAction::Transport(PlaybackCommand::Seek(17)))
                    .unwrap();
                assert!(
                    actual
                        .visit_prepared_stream_action(&token, 20, &mut |_| Err(crate::Invalid(
                            "visitor refused"
                        )))
                        .is_err()
                );
                assert_eq!(
                    (actual.performance_frame(), actual.arrangement_frame()),
                    before
                );
                let mut sought = Vec::new();
                let window = actual
                    .visit_prepared_stream_action(&token, 20, &mut |extent| {
                        sought.push(extent);
                        Ok(())
                    })
                    .unwrap();
                assert_eq!((window.arrangement_frame, window.frames), (17, 20));
                assert_eq!((sought[0].start, sought[0].end), (17, 37));
                actual.apply(SessionPlaybackCommand::Seek(17)).unwrap();
                oracle.apply(SessionPlaybackCommand::Seek(17)).unwrap();
            }
            if cycle == 2 {
                let before = (actual.performance_frame(), actual.arrangement_frame());
                let mut next = crate::SessionPlayback::new(Arc::clone(&streamed)).unwrap();
                let mut next_oracle = crate::SessionPlayback::new(Arc::clone(&resident)).unwrap();
                assert!(
                    next.preview_stream_handoff(&mut actual, 20, None, &mut |_| Err(
                        crate::Invalid("visitor refused")
                    ))
                    .is_err()
                );
                assert_eq!(
                    (actual.performance_frame(), actual.arrangement_frame()),
                    before
                );
                next.preview_stream_handoff(&mut actual, 20, None, &mut |_| Ok(()))
                    .unwrap();
                next.try_inherit_state_from(&mut actual).unwrap();
                next_oracle.try_inherit_state_from(&mut oracle).unwrap();
                actual = next;
                oracle = next_oracle;
            }
            let mut expected = vec![[0.0; 2]; 514];
            for block in expected.chunks_mut(partition) {
                oracle.render(block).unwrap();
            }
            let mut observed = vec![[0.0; 2]; expected.len()];
            let mut offset = 0;
            while offset < observed.len() {
                let mut extents = Vec::new();
                let window = actual
                    .visit_next_stream_chunk(
                        (observed.len() - offset).min(partition),
                        None,
                        &mut |extent| {
                            extents.push(extent);
                            Ok(())
                        },
                    )
                    .unwrap();
                assert!(window.frames > 0);
                assert_eq!(
                    extents,
                    [crate::StreamResourceExtent {
                        source: key.into(),
                        page_index: 0,
                        start: window.arrangement_frame as usize,
                        end: window.arrangement_frame as usize + window.frames
                    }]
                );
                actual
                    .render_with_pages(
                        &mut observed[offset..offset + window.frames],
                        &mut Pages {
                            samples: &f.samples,
                            key,
                            extents: &extents,
                        },
                    )
                    .unwrap();
                offset += window.frames;
            }
            assert_eq!(observed, expected, "partition={partition}, cycle={cycle}");
            assert_eq!(
                (actual.performance_frame(), actual.arrangement_frame()),
                (oracle.performance_frame(), oracle.arrangement_frame())
            );
        }
    }
}

#[test]
fn closed_main_latency_session_audio_demand_matches_resident_routes() {
    closed_session_audio_demand_matches_resident(None, false);
}

#[test]
fn closed_upstream_latency_session_audio_demand_matches_resident_routes() {
    for latency_track in [0, 1] {
        closed_session_audio_demand_matches_resident(Some(latency_track), false);
    }
}

#[test]
fn closed_grouped_session_audio_latency_demand_matches_resident_routes() {
    for latency_track in [0, 2] {
        closed_session_audio_demand_matches_resident(Some(latency_track), true);
    }
}

fn closed_session_audio_demand_matches_resident(latency_track: Option<usize>, grouped: bool) {
    use hex_project::{Command, Created};
    let mut f = if latency_track.is_some() {
        crate::signal_graph::tests::fixture(&[&[], &[]], 1024)
    } else {
        crate::signal_graph::tests::fixture(&[&[]], 1024)
    };
    let group = if grouped {
        let Created::Track(group) = f
            .editor
            .execute(Command::GroupTracks {
                tracks: hex_project::TrackSelectionIds::new(&f.tracks).unwrap(),
                name: "group".into(),
            })
            .unwrap()
        else {
            panic!("group")
        };
        Some(group)
    } else {
        None
    };
    let asset = *f.media.keys().next().unwrap();
    let key = StreamSourceKey::new(40, asset);
    let Created::Scene(scene) = f
        .editor
        .execute(Command::AddScene {
            name: "session".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(clip) = f
        .editor
        .execute(Command::CreateSessionAudioClip {
            track: f.tracks[0],
            scene,
            name: "clip".into(),
            asset,
            source_start: 256,
            source_end: 1024,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    f.editor
        .execute(Command::SetSessionAudioClipLoop {
            clip,
            loop_range: Some(hex_project::SessionAudioLoop::new(288, 320).unwrap()),
        })
        .unwrap();
    f.editor
        .execute(Command::SetLoopRegion {
            start: 0,
            end: 127,
            enabled: true,
        })
        .unwrap();
    let state = crate::default_native_device_state_for_class(
        crate::NativeDeviceClass::Effect,
        crate::DeviceTypeId::from_static(if latency_track.is_some() {
            "hex.limiter"
        } else {
            "hex.saturator"
        }),
    )
    .unwrap();
    let command = if let Some(index) = latency_track {
        Command::InsertDevice {
            track: if index == 2 {
                group.unwrap()
            } else {
                f.tracks[index]
            },
            before: None,
            state,
        }
    } else {
        Command::InsertMainDevice {
            before: None,
            state,
        }
    };
    f.editor.execute(command).unwrap();
    let mut sources: BTreeMap<_, _> = f
        .media
        .iter()
        .map(|(id, audio)| (*id, PreparedAudioSource::Resident(audio.clone())))
        .collect();
    sources.insert(
        asset,
        PreparedAudioSource::Streamed(Arc::new(
            StreamedSourceMetadata::new(
                key,
                48_000,
                48_000,
                1024,
                1024,
                hex_project::IDENTITY_CONVERSION,
                [40; 32],
            )
            .unwrap(),
        )),
    );
    let streamed = Arc::new(
        PreparedRevision::prepare_with_source_bank(
            f.editor.project(),
            crate::compile::SourceBank::Mixed(&sources, 40),
            Arc::new(SineTable::new()),
            None,
            &hex_devices::native_registry().unwrap(),
            &hex_devices::native_instrument_registry().unwrap(),
            &crate::audio_transform::AudioTransformPreparation::default(),
            None,
            None,
            &[],
        )
        .unwrap(),
    );
    let resident = Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &f.media,
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    );
    for partition in [1, 257] {
        let mut actual = crate::SessionPlayback::new(Arc::clone(&streamed)).unwrap();
        let mut oracle = crate::SessionPlayback::new(Arc::clone(&resident)).unwrap();
        actual.apply(SessionPlaybackCommand::Play).unwrap();
        oracle.apply(SessionPlaybackCommand::Play).unwrap();
        for step in 0..4 {
            let command = match step {
                0 => Some(SessionPlaybackCommand::LaunchClip {
                    clip,
                    quantization: LaunchQuantization::Immediate,
                }),
                1 => Some(SessionPlaybackCommand::Seek(17)),
                3 => Some(SessionPlaybackCommand::BackToArrangement {
                    quantization: LaunchQuantization::Immediate,
                }),
                _ => None,
            };
            if let Some(command) = command {
                let token = actual
                    .prepare_stream_action(PlaybackAction::Session(command))
                    .unwrap();
                let before = (actual.performance_frame(), actual.arrangement_frame());
                let mut demand = Vec::new();
                actual
                    .visit_prepared_stream_action(&token, 32, &mut |extent| {
                        demand.push(extent);
                        Ok(())
                    })
                    .unwrap();
                assert_eq!(
                    (actual.performance_frame(), actual.arrangement_frame()),
                    before
                );
                assert!(!demand.is_empty());
                actual.apply(command).unwrap();
                oracle.apply(command).unwrap();
            } else {
                let mut next = crate::SessionPlayback::new(Arc::clone(&streamed)).unwrap();
                let mut next_oracle = crate::SessionPlayback::new(Arc::clone(&resident)).unwrap();
                let mut demand = Vec::new();
                next.preview_stream_handoff(&mut actual, 32, None, &mut |extent| {
                    demand.push(extent);
                    Ok(())
                })
                .unwrap();
                assert!(!demand.is_empty());
                next.try_inherit_state_from(&mut actual).unwrap();
                next_oracle.try_inherit_state_from(&mut oracle).unwrap();
                actual = next;
                oracle = next_oracle;
            }
            let mut expected = vec![[0.0; 2]; 514];
            for block in expected.chunks_mut(partition) {
                oracle.render(block).unwrap();
            }
            let mut observed = vec![[0.0; 2]; expected.len()];
            let mut offset = 0;
            while offset < observed.len() {
                let mut extents = Vec::new();
                let window = actual
                    .visit_next_stream_chunk(
                        (observed.len() - offset).min(partition),
                        None,
                        &mut |extent| {
                            extents.push(extent);
                            Ok(())
                        },
                    )
                    .unwrap();
                assert!(window.frames > 0);
                actual
                    .render_with_pages(
                        &mut observed[offset..offset + window.frames],
                        &mut Pages {
                            samples: &f.samples,
                            key,
                            extents: &extents,
                        },
                    )
                    .unwrap();
                offset += window.frames;
            }
            assert_eq!(observed, expected, "partition {partition} step {step}");
        }
    }
}

#[test]
fn closed_routed_upstream_latency_with_or_without_session_audio_remains_refused() {
    for session_audio in [false, true] {
        let mut f = crate::signal_graph::tests::fixture(&[&[]], 1024);
        let asset = *f.media.keys().next().unwrap();
        let key = StreamSourceKey::new(39, asset);
        let saturator = crate::default_native_device_state_for_class(
            crate::NativeDeviceClass::Effect,
            crate::DeviceTypeId::from_static("hex.saturator"),
        )
        .unwrap();
        f.editor
            .execute(hex_project::Command::InsertDevice {
                track: f.tracks[0],
                before: None,
                state: saturator,
            })
            .unwrap();
        f.editor
            .execute(hex_project::Command::AddReturnTrack {
                name: "unproven route".into(),
            })
            .unwrap();
        if session_audio {
            let hex_project::Created::Scene(scene) = f
                .editor
                .execute(hex_project::Command::AddScene {
                    name: "scene".into(),
                })
                .unwrap()
            else {
                panic!("scene")
            };
            f.editor
                .execute(hex_project::Command::CreateSessionAudioClip {
                    track: f.tracks[0],
                    scene,
                    name: "clip".into(),
                    asset,
                    source_start: 0,
                    source_end: 1024,
                })
                .unwrap();
        }
        let sources = BTreeMap::from([(
            asset,
            PreparedAudioSource::Streamed(Arc::new(
                StreamedSourceMetadata::new(
                    key,
                    48_000,
                    48_000,
                    1024,
                    1024,
                    hex_project::IDENTITY_CONVERSION,
                    [39; 32],
                )
                .unwrap(),
            )),
        )]);
        let revision = Arc::new(
            PreparedRevision::prepare_with_source_bank(
                f.editor.project(),
                crate::compile::SourceBank::Mixed(&sources, 39),
                Arc::new(SineTable::new()),
                None,
                &hex_devices::native_registry().unwrap(),
                &hex_devices::native_instrument_registry().unwrap(),
                &crate::audio_transform::AudioTransformPreparation::default(),
                None,
                None,
                &[],
            )
            .unwrap(),
        );
        let mut playback = crate::SessionPlayback::new(revision).unwrap();
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        assert!(
            playback
                .visit_next_stream_chunk(64, None, &mut |_| Ok(()))
                .is_err()
        );
        assert!(
            playback
                .visit_ordinary_stream_chunk(64, None, &mut |_| Ok(()))
                .is_ok()
        );
    }
}

#[test]
fn closed_unknown_upstream_effect_stream_demand_remains_refused() {
    let f = crate::signal_graph::tests::fixture(&[&[7]], 1024);
    let asset = *f.media.keys().next().unwrap();
    let key = StreamSourceKey::new(39, asset);
    let source = StreamedSourceMetadata::new(
        key,
        48_000,
        48_000,
        1024,
        1024,
        hex_project::IDENTITY_CONVERSION,
        [39; 32],
    )
    .unwrap();
    let sources = BTreeMap::from([(asset, PreparedAudioSource::Streamed(Arc::new(source)))]);
    let definition = crate::signal_graph::tests::Definition;
    let definitions: [&dyn hex_lattice::NativeDeviceDefinition; 1] = [&definition];
    let effects = hex_lattice::NativeDeviceRegistry::new(&definitions).unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare_with_source_bank(
            f.editor.project(),
            crate::compile::SourceBank::Mixed(&sources, 39),
            Arc::new(SineTable::new()),
            None,
            &effects,
            &hex_devices::native_instrument_registry().unwrap(),
            &crate::audio_transform::AudioTransformPreparation::default(),
            None,
            None,
            &[],
        )
        .unwrap(),
    );
    let mut playback = crate::SessionPlayback::new(revision).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    assert!(
        playback
            .visit_next_stream_chunk(64, None, &mut |_| Ok(()))
            .is_err()
    );
    assert!(
        playback
            .visit_ordinary_stream_chunk(64, None, &mut |_| Ok(()))
            .is_ok()
    );
}
