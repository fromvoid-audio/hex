//! Grouping preserves rendered finite source output, media custody and silence.
use super::*;

fn group_fixture() -> (
    Editor,
    Vec<hex_project::ClipId>,
    BTreeMap<hex_project::AssetId, Audio>,
) {
    let (mut editor, first, mut media) = fixture(true);
    editor
        .execute(Command::SetAudioClipGain {
            clip: first,
            millidb: -3_500,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipPitch {
            clip: first,
            cents: 137,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipSourceGain {
            clip: first,
            points: vec![
                SourceGainPoint {
                    frame: 0,
                    gain_q16: 65_536,
                },
                SourceGainPoint {
                    frame: 3_000,
                    gain_q16: 32_768,
                },
                SourceGainPoint {
                    frame: 8_000,
                    gain_q16: 65_536,
                },
            ],
        })
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip: first,
            operation: hex_project::AudioEventEdit::Delete {
                start: 1_100,
                end: 2_100,
            },
        })
        .unwrap();
    let Created::Imported {
        clip: second,
        asset,
        ..
    } = editor
        .execute(Command::ImportAudioTrack {
            name: "second".into(),
            path: "second.wav".into(),
            sample_rate: 16_000,
            frames: 6_000,
        })
        .unwrap()
    else {
        panic!("import")
    };
    media.insert(
        asset,
        Audio::new(
            16_000,
            (0..6_000).map(|i| [i as f32 / 30_000.0, -0.15]).collect(),
        )
        .unwrap(),
    );
    let mut snapshot = editor.project().snapshot();
    let mut second_clip = snapshot.tracks[1].audio_clips_mut().unwrap().remove(0);
    second_clip.start = 11_000;
    second_clip.duration = 4_000;
    second_clip.source_offset = 37;
    second_clip.source_length = 997;
    second_clip.source_phase = 173;
    second_clip.looping = true;
    second_clip.fade = hex_project::ClipFade::new(4_000, 0, 731, 1_100).unwrap();
    second_clip.gain = hex_project::ClipGain::new(-1_001).unwrap();
    snapshot.tracks[0]
        .audio_clips_mut()
        .unwrap()
        .push(second_clip);
    snapshot.tracks.remove(1);
    (
        Editor::new(Project::from_snapshot(snapshot).unwrap(), 32).unwrap(),
        vec![first, second],
        media,
    )
}

#[test]
fn retained_event_gain_matches_edit_before_group_and_undo() {
    let (mut editor, ids, media) = group_fixture();
    let mut reference = Editor::new(editor.project().clone(), 32).unwrap();
    let context = isolated(16 * 1024 * 1024, 8 * 1024 * 1024, 16);
    let gain = hex_project::ClipGain::new(-6000).unwrap();
    reference
        .execute(Command::EditAudioEvents {
            clip: ids[0],
            operation: hex_project::AudioEventEdit::SetGain { event: 1, gain },
        })
        .unwrap();
    reference.execute(Command::GroupClips(ids.clone())).unwrap();
    let expected = render(&prepare(&reference, &media, &context), 0, 16000, 127);
    let Created::Clip(group) = editor.execute(Command::GroupClips(ids)).unwrap() else {
        panic!("group")
    };
    let baseline = render(&prepare(&editor, &media, &context), 0, 16000, 127);
    let source = editor.project().clip(group).unwrap().audio_events().events[0].source;
    editor
        .execute(Command::EditAudioEvents {
            clip: group,
            operation: hex_project::AudioEventEdit::SetRetainedGain {
                sources: vec![source],
                event: 1,
                gain,
            },
        })
        .unwrap();
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 16000, 257),
        expected
    );
    assert_ne!(expected, baseline);
    assert!(editor.undo());
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 16000, 64),
        baseline
    );
}

#[test]
fn retained_source_event_can_be_reactivated_without_losing_processing() {
    let (mut editor, ids, media) = group_fixture();
    let context = isolated(16 * 1024 * 1024, 8 * 1024 * 1024, 16);
    let expected = render(&prepare(&editor, &media, &context), 0, 16_000, 127);
    editor
        .execute(Command::EditAudioEvents {
            clip: ids[0],
            operation: hex_project::AudioEventEdit::SetActive {
                events: vec![1],
                active: false,
            },
        })
        .unwrap();
    let muted = render(&prepare(&editor, &media, &context), 0, 16_000, 127);
    assert_ne!(muted, expected);
    let Created::Clip(group) = editor.execute(Command::GroupClips(ids)).unwrap() else {
        panic!("group")
    };
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 16_000, 64),
        muted
    );
    let source = editor.project().clip(group).unwrap().audio_events().events[0].source;
    editor
        .execute(Command::EditAudioEvents {
            clip: group,
            operation: hex_project::AudioEventEdit::SetRetainedActive {
                source,
                events: vec![1],
                active: true,
            },
        })
        .unwrap();
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 16_000, 257),
        expected
    );
    assert!(editor.undo());
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 16_000, 127),
        muted
    );
}

