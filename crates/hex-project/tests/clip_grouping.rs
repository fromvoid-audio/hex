//! Transactional grouping at canonical clip/source/history boundaries.
use hex_project::{BeatTime, ClipId, Command, Created, Editor, Project, TrackId};
fn b(n: u64) -> BeatTime {
    BeatTime::from_quarters(n).unwrap()
}
fn track(e: &mut Editor) -> TrackId {
    let Created::Track(id) = e
        .execute(Command::AddTrack {
            name: "group".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    id
}
fn midi(e: &mut Editor, t: TrackId, start: u64) -> ClipId {
    let Created::Clip(id) = e
        .execute(Command::PlaceMidiClip {
            track: t,
            start: b(start),
            length: b(2),
        })
        .unwrap()
    else {
        panic!()
    };
    e.execute(Command::AddMidiNote {
        clip: id,
        start: BeatTime::ZERO,
        duration: b(1),
        pitch: 60,
        channel: 2,
        velocity: 93,
    })
    .unwrap();
    id
}
fn audio_fixture() -> (Editor, TrackId, Vec<ClipId>) {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let t = track(&mut e);
    let mut clips = Vec::new();
    for (i, start) in [0, 200, 400].into_iter().enumerate() {
        let Created::Asset(asset) = e
            .execute(Command::AddAsset {
                path: format!("source-{i}.wav").into(),
                sample_rate: 48_000,
                frames: 1000,
            })
            .unwrap()
        else {
            panic!()
        };
        let Created::Clip(id) = e
            .execute(Command::PlaceClip {
                track: t,
                asset,
                start,
                duration: 100,
                source_offset: 17,
                source_length: 100,
                looping: false,
            })
            .unwrap()
        else {
            panic!()
        };
        clips.push(id);
    }
    (Editor::new(e.project().clone(), 16).unwrap(), t, clips)
}

#[test]
fn grouping_mixed_activation_preserves_midi_notes_in_an_active_container() {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let t = track(&mut e);
    let active = midi(&mut e, t, 0);
    let inactive = midi(&mut e, t, 4);
    let first_note = e.project().midi_clip(active).unwrap().notes()[0].id();
    e.execute(Command::SetMidiNotesActive {
        clip: active,
        notes: vec![first_note],
        active: false,
    })
    .unwrap();
    e.execute(Command::SetClipActive {
        clip: inactive,
        active: false,
    })
    .unwrap();
    let Created::Clip(group) = e
        .execute(Command::GroupClips(vec![active, inactive]))
        .unwrap()
    else {
        panic!("grouped clip")
    };
    let group = e.project().midi_clip(group).unwrap();
    assert!(group.active());
    assert_eq!(group.notes().len(), 2);
    assert_eq!(
        group
            .notes()
            .iter()
            .map(|note| note.active())
            .collect::<Vec<_>>(),
        vec![false, false]
    );
    let grouped_snapshot = e.project().snapshot();
    assert!(e.undo());
    assert!(!e.project().midi_clip(active).unwrap().notes()[0].active());
    assert!(e.redo());
    let mut restored = e.project().snapshot();
    restored.next_id = grouped_snapshot.next_id;
    assert_eq!(restored, grouped_snapshot);
}

#[test]
fn note_activation_is_atomic_reversible_and_preserves_note_data() {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let t = track(&mut e);
    let clip = midi(&mut e, t, 0);
    let original = e.project().midi_clip(clip).unwrap().notes()[0].clone();
    let note = original.id();
    let before = e.project().snapshot();
    let mut activation = Command::SetMidiNotesActive {
        clip,
        notes: vec![note],
        active: false,
    };
    assert!(activation.supports_sampler_midi_edit());
    assert!(activation.prepare_sampler_midi_edit(e.project()).unwrap());
    assert!(
        e.execute(Command::SetMidiNotesActive {
            clip,
            notes: vec![note, note],
            active: false
        })
        .is_err()
    );
    assert_eq!(e.project().snapshot(), before);
    e.execute(Command::SetMidiNotesActive {
        clip,
        notes: vec![note],
        active: false,
    })
    .unwrap();
    assert!(!activation.prepare_sampler_midi_edit(e.project()).unwrap());
    assert!(e.next_midi_history_effect(false).is_some());
    let changed = &e.project().midi_clip(clip).unwrap().notes()[0];
    assert!(!changed.active());
    assert_eq!(
        (
            changed.id(),
            changed.start(),
            changed.duration(),
            changed.pitch(),
            changed.channel(),
            changed.velocity()
        ),
        (
            original.id(),
            original.start(),
            original.duration(),
            original.pitch(),
            original.channel(),
            original.velocity()
        )
    );
    assert!(e.undo());
    assert!(e.project().midi_clip(clip).unwrap().notes()[0].active());
    assert!(e.redo());
    assert!(!e.project().midi_clip(clip).unwrap().notes()[0].active());
    e.execute(Command::SetMidiNotesActive {
        clip,
        notes: vec![note],
        active: true,
    })
    .unwrap();
    assert!(e.project().midi_clip(clip).unwrap().notes()[0].active());
}

#[test]
fn note_activation_follows_alias_source_and_undo() {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let t = track(&mut e);
    let clip = midi(&mut e, t, 0);
    let Created::Clips { first: alias, .. } =
        e.execute(Command::DuplicateAliasClips(vec![clip])).unwrap()
    else {
        panic!("alias")
    };
    let note = e.project().midi_clip(clip).unwrap().notes()[0].id();
    e.execute(Command::SetMidiNotesActive {
        clip: alias,
        notes: vec![note],
        active: false,
    })
    .unwrap();
    assert!(!e.project().midi_clip(clip).unwrap().notes()[0].active());
    assert!(!e.project().midi_clip(alias).unwrap().notes()[0].active());
    assert!(e.undo());
    assert!(e.project().midi_clip(clip).unwrap().notes()[0].active());
    assert!(e.project().midi_clip(alias).unwrap().notes()[0].active());
}

#[test]
fn partial_time_group_keeps_midi_activation_inside_and_outside() {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let t = track(&mut e);
    let a = midi(&mut e, t, 0);
    let b_clip = midi(&mut e, t, 4);
    e.execute(Command::SetClipActive {
        clip: b_clip,
        active: false,
    })
    .unwrap();
    let before = e.project().snapshot();
    let end = e.project().tempo_map().beat_to_frame(b(5)).unwrap();
    e.execute(Command::GroupTimeRange(
        hex_project::TimeRange::new(vec![t], 0, end).unwrap(),
    ))
    .unwrap();
    let row = e.project().track(t).unwrap();
    let group = row
        .midi_clips()
        .iter()
        .find(|clip| clip.start() == b(0))
        .unwrap();
    assert!(group.active());
    assert_eq!(
        group
            .notes()
            .iter()
            .map(|note| note.active())
            .collect::<Vec<_>>(),
        vec![true, false]
    );
    assert!(
        row.midi_clips()
            .iter()
            .any(|clip| clip.start() == b(5) && !clip.active())
    );
    assert!(e.undo());
    let mut restored = e.project().snapshot();
    restored.next_id = before.next_id;
    assert_eq!(restored, before);
    assert!(e.project().midi_clip(a).is_some());
}

#[test]
fn grouping_only_inactive_midi_clips_keeps_an_active_container_with_silent_notes() {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let t = track(&mut e);
    let a = midi(&mut e, t, 0);
    let b_clip = midi(&mut e, t, 4);
    for clip in [a, b_clip] {
        e.execute(Command::SetClipActive {
            clip,
            active: false,
        })
        .unwrap();
    }
    let Created::Clip(group) = e.execute(Command::GroupClips(vec![a, b_clip])).unwrap() else {
        panic!("group")
    };
    let group = e.project().midi_clip(group).unwrap();
    assert!(group.active());
    assert_eq!(group.notes().len(), 2);
    assert!(group.notes().iter().all(|note| !note.active()));
}
#[test]
fn midi_group_materializes_loops_and_gaps_with_one_history_edge_and_fresh_ids() {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let t = track(&mut e);
    let a = midi(&mut e, t, 2);
    let c = midi(&mut e, t, 6);
    e.execute(Command::SetMidiClipSource {
        clip: a,
        source_length: b(2),
        source_start: BeatTime::ZERO,
        loop_start: BeatTime::ZERO,
        loop_end: b(1),
        looping: true,
        length: b(2),
    })
    .unwrap();
    let before = e.project().snapshot();
    let old_ids: Vec<_> = e.project().tracks()[0]
        .midi_clips()
        .iter()
        .flat_map(|c| c.notes().iter().map(|n| n.id()))
        .collect();
    let Created::Clip(id) = e.execute(Command::GroupClips(vec![c, a])).unwrap() else {
        panic!()
    };
    let group = e.project().midi_clip(id).unwrap();
    assert_eq!((group.start(), group.length()), (b(2), b(6)));
    assert!(!group.looping());
    assert!(group.volume().points().is_empty());
    assert_eq!(
        group
            .notes()
            .iter()
            .map(|n| (n.start(), n.duration()))
            .collect::<Vec<_>>(),
        vec![(b(0), b(1)), (b(1), b(1)), (b(4), b(1))]
    );
    assert!(group.notes().iter().all(|n| !old_ids.contains(&n.id())));
    let after = e.project().snapshot();
    assert!(e.undo());
    let mut restored = e.project().snapshot();
    restored.next_id = before.next_id;
    assert_eq!(restored, before);
    assert!(e.redo());
    assert_eq!(e.project().snapshot(), after);
}
#[test]
fn grouping_refusals_preserve_ids_history_and_nonselected_material() {
    let (mut e, t, clips) = audio_fixture();
    let before = e.project().snapshot();
    for ids in [
        vec![],
        vec![clips[0]],
        vec![clips[0], clips[0]],
        vec![clips[0], clips[2]],
    ] {
        assert!(e.execute(Command::GroupClips(ids)).is_err());
        assert_eq!(e.project().snapshot(), before);
        assert!(!e.can_undo());
    }
    let other = track(&mut e);
    let m = midi(&mut e, other, 10);
    let state = e.project().snapshot();
    assert!(e.execute(Command::GroupClips(vec![clips[0], m])).is_err());
    assert_eq!(e.project().snapshot(), state);
    e.execute(Command::MoveMidiClip {
        clip: m,
        track: t,
        start: b(10),
    })
    .unwrap();
    let state = e.project().snapshot();
    assert!(e.execute(Command::GroupClips(vec![clips[0], m])).is_err());
    assert_eq!(e.project().snapshot(), state);
}
#[test]
fn grouping_only_inactive_audio_clips_keeps_an_active_container() {
    let (mut editor, _, clips) = audio_fixture();
    for clip in &clips[..2] {
        editor
            .execute(Command::SetClipActive {
                clip: *clip,
                active: false,
            })
            .unwrap();
    }
    let Created::Clip(group) = editor
        .execute(Command::GroupClips(clips[..2].to_vec()))
        .unwrap()
    else {
        panic!("group")
    };
    let clip = editor.project().clip(group).unwrap();
    assert!(clip.active());
    assert_eq!(clip.audio_events().events.len(), 2);
    assert!(clip.audio_events().events.iter().all(|event| !event.active));
}

#[test]
fn retained_source_event_activation_stays_editable_after_grouping() {
    let (mut editor, _, clips) = audio_fixture();
    editor
        .execute(Command::EditAudioEvents {
            clip: clips[0],
            operation: hex_project::AudioEventEdit::SetActive {
                events: vec![1],
                active: false,
            },
        })
        .unwrap();
    let Created::Clip(group) = editor
        .execute(Command::GroupClips(clips[..2].to_vec()))
        .unwrap()
    else {
        panic!("group")
    };
    let roots = editor.project().clip(group).unwrap().audio_events();
    let source = roots.events[0].source;
    assert!(
        !roots
            .sources
            .iter()
            .find(|root| root.id == source)
            .unwrap()
            .clip
            .audio_events()
            .events[0]
            .active
    );
    let before = editor.project().snapshot();
    assert!(
        editor
            .execute(Command::EditAudioEvents {
                clip: group,
                operation: hex_project::AudioEventEdit::SetRetainedActive {
                    source,
                    events: vec![9],
                    active: true
                }
            })
            .is_err()
    );
    assert_eq!(editor.project().snapshot(), before);
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
    let changed = editor.project().clip(group).unwrap().audio_events();
    assert!(
        changed
            .sources
            .iter()
            .find(|root| root.id == source)
            .unwrap()
            .clip
            .audio_events()
            .events[0]
            .active
    );
    assert!(editor.undo());
    assert_eq!(editor.project().snapshot(), before);
    assert!(editor.redo());
}

#[test]
fn partial_audio_group_keeps_activation_inside_and_outside_selection() {
    let (mut editor, track, clips) = audio_fixture();
    editor
        .execute(Command::SetClipActive {
            clip: clips[1],
            active: false,
        })
        .unwrap();
    let before = editor.project().snapshot();
    let Created::Clip(group) = editor
        .execute(Command::GroupTimeRange(
            hex_project::TimeRange::new(vec![track], 50, 250).unwrap(),
        ))
        .unwrap()
    else {
        panic!("group")
    };
    let merged = editor.project().clip(group).unwrap();
    assert!(merged.active());
    assert_eq!(
        merged
            .audio_events()
            .events
            .iter()
            .map(|event| (event.start, event.end, event.active))
            .collect::<Vec<_>>(),
        vec![(0, 50, true), (150, 200, false)]
    );
    let outside: Vec<_> = editor
        .project()
        .track(track)
        .unwrap()
        .clips()
        .iter()
        .filter(|clip| clip.id() != group && clip.start() < 300)
        .map(|clip| (clip.start(), clip.duration(), clip.active()))
        .collect();
    assert_eq!(outside, vec![(0, 50, true), (250, 50, false)]);
    assert!(editor.undo());
    let mut restored = editor.project().snapshot();
    restored.next_id = before.next_id;
    assert_eq!(restored, before);
}

#[test]
fn mixed_audio_group_retains_event_activation_and_history() {
    let (mut editor, _, clips) = audio_fixture();
    editor
        .execute(Command::SetClipActive {
            clip: clips[1],
            active: false,
        })
        .unwrap();
    let before = editor.project().snapshot();
    let Created::Clip(group) = editor
        .execute(Command::GroupClips(clips[..2].to_vec()))
        .unwrap()
    else {
        panic!("group")
    };
    let grouped = editor.project().clip(group).unwrap();
    assert!(grouped.active());
    assert_eq!(
        grouped
            .audio_events()
            .events
            .iter()
            .map(|e| e.active)
            .collect::<Vec<_>>(),
        vec![true, false]
    );
    let inactive_id = grouped.audio_events().events[1].id;
    editor
        .execute(Command::EditAudioEvents {
            clip: group,
            operation: hex_project::AudioEventEdit::SetActive {
                events: vec![inactive_id],
                active: true,
            },
        })
        .unwrap();
    assert!(
        editor
            .project()
            .clip(group)
            .unwrap()
            .audio_events()
            .events
            .iter()
            .all(|e| e.active)
    );
    assert!(editor.undo());
    assert!(!editor.project().clip(group).unwrap().audio_events().events[1].active);
    let Created::Clip(nested) = editor
        .execute(Command::GroupClips(vec![group, clips[2]]))
        .unwrap()
    else {
        panic!("regroup")
    };
    assert_eq!(
        editor
            .project()
            .clip(nested)
            .unwrap()
            .audio_events()
            .events
            .iter()
            .map(|e| e.active)
            .collect::<Vec<_>>(),
        vec![true, false, true]
    );
    assert!(editor.undo());
    assert!(editor.undo());
    let mut restored = editor.project().snapshot();
    restored.next_id = before.next_id;
    assert_eq!(restored, before);
}

#[test]
fn audio_group_retains_all_dependencies_and_can_regroup_neutral_containers() {
    let (mut e, _, clips) = audio_fixture();
    let last = e.project().clip(clips[2]).unwrap().clone();
    let Created::Clip(id) = e.execute(Command::GroupClips(clips[..2].to_vec())).unwrap() else {
        panic!()
    };
    let group = e.project().clip(id).unwrap();
    assert_eq!((group.start(), group.duration()), (0, 300));
    let events = group.audio_events();
    assert_eq!(events.sources.len(), 2);
    assert_eq!(
        events
            .events
            .iter()
            .map(|e| (e.start, e.end))
            .collect::<Vec<_>>(),
        vec![(0, 100), (200, 300)]
    );
    assert_eq!(e.project().clip(clips[2]).unwrap(), &last);
    let source_length = group.source_length();
    for asset in group.referenced_assets().collect::<Vec<_>>() {
        assert!(e.execute(Command::RemoveAsset(asset)).is_err());
    }
    let unchanged = e.project().snapshot();
    assert!(
        e.execute(Command::TrimClip {
            clip: id,
            start: 0,
            duration: 300,
            source_offset: 1,
            source_length: source_length - 1,
        })
        .is_err()
    );
    assert_eq!(e.project().snapshot(), unchanged);
    assert!(
        e.execute(Command::EditAudioEvents {
            clip: id,
            operation: hex_project::AudioEventEdit::Paste {
                at: 0,
                length: 10,
                events: vec![hex_project::AudioEvent {
                    active: true,
                    gain: hex_project::ClipGain::UNITY,
                    fade: None,
                    id: 1,
                    source: 0,
                    start: 0,
                    end: 10,
                    source_start: 0
                }],
            }
        })
        .is_err()
    );
    assert_eq!(e.project().snapshot(), unchanged);
    assert!(
        e.execute(Command::GroupClips(vec![clips[0], clips[1]]))
            .is_err()
    );
    assert_eq!(e.project().snapshot(), unchanged);
    let Created::Clip(regrouped) = e.execute(Command::GroupClips(vec![id, clips[2]])).unwrap()
    else {
        panic!()
    };
    assert_eq!(
        e.project()
            .clip(regrouped)
            .unwrap()
            .audio_events()
            .sources
            .len(),
        3
    );
    assert_eq!(
        Project::from_snapshot(e.project().snapshot()).unwrap(),
        *e.project()
    );
}
#[test]
fn lower_audio_range_preserves_outer_fragments_and_is_one_undo() {
    let (mut e, _, clips) = audio_fixture();
    let before = e.project().snapshot();
    let Created::Clip(center) = e
        .execute(Command::GroupAudioEventRange {
            clip: clips[0],
            start: 20,
            end: 70,
        })
        .unwrap()
    else {
        panic!()
    };
    let center = e.project().clip(center).unwrap();
    assert_eq!((center.start(), center.duration()), (20, 50));
    assert!(center.audio_edit().is_some());
    assert_eq!(e.project().tracks()[0].clips().len(), 5);
    assert!(e.undo());
    let mut restored = e.project().snapshot();
    restored.next_id = before.next_id;
    assert_eq!(restored, before);
}

#[test]
fn midi_group_preserves_independent_clip_gain_and_curve_through_seams() {
    use hex_project::{ClipGain, MidiClipGain, MidiClipVolume, MidiClipVolumePoint};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let track = track(&mut editor);
    let first = midi(&mut editor, track, 0);
    let second = midi(&mut editor, track, 4);
    for (clip, db) in [(first, -6_000), (second, 24_000)] {
        let curve = MidiClipVolume::new(
            vec![
                MidiClipVolumePoint::linear(b(0), MidiClipGain::new(0.0).unwrap()),
                MidiClipVolumePoint::linear(b(2), MidiClipGain::new(4.0).unwrap()),
            ],
            b(2),
        )
        .unwrap();
        editor
            .execute(Command::ReplaceMidiClipVolume {
                clip,
                volume: curve,
            })
            .unwrap();
        editor
            .execute(Command::SetMidiClipGain {
                clip,
                gain: ClipGain::new(db).unwrap(),
            })
            .unwrap();
    }
    let originals = [
        editor.project().midi_clip(first).unwrap().clone(),
        editor.project().midi_clip(second).unwrap().clone(),
    ];
    let before = editor.project().snapshot();
    let Created::Clip(id) = editor
        .execute(Command::GroupClips(vec![first, second]))
        .unwrap()
    else {
        panic!("group")
    };
    let grouped = editor.project().midi_clip(id).unwrap();
    for original in originals {
        for raw in [0, 1 << 30, 1 << 31, 1 << 32, (2 << 32) - 1] {
            let source = original.volume().gain_at(BeatTime::from_raw(raw)).linear()
                * f64::from(original.volume().clip_gain().linear());
            let actual = grouped
                .volume()
                .gain_at(BeatTime::from_raw(original.start().raw() + raw))
                .linear()
                * f64::from(grouped.volume().clip_gain().linear());
            assert!((source - actual).abs() < 1.0e-12, "{source} != {actual}");
        }
    }
    let scalar = f64::from(grouped.volume().clip_gain().linear());
    let held = grouped.volume().gain_at(b(2)).linear() * scalar;
    assert!((grouped.volume().gain_at(b(3)).linear() * scalar - held).abs() < 1.0e-12);
    assert!((grouped.volume().gain_before(b(4)).linear() * scalar - held).abs() < 1.0e-12);
    assert_eq!(grouped.volume().gain_at(b(4)).linear(), 0.0);
    let after = editor.project().snapshot();
    assert!(editor.undo());
    let mut restored = editor.project().snapshot();
    restored.next_id = before.next_id;
    assert_eq!(restored, before);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
}

#[test]
fn midi_group_accepts_matching_constant_curves_and_preserves_history() {
    use hex_project::{MidiClipGain, MidiClipVolume, MidiClipVolumePoint};
    for gain in [0.0, 0.5, 1.0, 4.0] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let track = track(&mut editor);
        let first = midi(&mut editor, track, 0);
        let second = midi(&mut editor, track, 4);
        for clip in [first, second] {
            // Exercise implicit unity alongside an explicitly authored unity.
            if gain == 1.0 && clip == first {
                continue;
            }
            editor
                .execute(Command::ReplaceMidiClipVolume {
                    clip,
                    volume: MidiClipVolume::new(
                        vec![MidiClipVolumePoint::linear(
                            b(0),
                            MidiClipGain::new(gain).unwrap(),
                        )],
                        b(2),
                    )
                    .unwrap(),
                })
                .unwrap();
        }
        let before = editor.project().snapshot();
        let Created::Clip(group) = editor
            .execute(Command::GroupClips(vec![first, second]))
            .unwrap()
        else {
            panic!("group")
        };
        let grouped = editor.project().midi_clip(group).unwrap();
        assert_eq!(grouped.notes().len(), 2);
        for beat in [b(0), b(1), b(4), b(5)] {
            assert_eq!(grouped.volume().gain_at(beat).linear(), gain);
        }
        let after = editor.project().snapshot();
        assert!(editor.undo());
        let mut restored = editor.project().snapshot();
        restored.next_id = before.next_id;
        assert_eq!(restored, before);
        assert!(editor.redo());
        assert_eq!(editor.project().snapshot(), after);
    }
}

#[test]
fn midi_group_projection_keeps_its_bounds() {
    use hex_project::{MidiClipGain, MidiClipVolume, MidiClipVolumePoint};
    let mut e = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let t = track(&mut e);
    let a = midi(&mut e, t, 0);
    let c = midi(&mut e, t, 4);
    for (clip, left, right) in [(a, 0.25, 1.5), (c, 0.75, 0.5)] {
        e.execute(Command::ReplaceMidiClipVolume {
            clip,
            volume: MidiClipVolume::new(
                vec![
                    MidiClipVolumePoint::linear(b(0), MidiClipGain::new(left).unwrap()),
                    MidiClipVolumePoint::linear(b(2), MidiClipGain::new(right).unwrap()),
                ],
                b(2),
            )
            .unwrap(),
        })
        .unwrap();
    }
    let originals = [
        e.project().midi_clip(a).unwrap().clone(),
        e.project().midi_clip(c).unwrap().clone(),
    ];
    for original in originals {
        let projected = original
            .volume()
            .materialized(
                original.source_playback().unwrap(),
                BeatTime::ZERO,
                original.length(),
            )
            .unwrap();
        for raw in [0, 1 << 30, 1 << 31, 1 << 32, (2 << 32) - 1] {
            let local = BeatTime::from_raw(raw);
            assert_eq!(projected.gain_at(local), original.volume().gain_at(local));
        }
    }
    e.execute(Command::SetMidiClipSource {
        clip: a,
        source_length: b(2),
        source_start: b(0),
        loop_start: b(0),
        loop_end: BeatTime::from_raw(1),
        looping: true,
        length: b(2),
    })
    .unwrap();
    let before = e.project().snapshot();
    let clip = e.project().midi_clip(a).unwrap();
    assert!(
        clip.volume()
            .materialized(
                clip.source_playback().unwrap(),
                BeatTime::ZERO,
                clip.length()
            )
            .is_err()
    );
    assert_eq!(e.project().snapshot(), before);
    assert!(e.execute(Command::GroupClips(vec![a, c])).is_err());
    assert_eq!(e.project().snapshot(), before);
}

#[test]
fn regrouping_directed_composites_retains_content_and_history() {
    let (mut e, _, clips) = audio_fixture();
    let Created::Clip(group) = e.execute(Command::GroupClips(clips[..2].to_vec())).unwrap() else {
        panic!("group");
    };
    let forward = e.project().clip(group).unwrap().retained_snapshot();
    e.execute(Command::ReverseClips(vec![group])).unwrap();
    let before = e.project().snapshot();
    let directed = e.project().clip(group).unwrap().retained_snapshot();
    let Created::Clip(outer) = e
        .execute(Command::GroupClips(vec![group, clips[2]]))
        .unwrap()
    else {
        panic!("regroup")
    };
    assert!(
        e.project()
            .clip(outer)
            .unwrap()
            .audio_events()
            .sources
            .iter()
            .any(|source| source.clip.retained_snapshot() == directed)
    );
    assert!(e.undo());
    let mut restored = e.project().snapshot();
    restored.next_id = before.next_id;
    assert_eq!(restored, before);
    assert!(e.undo());
    assert_eq!(
        e.project().clip(group).unwrap().retained_snapshot(),
        forward
    );
    assert!(e.redo());
    let mut restored = e.project().snapshot();
    restored.next_id = before.next_id;
    assert_eq!(restored, before);
}

#[test]
fn directed_midi_group_materializes_note_gates_and_volume_in_forward_output_time() {
    use hex_project::{MidiClipGain, MidiClipVolume, MidiClipVolumePoint};
    let mut e = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let t = track(&mut e);
    let a = midi(&mut e, t, 0);
    let c = midi(&mut e, t, 4);
    for clip in [a, c] {
        e.execute(Command::ReplaceMidiClipVolume {
            clip,
            volume: MidiClipVolume::new(
                vec![
                    MidiClipVolumePoint::linear(b(0), MidiClipGain::new(0.25).unwrap()),
                    MidiClipVolumePoint::linear(b(2), MidiClipGain::new(1.5).unwrap()),
                ],
                b(2),
            )
            .unwrap(),
        })
        .unwrap();
    }
    e.execute(Command::ReverseClips(vec![a, c])).unwrap();
    for clip in [a, c] {
        let source = e.project().midi_clip(clip).unwrap();
        let projected = source
            .volume()
            .materialized(
                source.source_playback().unwrap(),
                BeatTime::ZERO,
                source.length(),
            )
            .unwrap();
        for (half, gain) in [(0, 1.5), (1, 1.1875), (2, 0.875), (3, 0.5625)] {
            assert_eq!(
                projected
                    .gain_at(BeatTime::from_raw(b(1).raw() * half / 2))
                    .linear(),
                gain
            );
        }
    }
    let before = e.project().snapshot();
    let Created::Clip(group) = e.execute(Command::GroupClips(vec![a, c])).unwrap() else {
        panic!("group");
    };
    let result = e.project().midi_clip(group).unwrap();
    assert_eq!(
        result
            .notes()
            .iter()
            .map(|n| (
                n.start(),
                n.duration(),
                n.pitch(),
                n.channel(),
                n.velocity()
            ))
            .collect::<Vec<_>>(),
        vec![(b(1), b(1), 60, 2, 93), (b(5), b(1), 60, 2, 93)]
    );
    for offset in [0, 4] {
        for (half, gain) in [(0, 1.5), (1, 1.1875), (2, 0.875), (3, 0.5625)] {
            assert_eq!(
                result
                    .volume()
                    .gain_at(BeatTime::from_raw(b(offset).raw() + b(1).raw() * half / 2))
                    .linear(),
                gain
            );
        }
    }
    let grouped = e.project().snapshot();
    assert!(e.undo());
    let mut restored = e.project().snapshot();
    restored.next_id = before.next_id;
    assert_eq!(restored, before);
    assert!(e.redo());
    assert_eq!(e.project().snapshot(), grouped);
}

#[test]
fn grouping_a_source_event_range_in_a_directed_view_refuses_atomically() {
    let (mut e, _, clips) = audio_fixture();
    e.execute(Command::ReverseClips(vec![clips[0]])).unwrap();
    let before = e.project().snapshot();
    assert!(
        e.execute(Command::GroupAudioEventRange {
            clip: clips[0],
            start: 20,
            end: 40
        })
        .is_err()
    );
    assert_eq!(e.project().snapshot(), before);
    assert!(e.undo());
    assert!(e.project().clip(clips[0]).unwrap().audio_edit().is_none());
    assert!(e.redo());
    assert_eq!(e.project().snapshot(), before);
}

#[test]
fn grouped_event_fade_bounds_follow_its_retained_source_not_container_length() {
    use hex_project::{AudioEventEdit, AudioEventFade, ClipFade};
    let (mut e, track, clips) = audio_fixture();
    let Created::Clip(group) = e.execute(Command::GroupClips(clips[..2].to_vec())).unwrap() else {
        panic!("group")
    };
    let Created::Clip(range) = e
        .execute(Command::GroupAudioEventRange {
            clip: group,
            start: 90,
            end: 100,
        })
        .unwrap()
    else {
        panic!("range")
    };
    let removed: Vec<_> = e.project().tracks()[0]
        .clips()
        .iter()
        .map(|c| c.id())
        .filter(|id| *id != range && *id != clips[2])
        .collect();
    for id in removed {
        e.execute(Command::RemoveClip(id)).unwrap();
    }
    let Created::Clip(tail) = e
        .execute(Command::SplitClip {
            clip: clips[2],
            frame: 410,
        })
        .unwrap()
    else {
        panic!("split")
    };
    e.execute(Command::RemoveClip(tail)).unwrap();
    e.execute(Command::MoveClip {
        clip: clips[2],
        track,
        start: 110,
    })
    .unwrap();
    let Created::Clip(group) = e
        .execute(Command::GroupClips(vec![range, clips[2]]))
        .unwrap()
    else {
        panic!("regroup")
    };
    let root = e.project().clip(group).unwrap().audio_event_root();
    let event = e.project().clip(group).unwrap().audio_events().events[0];
    assert!(event.source_start > root.domain.clock.length());
    assert!(
        root.root_frame_at(event.source_start, e.project().tempo_map())
            .is_err()
    );
    assert_eq!(
        root.event_source_clock(event.source).unwrap(),
        hex_project::AudioDomainClock::Frames(100)
    );
    assert_eq!(
        root.event_source_frame_at(event.source, event.source_start, e.project().tempo_map())
            .unwrap(),
        event.source_start
    );
    assert_eq!(
        root.event_source_position_at(event.source, event.source_start, e.project().tempo_map())
            .unwrap(),
        event.source_start
    );
    assert!(
        root.event_source_frame_at(event.source, 101, e.project().tempo_map())
            .is_err()
    );
    assert!(root.event_source_clock(u64::MAX).is_err());

    let fade = AudioEventFade::new(
        event.source_start,
        ClipFade::new(event.length(), 0, 3, 4).unwrap(),
    )
    .unwrap();
    e.execute(Command::EditAudioEvents {
        clip: group,
        operation: AudioEventEdit::SetFade {
            event: event.id,
            fade: Some(fade),
        },
    })
    .unwrap();
    let accepted = e.project().snapshot();
    assert_eq!(
        Project::from_snapshot(accepted.clone()).unwrap(),
        *e.project()
    );
    let invalid = AudioEventFade::new(
        event.source_start,
        ClipFade::new(event.length() + 1, 0, 3, 4).unwrap(),
    )
    .unwrap();
    assert!(
        e.execute(Command::EditAudioEvents {
            clip: group,
            operation: AudioEventEdit::SetFade {
                event: event.id,
                fade: Some(invalid)
            }
        })
        .is_err()
    );
    assert_eq!(e.project().snapshot(), accepted);
    assert!(e.undo());
    assert!(
        e.project().clip(group).unwrap().audio_events().events[0]
            .fade
            .is_none()
    );
    assert!(e.redo());
    assert_eq!(e.project().snapshot(), accepted);
}

#[test]
fn time_group_cuts_audio_edges_preserves_gaps_sources_and_one_history_step() {
    let (mut e, t, clips) = audio_fixture();
    let original = e.project().clone();
    let before = original.snapshot();
    let Created::Clip(group) = e
        .execute(Command::GroupTimeRange(
            hex_project::TimeRange::new(vec![t], 50, 450).unwrap(),
        ))
        .unwrap()
    else {
        panic!("group")
    };
    let row = e.project().track(t).unwrap();
    assert_eq!(row.clips().len(), 3);
    let first = e.project().clip(clips[0]).unwrap();
    assert_eq!(
        (first.start(), first.duration(), first.source_offset()),
        (0, 50, 17)
    );
    let tail = row.clips().iter().find(|c| c.start() == 450).unwrap();
    assert_eq!((tail.duration(), tail.source_offset()), (50, 67));
    let grouped = e.project().clip(group).unwrap();
    assert_eq!((grouped.start(), grouped.duration()), (50, 400));
    let events = grouped.audio_events();
    assert_eq!(
        events
            .events
            .iter()
            .map(|e| (e.start, e.end))
            .collect::<Vec<_>>(),
        vec![(0, 50), (150, 250), (350, 400)]
    );
    assert_eq!(
        events
            .sources
            .iter()
            .map(|r| (r.clip.asset(), r.clip.source_offset(), r.clip.duration()))
            .collect::<Vec<_>>(),
        vec![
            (original.clip(clips[0]).unwrap().asset(), 67, 50),
            (original.clip(clips[1]).unwrap().asset(), 17, 100),
            (original.clip(clips[2]).unwrap().asset(), 17, 50)
        ]
    );
    let after = e.project().snapshot();
    assert_eq!(Project::from_snapshot(after.clone()).unwrap(), *e.project());
    assert!(e.undo());
    let mut restored = e.project().snapshot();
    restored.next_id = before.next_id;
    assert_eq!(restored, before);
    assert!(!e.can_undo());
    assert!(e.redo());
    assert_eq!(e.project().snapshot(), after);
}

#[test]
fn time_group_merges_midi_per_track_and_can_extract_one_partial_clip() {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let tracks = [track(&mut e), track(&mut e)];
    for t in tracks {
        midi(&mut e, t, 0);
        midi(&mut e, t, 4);
    }
    let before = e.project().snapshot();
    let start = e.project().tempo_map().beat_to_frame(b(1)).unwrap();
    let end = e.project().tempo_map().beat_to_frame(b(5)).unwrap();
    e.execute(Command::GroupTimeRange(
        hex_project::TimeRange::new(tracks.to_vec(), start, end).unwrap(),
    ))
    .unwrap();
    for t in tracks {
        let row = e.project().track(t).unwrap();
        assert_eq!(row.midi_clips().len(), 3);
        let group = row.midi_clips().iter().find(|c| c.start() == b(1)).unwrap();
        assert_eq!(group.length(), b(4));
        assert_eq!(group.notes().len(), 1);
        assert_eq!(
            (group.notes()[0].start(), group.notes()[0].duration()),
            (b(3), b(1))
        );
    }
    assert!(e.undo());
    let mut restored = e.project().snapshot();
    restored.next_id = before.next_id;
    assert_eq!(restored, before);
    e.execute(Command::GroupTimeRange(
        hex_project::TimeRange::new(vec![tracks[0]], start, start + 12_000).unwrap(),
    ))
    .unwrap();
    assert_eq!(e.project().track(tracks[0]).unwrap().midi_clips().len(), 4);
}

#[test]
fn time_group_empty_stale_and_late_row_refusals_are_atomic() {
    let (fixture, t, _) = audio_fixture();
    let mut e = Editor::new(fixture.project().clone(), 16).unwrap();
    let Created::Track(other) = e
        .execute(Command::AddTrack {
            name: "mixed".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    // A later mixed row must refuse after the earlier row has already split/grouped.
    let asset = e.project().tracks()[0].clips()[0].asset();
    e.execute(Command::PlaceClip {
        track: other,
        asset,
        start: 0,
        duration: 100,
        source_offset: 0,
        source_length: 100,
        looping: false,
    })
    .unwrap();
    e.execute(Command::PlaceMidiClip {
        track: other,
        start: b(1),
        length: b(1),
    })
    .unwrap();
    let mut e = Editor::new(e.project().clone(), 16).unwrap();
    let before = e.project().clone();
    for range in [
        hex_project::TimeRange::new(vec![t], 600, 700).unwrap(),
        hex_project::TimeRange::new(vec![other, t], 50, 450).unwrap(),
        hex_project::TimeRange::new(vec![t, other], 50, 48_000).unwrap(),
    ] {
        assert!(e.execute(Command::GroupTimeRange(range)).is_err());
        assert_eq!(e.project(), &before);
        assert!(!e.can_undo());
    }
}

#[test]
fn time_group_includes_empty_audio_margins_for_single_and_multiple_clips() {
    for (start, end, expected) in [
        (150, 300, vec![(50, 150)]),
        (200, 350, vec![(0, 100)]),
        (150, 350, vec![(50, 150)]),
        (150, 550, vec![(50, 150), (250, 350)]),
    ] {
        let (mut e, t, _) = audio_fixture();
        let before = e.project().snapshot();
        let Created::Clip(id) = e
            .execute(Command::GroupTimeRange(
                hex_project::TimeRange::new(vec![t], start, end).unwrap(),
            ))
            .unwrap()
        else {
            panic!("group")
        };
        let group = e.project().clip(id).unwrap();
        assert_eq!((group.start(), group.duration()), (start, end - start));
        assert_eq!(
            group
                .audio_events()
                .events
                .iter()
                .map(|event| (event.start, event.end))
                .collect::<Vec<_>>(),
            expected
        );
        let after = e.project().snapshot();
        assert_eq!(Project::from_snapshot(after.clone()).unwrap(), *e.project());
        assert!(e.undo());
        let mut restored = e.project().snapshot();
        restored.next_id = before.next_id;
        assert_eq!(restored, before);
        assert!(!e.can_undo());
        assert!(e.redo());
        assert_eq!(e.project().snapshot(), after);
    }
}

#[test]
fn time_group_one_midi_clip_keeps_its_varying_curve_and_history() {
    use hex_project::{MidiClipGain, MidiClipVolume, MidiClipVolumePoint};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let track = track(&mut editor);
    let clip = midi(&mut editor, track, 1);
    editor
        .execute(Command::ReplaceMidiClipVolume {
            clip,
            volume: MidiClipVolume::new(
                vec![
                    MidiClipVolumePoint::linear(b(0), MidiClipGain::new(0.25).unwrap()),
                    MidiClipVolumePoint::linear(b(2), MidiClipGain::new(1.5).unwrap()),
                ],
                b(2),
            )
            .unwrap(),
        })
        .unwrap();
    let before = editor.project().snapshot();
    let start = editor.project().tempo_map().beat_to_frame(b(0)).unwrap();
    let end = editor.project().tempo_map().beat_to_frame(b(4)).unwrap();
    editor
        .execute(Command::GroupTimeRange(
            hex_project::TimeRange::new(vec![track], start, end).unwrap(),
        ))
        .unwrap();
    let grouped = &editor.project().track(track).unwrap().midi_clips()[0];
    assert_eq!(grouped.volume().gain_at(b(1)).linear(), 0.25);
    assert_eq!(grouped.volume().gain_at(b(2)).linear(), 0.875);
    let after = editor.project().snapshot();
    assert!(editor.undo());
    let mut restored = editor.project().snapshot();
    restored.next_id = before.next_id;
    assert_eq!(restored, before);
    assert!(editor.redo());
    assert_eq!(editor.project().snapshot(), after);
}

#[test]
fn time_group_includes_empty_midi_margins_per_populated_track() {
    let mut e = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let tracks = [track(&mut e), track(&mut e), track(&mut e)];
    midi(&mut e, tracks[0], 2);
    midi(&mut e, tracks[0], 6);
    midi(&mut e, tracks[1], 4);
    let mut e = Editor::new(e.project().clone(), 16).unwrap();
    let before = e.project().snapshot();
    let start = e.project().tempo_map().beat_to_frame(b(1)).unwrap();
    let end = e.project().tempo_map().beat_to_frame(b(9)).unwrap();
    e.execute(Command::GroupTimeRange(
        hex_project::TimeRange::new(tracks.to_vec(), start, end).unwrap(),
    ))
    .unwrap();
    for (t, onsets) in [(tracks[0], vec![b(1), b(5)]), (tracks[1], vec![b(3)])] {
        let row = e.project().track(t).unwrap();
        assert_eq!(row.midi_clips().len(), 1);
        let group = &row.midi_clips()[0];
        assert_eq!((group.start(), group.length()), (b(1), b(8)));
        assert_eq!(
            group
                .notes()
                .iter()
                .map(|note| note.start())
                .collect::<Vec<_>>(),
            onsets
        );
    }
    assert!(
        e.project()
            .track(tracks[2])
            .unwrap()
            .midi_clips()
            .is_empty()
    );
    let after = e.project().snapshot();
    assert!(e.undo());
    let mut restored = e.project().snapshot();
    restored.next_id = before.next_id;
    assert_eq!(restored, before);
    assert!(!e.can_undo());
    assert!(e.redo());
    assert_eq!(e.project().snapshot(), after);
}

#[test]
fn time_group_exact_single_clip_is_unchanged_and_oversized_margins_refuse_atomically() {
    let (mut e, t, clips) = audio_fixture();
    let before = e.project().clone();
    let created = e
        .execute(Command::GroupTimeRange(
            hex_project::TimeRange::new(vec![t], 200, 300).unwrap(),
        ))
        .unwrap();
    assert_eq!(created, Created::Clip(clips[1]));
    assert_eq!(e.project(), &before);
    assert!(!e.can_undo());
    let error = e
        .execute(Command::GroupTimeRange(
            hex_project::TimeRange::new(vec![t], 150, 151 + hex_project::MAX_AUDIO_DOMAIN_FRAMES)
                .unwrap(),
        ))
        .unwrap_err();
    assert!(error.0.contains("bounded preparation length"));
    assert_eq!(e.project(), &before);
    assert!(!e.can_undo());
}

#[test]
fn processed_repeated_group_keeps_inactive_source_editable_when_regrouped() {
    let (mut editor, track, ids) = audio_fixture();
    editor
        .execute(Command::MoveClip {
            clip: ids[2],
            track,
            start: 900,
        })
        .unwrap();
    editor
        .execute(Command::SetClipActive {
            clip: ids[1],
            active: false,
        })
        .unwrap();
    let Created::Clip(first) = editor
        .execute(Command::GroupClips(ids[..2].to_vec()))
        .unwrap()
    else {
        panic!("group")
    };
    editor
        .execute(Command::ResizeAudioClipView {
            clip: first,
            operation: hex_project::AudioViewResizeOperation::RepeatOuterEnd { project_frame: 650 },
        })
        .unwrap();
    let before = editor.project().snapshot();
    let Created::Clip(grouped) = editor
        .execute(Command::GroupClips(vec![first, ids[2]]))
        .unwrap()
    else {
        panic!("nested group")
    };
    let output = editor.project().clip(grouped).unwrap();
    assert!(output.active());
    let roots = output.audio_events();
    let nested = roots
        .sources
        .iter()
        .find(|root| root.clip.has_retained_audio_roots())
        .expect("retained processed group");
    assert!(
        nested
            .clip
            .audio_events()
            .events
            .iter()
            .any(|event| !event.active)
    );
    let source = nested.id;
    let inner_id = nested
        .clip
        .audio_events()
        .events
        .iter()
        .find(|event| !event.active)
        .unwrap()
        .id;
    assert!(editor.undo());
    let mut restored = editor.project().snapshot();
    restored.next_id = before.next_id;
    assert_eq!(restored, before);
    assert!(editor.redo());
    let Created::Clip(copy) = editor.execute(Command::DuplicateClip(grouped)).unwrap() else {
        panic!("copy")
    };
    editor
        .execute(Command::EditAudioEvents {
            clip: copy,
            operation: hex_project::AudioEventEdit::SetRetainedPathActive {
                sources: vec![source],
                events: vec![inner_id],
                active: true,
            },
        })
        .unwrap();
    let original = editor.project().clip(grouped).unwrap().audio_events();
    assert!(
        !original
            .sources
            .iter()
            .find(|root| root.id == source)
            .unwrap()
            .clip
            .audio_events()
            .events
            .iter()
            .find(|event| event.id == inner_id)
            .unwrap()
            .active
    );
    assert!(editor.undo());
    let copied = editor.project().clip(copy).unwrap().audio_events();
    assert!(
        !copied
            .sources
            .iter()
            .find(|root| root.id == source)
            .unwrap()
            .clip
            .audio_events()
            .events
            .iter()
            .find(|event| event.id == inner_id)
            .unwrap()
            .active
    );
}

#[test]
fn partial_time_group_retains_processed_source_and_outside_fragments() {
    let (mut editor, track, ids) = audio_fixture();
    editor
        .execute(Command::MoveClip {
            clip: ids[2],
            track,
            start: 900,
        })
        .unwrap();
    editor
        .execute(Command::SetClipActive {
            clip: ids[1],
            active: false,
        })
        .unwrap();
    let Created::Clip(group) = editor
        .execute(Command::GroupClips(ids[..2].to_vec()))
        .unwrap()
    else {
        panic!("group")
    };
    editor
        .execute(Command::ResizeAudioClipView {
            clip: group,
            operation: hex_project::AudioViewResizeOperation::RepeatOuterEnd { project_frame: 650 },
        })
        .unwrap();
    let before = editor.project().snapshot();
    let Created::Clip(result) = editor
        .execute(Command::GroupTimeRange(
            hex_project::TimeRange::new(vec![track], 50, 950).unwrap(),
        ))
        .unwrap()
    else {
        panic!("time group")
    };
    let row = editor.project().track(track).unwrap();
    assert_eq!(row.clips().len(), 3);
    assert!(
        row.clips()
            .iter()
            .any(|clip| clip.start() == 0 && clip.duration() == 50)
    );
    assert!(
        row.clips()
            .iter()
            .any(|clip| clip.start() == 950 && clip.duration() == 50)
    );
    assert!(
        row.clips()
            .iter()
            .find(|clip| clip.id() == result)
            .unwrap()
            .audio_events()
            .sources
            .iter()
            .any(|root| root.clip.has_retained_audio_roots())
    );
    assert!(editor.undo());
    let mut restored = editor.project().snapshot();
    restored.next_id = before.next_id;
    assert_eq!(restored, before);
}

#[test]
fn rendered_group_refuses_to_flatten_recoverable_inactive_events() {
    let (mut editor, track, ids) = audio_fixture();
    editor
        .execute(Command::MoveClip {
            clip: ids[2],
            track,
            start: 900,
        })
        .unwrap();
    let Created::Clip(group) = editor
        .execute(Command::GroupClips(ids[..2].to_vec()))
        .unwrap()
    else {
        panic!("group")
    };
    editor
        .execute(Command::EditAudioEvents {
            clip: group,
            operation: hex_project::AudioEventEdit::SetActive {
                events: vec![1],
                active: false,
            },
        })
        .unwrap();
    editor
        .execute(Command::ResizeAudioClipView {
            clip: group,
            operation: hex_project::AudioViewResizeOperation::RepeatOuterEnd { project_frame: 650 },
        })
        .unwrap();
    let before = editor.project().snapshot();
    let error = editor
        .project()
        .audio_group_render_target(&Command::GroupClips(vec![group, ids[2]]))
        .unwrap_err();
    assert!(error.to_string().contains("cannot retain inactive events"));
    assert_eq!(editor.project().snapshot(), before);
}

#[test]
fn processed_repeated_audio_group_has_a_bounded_prepared_adoption_and_one_history_edge() {
    let (mut e, track, ids) = audio_fixture();
    e.execute(Command::MoveClip {
        clip: ids[2],
        track,
        start: 900,
    })
    .unwrap();
    let Created::Clip(group) = e.execute(Command::GroupClips(ids[..2].to_vec())).unwrap() else {
        panic!()
    };
    e.execute(Command::ResizeAudioClipView {
        clip: group,
        operation: hex_project::AudioViewResizeOperation::RepeatOuterEnd { project_frame: 650 },
    })
    .unwrap();
    e.execute(Command::SetAudioClipGain {
        clip: group,
        millidb: -3111,
    })
    .unwrap();
    let before = e.project().snapshot();
    // Canonical grouping retains the processed source without a render.
    let mut retained = Editor::new(e.project().clone(), 8).unwrap();
    let Created::Clip(result) = retained
        .execute(Command::GroupClips(vec![group, ids[2]]))
        .unwrap()
    else {
        panic!("group")
    };
    assert!(
        retained
            .project()
            .clip(result)
            .unwrap()
            .audio_events()
            .sources
            .iter()
            .any(|root| root.clip.has_retained_audio_roots())
    );
    assert_eq!(e.project().snapshot(), before);
    let mut oversized = Editor::new(e.project().clone(), 8).unwrap();
    oversized
        .execute(Command::MoveClip {
            clip: ids[2],
            track,
            start: hex_project::MAX_ARRANGEMENT_BOUNCE_FRAMES,
        })
        .unwrap();
    assert!(
        oversized
            .project()
            .audio_group_render_target(&Command::GroupClips(vec![group, ids[2]]))
            .unwrap_err()
            .to_string()
            .contains("finite render frame budget")
    );
    let target = e
        .project()
        .audio_group_render_target(&Command::GroupClips(vec![group, ids[2]]))
        .unwrap()
        .unwrap();
    assert!(!target.bakes_devices(true));
    let plan = e.project().plan_arrangement_bounce(&target, true).unwrap();
    assert_eq!((plan[0].start, plan[0].end), (0, 1000));
    let command = |frames| Command::CommitArrangementBounce {
        target: target.clone(),
        in_place: true,
        adoption: hex_project::BounceAdoption::NewTracks,
        files: vec![hex_project::BounceFile {
            source: hex_project::BounceSource::Arrangement,
            track,
            start: 0,
            frames,
            path: "prepared-group.wav".into(),
        }],
    };
    assert!(e.execute(command(999)).is_err());
    assert_eq!(e.project().snapshot(), before);
    let Created::Clip(result) = e.execute(command(1000)).unwrap() else {
        panic!()
    };
    let after = e.project().snapshot();
    assert_eq!(e.project().track(track).unwrap().clips().len(), 1);
    let clip = e.project().clip(result).unwrap();
    assert!(!clip.looping() && !clip.has_retained_audio_roots());
    assert_eq!(clip.duration(), 1000);
    assert!(!e.next_history_requires_stopped_bounce(false));
    assert!(e.undo());
    let mut restored = e.project().snapshot();
    restored.next_id = before.next_id;
    assert_eq!(restored, before);
    assert!(e.redo());
    assert_eq!(e.project().snapshot(), after);
}
