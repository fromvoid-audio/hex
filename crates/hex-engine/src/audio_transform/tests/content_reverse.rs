use super::*;
use hex_project::{AudioViewMode, AudioViewPlacement, ClipFade};
#[test]
fn reverse_frame_audio_uses_exact_finite_pcm_order_and_keeps_destination_fades() {
    for outer in [false, true] {
        let (mut editor, clip, media) = fixture(false);
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
        // A real existing crop and Repeat phase exercise outer sequence reversal.
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 1 })
            .unwrap();
        editor
            .execute(Command::SetAudioClipPitch { clip, cents: 0 })
            .unwrap();
        let mut snapshot = editor.project().snapshot();
        let c = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
        c.edit.as_mut().unwrap().view_start = 1_000;
        c.edit.as_mut().unwrap().view_end = 3_000;
        c.duration = 2_000;
        if outer {
            c.edit.as_mut().unwrap().placement = Some(AudioViewPlacement {
                mode: AudioViewMode::Repeat,
                outer_length: AudioDomainClock::Frames(5_200),
                cycle_phase: 500,
            });
            c.duration = 5_200;
        }
        c.fade = ClipFade::neutral(c.duration).unwrap();
        editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 32).unwrap();
        let context = AudioTransformPreparation::default();
        let before = prepare(&editor, &media, &context);
        let length = editor.project().clip(clip).unwrap().duration() as usize;
        let original = render(&before, 0, length, 127);
        editor.execute(Command::ReverseClips(vec![clip])).unwrap();
        let after = prepare(&editor, &media, &context);
        let expected = original.into_iter().rev().collect::<Vec<_>>();
        for block in [1, 127, 257] {
            assert_eq!(render(&after, 0, length, block), expected);
        }
        assert_eq!(render(&after, 73, 201, 31), expected[73..274]);
        assert!(
            after.arrangement().tracks()[0].clips[0]
                .audio
                .as_ref()
                .unwrap()
                .frames()
                .iter()
                .any(|f| f[0] != 0.0)
        );
        // New fade settings remain ordinary destination-side controls.
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 300,
                fade_out: 500,
            })
            .unwrap();
        let faded = prepare(&editor, &media, &context);
        let actual = render(&faded, 0, length, 127);
        let domain = &editor
            .project()
            .clip(clip)
            .unwrap()
            .audio_edit()
            .unwrap()
            .domain;
        if !outer {
            for i in 0..length {
                let gain = domain.fade.gain_at(i as u64);
                assert_eq!(actual[i], [expected[i][0] * gain, expected[i][1] * gain]);
            }
        }
    }
}