#[test]
fn repeated_nested_group_preserves_pcm_and_reactivates_inner_event() {
    let (mut editor, ids, mut media) = group_fixture();
    editor
        .execute(Command::SetClipActive {
            clip: ids[1],
            active: false,
        })
        .unwrap();
    let Created::Clip(group) = editor.execute(Command::GroupClips(ids)).unwrap() else {
        panic!("group")
    };
    editor
        .execute(Command::ResizeAudioClipView {
            clip: group,
            operation: hex_project::AudioViewResizeOperation::RepeatOuterEnd {
                project_frame: 20_000,
            },
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipGain {
            clip: group,
            millidb: -2_300,
        })
        .unwrap();
    let inner_event = editor
        .project()
        .clip(group)
        .unwrap()
        .audio_events()
        .events
        .iter()
        .find(|event| !event.active)
        .unwrap()
        .id;
    let Created::Imported {
        clip: third, asset, ..
    } = editor
        .execute(Command::ImportAudioTrack {
            name: "third".into(),
            path: "third.wav".into(),
            sample_rate: 16_000,
            frames: 2_000,
        })
        .unwrap()
    else {
        panic!("import")
    };
    media.insert(asset, Audio::new(16_000, vec![[0.2, -0.1]; 2_000]).unwrap());
    let mut snapshot = editor.project().snapshot();
    let mut third_clip = snapshot.tracks[1].audio_clips_mut().unwrap().remove(0);
    third_clip.start = 24_000;
    snapshot.tracks[0]
        .audio_clips_mut()
        .unwrap()
        .push(third_clip);
    snapshot.tracks.remove(1);
    let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 32).unwrap();
    let context = isolated(32 * 1024 * 1024, 16 * 1024 * 1024, 32);
    let muted = render(&prepare(&editor, &media, &context), 0, 27_000, 127);
    editor
        .execute(Command::EditAudioEvents {
            clip: group,
            operation: hex_project::AudioEventEdit::SetActive {
                events: vec![inner_event],
                active: true,
            },
        })
        .unwrap();
    let active = render(&prepare(&editor, &media, &context), 0, 27_000, 257);
    assert_ne!(active, muted);
    assert!(editor.undo());
    let Created::Clip(outer) = editor
        .execute(Command::GroupClips(vec![group, third]))
        .unwrap()
    else {
        panic!("regroup")
    };
    for block in [64, 257] {
        assert_eq!(
            render(&prepare(&editor, &media, &context), 0, 27_000, block),
            muted
        );
    }
    let source = editor
        .project()
        .clip(outer)
        .unwrap()
        .audio_events()
        .sources
        .iter()
        .find(|root| root.clip.has_retained_audio_roots())
        .unwrap()
        .id;
    editor
        .execute(Command::EditAudioEvents {
            clip: outer,
            operation: hex_project::AudioEventEdit::SetRetainedPathActive {
                sources: vec![source],
                events: vec![inner_event],
                active: true,
            },
        })
        .unwrap();
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 27_000, 127),
        active
    );
    assert!(editor.undo());
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 27_000, 64),
        muted
    );
    let track = editor.project().tracks()[0].id();
    editor
        .execute(Command::GroupTimeRange(
            hex_project::TimeRange::new(vec![track], 1_000, 25_000).unwrap(),
        ))
        .unwrap();
    for block in [64, 257] {
        assert_eq!(
            render(&prepare(&editor, &media, &context), 0, 27_000, block),
            muted
        );
    }
}

#[test]
fn directed_nested_group_preserves_pcm_and_undo() {
    let (mut editor, ids, media) = group_fixture();
    let Created::Clip(group) = editor.execute(Command::GroupClips(ids)).unwrap() else {
        panic!("group")
    };
    editor.execute(Command::ReverseClips(vec![group])).unwrap();
    let track = editor.project().tracks()[0].id();
    let asset = editor.project().clip(group).unwrap().asset();
    let Created::Clip(third) = editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 17_000,
            duration: 1_000,
            source_offset: 0,
            source_length: 1_000,
            looping: false,
        })
        .unwrap()
    else {
        panic!("third")
    };
    let context = isolated(32 * 1024 * 1024, 16 * 1024 * 1024, 32);
    let expected = render(&prepare(&editor, &media, &context), 0, 19_000, 127);
    editor
        .execute(Command::GroupClips(vec![group, third]))
        .unwrap();
    for block in [64, 257] {
        assert_eq!(
            render(&prepare(&editor, &media, &context), 0, 19_000, block),
            expected
        );
    }
    assert!(editor.undo());
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 19_000, 64),
        expected
    );
}

