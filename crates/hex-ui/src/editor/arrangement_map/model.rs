//! Canonical, revision-keyed arrangement miniature projection.
use super::super::viewport::Viewport;
use hex_project::{BeatTime, Project, TrackId};
use std::{cell::RefCell, rc::Rc};

pub(in crate::editor) const MAX_ROWS: usize = 32;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::editor) struct Interval {
    pub(in crate::editor) start: u64,
    pub(in crate::editor) end: u64,
}

pub(in crate::editor) struct MapCache {
    generation: u64,
    revision: u64,
    focus: Rc<[(u64, u64)]>,
    revealed: Option<TrackId>,
    pub(super) rows: Vec<Vec<Interval>>,
}

pub(in crate::editor) fn merge(row: &mut Vec<Interval>) {
    row.sort_unstable_by_key(|span| (span.start, span.end));
    let mut count = 0;
    for read in 0..row.len() {
        let span = row[read];
        if span.start >= span.end {
            continue;
        }
        if count > 0 && row[count - 1].end > span.start {
            row[count - 1].end = row[count - 1].end.max(span.end);
        } else {
            row[count] = span;
            count += 1;
        }
    }
    row.truncate(count);
}

pub(in crate::editor) fn project_rows(
    project: &Project,
    focus: &[(u64, u64)],
    revealed: Option<TrackId>,
) -> Vec<Vec<Interval>> {
    let visible = |track: &&hex_project::Track| {
        revealed == Some(track.id())
            || super::super::arrangement_focus::track_intersects_sections(project, track, focus)
    };
    let count = project.tracks().iter().filter(visible).count();
    let mut rows = vec![Vec::new(); count.min(MAX_ROWS)];
    if rows.is_empty() {
        return rows;
    }
    let row_count = rows.len();
    for (index, track) in project.tracks().iter().filter(visible).enumerate() {
        let row = &mut rows[index * row_count / count];
        append_track_intervals(project, track, row);
    }
    for row in &mut rows {
        merge(row);
    }
    rows
}

/// Canonical clip containers, independent of view filters and preview data.
pub(in crate::editor) fn append_track_intervals(
    project: &Project,
    track: &hex_project::Track,
    row: &mut Vec<Interval>,
) {
    for clip in track.clips() {
        if let Some(end) = clip.start().checked_add(clip.duration()) {
            row.push(Interval {
                start: clip.start(),
                end,
            });
        }
    }
    for clip in track.midi_clips() {
        let Some(end) = clip.start().raw().checked_add(clip.length().raw()) else {
            continue;
        };
        if let (Ok(start), Ok(end)) = (
            project.tempo_map().beat_to_frame(clip.start()),
            project.tempo_map().beat_to_frame(BeatTime::from_raw(end)),
        ) {
            row.push(Interval { start, end });
        }
    }
}

pub(in crate::editor) fn cached(
    cache: &RefCell<Option<Rc<MapCache>>>,
    generation: u64,
    revision: u64,
    project: &Project,
    focus: Rc<[(u64, u64)]>,
    revealed: Option<TrackId>,
) -> Rc<MapCache> {
    let mut slot = cache.borrow_mut();
    if let Some(current) = slot.as_ref().filter(|data| {
        data.generation == generation
            && data.revision == revision
            && data.focus == focus
            && data.revealed == revealed
    }) {
        return Rc::clone(current);
    }
    let next = Rc::new(MapCache {
        generation,
        revision,
        rows: project_rows(project, &focus, revealed),
        focus,
        revealed,
    });
    *slot = Some(Rc::clone(&next));
    next
}

pub(in crate::editor) fn project_interval(
    span: Interval,
    viewport: Viewport,
    width: f32,
) -> Option<(f32, f32)> {
    let limit = viewport.end()?;
    if viewport.length == 0 || !width.is_finite() || width <= 0.0 {
        return None;
    }
    let start = span.start.max(viewport.start);
    let end = span.end.min(limit);
    if start >= end {
        return None;
    }
    let x = |frame: u64| {
        ((frame - viewport.start) as f64 / viewport.length as f64 * f64::from(width)) as f32
    };
    Some((x(start), x(end)))
}