#[test]
fn reverse_musical_audio_reflects_beats_across_tempo_seams_and_keeps_live_warp() {
    let (mut editor, clip, media) = fixture(false);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    snapshot.tempo_points = vec![
        hex_project::TempoPoint::new(BeatTime::ZERO, Tempo::DEFAULT),
        hex_project::TempoPoint::new(
            BeatTime::from_ratio(1, 2).unwrap(),
            Tempo::new(60_000_000).unwrap(),
        ),
    ];
    // Reproject through the canonical tempo command rather than stale frame caches.
    let points = snapshot.tempo_points;
    for point in points {
        editor
            .execute(Command::SetTempoPoint {
                beat: point.beat(),
                tempo: point.tempo(),
            })
            .unwrap();
    }
    let context = AudioTransformPreparation::default();
    let before = prepare(&editor, &media, &context);
    let c = editor.project().clip(clip).unwrap();
    let length = c.duration() as usize;
    let original = render(&before, c.start(), length, 127);
    editor.execute(Command::ReverseClips(vec![clip])).unwrap();
    let after = prepare(&editor, &media, &context);
    let reflected = original.iter().rev().copied().collect::<Vec<_>>();
    let q = 1_i128 << 32;
    let map = hex_warp::TimeMap::new(
        vec![
            hex_warp::MapSegment {
                output_start: 0,
                output_end: 4_000,
                source_start_q32: 0,
                source_end_q32: 8_000 * q,
            },
            hex_warp::MapSegment {
                output_start: 4_000,
                output_end: 12_000,
                source_start_q32: 8_000 * q,
                source_end_q32: 12_000 * q,
            },
        ],
        length,
    )
    .unwrap();
    // A new directed view reads with the current v2 law.
    let oracle = hex_warp::render_with_resampler(
        &hex_warp::Request {
            attacks: &[],
            input: hex_warp::Input {
                frames: &reflected,
                sample_rate: 16_000,
                region_start: 0,
                region_length: reflected.len() as u64,
                phase: 0,
                looping: false,
                envelope: &[],
            },
            map: &map,
            output_frames: length,
            pitch_cents: 0,
            mode: hex_warp::Mode::RePitch,
        },
        hex_warp::Resampler::V2,
        &mut || false,
    )
    .unwrap()
    .frames;
    assert_eq!(render(&after, 0, length, 257), oracle);
    assert_ne!(oracle, original.iter().rev().copied().collect::<Vec<_>>());
    let settings = editor
        .project()
        .clip(clip)
        .unwrap()
        .audio_edit()
        .unwrap()
        .domain
        .warp
        .clone();
    editor
        .execute(Command::SetAudioClipSourceGain {
            clip,
            points: vec![SourceGainPoint {
                frame: 0,
                gain_q16: 32_768,
            }],
        })
        .unwrap();
    let quieter = render(&prepare(&editor, &media, &context), 0, length, 127);
    for (actual, previous) in quieter.iter().zip(&oracle) {
        assert_eq!(*actual, [previous[0] * 0.5, previous[1] * 0.5]);
    }
    assert_eq!(
        editor
            .project()
            .clip(clip)
            .unwrap()
            .audio_edit()
            .unwrap()
            .domain
            .warp,
        settings
    );
}

#[test]
fn reverse_trim_split_move_and_retained_source_extension_preserve_content() {
    let (mut editor, clip, media) = fixture(false);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 1 })
        .unwrap();
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 0 })
        .unwrap();
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: hex_project::TrimEdge::Left,
            frame: 1_000,
        })
        .unwrap();
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: hex_project::TrimEdge::Right,
            frame: 3_000,
        })
        .unwrap();
    let context = AudioTransformPreparation::default();
    let original = render(&prepare(&editor, &media, &context), 1_000, 2_000, 127);
    editor.execute(Command::ReverseClips(vec![clip])).unwrap();
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: hex_project::TrimEdge::Right,
            frame: 3_500,
        })
        .unwrap();
    let extension = render(&prepare(&editor, &media, &context), 1_000, 2_500, 127);
    assert_eq!(
        &extension[..2_000],
        original.iter().rev().copied().collect::<Vec<_>>()
    );
    let source = media.values().next().unwrap();
    assert_eq!(
        &extension[2_000..],
        source.frames()[500..1_000]
            .iter()
            .rev()
            .copied()
            .collect::<Vec<_>>()
    );
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: hex_project::TrimEdge::Left,
            frame: 1_300,
        })
        .unwrap();
    let cropped = render(&prepare(&editor, &media, &context), 1_300, 2_200, 127);
    assert_eq!(cropped, extension[300..]);
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::MoveClip {
            clip,
            track,
            start: 0,
        })
        .unwrap();
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 2_200, 127),
        cropped
    );
    editor
        .execute(Command::SplitClip { clip, frame: 731 })
        .unwrap();
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 2_200, 127),
        cropped
    );
}