#[test]
fn mixed_group_preserves_silence_and_reactivation_restores_original_audio() {
    let (mut editor, ids, media) = group_fixture();
    let context = isolated(16 * 1024 * 1024, 8 * 1024 * 1024, 16);
    let active = render(&prepare(&editor, &media, &context), 0, 16_000, 127);
    editor
        .execute(Command::SetClipActive {
            clip: ids[1],
            active: false,
        })
        .unwrap();
    let muted = render(&prepare(&editor, &media, &context), 0, 16_000, 127);
    assert_ne!(active, muted);
    let Created::Clip(group) = editor.execute(Command::GroupClips(ids)).unwrap() else {
        panic!("group")
    };
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 16_000, 257),
        muted
    );
    let event = editor
        .project()
        .clip(group)
        .unwrap()
        .audio_events()
        .events
        .iter()
        .find(|event| !event.active)
        .unwrap()
        .id;
    editor
        .execute(Command::EditAudioEvents {
            clip: group,
            operation: hex_project::AudioEventEdit::SetActive {
                events: vec![event],
                active: true,
            },
        })
        .unwrap();
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 16_000, 64),
        active
    );
    assert!(editor.undo());
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 16_000, 127),
        muted
    );
}

#[test]
fn grouped_sources_preserve_exact_pcm_gaps_gains_fades_loops_offsets_and_undo() {
    let (mut editor, ids, media) = group_fixture();
    let context = isolated(16 * 1024 * 1024, 8 * 1024 * 1024, 16);
    let before = prepare(&editor, &media, &context);
    let expected = render(&before, 0, 16_000, 127);
    let Created::Clip(group) = editor.execute(Command::GroupClips(ids)).unwrap() else {
        panic!("group")
    };
    let after = prepare(&editor, &media, &context);
    for block in [1, 64, 257, 1_024] {
        assert_eq!(render(&after, 0, 16_000, block), expected);
        assert_eq!(
            render(&after, 1_031, 13_200, block),
            expected[1_031..14_231]
        );
    }
    assert!(
        expected[8_000..11_000]
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
    let hits = context.cache_hits();
    let _again = prepare(&editor, &media, &context);
    assert!(context.cache_hits() > hits);
    assert_eq!(
        editor
            .project()
            .clip(group)
            .unwrap()
            .audio_events()
            .sources
            .len(),
        2
    );
    assert!(editor.undo());
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 16_000, 311),
        expected
    );
    assert!(editor.redo());
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 16_000, 311),
        expected
    );
    editor
        .execute(Command::SetClipFades {
            clip: group,
            fade_in: 1_500,
            fade_out: 2_100,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipGain {
            clip: group,
            millidb: -2_000,
        })
        .unwrap();
    let fade = hex_project::ClipFade::new(15_000, 0, 1_500, 2_100).unwrap();
    let gain = hex_project::ClipGain::new(-2_000).unwrap().linear();
    let mut faded = expected.clone();
    for (index, frame) in faded.iter_mut().take(15_000).enumerate() {
        let factor = fade.gain_at(index as u64);
        frame[0] *= factor;
        frame[1] *= factor;
        frame[0] *= gain;
        frame[1] *= gain;
    }
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 16_000, 127),
        faded
    );
    assert!(editor.undo());
    assert!(editor.undo());
    // A changed non-anchor decoded asset must invalidate the group cache.
    let second_asset = editor.project().clip(group).unwrap().audio_events().sources[1]
        .clip
        .asset();
    let mut changed_media = media.clone();
    changed_media.insert(
        second_asset,
        Audio::new(16_000, vec![[0.0; 2]; 6_000]).unwrap(),
    );
    let changed = render(&prepare(&editor, &changed_media, &context), 0, 16_000, 257);
    assert_eq!(&changed[..11_000], &expected[..11_000]);
    assert!(changed[11_000..].iter().all(|frame| *frame == [0.0; 2]));
}

#[test]
fn grouped_event_moves_outer_crops_and_repeat_use_retained_source_coordinates() {
    let (mut editor, ids, media) = group_fixture();
    let context = isolated(16 * 1024 * 1024, 8 * 1024 * 1024, 16);
    let original = render(&prepare(&editor, &media, &context), 0, 16_000, 127);
    let Created::Clip(group) = editor.execute(Command::GroupClips(ids)).unwrap() else {
        panic!("group")
    };
    editor
        .execute(Command::EditAudioEvents {
            clip: group,
            operation: hex_project::AudioEventEdit::Split { at: 1_000 },
        })
        .unwrap();
    let moved = editor
        .project()
        .clip(group)
        .unwrap()
        .audio_events()
        .events
        .iter()
        .find(|event| event.start == 1_000)
        .unwrap()
        .id;
    editor
        .execute(Command::EditAudioEvents {
            clip: group,
            operation: hex_project::AudioEventEdit::Trim {
                event: moved,
                start: 1_200,
                end: 3_000,
            },
        })
        .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip: group,
            operation: hex_project::AudioEventEdit::Move {
                event: moved,
                start: 7_100,
            },
        })
        .unwrap();
    let edited = render(&prepare(&editor, &media, &context), 0, 16_000, 257);
    assert_eq!(&edited[7_100..8_900], &original[1_200..3_000]);
    let Created::Clip(right) = editor
        .execute(Command::SplitClip {
            clip: group,
            frame: 5_500,
        })
        .unwrap()
    else {
        panic!("split")
    };
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 16_000, 71),
        edited
    );
    editor.execute(Command::RemoveClip(right)).unwrap();
    editor
        .execute(Command::ResizeAudioClipView {
            clip: group,
            operation: hex_project::AudioViewResizeOperation::RepeatOuterEnd {
                project_frame: 12_100,
            },
        })
        .unwrap();
    let repeated = render(&prepare(&editor, &media, &context), 0, 13_000, 257);
    for index in 0..12_100 {
        assert_eq!(repeated[index], edited[index % 5_500]);
    }
}

