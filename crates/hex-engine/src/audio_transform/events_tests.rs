use super::*;
use crate::{Audio, audio_transform::AudioTransformPreparation};
use hex_project::{
    AudioEvent, AudioEvents, BeatTime, ClipGain, Command, Created, Editor, Project, Tempo,
    WarpMode, default_audio_warp,
};
use std::sync::Arc;

#[test]
fn signed_musical_destinations_read_original_warped_pcm_across_a_tempo_seam() {
    let beat = BeatTime::ONE.raw();
    let half = beat / 2;
    for warped in [false, true] {
        let mut editor = Editor::new(Project::new(16_000).unwrap(), 8).unwrap();
        let Created::Imported { clip, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "source.wav".into(),
                sample_rate: 16_000,
                frames: 8_000,
            })
            .unwrap()
        else {
            panic!("audio fixture")
        };
        editor
            .execute(Command::SetAudioClipMusical {
                clip,
                enabled: true,
            })
            .unwrap();
        if warped {
            let project = editor.project();
            let settings = default_audio_warp(
                project.clip(clip).unwrap(),
                project.tempo_map(),
                WarpMode::RePitch,
            )
            .unwrap();
            editor
                .execute(Command::SetAudioClipWarp {
                    clip,
                    settings: Some(settings),
                })
                .unwrap();
        }
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::from_raw(half),
                tempo: Tempo::new(240_000_000).unwrap(),
            })
            .unwrap();
        let project = editor.project();
        let tempo = project.tempo_map();
        let source_root = project.clip(clip).unwrap().audio_event_root();
        assert_eq!(source_root.domain.clock.length(), beat);
        let audio = Audio::new(
            16_000,
            (0..8_000)
                .map(|i| {
                    let x = (i + 1) as f32 / 8192.0;
                    [x, -x]
                })
                .collect(),
        )
        .unwrap();
        let context = AudioTransformPreparation::default();
        let source = context
            .prepare_audio_at_tempo(tempo, &source_root, project.asset(asset).unwrap(), &audio)
            .unwrap();
        assert_eq!(source.frames().len(), 6_000);
        let cancelled = AtomicBool::new(false);
        // Both cases retain the source at beat zero. One destination starts
        // a beat earlier, the other extends through the faster tempo.
        for (lead, target, expected_start, stride) in [(beat, 0, 0, 1), (0, beat + half, 8_000, 2)]
        {
            let event = |id, start, source_start| AudioEvent {
                id,
                start,
                end: start + half,
                source_start,
                source: 0,
                active: true,
                gain: ClipGain::UNITY,
                fade: None,
            };
            let moved = event(1, target, 0);
            let untouched = event(2, lead + half, half);
            let mut sequence = vec![moved, untouched];
            sequence.sort_by_key(|event| event.start);
            let mut destination = source_root.clone();
            destination.view_start += lead;
            destination.view_end += lead;
            let domain = Arc::make_mut(&mut destination.domain);
            domain.clock =
                AudioDomainClock::Beats(BeatTime::from_raw((beat + lead).max(moved.end)));
            domain.events = Some(Arc::new(AudioEvents {
                sources: Vec::new(),
                next_id: 3,
                events: sequence,
            }));
            let prepared = || EventSource {
                frames: source.frames(),
                root: &source_root,
                zero_in_destination: lead,
            };
            let output = compose(prepared(), &destination, tempo, &cancelled).unwrap();
            let unchanged_start = if lead == 0 { 4_000 } else { 12_000 };
            assert_eq!(
                &output[unchanged_start..unchanged_start + 2_000],
                &source.frames()[4_000..]
            );
            let moved_length = 4_000 / stride;
            for (index, sample) in output[expected_start..expected_start + moved_length]
                .iter()
                .enumerate()
            {
                assert_eq!(*sample, source.frames()[index * stride]);
            }
            for (index, sample) in output.iter().enumerate() {
                if !(expected_start..expected_start + moved_length).contains(&index)
                    && !(unchanged_start..unchanged_start + 2_000).contains(&index)
                {
                    assert_eq!(*sample, [0.0; 2]);
                }
            }

            // Valid destination extent cannot legitimize an overrun of the
            // smaller original source extent.
            let domain = Arc::make_mut(&mut destination.domain);
            let events = Arc::make_mut(domain.events.as_mut().unwrap());
            events
                .events
                .iter_mut()
                .find(|event| event.id == 1)
                .unwrap()
                .source_start = beat;
            assert_eq!(
                compose(prepared(), &destination, tempo, &cancelled)
                    .unwrap_err()
                    .0,
                "audio event exceeds its retained source clock"
            );
        }
    }
}