#[test]
fn reverse_finite_midi_keeps_occurrence_identity_and_static_resident_gate_agreement() {
    let b = |n| BeatTime::from_quarters(n).unwrap();
    let mut editor = Editor::new(Project::new(16_000).unwrap(), 32).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "midi".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::InsertInstrument {
            track,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: b(1),
            length: b(12),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::SetMidiClipSource {
            clip,
            source_length: b(8),
            source_start: b(1),
            loop_start: b(2),
            loop_end: b(5),
            looping: true,
            length: b(12),
        })
        .unwrap();
    for (start, duration, pitch, channel, velocity) in [(1, 2, 60, 3, 91), (4, 1, 65, 7, 73)] {
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: b(start),
                duration: b(duration),
                pitch,
                channel,
                velocity,
            })
            .unwrap();
    }
    editor
        .execute(Command::SetTempoPoint {
            beat: b(4),
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    let original = editor.project().midi_clip(clip).unwrap().clone();
    let mut expected = Vec::new();
    for note in original.notes() {
        for occurrence in original
            .source_playback()
            .unwrap()
            .note_occurrences(note.start(), note.duration())
            .unwrap()
            .iter()
        {
            let end = original.length().raw() - occurrence.timeline_start().raw();
            let start = end - occurrence.gate_duration().raw();
            for (raw, kind) in [
                (start, crate::midi_compile::MidiEventKind::NoteOn),
                (end, crate::midi_compile::MidiEventKind::NoteOff),
            ] {
                expected.push((
                    editor
                        .project()
                        .tempo_map()
                        .beat_to_frame(BeatTime::from_raw(original.start().raw() + raw))
                        .unwrap(),
                    kind,
                    note.id(),
                    occurrence.iteration(),
                    note.pitch(),
                    note.channel(),
                    note.velocity(),
                ));
            }
        }
    }
    editor.execute(Command::ReverseClips(vec![clip])).unwrap();
    let changed = editor.project().midi_clip(clip).unwrap();
    assert_eq!(changed.source_identity(), original.source_identity());
    assert_eq!(changed.notes(), original.notes());
    let events = crate::midi_compile::midi_events(editor.project()).unwrap();
    assert_eq!(events.len(), expected.len());
    for event in events {
        assert!(expected.contains(&(
            event.frame,
            event.kind,
            event.note,
            event.iteration,
            event.pitch,
            event.channel,
            event.velocity
        )));
    }
    let revision = prepare(
        &editor,
        &BTreeMap::new(),
        &AudioTransformPreparation::default(),
    );
    let length = editor.project().tempo_map().beat_to_frame(b(13)).unwrap() as usize + 257;
    let ordinary = render(&revision, 0, length, 257);
    assert!(ordinary.iter().flatten().any(|s| s.abs() > 0.01));
    let mut resident =
        crate::playback::Playback::try_from_prepared_with_session(Arc::new(revision)).unwrap();
    resident.apply(crate::playback::PlaybackCommand::Play);
    let mut actual = vec![[0.0; 2]; length];
    for block in actual.chunks_mut(127) {
        resident.render(block).unwrap();
    }
    assert_eq!(actual, ordinary);
}

#[test]
fn reverse_musical_compression_filters_aliases_and_preserves_passband_tone() {
    for tone in [false, true] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Imported { clip, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "filter witness".into(),
                path: "original.wav".into(),
                sample_rate: 48_000,
                frames: 36_000,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::from_ratio(1, 2).unwrap(),
                tempo: Tempo::new(60_000_000).unwrap(),
            })
            .unwrap();
        editor
            .execute(Command::SetAudioClipMusical {
                clip,
                enabled: true,
            })
            .unwrap();
        assert_eq!(editor.project().clip(clip).unwrap().duration(), 36_000);
        let frames = (0..36_000)
            .map(|i| {
                let x = if tone {
                    0.25 * (std::f32::consts::TAU * 1_000.0 * i as f32 / 48_000.0).sin()
                } else if i % 2 == 0 {
                    1.0
                } else {
                    -1.0
                };
                [x, x]
            })
            .collect();
        let media = BTreeMap::from([(asset, Audio::new(48_000, frames).unwrap())]);
        editor.execute(Command::ReverseClips(vec![clip])).unwrap();
        let actual = render(
            &prepare(&editor, &media, &AudioTransformPreparation::default()),
            0,
            36_000,
            257,
        );
        // The first half beat compresses 24000 source frames into 12000 output
        // frames. Unfiltered decimation turns alternating Nyquist samples into
        // DC; a 1kHz passband tone must survive as a 2kHz tone instead of silence.
        let interior = &actual[1_000..11_000];
        let rms = (interior
            .iter()
            .map(|f| f64::from(f[0]).powi(2))
            .sum::<f64>()
            / interior.len() as f64)
            .sqrt();
        if tone {
            assert!((0.15..0.20).contains(&rms), "passband rms {rms}");
            let crossings = interior
                .windows(2)
                .filter(|p| p[0][0] <= 0.0 && p[1][0] > 0.0)
                .count();
            assert!(
                (415..=419).contains(&crossings),
                "2khz passband crossing count {crossings}"
            );
        } else {
            assert!(
                rms < 0.01,
                "compressed nyquist signal aliases into rms {rms}"
            );
        }
    }
}