#[test]
fn grouped_preparation_refusal_keeps_history_and_pending_pcm_atomic() {
    let (mut editor, ids, media) = group_fixture();
    let before = editor.project().clone();
    let context = isolated(1_024, 1_024, 4);
    assert!(
        editor
            .execute_prepared(Command::GroupClips(ids.clone()), |candidate| {
                crate::PreparedRevision::prepare_with_audio_transforms(
                    candidate,
                    &media,
                    Arc::new(crate::SineTable::new()),
                    None,
                    &context,
                )
            })
            .is_err()
    );
    assert_eq!(*editor.project(), before);
    assert_eq!(context.retained_pcm_bytes(), 0);
    assert_eq!(context.cached_pcm_bytes().unwrap(), 0);
    let context = isolated(16 * 1024 * 1024, 8 * 1024 * 1024, 16);
    context.cancel_after_polls.set(Some(8));
    assert!(
        editor
            .execute_prepared(Command::GroupClips(ids), |candidate| {
                crate::PreparedRevision::prepare_with_audio_transforms(
                    candidate,
                    &media,
                    Arc::new(crate::SineTable::new()),
                    None,
                    &context,
                )
            })
            .is_err()
    );
    assert_eq!(*editor.project(), before);
    assert_eq!(context.retained_pcm_bytes(), 0);
    assert_eq!(context.cached_pcm_bytes().unwrap(), 0);
}

#[test]
fn grouped_warped_sources_keep_captured_tempo_and_resident_source_requirement() {
    let (mut editor, ids, media) = group_fixture();
    let first = ids[0];
    let settings = default_audio_warp(
        editor.project().clip(first).unwrap(),
        editor.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip: first,
            settings: Some(settings),
        })
        .unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_raw(BeatTime::ONE.raw() / 2),
            tempo: Tempo::new(150_000_000).unwrap(),
        })
        .unwrap();
    let context = isolated(16 * 1024 * 1024, 8 * 1024 * 1024, 16);
    let expected = render(&prepare(&editor, &media, &context), 0, 16_000, 127);
    let Created::Clip(group) = editor.execute(Command::GroupClips(ids)).unwrap() else {
        panic!("group")
    };
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 16_000, 257),
        expected
    );
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 16_000, 311),
        expected
    );
    let sources: BTreeMap<_, _> = media
        .iter()
        .map(|(id, audio)| {
            (
                *id,
                crate::compile::PreparedAudioSource::Resident(audio.clone()),
            )
        })
        .collect();
    let mut streamed = sources.clone();
    let asset_id = editor.project().clip(group).unwrap().audio_events().sources[1]
        .clip
        .asset();
    let asset = editor.project().asset(asset_id).unwrap();
    streamed.insert(
        asset_id,
        crate::compile::PreparedAudioSource::Streamed(Arc::new(
            crate::StreamedSourceMetadata::new(
                crate::StreamSourceKey::new(1, asset_id),
                asset.source_sample_rate(),
                editor.project().sample_rate(),
                asset.source_frames(),
                asset.playback_frames(),
                asset.conversion_version(),
                [7; 32],
            )
            .unwrap(),
        )),
    );
    assert!(
        crate::compile::project_with_prepared_audio_sources(editor.project(), &sources, 1).is_ok()
    );
    let before = editor.project().clone();
    assert!(
        editor
            .execute_prepared(
                Command::EditAudioEvents {
                    clip: group,
                    operation: hex_project::AudioEventEdit::Split { at: 997 }
                },
                |candidate| crate::compile::project_with_prepared_audio_sources(
                    candidate, &streamed, 1
                )
            )
            .is_err()
    );
    assert_eq!(*editor.project(), before);
}