#[test]
fn canonical_musical_moves_preserve_the_gate_and_original_source_across_tempo_edits() {
    use crate::audio_transform::tests::{fixture, prepare, render};
    use hex_project::{AudioEventEdit, TrimEdge};

    let half = BeatTime::ONE.raw() / 2;
    for warped in [false, true] {
        let (mut editor, clip, media) = fixture(false);
        let track = editor.project().tracks()[0].id();
        for command in [
            Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            },
            Command::MoveClip {
                clip,
                track,
                start: 16_000,
            },
            Command::SetAudioClipMusical {
                clip,
                enabled: true,
            },
        ] {
            editor.execute(command).unwrap();
        }
        if warped {
            let project = editor.project();
            let settings = default_audio_warp(
                project.clip(clip).unwrap(),
                project.tempo_map(),
                WarpMode::RePitch,
            )
            .unwrap();
            editor
                .execute(Command::SetAudioClipWarp {
                    clip,
                    settings: Some(settings),
                })
                .unwrap();
        }
        let seam = BeatTime::from_raw(2 * BeatTime::ONE.raw() + half);
        editor
            .execute(Command::SetTempoPoint {
                beat: seam,
                tempo: Tempo::new(240_000_000).unwrap(),
            })
            .unwrap();
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Split { at: half },
            })
            .unwrap();
        let original = editor.project().clone();
        let baseline = render(
            &prepare(&editor, &media, &AudioTransformPreparation::default()),
            0,
            28_000,
            57,
        );
        for (destination, from, to, stride, edge) in [
            (
                -(BeatTime::ONE.raw() as i64),
                8_000,
                12_000,
                1,
                TrimEdge::Left,
            ),
            ((3 * half) as i64, 24_000, 26_000, 2, TrimEdge::Right),
        ] {
            editor
                .execute(Command::EditAudioEvents {
                    clip,
                    operation: AudioEventEdit::Place {
                        event: 1,
                        start: destination,
                    },
                })
                .unwrap();
            let placed = editor.project().clone();
            let c = placed.clip(clip).unwrap();
            assert_eq!((c.start(), c.duration()), (16_000, 6_000));
            let event = c
                .audio_events()
                .events
                .into_iter()
                .find(|event| event.id == 1)
                .unwrap();
            assert_eq!(event.source_start, 0);
            let rendered = render(
                &prepare(&editor, &media, &AudioTransformPreparation::default()),
                0,
                28_000,
                7,
            );
            assert!(rendered[from..to].iter().all(|sample| *sample == [0.0; 2]));
            assert!(
                rendered[16_000..20_000]
                    .iter()
                    .all(|sample| *sample == [0.0; 2])
            );
            assert_eq!(&rendered[20_000..22_000], &baseline[20_000..22_000]);
            let change = Command::SetTempoPoint {
                beat: seam,
                tempo: Tempo::new(160_000_000).unwrap(),
            };
            let mut reference = Editor::new(original.clone(), 8).unwrap();
            reference.execute(change.clone()).unwrap();
            editor.execute(change).unwrap();
            let current = render(
                &prepare(&editor, &media, &AudioTransformPreparation::default()),
                0,
                28_000,
                101,
            );
            let expected = render(
                &prepare(&reference, &media, &AudioTransformPreparation::default()),
                0,
                28_000,
                57,
            );
            assert_eq!(&current[20_000..23_000], &expected[20_000..23_000]);
            assert!(
                current[16_000..20_000]
                    .iter()
                    .all(|sample| *sample == [0.0; 2])
            );
            assert!(editor.undo());
            assert_eq!(editor.project(), &placed);
            assert!(editor.undo());
            assert_eq!(editor.project(), &original);
            assert!(editor.redo());
            assert_eq!(editor.project(), &placed);
            let frame = if edge == TrimEdge::Left { from } else { to };
            editor
                .execute(Command::TrimClipEdge {
                    clip,
                    edge,
                    frame: frame as u64,
                })
                .unwrap();
            let revealed = render(
                &prepare(&editor, &media, &AudioTransformPreparation::default()),
                0,
                28_000,
                127,
            );
            for (index, sample) in revealed[from..to].iter().enumerate() {
                assert_eq!(*sample, baseline[16_000 + stride * index]);
            }
            assert_eq!(&revealed[20_000..22_000], &baseline[20_000..22_000]);
            assert!(editor.undo());
            assert!(editor.undo());
            assert_eq!(editor.project(), &original);
        }
    }
}