#[test]
fn directed_audio_retains_live_warp_bpm_loop_and_anchor_edits() {
    let (mut editor, clip, media) = fixture(false);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: hex_project::TrimEdge::Left,
            frame: 1_000,
        })
        .unwrap();
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: hex_project::TrimEdge::Right,
            frame: 6_000,
        })
        .unwrap();
    editor.execute(Command::ReverseClips(vec![clip])).unwrap();
    let settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipSampleBpm {
            clip,
            tempo: Tempo::new(100_000_000).unwrap(),
        })
        .unwrap();
    let end = editor.project().clip(clip).unwrap().start()
        + editor.project().clip(clip).unwrap().duration() * 2;
    editor
        .execute(Command::ResizeAudioClipView {
            clip,
            operation: hex_project::AudioViewResizeOperation::RepeatOuterEnd { project_frame: end },
        })
        .unwrap();
    let context = AudioTransformPreparation::default();
    let c = editor.project().clip(clip).unwrap();
    assert!(
        c.audio_edit()
            .unwrap()
            .content_view
            .unwrap()
            .window
            .reversed
    );
    assert!(
        render(
            &prepare(&editor, &media, &context),
            c.start(),
            c.duration() as usize,
            127
        )
        .iter()
        .flatten()
        .any(|s| s.abs() > 0.01)
    );
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: None,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipMusical {
            clip,
            enabled: false,
        })
        .unwrap();
    let c = editor.project().clip(clip).unwrap();
    assert!(matches!(
        c.audio_edit().unwrap().domain.clock,
        AudioDomainClock::Frames(_)
    ));
    assert!(
        render(
            &prepare(&editor, &media, &context),
            c.start(),
            c.duration() as usize,
            257
        )
        .iter()
        .flatten()
        .any(|s| s.abs() > 0.01)
    );
    let before = editor.project().snapshot();
    editor.execute(Command::ReverseClips(vec![clip])).unwrap();
    editor.execute(Command::ReverseClips(vec![clip])).unwrap();
    assert_eq!(editor.project().snapshot(), before);
}

#[test]
fn directed_audio_refuses_unsupported_tempo_ratio_before_warming_pcm() {
    let (mut editor, clip, media) = fixture(false);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: Tempo::new(1_000_000_000).unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_ratio(1, 2).unwrap(),
            tempo: Tempo::new(1_000_000).unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipMusical {
            clip,
            enabled: true,
        })
        .unwrap();
    editor.execute(Command::ReverseClips(vec![clip])).unwrap();
    let context = isolated(64 * 1024 * 1024, 32 * 1024 * 1024, 8);
    let result = crate::PreparedRevision::prepare_with_audio_transforms(
        editor.project(),
        &media,
        Arc::new(crate::SineTable::new()),
        None,
        &context,
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().0.contains("1/8..8"));
    assert_eq!(context.retained_pcm_bytes(), 0);
}

