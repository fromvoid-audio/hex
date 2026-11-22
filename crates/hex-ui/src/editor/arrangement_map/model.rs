//! Canonical, revision-keyed arrangement miniature projection.
use super::super::viewport::Viewport;
use hex_project::{BeatTime, Project, TrackId};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::Rc,
    sync::{Arc, Weak},
};

pub(in crate::editor) const MAX_ROWS: usize = 32;
pub(in crate::editor) type SectionRows = Rc<[Box<[Interval]>]>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::editor) struct Interval {
    pub(in crate::editor) start: u64,
    pub(in crate::editor) end: u64,
}

pub(in crate::editor) struct MapCache {
    generation: u64,
    revision: u64,
    focus: Box<[(u64, u64)]>,
    revealed: Option<TrackId>,
    pub(super) rows: Vec<Vec<Interval>>,
}

/// Revision-keyed per-track clip intervals for section clip previews.
/// Captures all tracks in canonical order regardless of arranger visibility
/// filters (section focus, tags, group folds, scroll) and caches each
/// section's own clipped row projection by its full frame span.
pub(in crate::editor) struct SectionMapCache {
    generation: u64,
    revision: u64,
    project: Weak<Project>,
    tracks: Vec<Box<[Interval]>>,
    sections: RefCell<BTreeMap<(u64, u64), SectionRows>>,
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
    focus: &[(u64, u64)],
    revealed: Option<TrackId>,
) -> Rc<MapCache> {
    let mut slot = cache.borrow_mut();
    if let Some(current) = slot.as_ref().filter(|data| {
        data.generation == generation
            && data.revision == revision
            && data.focus.as_ref() == focus
            && data.revealed == revealed
    }) {
        return Rc::clone(current);
    }
    let next = Rc::new(MapCache {
        generation,
        revision,
        rows: project_rows(project, focus, revealed),
        focus: focus.into(),
        revealed,
    });
    *slot = Some(Rc::clone(&next));
    next
}

fn project_track_intervals(project: &Project) -> Vec<Box<[Interval]>> {
    let mut tracks = Vec::new();
    for track in project.tracks() {
        let mut intervals = Vec::new();
        append_track_intervals(project, track, &mut intervals);
        merge(&mut intervals);
        if !intervals.is_empty() {
            tracks.push(intervals.into_boxed_slice());
        }
    }
    tracks
}

fn track_intersects_range(row: &[Interval], start: u64, end: u64) -> bool {
    if start >= end {
        return false;
    }
    let first = row.partition_point(|span| span.end <= start);
    row.get(first).is_some_and(|span| span.start < end)
}

fn section_rows_from_tracks(
    tracks: &[Box<[Interval]>],
    section_start: u64,
    section_end: u64,
) -> SectionRows {
    if section_start >= section_end {
        return Rc::from([]);
    }
    let count = tracks
        .iter()
        .filter(|row| track_intersects_range(row, section_start, section_end))
        .count();
    if count == 0 {
        return Rc::from([]);
    }
    let row_count = count.min(MAX_ROWS);
    let mut rows = vec![Vec::new(); row_count];
    for (index, track) in tracks
        .iter()
        .filter(|row| track_intersects_range(row, section_start, section_end))
        .enumerate()
    {
        let dst = &mut rows[index * row_count / count];
        let first = track.partition_point(|span| span.end <= section_start);
        for &span in track[first..]
            .iter()
            .take_while(|span| span.start < section_end)
        {
            let start = span.start.max(section_start);
            let end = span.end.min(section_end);
            if start < end {
                dst.push(Interval { start, end });
            }
        }
    }
    if count > row_count {
        for row in &mut rows {
            merge(row);
        }
    }
    rows.into_iter()
        .map(Vec::into_boxed_slice)
        .collect::<Vec<_>>()
        .into()
}

/// Project only the tracks and clipped intervals intersecting `[section_start, section_end)`.
/// Independent of arranger track visibility (section focus, tag filters, group
/// folds, scroll) and includes muted clips.
#[cfg(all(test, feature = "ui-unit-tier"))]
fn project_section_rows(
    project: &Project,
    section_start: u64,
    section_end: u64,
) -> Vec<Vec<Interval>> {
    let tracks = project_track_intervals(project);
    section_rows_from_tracks(&tracks, section_start, section_end)
        .iter()
        .map(|row| row.to_vec())
        .collect()
}

