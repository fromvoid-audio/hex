//! Borrowed root/default facts and exact virtual split streams, not UI acceptance.
use hex_project::{
    AudioDomainClock, AudioEvent, AudioEvents, BeatTime, Clip, ClipId, Command, Created, Editor,
    Project, SourceGainPoint, Tempo, TempoPoint, WarpMode, default_audio_warp,
};
use std::sync::Arc;
fn base() -> (Editor, ClipId) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "root".into(),
            path: "not-opened.wav".into(),
            sample_rate: 48_000,
            frames: 128,
        })
        .unwrap()
    else {
        panic!("import")
    };
    (editor, clip)
}
fn check_root(clip: &Clip, tempo: &hex_project::TempoMap) {
    let expected = clip.audio_event_root();
    clip.with_audio_event_root(tempo, |read| {
        let facts = read.facts();
        let domain = &expected.domain;
        assert_eq!(
            (
                facts.view_start,
                facts.view_end,
                facts.musical_start,
                facts.placement
            ),
            (
                expected.view_start,
                expected.view_end,
                expected.musical_start,
                expected.placement
            )
        );
        assert_eq!(
            (
                facts.domain.source_offset,
                facts.domain.source_length,
                facts.domain.source_phase,
                facts.domain.looping,
                facts.domain.clock,
                facts.domain.pitch_cents,
                facts.domain.sample_bpm,
                facts.domain.warp_enabled,
                facts.domain.fade,
                facts.domain.fade_start
            ),
            (
                domain.source_offset,
                domain.source_length,
                domain.source_phase,
                domain.looping,
                domain.clock,
                domain.pitch_cents,
                domain.sample_bpm,
                domain.warp_enabled,
                domain.fade,
                domain.fade_start
            )
        );
        assert_eq!(facts.domain.warp_mode, domain.warp.as_ref().map(|w| w.mode));
        assert_eq!(facts.domain.warp_marker_count, read.warp_markers().len());
        assert_eq!(
            facts.domain.source_gain_point_count,
            read.source_gain_points().len()
        );
        assert!(
            read.warp_markers().eq(domain
                .warp
                .as_ref()
                .map_or(&[][..], |w| w.markers.as_slice())
                .iter()
                .copied())
        );
        assert!(
            read.source_gain_points()
                .eq(domain.source_gain.iter().copied())
        );
        if let Some(projection) = &domain.fade_projection {
            let actual = facts.domain.fade_projection.unwrap();
            assert_eq!(
                (
                    actual.sample_rate,
                    actual.origin_beat_raw,
                    actual.local_start_raw,
                    actual.tempo_point_count
                ),
                (
                    projection.tempo.sample_rate(),
                    projection.origin_beat_raw,
                    projection.local_start_raw,
                    projection.tempo.points().len()
                )
            );
            assert!(
                read.fade_projection_tempo_points()
                    .eq(projection.tempo.points().iter().copied())
            );
        } else {
            assert!(facts.domain.fade_projection.is_none());
            assert_eq!(read.fade_projection_tempo_points().len(), 0);
        }
        for position in [
            0,
            facts.view_start,
            facts.view_end,
            facts.domain.clock.length(),
        ] {
            assert_eq!(
                read.root_frame_at(position),
                expected.root_frame_at(position, tempo)
            );
        }
        for frame in [0, 1, 127, 1000] {
            assert_eq!(
                read.root_position_at(frame),
                expected.root_position_at(frame, tempo)
            );
        }
    });
}
#[test]
fn default_root_uses_capture_defaults_and_duration_instead_of_source_length() {
    let (mut editor, _) = base();
    let track = editor.project().tracks()[0].id();
    let asset = editor.project().assets().next().unwrap().id();
    let Created::Clip(id) = editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 10,
            duration: 30,
            source_offset: 5,
            source_length: 60,
            looping: true,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    let clip = editor.project().clip(id).unwrap();
    let tempo = editor.project().tempo_map();
    check_root(clip, tempo);
    clip.with_audio_event_root(tempo, |read| {
        let facts = read.facts();
        assert_eq!(facts.domain.clock, AudioDomainClock::Frames(30));
        assert_eq!((facts.view_start, facts.view_end), (0, 30));
        assert_eq!(
            (
                facts.domain.pitch_cents,
                facts.domain.sample_bpm,
                facts.domain.warp_mode,
                facts.domain.warp_enabled
            ),
            (0, None, None, false)
        );
        let events = read.editable_events().unwrap();
        assert_eq!(events.next_id(), 2);
        assert_eq!(events.len(), 1);
        assert!(events.iter().eq([AudioEvent {
            id: 1,
            start: 0,
            end: 30,
            source_start: 0
        }]));
    });
}
#[test]
fn retained_root_exposes_actual_points_and_frozen_projection_tempo() {
    let (mut editor, id) = base();
    let beat = BeatTime::from_raw(BeatTime::ONE.raw() / 1024);
    editor
        .execute(Command::SetTempoPoint {
            beat,
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    let settings = default_audio_warp(
        editor.project().clip(id).unwrap(),
        editor.project().tempo_map(),
        WarpMode::RePitch,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip: id,
            settings: Some(settings),
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipSourceGain {
            clip: id,
            points: vec![
                SourceGainPoint {
                    frame: 0,
                    gain_q16: 32768,
                },
                SourceGainPoint {
                    frame: 128,
                    gain_q16: 65536,
                },
            ],
        })
        .unwrap();
    editor
        .execute(Command::SetTempoPoint {
            beat,
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    let clip = editor.project().clip(id).unwrap();
    let tempo = editor.project().tempo_map();
    check_root(clip, tempo);
    clip.with_audio_event_root(tempo, |read| {
        assert_eq!(read.facts().domain.warp_mode, Some(WarpMode::RePitch));
        assert_eq!(read.facts().domain.warp_marker_count, 2);
        assert!(read.fade_projection_tempo_points().eq([
            TempoPoint::new(BeatTime::ZERO, Tempo::new(120_000_000).unwrap()),
            TempoPoint::new(beat, Tempo::new(60_000_000).unwrap())
        ]));
    });
}
fn cropped(events: Option<AudioEvents>, start: u64, end: u64) -> (Project, ClipId) {
    let (editor, id) = base();
    let root = editor.project().clip(id).unwrap().audio_event_root();
    let mut snapshot = editor.project().snapshot();
    let clip = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
    let mut root = root;
    Arc::make_mut(&mut root.domain).events = events.map(Arc::new);
    root.view_start = start;
    root.view_end = end;
    clip.start = start;
    clip.duration = end - start;
    clip.fade = hex_project::ClipFade::neutral(clip.duration).unwrap();
    clip.edit = Some(root);
    (Project::from_snapshot(snapshot).unwrap(), id)
}
fn check_events(clip: &Clip, tempo: &hex_project::TempoMap) {
    let expected = clip.editable_audio_events().unwrap();
    clip.with_audio_event_root(tempo, |read| {
        let events = read.editable_events().unwrap();
        assert_eq!(events.next_id(), expected.next_id);
        assert_eq!(events.len(), expected.events.len());
        assert_eq!(events.is_empty(), expected.events.is_empty());
        let mut actual = events.iter();
        assert_eq!(actual.len(), expected.events.len());
        for (index, expected) in expected.events.iter().enumerate() {
            assert_eq!(actual.next(), Some(*expected));
            assert_eq!(actual.len(), events.len() - index - 1);
        }
        assert_eq!(actual.next(), None);
        assert_eq!(actual.next(), None);
    });
}
#[test]
fn implicit_dual_split_preserves_literal_ids_source_offsets_and_hidden_intervals() {
    let (project, id) = cropped(None, 20, 40);
    let clip = project.clip(id).unwrap();
    check_events(clip, project.tempo_map());
    clip.with_audio_event_root(project.tempo_map(), |read| {
        let events = read.editable_events().unwrap();
        assert_eq!(events.next_id(), 4);
        assert!(events.iter().eq([
            AudioEvent {
                id: 1,
                start: 0,
                end: 20,
                source_start: 0
            },
            AudioEvent {
                id: 2,
                start: 20,
                end: 40,
                source_start: 20
            },
            AudioEvent {
                id: 3,
                start: 40,
                end: 128,
                source_start: 40
            }
        ]));
    });
}
#[test]
fn existing_boundaries_holes_and_authored_empty_do_not_consume_identities() {
    let authored = AudioEvents {
        next_id: 9,
        events: vec![
            AudioEvent {
                id: 7,
                start: 0,
                end: 20,
                source_start: 30,
            },
            AudioEvent {
                id: 2,
                start: 40,
                end: 80,
                source_start: 80,
            },
        ],
    };
    for (events, start, end) in [
        (Some(authored.clone()), 20, 40),
        (Some(authored), 25, 40),
        (
            Some(AudioEvents {
                next_id: 9,
                events: vec![],
            }),
            20,
            40,
        ),
    ] {
        let (project, id) = cropped(events, start, end);
        let clip = project.clip(id).unwrap();
        check_events(clip, project.tempo_map());
        clip.with_audio_event_root(project.tempo_map(), |read| {
            assert_eq!(read.editable_events().unwrap().next_id(), 9)
        });
    }
}