#[test]
fn grouped_aggregate_source_work_is_admitted_before_pcm_allocation() {
    let (mut editor, ids, media) = group_fixture();
    // Retain a finite looping source longer than the group reads. This requires
    // no large source PCM, but must still be charged as finite preparation work.
    let mut source_snapshot = editor.project().snapshot();
    let clips = source_snapshot.tracks[0].audio_clips_mut().unwrap();
    clips.remove(0);
    clips[0].start = 0;
    clips[0].duration = hex_project::MAX_AUDIO_DOMAIN_FRAMES;
    clips[0].fade = hex_project::ClipFade::neutral(clips[0].duration).unwrap();
    let source = Project::from_snapshot(source_snapshot)
        .unwrap()
        .clip(ids[1])
        .unwrap()
        .clone();
    let Created::Clip(group) = editor.execute(Command::GroupClips(ids)).unwrap() else {
        panic!("group")
    };
    let mut snapshot = editor.project().snapshot();
    let clip = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
    let edit = clip.edit.as_mut().unwrap();
    let sequence = Arc::make_mut(Arc::make_mut(&mut edit.domain).events.as_mut().unwrap());
    sequence.sources[1].clip = source;
    let editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 8).unwrap();
    assert_eq!(editor.project().clip(group).unwrap().duration(), 15_000);
    let context = isolated(16 * 1024 * 1024, 8 * 1024 * 1024, 16);
    let result = crate::PreparedRevision::prepare_with_audio_transforms(
        editor.project(),
        &media,
        Arc::new(crate::SineTable::new()),
        None,
        &context,
    );
    assert!(matches!(
        result,
        Err(Invalid(
            "grouped audio exceeds bounded aggregate source preparation work"
        ))
    ));
    assert_eq!(context.retained_pcm_bytes(), 0);
    assert_eq!(context.cached_pcm_bytes().unwrap(), 0);
}

#[test]
fn grouped_finite_source_placements_keep_processed_root_crops_and_phase() {
    let (editor, ids, media) = group_fixture();
    let mut snapshot = editor.project().snapshot();
    let clip = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
    let edit = clip.edit.as_mut().unwrap();
    edit.view_start = 300;
    edit.view_end = 2_300;
    edit.placement = Some(hex_project::AudioViewPlacement {
        mode: hex_project::AudioViewMode::Repeat,
        outer_length: AudioDomainClock::Frames(8_000),
        cycle_phase: 173,
    });
    let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap();
    let context = isolated(16 * 1024 * 1024, 8 * 1024 * 1024, 16);
    let expected = render(&prepare(&editor, &media, &context), 0, 16_000, 127);
    editor.execute(Command::GroupClips(ids)).unwrap();
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 16_000, 257),
        expected
    );
}

#[test]
fn grouped_audio_event_range_preserves_remainders_holes_pcm_and_one_history_edge() {
    let (mut editor, ids, media) = group_fixture();
    let context = isolated(16 * 1024 * 1024, 8 * 1024 * 1024, 16);
    let expected = render(&prepare(&editor, &media, &context), 0, 16_000, 127);
    let Created::Clip(group) = editor.execute(Command::GroupClips(ids)).unwrap() else {
        panic!("group")
    };
    let Created::Clip(center) = editor
        .execute(Command::GroupAudioEventRange {
            clip: group,
            start: 1_000,
            end: 13_000,
        })
        .unwrap()
    else {
        panic!("group event range")
    };
    let mut placed: Vec<_> = editor.project().tracks()[0]
        .clips()
        .iter()
        .map(|clip| (clip.id(), clip.start(), clip.duration()))
        .collect();
    placed.sort_by_key(|(_, start, _)| *start);
    assert_eq!(
        placed
            .iter()
            .map(|(_, start, duration)| (*start, *duration))
            .collect::<Vec<_>>(),
        vec![(0, 1_000), (1_000, 12_000), (13_000, 2_000)]
    );
    assert_eq!(placed[1].0, center);
    let revision = prepare(&editor, &media, &context);
    for block in [1, 127, 257] {
        assert_eq!(render(&revision, 0, 16_000, block), expected);
        assert_eq!(render(&revision, 997, 13_010, block), expected[997..14_007]);
    }
    assert!(editor.undo());
    assert_eq!(editor.project().tracks()[0].clips().len(), 1);
    assert!(editor.project().clip(group).is_some());
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 16_000, 311),
        expected
    );
    assert!(editor.redo());
    let mut restored: Vec<_> = editor.project().tracks()[0]
        .clips()
        .iter()
        .map(|clip| (clip.id(), clip.start(), clip.duration()))
        .collect();
    restored.sort_by_key(|(_, start, _)| *start);
    assert_eq!(restored, placed);
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 16_000, 71),
        expected
    );
}