pub(in crate::editor) fn cached_section_rows(
    cache: &RefCell<Option<Rc<SectionMapCache>>>,
    generation: u64,
    revision: u64,
    project: &Arc<Project>,
    section_start: u64,
    section_end: u64,
) -> SectionRows {
    if section_start >= section_end {
        return Rc::from([]);
    }
    let entry = {
        let mut slot = cache.borrow_mut();
        if let Some(current) = slot.as_ref().filter(|data| {
            data.generation == generation
                && data.revision == revision
                && data.project.as_ptr() == Arc::as_ptr(project)
        }) {
            Rc::clone(current)
        } else {
            let next = Rc::new(SectionMapCache {
                generation,
                revision,
                project: Arc::downgrade(project),
                tracks: project_track_intervals(project),
                sections: RefCell::new(BTreeMap::new()),
            });
            *slot = Some(Rc::clone(&next));
            next
        }
    };
    let key = (section_start, section_end);
    let mut sections = entry.sections.borrow_mut();
    if let Some(rows) = sections.get(&key) {
        return Rc::clone(rows);
    }
    let rows = section_rows_from_tracks(&entry.tracks, section_start, section_end);
    if sections.len() >= hex_project::MAX_ARRANGEMENT_SECTIONS {
        sections.clear();
    }
    sections.insert(key, Rc::clone(&rows));
    rows
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

#[cfg(all(test, any(feature = "ui-test-support", feature = "ui-unit-tier")))]
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
    #[cfg(all(test, feature = "ui-unit-tier"))]
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

        let audio = project.tracks()[0].clips()[0].id();
        let mut editor = Editor::new(project, 8).unwrap();
        editor
            .execute(Command::SetClipActive {
                clip: audio,
                active: false,
            })
            .unwrap();
        let muted = editor.project();
        let all = project_rows(muted, &[], None);
        assert_eq!(all.len(), 2);
        assert_eq!(project_rows(muted, &[(48_000, 96_000)], None), all[1..]);
    }
    #[cfg(all(test, feature = "ui-unit-tier"))]
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
    #[cfg(all(test, feature = "ui-unit-tier"))]
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
    #[cfg(all(test, feature = "ui-unit-tier"))]
    #[test]
    fn cache_reuses_pan_independent_data_and_invalidates_owner_context() {
        let project = project();
        let cache = RefCell::new(None);
        let first = cached(&cache, 1, 1, &project, &[], None);
        assert!(Rc::ptr_eq(
            &first,
            &cached(&cache, 1, 1, &project, &[], None)
        ));
        let second = cached(&cache, 1, 2, &project, &[], None);
        assert!(!Rc::ptr_eq(&first, &second));
        let third = cached(&cache, 2, 2, &project, &[], None);
        assert!(!Rc::ptr_eq(&second, &third));
        let fourth = cached(&cache, 2, 2, &project, &[(0, 24_001)], None);
        assert!(!Rc::ptr_eq(&third, &fourth));
        assert!(project_rows(&Project::new(48_000).unwrap(), &[], None).is_empty());
    }
    #[cfg(all(test, feature = "ui-unit-tier"))]
    #[test]
    fn revisioned_edits_and_undo_restore_canonical_ranges() {
        let mut model = Editor::new(project(), 8).unwrap();
        let track = model.project().tracks()[0].id();
        let clip = model.project().tracks()[0].clips()[0].id();
        let cache = RefCell::new(None);
        let original = cached(&cache, 1, 1, model.project(), &[], None);
        model.execute(Command::RemoveClip(clip)).unwrap();
        let removed = cached(&cache, 1, 2, model.project(), &[], None);
        assert!(removed.rows[0].is_empty());
        assert!(model.undo());
        let restored = cached(&cache, 1, 3, model.project(), &[], None);
        assert_eq!(restored.rows, original.rows);
        model
            .execute(Command::SetTrackColor {
                track,
                color: hex_project::ElementColor::Violet,
            })
            .unwrap();
        let recolored = cached(&cache, 1, 4, model.project(), &[], None);
        assert_eq!(recolored.rows, restored.rows);
        assert!(!Rc::ptr_eq(&recolored, &restored));
    }
    #[cfg(all(test, feature = "ui-unit-tier"))]
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
    #[cfg(all(test, feature = "ui-unit-tier"))]
    #[test]
    fn section_rows_keep_only_intersecting_clipped_clips_including_muted_and_grouped_tracks() {
        let mut model = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let mut tracks = Vec::new();
        for name in ["empty", "a-only", "spanning", "b-only", "muted-a"] {
            let Created::Track(id) = model
                .execute(Command::AddTrack { name: name.into() })
                .unwrap()
            else {
                panic!("track")
            };
            tracks.push(id);
        }
        // Group track owns no clips of its own and must not add an empty row.
        model
            .execute(Command::GroupTracks {
                tracks: hex_project::TrackSelectionIds::new(&[tracks[1], tracks[2]]).unwrap(),
                name: "group".into(),
            })
            .unwrap();
        // Adjacent clips inside section A [0, 48_000) stay distinct; touching at
        // 48_000 does not leak into section B [48_000, 96_000).
        for (start, duration) in [(0, 24_000), (24_000, 24_000)] {
            model
                .execute(Command::ImportPreparedAudioClip {
                    track: tracks[1],
                    start,
                    path: "/tmp/section-a.wav".into(),
                    source_sample_rate: 48_000,
                    source_frames: duration,
                    playback_frames: duration,
                    conversion_version: IDENTITY_CONVERSION,
                })
                .unwrap();
        }
        // Spans both sections [24_000, 72_000) at default 120 BPM (beats 1..3).
        model
            .execute(Command::PlaceMidiClip {
                track: tracks[2],
                start: BeatTime::ONE,
                length: BeatTime::from_quarters(2).unwrap(),
            })
            .unwrap();
        // Starts right at section B [48_000, 96_000) — must not enter section A.
        model
            .execute(Command::PlaceMidiClip {
                track: tracks[3],
                start: BeatTime::from_quarters(2).unwrap(),
                length: BeatTime::from_quarters(2).unwrap(),
            })
            .unwrap();
        // Muted clip inside section A must still render in section A's preview.
        model
            .execute(Command::ImportPreparedAudioClip {
                track: tracks[4],
                start: 12_000,
                path: "/tmp/muted-a.wav".into(),
                source_sample_rate: 48_000,
                source_frames: 24_000,
                playback_frames: 24_000,
                conversion_version: IDENTITY_CONVERSION,
            })
            .unwrap();
        let muted = model.project().track(tracks[4]).unwrap().clips()[0].id();
        model
            .execute(Command::SetClipActive {
                clip: muted,
                active: false,
            })
            .unwrap();

        let project = model.project();
        let before = project.snapshot();
        assert_eq!(
            project_section_rows(project, 0, 48_000),
            vec![
                vec![
                    Interval {
                        start: 0,
                        end: 24_000
                    },
                    Interval {
                        start: 24_000,
                        end: 48_000
                    },
                ],
                vec![Interval {
                    start: 24_000,
                    end: 48_000
                }],
                vec![Interval {
                    start: 12_000,
                    end: 36_000
                }],
            ]
        );
        assert_eq!(
            project_section_rows(project, 48_000, 96_000),
            vec![
                vec![Interval {
                    start: 48_000,
                    end: 72_000
                }],
                vec![Interval {
                    start: 48_000,
                    end: 96_000
                }],
            ]
        );
        assert!(project_section_rows(project, 96_000, 144_000).is_empty());
        assert!(project_section_rows(project, 48_000, 48_000).is_empty());
        assert_eq!(project.snapshot(), before);
    }
    #[cfg(all(test, feature = "ui-unit-tier"))]
    #[test]
    fn section_cache_reuses_rows_distinguishes_preview_layouts_and_bounds_dense_sections() {
        let mut model = Editor::new(Project::new(48_000).unwrap(), 128).unwrap();
        for index in 0..64 {
            let Created::Track(track) = model
                .execute(Command::AddTrack {
                    name: index.to_string(),
                })
                .unwrap()
            else {
                panic!("track")
            };
            // First 48 tracks play in section A [0, 24_000); last 2 tracks play in section B [24_000, 48_000).
            if index < 48 {
                model
                    .execute(Command::PlaceMidiClip {
                        track,
                        start: BeatTime::ZERO,
                        length: BeatTime::ONE,
                    })
                    .unwrap();
            } else if index >= 62 {
                model
                    .execute(Command::PlaceMidiClip {
                        track,
                        start: BeatTime::ONE,
                        length: BeatTime::ONE,
                    })
                    .unwrap();
            }
        }
        let cache = RefCell::new(None);
        let canonical = Arc::new(model.project().clone());
        let dense = cached_section_rows(&cache, 1, 1, &canonical, 0, 24_000);
        assert_eq!(dense.len(), MAX_ROWS);
        assert!(Rc::ptr_eq(
            &dense,
            &cached_section_rows(&cache, 1, 1, &canonical, 0, 24_000)
        ));
        let sparse = cached_section_rows(&cache, 1, 1, &canonical, 24_000, 48_000);
        assert_eq!(sparse.len(), 2, "sparse section gets only its own two rows");
        assert!(Rc::ptr_eq(
            &sparse,
            &cached_section_rows(&cache, 1, 1, &canonical, 24_000, 48_000)
        ));

        // A pending layout preview at the same (generation, revision) uses a
        // distinct Arc<Project> and must never serve stale canonical rows.
        let mut preview_model = Editor::new((*canonical).clone(), 4).unwrap();
        let clip = preview_model.project().tracks()[62].midi_clips()[0].id();
        preview_model
            .execute(Command::RemoveMidiClip(clip))
            .unwrap();
        let preview = Arc::new(preview_model.project().clone());
        let preview_rows = cached_section_rows(&cache, 1, 1, &preview, 24_000, 48_000);
        assert_eq!(preview_rows.len(), 1);
        assert!(!Rc::ptr_eq(&sparse, &preview_rows));
    }
}
