//! Atomic Arrangement cuts through the canonical audio/MIDI split helpers.
//! A range cuts end before start so retained left identities remain valid for
//! both boundaries. One private candidate owns all cuts, including direct
//! Project::apply callers; Editor/session retain history and preparation custody.
use crate::{Command, Created, Invalid, MAX_TRACKS, Project, TimeRange, TrackId};
use std::collections::HashSet;

pub(crate) fn apply(
    project: &mut Project,
    tracks: &[TrackId],
    frame: u64,
) -> Result<Created, Invalid> {
    if apply_at_frames(project, tracks, &[frame])? == 0 {
        return Err(Invalid("split cursor must be inside a clip to split"));
    }
    Ok(Created::None)
}

pub(crate) fn apply_range(project: &mut Project, range: &TimeRange) -> Result<Created, Invalid> {
    range.validate(project)?;
    apply_at_frames(project, range.tracks(), &[range.end(), range.start()])?;
    Ok(Created::None)
}

// Only the one-cursor/two-range-boundary entry points above supply frames.
fn apply_at_frames(
    project: &mut Project,
    tracks: &[TrackId],
    frames: &[u64],
) -> Result<usize, Invalid> {
    if tracks.is_empty() {
        return Err(Invalid("split requires at least one track"));
    }
    if tracks.len() > MAX_TRACKS {
        return Err(Invalid("split track count exceeds project limit"));
    }
    let mut seen = HashSet::new();
    seen.try_reserve(tracks.len())
        .map_err(|_| Invalid("could not reserve split track validation"))?;
    for track_id in tracks {
        if !seen.insert(*track_id) {
            return Err(Invalid("split tracks must be unique"));
        }
        if project.track(*track_id).is_none() {
            return Err(Invalid("split track no longer exists"));
        }
    }
    let clip_capacity = tracks
        .iter()
        .filter_map(|id| project.track(*id))
        .map(|track| track.clips().len() + track.midi_clips().len())
        .sum::<usize>();
    let capacity = clip_capacity
        .checked_mul(frames.len())
        .ok_or(Invalid("split command count overflow"))?;
    let mut commands = Vec::new();
    commands
        .try_reserve(capacity)
        .map_err(|_| Invalid("could not reserve split commands"))?;
    // A cut never moves global time: track gain curves/touch spans remain
    // unchanged, just as for the individual SplitClip/SplitMidiClip commands.
    for &frame in frames {
        let beat = project.tempo_map().frame_to_beat(frame)?;
        for track_id in tracks {
            let track = project
                .track(*track_id)
                .ok_or(Invalid("split track no longer exists"))?;
            for clip in track.clips() {
                let end = clip
                    .start()
                    .checked_add(clip.duration())
                    .ok_or(Invalid("clip timeline end overflow"))?;
                if clip.start() < frame && frame < end {
                    commands.push(Command::SplitClip {
                        clip: clip.id(),
                        frame,
                    });
                }
            }
            for clip in track.midi_clips() {
                let end = clip
                    .start()
                    .raw()
                    .checked_add(clip.length().raw())
                    .ok_or(Invalid("midi clip timeline end overflow"))?;
                if clip.start() < beat && beat.raw() < end {
                    commands.push(Command::SplitMidiClip {
                        clip: clip.id(),
                        beat,
                    });
                }
            }
        }
    }
    let count = commands.len();
    if count == 0 {
        return Ok(0);
    }
    let mut candidate = project.clone();
    for command in commands {
        candidate.apply(command)?;
    }
    *project = candidate;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BeatTime, Editor};
    use std::path::PathBuf;

    fn fixture() -> (Editor, TrackId, TrackId) {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 8).expect("editor");
        let Created::Track(audio_track) = editor
            .execute(Command::AddTrack {
                name: "audio".into(),
            })
            .expect("audio track")
        else {
            panic!("expected audio track")
        };
        let Created::Track(midi_track) = editor
            .execute(Command::AddTrack {
                name: "midi".into(),
            })
            .expect("midi track")
        else {
            panic!("expected midi track")
        };
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: PathBuf::from("fixture.wav"),
                sample_rate: 48_000,
                frames: 96_000,
            })
            .expect("asset")
        else {
            panic!("expected asset")
        };
        editor
            .execute(Command::PlaceClip {
                track: audio_track,
                asset,
                start: 0,
                duration: 96_000,
                source_offset: 0,
                source_length: 96_000,
                looping: false,
            })
            .expect("audio clip");
        editor
            .execute(Command::PlaceMidiClip {
                track: midi_track,
                start: BeatTime::ZERO,
                length: BeatTime::from_raw(4 * BeatTime::ONE.raw()),
            })
            .expect("midi clip");
        let midi_clip = editor
            .project()
            .track(midi_track)
            .expect("midi track")
            .midi_clips()[0]
            .id();
        editor
            .execute(Command::AddMidiNote {
                clip: midi_clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            })
            .expect("midi note");
        (editor, audio_track, midi_track)
    }

    type Intervals = Vec<(u64, u64)>;

    fn intervals(project: &Project, audio: TrackId, midi: TrackId) -> (Intervals, Intervals) {
        let audio = project
            .track(audio)
            .unwrap()
            .clips()
            .iter()
            .map(|c| (c.start(), c.start() + c.duration()))
            .collect();
        let midi = project
            .track(midi)
            .unwrap()
            .midi_clips()
            .iter()
            .map(|c| {
                (
                    project.tempo_map().beat_to_frame(c.start()).unwrap(),
                    project
                        .tempo_map()
                        .beat_to_frame(BeatTime::from_raw(c.start().raw() + c.length().raw()))
                        .unwrap(),
                )
            })
            .collect();
        (audio, midi)
    }

    #[test]
    fn selected_edges_preserve_canonical_split_content_and_one_history_step() {
        let (mut fixture, audio, midi) = fixture();
        fixture
            .execute(Command::ReplaceTrackGainEnvelope {
                track: audio,
                points: vec![crate::GainPoint {
                    incoming_gain: None,
                    beat: BeatTime::ONE,
                    gain: 0.5,
                    outgoing: crate::GainInterpolation::Linear,
                }],
            })
            .unwrap();
        let audio_clip = fixture.project().track(audio).unwrap().clips()[0].id();
        fixture
            .execute(Command::SetClipFades {
                clip: audio_clip,
                fade_in: 40_000,
                fade_out: 40_000,
            })
            .unwrap();
        fixture
            .execute(Command::SpliceGainTouch {
                track: audio,
                expected_epoch: 0,
                span: crate::GainTouchSpan {
                    source_start: BeatTime::ZERO,
                    start: BeatTime::ZERO,
                    end: BeatTime::ONE,
                    context: crate::GainTouchContext {
                        recording_epoch: 1,
                        project_generation: 0,
                        canonical_revision: 1,
                        audio_revision: 1,
                        transport_epoch: 1,
                        gesture_epoch: 1,
                        gesture_serial: 1,
                        sample_rate: 48_000,
                        loop_pass: 0,
                    },
                    steps: vec![crate::GainTouchStep {
                        beat: BeatTime::ZERO,
                        gain: 0.75,
                        origin_beat: BeatTime::ZERO,
                        source_sequence: 1,
                    }],
                },
            })
            .unwrap();
        let midi_clip = fixture.project().track(midi).unwrap().midi_clips()[0].id();
        fixture
            .execute(Command::ReplaceMidiClipVolume {
                clip: midi_clip,
                volume: crate::MidiClipVolume::new(
                    vec![
                        crate::MidiClipVolumePoint::linear(
                            BeatTime::ZERO,
                            crate::MidiClipGain::new(0.5).unwrap(),
                        ),
                        crate::MidiClipVolumePoint::linear(
                            BeatTime::from_raw(4 * BeatTime::ONE.raw()),
                            crate::MidiClipGain::new(1.5).unwrap(),
                        ),
                    ],
                    BeatTime::from_raw(4 * BeatTime::ONE.raw()),
                )
                .unwrap(),
            })
            .unwrap();
        let before = fixture.project().clone();
        // The batch must retain the complete semantics of the established
        // split commands, including source identities, note gates and fades.
        let mut expected = before.clone();
        for frame in [72_000, 24_000] {
            apply(&mut expected, &[audio, midi], frame).unwrap();
        }
        let mut editor = Editor::new(before.clone(), 8).unwrap();
        editor
            .execute(Command::SplitTimeRange {
                range: TimeRange::new(vec![audio, midi], 24_000, 72_000).unwrap(),
            })
            .unwrap();
        assert_eq!(editor.project(), &expected);
        let edges = vec![(0, 24_000), (24_000, 72_000), (72_000, 96_000)];
        assert_eq!(
            intervals(editor.project(), audio, midi),
            (edges.clone(), edges)
        );
        assert!(editor.undo());
        let mut restored = before.snapshot();
        restored.next_id = expected.snapshot().next_id;
        assert_eq!(editor.project().snapshot(), restored);
        assert!(!editor.can_undo());
        assert!(editor.redo());
        assert_eq!(editor.project(), &expected);
    }

    #[test]
    fn selection_preserves_existing_cuts_and_edges_are_noops_with_redo_intact() {
        let (fixture, audio, midi) = fixture();
        let mut editor = Editor::new(fixture.project().clone(), 8).unwrap();
        editor
            .execute(Command::SplitTracksAtFrame {
                tracks: vec![audio, midi],
                frame: 48_000,
            })
            .unwrap();
        editor
            .execute(Command::SplitTimeRange {
                range: TimeRange::new(vec![audio, midi], 24_000, 72_000).unwrap(),
            })
            .unwrap();
        let edges = vec![
            (0, 24_000),
            (24_000, 48_000),
            (48_000, 72_000),
            (72_000, 96_000),
        ];
        assert_eq!(
            intervals(editor.project(), audio, midi),
            (edges.clone(), edges)
        );
        assert!(editor.undo());
        let before_noop = editor.project().clone();
        // Both exact existing edges and a surrounding interval in gaps have
        // no cuts. They must not create history or erase the redo branch.
        for (start, end) in [(0, 48_000), (0, 120_000)] {
            editor
                .execute(Command::SplitTimeRange {
                    range: TimeRange::new(vec![audio, midi], start, end).unwrap(),
                })
                .unwrap();
            assert_eq!(editor.project(), &before_noop);
            assert!(editor.can_redo());
        }
        assert!(editor.redo());
        let edges = vec![
            (0, 24_000),
            (24_000, 48_000),
            (48_000, 72_000),
            (72_000, 96_000),
        ];
        assert_eq!(
            intervals(editor.project(), audio, midi),
            (edges.clone(), edges)
        );
    }

    #[test]
    fn selection_boundaries_in_gaps_cut_only_intersected_material() {
        let (fixture, audio, midi) = fixture();
        let mut source = fixture.project().clone();
        let clip = source.track(audio).unwrap().clips()[0].id();
        source
            .apply(Command::MoveClip {
                clip,
                track: audio,
                start: 24_000,
            })
            .unwrap();
        let clip = source.track(midi).unwrap().midi_clips()[0].id();
        source
            .apply(Command::MoveMidiClip {
                clip,
                track: midi,
                start: BeatTime::ONE,
            })
            .unwrap();
        for (start, end, cuts) in [
            (12_000, 60_000, vec![24_000, 60_000, 120_000]),
            (60_000, 132_000, vec![24_000, 60_000, 120_000]),
            (12_000, 132_000, vec![24_000, 120_000]),
            (24_000, 120_000, vec![24_000, 120_000]),
        ] {
            let mut project = source.clone();
            project
                .apply(Command::SplitTimeRange {
                    range: TimeRange::new(vec![audio, midi], start, end).unwrap(),
                })
                .unwrap();
            let expected: Vec<_> = cuts.windows(2).map(|w| (w[0], w[1])).collect();
            assert_eq!(
                intervals(&project, audio, midi),
                (expected.clone(), expected),
                "{start}..{end}"
            );
        }
    }

    #[test]
    fn range_stale_scope_preparation_and_late_id_failure_leave_no_partial_cuts() {
        let (fixture, audio, midi) = fixture();
        let before = fixture.project().clone();
        let mut project = before.clone();
        assert!(
            project
                .apply(Command::SplitTimeRange {
                    range: TimeRange::new(vec![midi, audio], 24_000, 72_000).unwrap(),
                })
                .is_err()
        );
        assert_eq!(project, before);
        let mut editor = Editor::new(before, 8).unwrap();
        let result: Result<(_, ()), Invalid> = editor.execute_prepared_transition(
            Command::SplitTimeRange {
                range: TimeRange::new(vec![audio, midi], 24_000, 72_000).unwrap(),
            },
            |source, candidate| {
                assert_eq!(source.track(audio).unwrap().clips().len(), 1);
                assert_eq!(candidate.track(audio).unwrap().clips().len(), 3);
                Err(Invalid("test preparation refused"))
            },
        );
        assert!(result.is_err());
        assert_eq!(editor.project(), &project);
        assert!(!editor.can_undo());
        // At least the end cuts succeed privately before ID exhaustion at
        // the start boundary. Direct callers also retain all-or-nothing edits.
        project.next_id = u64::MAX - 2;
        let before = project.clone();
        assert!(
            project
                .apply(Command::SplitTimeRange {
                    range: TimeRange::new(vec![audio, midi], 24_000, 72_000).unwrap(),
                })
                .is_err()
        );
        assert_eq!(project, before);
    }

    #[test]
    fn mixed_audio_midi_splits_are_one_undoable_edit() {
        let (fixture, audio_track, midi_track) = fixture();
        let before = fixture.project().clone();
        let mut editor = Editor::new(before.clone(), 8).expect("fresh history");
        assert_eq!(
            editor
                .execute(Command::SplitTracksAtFrame {
                    tracks: vec![audio_track, midi_track],
                    frame: 24_000,
                })
                .expect("split all tracks"),
            Created::None
        );
        assert_eq!(
            editor.project().track(audio_track).unwrap().clips().len(),
            2
        );
        assert_eq!(
            editor
                .project()
                .track(midi_track)
                .unwrap()
                .midi_clips()
                .len(),
            2
        );
        assert!(editor.can_undo());
        assert!(editor.undo());
        assert_eq!(
            editor.project().track(audio_track).unwrap().clips(),
            before.track(audio_track).unwrap().clips()
        );
        assert_eq!(
            editor.project().track(midi_track).unwrap().midi_clips(),
            before.track(midi_track).unwrap().midi_clips()
        );
        assert!(!editor.can_undo(), "all splits share one history step");
        assert!(editor.can_redo());
        assert!(editor.redo());
        assert_eq!(
            editor.project().track(audio_track).unwrap().clips().len(),
            2
        );
        assert_eq!(
            editor
                .project()
                .track(midi_track)
                .unwrap()
                .midi_clips()
                .len(),
            2
        );
    }

    #[test]
    fn split_scales_to_one_hundred_mixed_tracks_in_one_history_step() {
        let mut builder = Editor::new(Project::new(48_000).expect("project"), 8).expect("builder");
        let Created::Asset(asset) = builder
            .execute(Command::AddAsset {
                path: PathBuf::from("fixture.wav"),
                sample_rate: 48_000,
                frames: 96_000,
            })
            .expect("asset")
        else {
            panic!("expected asset")
        };
        let mut tracks = Vec::new();
        for index in 0..100 {
            let Created::Track(track) = builder
                .execute(Command::AddTrack {
                    name: format!("track {index}"),
                })
                .expect("track")
            else {
                panic!("expected track")
            };
            tracks.push(track);
            if index % 2 == 0 {
                builder
                    .execute(Command::PlaceClip {
                        track,
                        asset,
                        start: 0,
                        duration: 96_000,
                        source_offset: 0,
                        source_length: 96_000,
                        looping: false,
                    })
                    .expect("audio clip");
            } else {
                builder
                    .execute(Command::PlaceMidiClip {
                        track,
                        start: BeatTime::ZERO,
                        length: BeatTime::from_raw(4 * BeatTime::ONE.raw()),
                    })
                    .expect("midi clip");
            }
        }
        let before = builder.project().clone();
        let mut editor = Editor::new(before, 8).expect("fresh history");
        editor
            .execute(Command::SplitTracksAtFrame {
                tracks,
                frame: 24_000,
            })
            .expect("split one hundred tracks");
        for (index, track) in editor.project().tracks().iter().enumerate() {
            if index % 2 == 0 {
                assert_eq!(track.clips().len(), 2);
                assert!(track.midi_clips().is_empty());
            } else {
                assert_eq!(track.midi_clips().len(), 2);
                assert!(track.clips().is_empty());
            }
        }
        assert!(editor.undo());
        assert!(
            editor
                .project()
                .tracks()
                .iter()
                .all(|track| { track.clips().len() + track.midi_clips().len() == 1 })
        );
        assert!(!editor.can_undo());
    }

    #[test]
    fn invalid_track_sets_and_boundary_cursor_are_atomic() {
        let (fixture, audio_track, midi_track) = fixture();
        let mut project = fixture.project().clone();
        let Created::Track(missing) = project
            .apply(Command::AddTrack {
                name: "temporary".into(),
            })
            .expect("temporary track")
        else {
            panic!("expected temporary track")
        };
        project
            .apply(Command::RemoveTrack(missing))
            .expect("remove temporary track");
        let before = project.clone();
        let mut editor = Editor::new(project, 8).expect("fresh history");
        for tracks in [
            Vec::new(),
            vec![audio_track, audio_track],
            vec![audio_track, missing],
        ] {
            assert!(
                editor
                    .execute(Command::SplitTracksAtFrame {
                        tracks,
                        frame: 24_000
                    })
                    .is_err()
            );
            assert_eq!(editor.project(), &before);
            assert!(!editor.can_undo());
        }

        // Frame 96,000 is the clips' shared end: touching the edge is not a split.
        assert!(
            editor
                .execute(Command::SplitTracksAtFrame {
                    tracks: vec![audio_track, midi_track],
                    frame: 96_000,
                })
                .is_err()
        );
        assert_eq!(editor.project(), &before);
        assert!(!editor.can_undo());

        assert!(
            editor
                .execute(Command::SplitTracksAtFrame {
                    tracks: vec![audio_track, midi_track],
                    frame: u64::MAX,
                })
                .is_err()
        );
        assert_eq!(editor.project(), &before);
        assert!(!editor.can_undo());
    }

    #[test]
    fn project_apply_rejects_late_failure_without_partial_split() {
        let (editor, audio_track, midi_track) = fixture();
        let mut project = editor.project().clone();
        // The audio split can allocate the final usable ID, then the following
        // MIDI split fails while allocating its new clip ID.
        project.next_id = u64::MAX - 1;
        let before = project.clone();
        assert!(
            project
                .apply(Command::SplitTracksAtFrame {
                    tracks: vec![audio_track, midi_track],
                    frame: 24_000,
                })
                .is_err()
        );
        assert_eq!(project, before);
    }
}
