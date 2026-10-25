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
    struct Pages<'a> {
        samples: &'a [Frame],
        key: StreamSourceKey,
        extents: &'a [crate::StreamPageExtent],
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