#[test]
fn reverse_musical_repeat_filter_uses_neighbor_occurrences_at_internal_joins() {
    let (mut editor, clip, mut media) = fixture(true);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_raw(2 * BeatTime::ONE.raw()),
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    let c = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
    let edit = c.edit.as_mut().unwrap();
    edit.view_start = 0;
    edit.view_end = BeatTime::ONE.raw();
    edit.placement = Some(AudioViewPlacement {
        mode: AudioViewMode::Repeat,
        outer_length: AudioDomainClock::Beats(BeatTime::from_raw(4 * BeatTime::ONE.raw())),
        cycle_phase: 0,
    });
    c.duration = 48_000;
    c.fade = ClipFade::neutral(c.duration).unwrap();
    let asset = *media.keys().next().unwrap();
    media.insert(asset, Audio::new(16_000, vec![[0.25; 2]; 8_000]).unwrap());
    editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 32).unwrap();
    editor.execute(Command::ReverseClips(vec![clip])).unwrap();
    let actual = render(
        &prepare(&editor, &media, &AudioTransformPreparation::default()),
        0,
        48_000,
        127,
    );
    // Two reflected one-beat cycles occupy the faster first two beats. Their
    // join is continuous content, not a zero-padded root boundary.
    for join in [8_000] {
        for (offset, frame) in actual[join - 24..join + 24].iter().enumerate() {
            assert!(
                (frame[0] - 0.25).abs() < 0.0001,
                "join {join} offset {offset}: {}",
                frame[0]
            );
        }
    }
}

#[test]
fn reversed_edited_repeat_keeps_pcm_when_musical_placement_is_disabled() {
    let (mut editor, clip, media) = fixture(false);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let settings = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(settings),
        })
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip,
            operation: hex_project::AudioEventEdit::Split {
                at: BeatTime::ONE.raw() / 2,
            },
        })
        .unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_ratio(1, 2).unwrap(),
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    let duration = editor
        .project()
        .tempo_map()
        .beat_to_frame(BeatTime::from_ratio(2, 1).unwrap())
        .unwrap();
    let mut snapshot = editor.project().snapshot();
    let c = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
    c.edit.as_mut().unwrap().placement = Some(AudioViewPlacement {
        mode: AudioViewMode::Repeat,
        outer_length: AudioDomainClock::Beats(BeatTime::from_ratio(2, 1).unwrap()),
        cycle_phase: BeatTime::ONE.raw() / 4,
    });
    c.duration = duration;
    c.fade = ClipFade::neutral(duration).unwrap();
    editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 32).unwrap();
    editor.execute(Command::ReverseClips(vec![clip])).unwrap();
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: hex_project::TrimEdge::Left,
            frame: 2_000,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: None,
        })
        .unwrap();
    let before = editor.project().snapshot();
    let c = editor.project().clip(clip).unwrap();
    let bounds = (c.start(), c.duration());
    let edit = c.audio_edit().unwrap();
    let origin = edit.processing_origin().unwrap();
    let content = edit.content_view;
    let context = AudioTransformPreparation::default();
    let pcm = render(
        &prepare(&editor, &media, &context),
        bounds.0,
        bounds.1 as usize,
        127,
    );
    assert!(pcm.iter().flatten().any(|sample| sample.abs() > 0.01));
    editor
        .execute(Command::SetAudioClipMusical {
            clip,
            enabled: false,
        })
        .unwrap();
    let frozen = editor.project().snapshot();
    let c = editor.project().clip(clip).unwrap();
    assert_eq!((c.start(), c.duration()), bounds);
    assert_eq!(c.audio_edit().unwrap().content_view, content);
    assert_eq!(c.audio_edit().unwrap().processing_origin().unwrap(), origin);
    assert_eq!(
        render(
            &prepare(&editor, &media, &context),
            bounds.0,
            bounds.1 as usize,
            257
        ),
        pcm
    );
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), frozen);
    // Global tempo changes retain musical starts even for unwarped audio,
    // while the frozen processing clock preserves duration and relative PCM.
    let onset = editor
        .project()
        .tempo_map()
        .frame_to_beat(bounds.0)
        .unwrap();
    editor
        .execute(Command::SetProjectTempo {
            tempo: Tempo::new(143_000_000).unwrap(),
        })
        .unwrap();
    let c = editor.project().clip(clip).unwrap();
    assert_eq!(
        c.start(),
        editor.project().tempo_map().beat_to_frame(onset).unwrap()
    );
    assert_eq!(c.duration(), bounds.1);
    assert_eq!(
        render(
            &prepare(&editor, &media, &context),
            c.start(),
            bounds.1 as usize,
            31
        ),
        pcm
    );
}