#[test]
fn grouped_finite_preview_keeps_complete_composite_root_and_separate_placement_cache() {
    let (mut editor, ids, mut media) = group_fixture();
    let context = isolated(16 * 1024 * 1024, 8 * 1024 * 1024, 16);
    let expected = render(&prepare(&editor, &media, &context), 0, 15_000, 127);
    let Created::Clip(group) = editor.execute(Command::GroupClips(ids)).unwrap() else {
        panic!("group")
    };
    let Created::Clip(tail) = editor
        .execute(Command::SplitClip {
            clip: group,
            frame: 5_500,
        })
        .unwrap()
    else {
        panic!("split")
    };
    editor.execute(Command::RemoveClip(tail)).unwrap();
    editor
        .execute(Command::ResizeAudioClipView {
            clip: group,
            operation: hex_project::AudioViewResizeOperation::RepeatOuterEnd {
                project_frame: 12_100,
            },
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipGain {
            clip: group,
            millidb: -6_000,
        })
        .unwrap();
    let root = context
        .preview_root_with_sources(editor.project(), group, |asset| media.get(&asset))
        .unwrap()
        .unwrap();
    assert_eq!(root.audio.frames(), expected);
    assert!(
        root.audio.frames()[8_000..11_000]
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
    let again = context
        .preview_root_with_sources(editor.project(), group, |asset| media.get(&asset))
        .unwrap()
        .unwrap();
    assert!(root.audio.shares_pcm_with(&again.audio));
    let gain = editor.project().clip(group).unwrap().gain().linear();
    let repeated = render(&prepare(&editor, &media, &context), 0, 12_100, 257);
    for (index, actual) in repeated.iter().enumerate() {
        assert_eq!(*actual, expected[index % 5_500].map(|sample| sample * gain));
    }
    let anchor = editor.project().clip(group).unwrap().asset();
    assert!(
        context
            .preview_root(editor.project(), group, &media[&anchor])
            .is_err()
    );
    let second = *media.keys().find(|asset| **asset != anchor).unwrap();
    assert!(
        context
            .preview_root_with_sources(editor.project(), group, |asset| {
                (asset != second).then(|| &media[&asset])
            })
            .is_err()
    );
    media.insert(second, Audio::new(16_000, vec![[0.2; 2]; 6_000]).unwrap());
    let changed = context
        .preview_root_with_sources(editor.project(), group, |asset| media.get(&asset))
        .unwrap()
        .unwrap();
    assert!(!changed.audio.shares_pcm_with(&root.audio));
    assert_ne!(
        &changed.audio.frames()[11_000..15_000],
        &expected[11_000..15_000]
    );
}

#[test]
fn grouping_directed_children_retains_their_captured_processing_clock() {
    let (mut editor, ids, media) = group_fixture();
    let first = ids[0];
    let settings = default_audio_warp(
        editor.project().clip(first).unwrap(),
        editor.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip: first,
            settings: Some(settings),
        })
        .unwrap();
    editor.execute(Command::ReverseClips(ids.clone())).unwrap();
    let context = AudioTransformPreparation::default();
    let expected = render(&prepare(&editor, &media, &context), 0, 16000, 127);
    editor.execute(Command::GroupClips(ids)).unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: Tempo::new(180_000_000).unwrap(),
        })
        .unwrap();

    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 16000, 257),
        expected
    );
}
#[test]
fn reversing_a_group_reflects_composite_pcm_but_keeps_destination_fades_forward() {
    let (mut editor, ids, media) = group_fixture();
    let context = AudioTransformPreparation::default();
    let Created::Clip(group) = editor.execute(Command::GroupClips(ids)).unwrap() else {
        panic!("group");
    };
    let duration = editor.project().clip(group).unwrap().duration() as usize;
    let forward = render(&prepare(&editor, &media, &context), 0, duration, 127);
    editor
        .execute(Command::SetClipFades {
            clip: group,
            fade_in: 101,
            fade_out: 179,
        })
        .unwrap();
    editor.execute(Command::ReverseClips(vec![group])).unwrap();
    let expected: Vec<_> = forward
        .iter()
        .rev()
        .enumerate()
        .map(|(i, frame)| {
            let gain =
                ((i as f64 / 100.0).min(1.0) * ((duration - 1 - i) as f64 / 178.0).min(1.0)) as f32;
            [frame[0] * gain, frame[1] * gain]
        })
        .collect();
    let actual = render(&prepare(&editor, &media, &context), 0, duration, 257);
    let mismatch = actual.iter().zip(&expected).position(|(a, b)| a != b);
    assert!(
        mismatch.is_none(),
        "composite reflection/fade differs at {mismatch:?}"
    );
}