#[cfg(all(test, any(feature = "ui-unit-tier", feature = "ui-test-support")))]
pub(in crate::editor) mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, IDENTITY_CONVERSION, Tempo};
    pub(in crate::editor) fn project() -> Project {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let mut tracks = Vec::new();
        for name in ["audio", "midi"] {
            let Created::Track(id) = editor
                .execute(Command::AddTrack { name: name.into() })
                .unwrap()
            else {
                panic!("track");
            };
            tracks.push(id);
        }
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::ZERO,
                tempo: Tempo::new(120_000_000).unwrap(),
            })
            .unwrap();
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::from_quarters(2).unwrap(),
                tempo: Tempo::new(60_000_000).unwrap(),
            })
            .unwrap();
        editor
            .execute(Command::ImportPreparedAudioClip {
                track: tracks[0],
                start: 48_000,
                path: "/tmp/main-map.wav".into(),
                source_sample_rate: 48_000,
                source_frames: 48_000,
                playback_frames: 48_000,
                conversion_version: IDENTITY_CONVERSION,
            })
            .unwrap();
        editor
            .execute(Command::PlaceMidiClip {
                track: tracks[1],
                start: BeatTime::ONE,
                length: BeatTime::from_quarters(3).unwrap(),
            })
            .unwrap();
        editor.project().clone()
    }
    #[cfg(feature = "ui-unit-tier")]
    #[test]
    fn canonical_rows_convert_midi_tempo_and_focus_without_changing_project() {
        let project = project();
        let before = project.snapshot();
        assert_eq!(
            project_rows(&project, &[], None),
            vec![
                vec![Interval {
                    start: 48_000,
                    end: 96_000
                }],
                vec![Interval {
                    start: 24_000,
                    end: 144_000
                }]
            ]
        );
        assert_eq!(project_rows(&project, &[(0, 24_001)], None).len(), 1);
        assert_eq!(
            project_rows(&project, &[(0, 24_001)], Some(project.tracks()[0].id())).len(),
            2
        );
        assert_eq!(project.snapshot(), before);
    }
    #[cfg(feature = "ui-unit-tier")]
    #[test]
    fn overlapping_ranges_merge_but_adjacent_clips_remain_distinct() {
        let mut row = vec![
            Interval { start: 4, end: 6 },
            Interval { start: 0, end: 5 },
            Interval { start: 6, end: 9 },
            Interval { start: 10, end: 10 },
        ];
        merge(&mut row);
        assert_eq!(
            row,
            vec![Interval { start: 0, end: 6 }, Interval { start: 6, end: 9 }]
        );
    }
    #[cfg(feature = "ui-unit-tier")]
    #[test]
    fn exact_view_projection_clips_and_rejects_invalid_geometry() {
        let viewport = Viewport {
            start: 100,
            length: 100,
        };
        assert_eq!(
            project_interval(
                Interval {
                    start: 125,
                    end: 175
                },
                viewport,
                200.0
            ),
            Some((50.0, 150.0))
        );
        assert_eq!(
            project_interval(
                Interval {
                    start: 50,
                    end: 250
                },
                viewport,
                200.0
            ),
            Some((0.0, 200.0))
        );
        for width in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(
                project_interval(
                    Interval {
                        start: 100,
                        end: 200
                    },
                    viewport,
                    width
                ),
                None
            );
        }
        assert_eq!(
            project_interval(
                Interval {
                    start: 200,
                    end: 250
                },
                viewport,
                100.0
            ),
            None
        );
        assert_eq!(
            project_interval(
                Interval { start: 1, end: 2 },
                Viewport {
                    start: u64::MAX,
                    length: 1
                },
                100.0
            ),
            None
        );
    }
    #[cfg(feature = "ui-unit-tier")]
    #[test]
    fn cache_reuses_pan_independent_data_and_invalidates_owner_context() {
        let project = project();
        let cache = RefCell::new(None);
        let focus: Rc<[(u64, u64)]> = Rc::from([]);
        let first = cached(&cache, 1, 1, &project, Rc::clone(&focus), None);
        assert!(Rc::ptr_eq(
            &first,
            &cached(&cache, 1, 1, &project, Rc::clone(&focus), None)
        ));
        let second = cached(&cache, 1, 2, &project, Rc::clone(&focus), None);
        assert!(!Rc::ptr_eq(&first, &second));
        let third = cached(&cache, 2, 2, &project, Rc::clone(&focus), None);
        assert!(!Rc::ptr_eq(&second, &third));
        let fourth = cached(&cache, 2, 2, &project, Rc::from([(0, 24_001)]), None);
        assert!(!Rc::ptr_eq(&third, &fourth));
        assert!(project_rows(&Project::new(48_000).unwrap(), &[], None).is_empty());
    }
    #[cfg(feature = "ui-unit-tier")]
    #[test]
    fn revisioned_edits_and_undo_restore_canonical_ranges() {
        let mut model = Editor::new(project(), 8).unwrap();
        let track = model.project().tracks()[0].id();
        let clip = model.project().tracks()[0].clips()[0].id();
        let cache = RefCell::new(None);
        let original = cached(&cache, 1, 1, model.project(), Rc::from([]), None);
        model.execute(Command::RemoveClip(clip)).unwrap();
        let removed = cached(&cache, 1, 2, model.project(), Rc::from([]), None);
        assert!(removed.rows[0].is_empty());
        assert!(model.undo());
        let restored = cached(&cache, 1, 3, model.project(), Rc::from([]), None);
        assert_eq!(restored.rows, original.rows);
        model
            .execute(Command::SetTrackColor {
                track,
                color: hex_project::ElementColor::Violet,
            })
            .unwrap();
        let recolored = cached(&cache, 1, 4, model.project(), Rc::from([]), None);
        assert_eq!(recolored.rows, restored.rows);
        assert!(!Rc::ptr_eq(&recolored, &restored));
    }
    #[cfg(feature = "ui-unit-tier")]
    #[test]
    fn dense_projects_are_bounded_without_losing_the_bottom_track() {
        let mut model = Editor::new(Project::new(48_000).unwrap(), 64).unwrap();
        for index in 0..64 {
            let Created::Track(track) = model
                .execute(Command::AddTrack {
                    name: index.to_string(),
                })
                .unwrap()
            else {
                panic!("track")
            };
            model
                .execute(Command::PlaceMidiClip {
                    track,
                    start: BeatTime::ZERO,
                    length: BeatTime::ONE,
                })
                .unwrap();
        }
        let rows = project_rows(model.project(), &[], None);
        assert_eq!(rows.len(), MAX_ROWS);
        assert!(
            rows.iter()
                .all(|row| row.len() == 1 && row[0].start == 0 && row[0].end == 24_000)
        );
    }
}
