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
struct Pages<'a> {
    samples: &'a [Frame],
    key: StreamSourceKey,
    extents: &'a [crate::StreamPageExtent],
}

#[test]
fn callback_demand_skips_muted_source_without_a_raw_audio_consumer() {
    let f = crate::signal_graph::tests::fixture(&[&[]], 1024);
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
    let mut snapshot = f.editor.project().snapshot();
    snapshot.tracks[0].muted = true;
    let project = hex_project::Project::from_snapshot(snapshot).unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare_with_source_bank(
            &project,
            crate::compile::SourceBank::Mixed(&sources, 31),
            Arc::new(SineTable::new()),
            None,
            &hex_devices::native_registry().unwrap(),
            &hex_devices::native_instrument_registry().unwrap(),
            &crate::audio_transform::AudioTransformPreparation::default(),
            None,
        )
        .unwrap(),
    );
    let mut playback = Playback::from_prepared(revision);
    playback.apply(PlaybackCommand::Play);
    let context = crate::AudioSourceContext {
        project_generation: 9,
        editor_revision: 2,
    };
    let mut exact = Vec::new();
    playback
        .stream_playback(context)
        .visit_next_stream_chunk(32, None, &mut |row| {
            exact.push(row);
            Ok(())
        })
        .unwrap();
    let mut callback = Vec::new();
    playback
        .stream_playback(context)
        .visit_stream_callback(32, None, &mut |row| {
            callback.push(row);
            Ok(())
        })
        .unwrap();
    assert!(exact.is_empty());
    assert!(
        callback.is_empty(),
        "a muted source with no raw consumer must not consume page capacity"
    );
    assert_eq!((playback.position(), playback.performance_frame()), (0, 0));
}

#[test]
fn callback_demand_retains_muted_source_for_compensated_graph() {
    let f = crate::signal_graph::tests::fixture(&[&[], &[7]], 1024);
    let asset = *f.media.keys().next().unwrap();
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
    let mut snapshot = f.editor.project().snapshot();
    snapshot.tracks[0].muted = true;
    let project = hex_project::Project::from_snapshot(snapshot).unwrap();
    let definition = crate::signal_graph::tests::Definition;
    let definitions: [&dyn hex_lattice::NativeDeviceDefinition; 1] = [&definition];
    let effects = hex_lattice::NativeDeviceRegistry::new(&definitions).unwrap();
    let instruments = hex_devices::native_instrument_registry().unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare_with_source_bank(
            &project,
            crate::compile::SourceBank::Mixed(&sources, 31),
            Arc::new(SineTable::new()),
            None,
            &effects,
            &instruments,
            &crate::audio_transform::AudioTransformPreparation::default(),
            None,
        )
        .unwrap(),
    );
    let mut playback = Playback::from_prepared(revision);
    playback.apply(PlaybackCommand::Play);
    let context = crate::AudioSourceContext {
        project_generation: 9,
        editor_revision: 2,
    };
    let mut extents = Vec::new();
    let window = playback
        .stream_playback(context)
        .visit_stream_callback(32, None, &mut |row| {
            extents.push(row);
            Ok(())
        })
        .unwrap();
    assert_eq!(window.frames, 32);
    assert!(
        extents
            .iter()
            .any(|row| row.source == key && row.start == 0 && row.end == 32)
    );
    let mut output = [[0.0; 2]; 32];
    playback
        .stream_playback(context)
        .render_with_pages_and_signal_observers(
            &mut output,
            &mut Pages {
                samples: &f.samples,
                key,
                extents: &extents,
            },
            None,
            None,
        )
        .unwrap();
    assert_eq!(playback.position(), 32);
}
impl MediaPages for Pages<'_> {
    fn page(&mut self, key: StreamSourceKey, index: u64) -> Option<BorrowedMediaPage<'_>> {
        assert!(
            self.extents
                .iter()
                .any(|extent| extent.source == key && extent.page_index == index)
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
            assert_eq!(window.frames, 1);
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
            let before = (actual.position(), actual.performance_frame());
            let mut callback_demand = Vec::new();
            let callback = actual
                .stream_playback(context)
                .visit_stream_callback(count, None, &mut |extent| {
                    callback_demand.push(extent);
                    Ok(())
                })
                .unwrap();
            assert_eq!(callback.frames, count);
            assert_eq!((actual.position(), actual.performance_frame()), before);
            for offset in 0..count {
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
                    window.frames, 1,
                    "positive latency uses the actual renderer's one-frame source quantum"
                );
                assert_eq!((actual.position(), actual.performance_frame()), clock);
                for extent in &extents {
                    assert!(
                        callback_demand.iter().any(|planned| {
                            planned.source == extent.source
                                && planned.page_index == extent.page_index
                                && planned.start <= extent.start
                                && planned.end >= extent.end
                        }),
                        "callback demand must cover each actual source chunk: {extent:?}"
                    );
                }
                actual
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
            assert_eq!(
                (actual.position(), actual.performance_frame()),
                (oracle.position(), oracle.performance_frame())
            );
        }
    }
    let mut successor = Playback::from_prepared(streamed);
    let mut resident_successor = Playback::from_prepared(resident);
    let clock = (actual.position(), actual.performance_frame());
    let window = successor
        .stream_playback(context)
        .preview_stream_handoff(&mut actual.stream_playback(context), 32, None, &mut |_| {
            Ok(())
        })
        .unwrap();
    assert_eq!(window.frames, 1);
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
            let original_clock = (actual.position(), actual.performance_frame());
            let mut callback_demand = Vec::new();
            let window = actual
                .stream_playback(context)
                .visit_stream_callback(count, None, &mut |extent| {
                    callback_demand.push(extent);
                    Ok(())
                })
                .unwrap();
            assert_eq!(window.frames, count);
            assert_eq!(
                (actual.position(), actual.performance_frame()),
                original_clock
            );
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
                for extent in &extents {
                    assert!(
                        callback_demand.iter().any(|planned| {
                            planned.source == extent.source
                                && planned.page_index == extent.page_index
                                && planned.start <= extent.start
                                && planned.end >= extent.end
                        }),
                        "loop callback must cover actual source chunk: {extent:?}"
                    );
                }
                assert_eq!(
                    extents,
                    [crate::StreamPageExtent {
                        source: key,
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