#[test]
fn grouped_child_and_event_fades_keep_source_phase_through_move_and_history() {
    let (mut editor, ids, media) = group_fixture();
    let child = ids[0];
    let event = editor.project().clip(child).unwrap().audio_events().events[0];
    editor
        .execute(Command::EditAudioEvents {
            clip: child,
            operation: hex_project::AudioEventEdit::SetFade {
                event: event.id,
                fade: Some(
                    hex_project::AudioEventFade::new(
                        event.source_start,
                        hex_project::ClipFade::new(event.length(), 0, 37, 53).unwrap(),
                    )
                    .unwrap(),
                ),
            },
        })
        .unwrap();
    let context = AudioTransformPreparation::default();
    let before = render(&prepare(&editor, &media, &context), 0, 15_000, 127);
    let Created::Clip(group) = editor.execute(Command::GroupClips(ids)).unwrap() else {
        panic!("group")
    };
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 15_000, 17),
        before
    );
    let first = editor.project().clip(group).unwrap().audio_events().events[0];
    let length = first.length() as usize;
    let fade = hex_project::AudioEventFade::new(
        first.source_start,
        hex_project::ClipFade::new(first.length(), 0, 101, 179).unwrap(),
    )
    .unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip: group,
            operation: hex_project::AudioEventEdit::SetFade {
                event: first.id,
                fade: Some(fade),
            },
        })
        .unwrap();
    let mut expected = before.clone();
    for (i, frame) in expected.iter_mut().take(length).enumerate() {
        let gain =
            ((i as f64 / 100.0).min(1.0) * ((length - 1 - i) as f64 / 178.0).min(1.0)) as f32;
        frame[0] *= gain;
        frame[1] *= gain;
    }
    for block in [1, 17, 257] {
        assert_eq!(
            render(&prepare(&editor, &media, &context), 0, 15_000, block),
            expected
        );
    }
    editor
        .execute(Command::EditAudioEvents {
            clip: group,
            operation: hex_project::AudioEventEdit::Move {
                event: first.id,
                start: 1_000,
            },
        })
        .unwrap();
    let mut moved = expected.clone();
    moved[..length + 1_000].fill([0.0; 2]);
    moved[1_000..length + 1_000].copy_from_slice(&expected[..length]);
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 15_000, 31),
        moved
    );
    assert!(editor.undo());
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 15_000, 257),
        expected
    );
    assert!(editor.redo());
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 15_000, 17),
        moved
    );
}

#[test]
fn thousand_group_instances_share_one_rendered_sample_and_keep_history_pcm() {
    let (mut editor, ids, media) = group_fixture();
    let Created::Clip(group) = editor.execute(Command::GroupClips(ids)).unwrap() else {
        panic!("group");
    };
    let context = isolated(16 * 1024 * 1024, 8 * 1024 * 1024, 16);
    let original = prepare(&editor, &media, &context);
    let pcm_bytes = context.retained_pcm_bytes();
    let audio = original.arrangement().audio_clip_views()[0].audio.clone();
    let expected = render(&original, 0, 15_000, 127);
    let track = editor.project().tracks()[0].id();
    let clipboard = Arc::new(editor.project().capture_clips(&[group]).unwrap());
    // Check a real paste before constructing the large population.
    let paste = |index: u64| Command::PasteClips {
        clipboard: Arc::clone(&clipboard),
        track,
        frame: index * 15_000,
        insert: false,
    };
    editor.execute(paste(1)).unwrap();
    let first = prepare(&editor, &media, &context);
    assert_eq!(context.retained_pcm_bytes(), pcm_bytes);
    assert!(
        first
            .arrangement()
            .audio_clip_views()
            .iter()
            .all(|view| view.audio.shares_pcm_with(&audio))
    );
    assert_eq!(render(&first, 15_000, 15_000, 63), expected);
    // Populate from the actual pasted instance; the last paste stays a real
    // editor command so history still removes and restores precisely that copy.
    let mut snapshot = editor.project().snapshot();
    let copied = snapshot.tracks[0]
        .audio_clips_mut()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    for index in 2..1_000_u64 {
        let mut instance = copied.clone();
        instance.id = snapshot.next_id;
        snapshot.next_id += 1;
        instance.start = index * 15_000;
        snapshot.tracks[0].audio_clips_mut().unwrap().push(instance);
    }
    editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 32).unwrap();
    editor.execute(paste(1_000)).unwrap();
    let prepared = prepare(&editor, &media, &context);
    assert_eq!(prepared.arrangement().audio_clip_views().len(), 1_001);
    assert_eq!(
        context.retained_pcm_bytes(),
        pcm_bytes,
        "copying a frame group must retain references, not new destination-sized PCM"
    );
    assert!(
        prepared
            .arrangement()
            .audio_clip_views()
            .iter()
            .all(|view| view.audio.shares_pcm_with(&audio))
    );
    assert_eq!(render(&prepared, 15_000_000, 15_000, 63), expected);
    assert!(editor.undo());
    let undone = prepare(&editor, &media, &context);
    assert!(
        render(&undone, 15_000_000, 15_000, 257)
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
    assert!(editor.redo());
    let redone = prepare(&editor, &media, &context);
    assert_eq!(render(&redone, 15_000_000, 15_000, 257), expected);
    assert_eq!(context.retained_pcm_bytes(), pcm_bytes);
}

