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
fn midi_group_preserves_volume_seams_and_refuses_unbounded_loop_expansion() {
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
    let Created::Clip(id) = e.execute(Command::GroupClips(vec![a, c])).unwrap() else {
        panic!()
    };
    let group = e.project().midi_clip(id).unwrap();
    for original in originals {
        for raw in [0, 1 << 30, 1 << 31, 1 << 32, (2 << 32) - 1] {
            let local = BeatTime::from_raw(raw);
            assert_eq!(
                group
                    .volume()
                    .gain_at(BeatTime::from_raw(original.start().raw() + raw)),
                original.volume().gain_at(local)
            );
        }
    }
    assert!(e.undo());
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
    assert!(e.execute(Command::GroupClips(vec![a, c])).is_err());
    assert_eq!(e.project().snapshot(), before);
}

#[test]
fn regrouping_directed_composites_refuses_without_losing_content_or_history() {
    let (mut e, _, clips) = audio_fixture();
    let Created::Clip(group) = e.execute(Command::GroupClips(clips[..2].to_vec())).unwrap() else {
        panic!("group");
    };
    let forward = e.project().clip(group).unwrap().retained_snapshot();
    e.execute(Command::ReverseClips(vec![group])).unwrap();
    let before = e.project().snapshot();
    assert!(
        e.execute(Command::GroupClips(vec![group, clips[2]]))
            .is_err()
    );
    assert_eq!(e.project().snapshot(), before);
    assert!(e.undo());
    assert_eq!(
        e.project().clip(group).unwrap().retained_snapshot(),
        forward
    );
    assert!(e.redo());
    assert_eq!(e.project().snapshot(), before);
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