#[test]
fn split_and_copied_repeating_group_pieces_share_one_composite_root() {
    let (mut editor, ids, media) = group_fixture();
    let Created::Clip(group) = editor.execute(Command::GroupClips(ids)).unwrap() else {
        panic!("group");
    };
    let mut snapshot = editor.project().snapshot();
    let row = snapshot.tracks[0]
        .audio_clips_mut()
        .unwrap()
        .iter_mut()
        .find(|row| row.id == group.value())
        .unwrap();
    let edit = row.edit.as_mut().unwrap();
    edit.view_start = 2_000;
    edit.view_end = 9_000;
    edit.placement = Some(hex_project::AudioViewPlacement {
        mode: hex_project::AudioViewMode::Repeat,
        outer_length: AudioDomainClock::Frames(40_000),
        cycle_phase: 1_500,
    });
    row.duration = 40_000;
    row.fade = hex_project::ClipFade::neutral(row.duration).unwrap();
    let start = row.start;
    let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 32).unwrap();
    let context = isolated(16 * 1024 * 1024, 8 * 1024 * 1024, 16);
    let unsplit = prepare(&editor, &media, &context);
    let expected = render(&unsplit, start, 40_000, 127);
    assert!(expected.iter().any(|frame| *frame != [0.0; 2]));
    let root = unsplit.arrangement().audio_clip_views()[0].audio.clone();
    // The composite root plus its pitched child render; no outer placement PCM.
    let retained = context.retained_pcm_bytes();
    assert!(retained < 40_000 * std::mem::size_of::<crate::Frame>());
    let Created::Clip(tail) = editor
        .execute(Command::SplitClip {
            clip: group,
            frame: start + 23_456,
        })
        .unwrap()
    else {
        panic!("split")
    };
    let track = editor.project().tracks()[0].id();
    let clipboard = Arc::new(editor.project().capture_clips(&[tail]).unwrap());
    for copy in 0..8 {
        editor
            .execute(Command::PasteClips {
                clipboard: Arc::clone(&clipboard),
                track,
                frame: start + 40_000 + copy * 16_544,
                insert: false,
            })
            .unwrap();
    }
    let copied = prepare(&editor, &media, &context);
    assert!(
        copied
            .arrangement()
            .audio_clip_views()
            .iter()
            .all(|view| view.audio.shares_pcm_with(&root))
    );
    assert_eq!(context.retained_pcm_bytes(), retained);
    assert_eq!(render(&copied, start, 40_000, 257), expected);
    assert_eq!(
        render(&copied, start + 40_000 + 7 * 16_544, 16_544, 31),
        expected[23_456..]
    );
}

#[test]
fn distant_events_do_not_increase_finite_once_playback_pcm_budget() {
    use hex_project::AudioEventEdit;
    for destination in [-1_000_000_i64, 1_000_000] {
        let (mut editor, clip, media) = fixture(false);
        let reference_context = isolated(128 * 1024, 0, 0);
        let original = render(&prepare(&editor, &media, &reference_context), 0, 8_032, 57);
        for operation in [
            AudioEventEdit::Split { at: 1_000 },
            AudioEventEdit::Place {
                event: 1,
                start: destination,
            },
        ] {
            editor
                .execute(Command::EditAudioEvents { clip, operation })
                .unwrap();
        }
        let first = editor
            .project()
            .clip(clip)
            .unwrap()
            .audio_event_root()
            .view_start;
        editor
            .execute(Command::ResizeAudioClipView {
                clip,
                operation: hex_project::AudioViewResizeOperation::CycleRange {
                    start: first,
                    end: first + 4_000,
                    looping: false,
                },
            })
            .unwrap();
        let context = isolated(128 * 1024, 0, 0);
        let prepared = prepare(&editor, &media, &context);
        assert_eq!(
            context.retained_pcm_bytes(),
            8_000 * std::mem::size_of::<crate::Frame>()
        );
        for block in [1, 57, 512] {
            let actual = render(&prepared, 0, 8_032, block);
            assert!(actual[..1_000].iter().all(|sample| *sample == [0.0; 2]));
            assert_eq!(actual[1_000..4_000], original[1_000..4_000]);
            assert!(actual[4_000..].iter().all(|sample| *sample == [0.0; 2]));
        }
        // A moved event straddles the beginning of the gate. The retained
        // source advances by the omitted samples, and the container fade
        // remains at its Arrangement position.
        let placed = editor.project().clip(clip).unwrap();
        let start = placed.audio_event_root().view_start as i64 - 100;
        let fade = placed.audio_event_root().domain.fade;
        editor
            .execute(Command::EditAudioEvents {
                clip,
                operation: AudioEventEdit::Place { event: 1, start },
            })
            .unwrap();
        drop(prepared);
        let prepared = prepare(&editor, &media, &context);
        let actual = render(&prepared, 0, 8_032, 57);
        let source = media.values().next().unwrap();
        for (frame, sample) in actual[..900].iter().enumerate() {
            assert_eq!(
                *sample,
                source.frames()[frame + 100].map(|x| x * fade.gain_at(frame as u64))
            );
        }
        assert!(actual[900..1_000].iter().all(|sample| *sample == [0.0; 2]));
        assert_eq!(actual[1_000..4_000], original[1_000..4_000]);
        assert!(actual[4_000..].iter().all(|sample| *sample == [0.0; 2]));
    }
}
