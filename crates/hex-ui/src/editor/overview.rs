//! Compact, revision-cached Arrangement overview raster and viewport navigation.
use super::{EditorView, ViewState, viewport::Viewport};
use crate::theme;
use gpui::{
    Bounds, Div, MouseButton, MouseDownEvent, Window, canvas, div, fill, point, prelude::*, px,
    rgb, size,
};
use hex_project::{Project, TrackId};
use std::{cell::RefCell, rc::Rc};

const OVERVIEW_COLUMNS: usize = 128;
const OVERVIEW_ROWS: usize = 16;
const OVERVIEW_CELLS: usize = OVERVIEW_COLUMNS * OVERVIEW_ROWS;
const OVERVIEW_HEIGHT: f32 = 24.0;

const AUDIO_CELL: u8 = 1;
const MIDI_CELL: u8 = 2;
const MIXED_CELL: u8 = 3;

#[derive(Clone, Debug)]
pub(super) struct OverviewCache {
    generation: u64,
    revision: u64,
    horizon: u64,
    track_count: usize,
    cells: Vec<u8>,
    focus_ranges: Rc<[(u64, u64)]>,
    revealed_track: Option<TrackId>,
}

impl OverviewCache {
    pub(super) fn for_state(
        generation: u64,
        state: &ViewState,
        horizon: u64,
        focus_ranges: Rc<[(u64, u64)]>,
        revealed_track: Option<TrackId>,
    ) -> Self {
        Self {
            generation,
            revision: state.revision,
            horizon: horizon.max(1),
            track_count: state.project.tracks().len(),
            cells: filtered_cells(
                &state.project,
                horizon.max(1),
                &focus_ranges,
                revealed_track,
            ),
            focus_ranges,
            revealed_track,
        }
    }

    pub(super) fn matches(
        &self,
        generation: u64,
        state: &ViewState,
        horizon: u64,
        focus_ranges: &[(u64, u64)],
        revealed_track: Option<TrackId>,
    ) -> bool {
        self.focus_ranges.as_ref() == focus_ranges
            && self.revealed_track == revealed_track
            && self.generation == generation
            && self.revision == state.revision
            && self.horizon == horizon.max(1)
            && self.track_count == state.project.tracks().len()
    }

    pub(super) fn horizon(&self) -> u64 {
        self.horizon
    }

    fn cell(&self, row: usize, column: usize) -> u8 {
        self.cells
            .get(row.saturating_mul(OVERVIEW_COLUMNS).saturating_add(column))
            .copied()
            .unwrap_or(0)
    }
}

#[cfg(test)]
fn overview_cells(project: &Project, horizon: u64) -> Vec<u8> {
    filtered_cells(project, horizon, &[], None)
}
fn filtered_cells(
    project: &Project,
    horizon: u64,
    focus_ranges: &[(u64, u64)],
    revealed_track: Option<TrackId>,
) -> Vec<u8> {
    let mut cells = vec![0; OVERVIEW_CELLS];
    let track_count = project.tracks().len();
    if track_count == 0 || horizon == 0 {
        return cells;
    }
    for (track_index, track) in project.tracks().iter().enumerate() {
        if revealed_track != Some(track.id())
            && !super::arrangement_focus::track_intersects_sections(project, track, focus_ranges)
        {
            continue;
        }
        let row = track_index
            .saturating_mul(OVERVIEW_ROWS)
            .checked_div(track_count)
            .unwrap_or(0)
            .min(OVERVIEW_ROWS - 1);
        for clip in track.clips() {
            if let Some(end) = clip.start().checked_add(clip.duration()) {
                mark_interval(&mut cells, row, horizon, clip.start(), end, AUDIO_CELL);
            }
        }
        for clip in track.midi_clips() {
            let Some(end_beat) = clip.start().raw().checked_add(clip.length().raw()) else {
                continue;
            };
            let (Ok(start), Ok(end)) = (
                project.tempo_map().beat_to_frame(clip.start()),
                project
                    .tempo_map()
                    .beat_to_frame(hex_project::BeatTime::from_raw(end_beat)),
            ) else {
                continue;
            };
            mark_interval(&mut cells, row, horizon, start, end, MIDI_CELL);
        }
    }
    cells
}

fn mark_interval(cells: &mut [u8], row: usize, horizon: u64, start: u64, end: u64, kind: u8) {
    if horizon == 0 || row >= OVERVIEW_ROWS || start >= end || start >= horizon {
        return;
    }
    let clipped_end = end.min(horizon);
    let left = (u128::from(start) * OVERVIEW_COLUMNS as u128 / u128::from(horizon)) as usize;
    let right = (u128::from(clipped_end) * OVERVIEW_COLUMNS as u128)
        .div_ceil(u128::from(horizon))
        .min(OVERVIEW_COLUMNS as u128) as usize;
    let left = left.min(OVERVIEW_COLUMNS - 1);
    let right = right.max(left + 1).min(OVERVIEW_COLUMNS);
    for column in left..right {
        let index = row * OVERVIEW_COLUMNS + column;
        cells[index] = match (cells[index], kind) {
            (0, kind) => kind,
            (current, kind) if current == kind => current,
            _ => MIXED_CELL,
        };
    }
}

pub(super) fn widget(
    cache: Rc<OverviewCache>,
    viewport: Viewport,
    header_width: f32,
    entity: gpui::Entity<EditorView>,
) -> Div {
    let horizon = cache.horizon();
    let paint_cache = Rc::clone(&cache);
    div()
        .debug_selector(|| "arrangement-overview".to_owned())
        .relative()
        .h(px(OVERVIEW_HEIGHT))
        .flex_shrink_0()
        .bg(rgb(theme::color(theme::PaletteRole::Raised)))
        .child(
            div()
                .debug_selector(|| "arrangement-overview-map".to_owned())
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .right(px(header_width))
                .children([canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        paint_overview(&paint_cache, viewport, horizon, bounds, window);
                        let click_entity = entity.clone();
                        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                            if !phase.bubble()
                                || event.button != MouseButton::Left
                                || !bounds.contains(&event.position)
                            {
                                return;
                            }
                            let width = f32::from(bounds.size.width);
                            let local_x = f32::from(event.position.x - bounds.origin.x);
                            if !width.is_finite() || width <= 0.0 || !local_x.is_finite() {
                                return;
                            }
                            let fraction = (local_x / width).clamp(0.0, 1.0);
                            click_entity.update(cx, |view, _| {
                                view.center_timeline_overview(fraction);
                            });
                            window.refresh();
                        });
                    },
                )
                .absolute()
                .inset_0()]),
        )
}

fn paint_overview(
    cache: &OverviewCache,
    viewport: Viewport,
    horizon: u64,
    bounds: Bounds<gpui::Pixels>,
    window: &mut Window,
) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 || horizon == 0 {
        return;
    }
    let column_width = width / OVERVIEW_COLUMNS as f32;
    let row_height = height / OVERVIEW_ROWS as f32;
    for row in 0..OVERVIEW_ROWS {
        let mut column = 0;
        while column < OVERVIEW_COLUMNS {
            let kind = cache.cell(row, column);
            if kind == 0 {
                column += 1;
                continue;
            }
            let first = column;
            column += 1;
            while column < OVERVIEW_COLUMNS && cache.cell(row, column) == kind {
                column += 1;
            }
            let color = match kind {
                AUDIO_CELL => theme::color(theme::PaletteRole::Clip),
                MIDI_CELL => theme::color(theme::PaletteRole::Secondary),
                MIXED_CELL => theme::color(theme::PaletteRole::Primary),
                _ => theme::color(theme::PaletteRole::Muted),
            };
            window.paint_quad(fill(
                Bounds::new(
                    point(
                        bounds.origin.x + px(first as f32 * column_width),
                        bounds.origin.y + px(row as f32 * row_height),
                    ),
                    size(
                        px((column - first) as f32 * column_width),
                        px(row_height.max(1.0)),
                    ),
                ),
                rgb(color),
            ));
        }
    }
    let left = (viewport.start as f64 / horizon as f64 * f64::from(width)) as f32;
    let viewport_end = viewport.start.saturating_add(viewport.length).min(horizon);
    let right = (viewport_end as f64 / horizon as f64 * f64::from(width)) as f32;
    let left = left.clamp(0.0, width);
    let right = right.clamp(left, width);
    let color = rgb(theme::color(theme::PaletteRole::Text));
    let rects = [
        (left, 0.0, (right - left).max(1.0), 1.0),
        (left, height - 1.0, (right - left).max(1.0), 1.0),
        (left, 0.0, 1.0, height),
        ((right - 1.0).max(left), 0.0, 1.0, height),
    ];
    for (x, y, w, h) in rects {
        window.paint_quad(fill(
            Bounds::new(
                point(bounds.origin.x + px(x), bounds.origin.y + px(y)),
                size(px(w.max(1.0)), px(h.max(1.0))),
            ),
            color,
        ));
    }
}

impl EditorView {
    pub(super) fn center_timeline_overview(&mut self, fraction: f32) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        if !fraction.is_finite() || state.timeline_frames == 0 {
            return;
        }
        let horizon = self.viewport_horizon(state.timeline_frames);
        let current = self.visible_viewport(horizon);
        let length = current.length.max(1).min(horizon);
        let center = (f64::from(fraction.clamp(0.0, 1.0)) * horizon as f64)
            .round()
            .clamp(0.0, horizon as f64) as u64;
        let next = Viewport {
            start: center.saturating_sub(length / 2),
            length,
        }
        .clamp(horizon);
        self.install_arranger_navigation(next);
    }
}

pub(super) fn cached(
    cache: &RefCell<Option<Rc<OverviewCache>>>,
    generation: u64,
    state: &ViewState,
    horizon: u64,
    focus_ranges: Rc<[(u64, u64)]>,
    revealed_track: Option<TrackId>,
) -> Rc<OverviewCache> {
    let mut cache = cache.borrow_mut();
    if let Some(current) = cache.as_ref().filter(|current| {
        current.matches(generation, state, horizon, &focus_ranges, revealed_track)
    }) {
        return Rc::clone(current);
    }
    let next = Rc::new(OverviewCache::for_state(
        generation,
        state,
        horizon,
        focus_ranges,
        revealed_track,
    ));
    *cache = Some(Rc::clone(&next));
    next
}

#[cfg(test)]
mod tests {
    use super::{
        AUDIO_CELL, MIDI_CELL, MIXED_CELL, OVERVIEW_COLUMNS, OVERVIEW_ROWS, OverviewCache,
        mark_interval, overview_cells,
    };
    use hex_project::{BeatTime, Command, Created, Editor, IDENTITY_CONVERSION, Project, Tempo};
    use hex_session::service::ViewState;
    use std::{path::PathBuf, rc::Rc, sync::Arc};

    fn overview_project() -> Project {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 2).expect("editor");
        let mut last = None;
        for index in 0..100 {
            let Created::Track(track) = editor
                .execute(Command::AddTrack {
                    name: format!("track {index}"),
                })
                .expect("track")
            else {
                panic!("track result")
            };
            last = Some(track);
        }
        let track = last.expect("last track");
        editor
            .execute(Command::SetTempoPoint {
                beat: BeatTime::from_quarters(2).expect("tempo beat"),
                tempo: Tempo::new(60_000_000).expect("tempo"),
            })
            .expect("tempo point");
        editor
            .execute(Command::ImportPreparedAudioClip {
                track,
                start: 120_000,
                path: PathBuf::from("/tmp/overview.wav"),
                source_sample_rate: 48_000,
                source_frames: 48_000,
                playback_frames: 48_000,
                conversion_version: IDENTITY_CONVERSION,
            })
            .expect("audio clip");
        editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::from_quarters(4).expect("beat"),
                length: BeatTime::from_quarters(2).expect("length"),
            })
            .expect("midi clip");
        editor.project().clone()
    }

    #[test]
    fn focused_overview_keeps_all_clips_on_intersecting_tracks_and_rebuilds() {
        let project = overview_project();
        assert!(
            super::filtered_cells(&project, 300_000, &[(1, 100)], None)
                .iter()
                .all(|c| *c == 0)
        );
        assert_eq!(
            super::filtered_cells(&project, 300_000, &[(130_000, 140_000)], None),
            overview_cells(&project, 300_000),
            "the retained track includes MIDI beyond the selected interval"
        );
        let revealed = project.tracks().last().unwrap().id();
        assert_eq!(
            super::filtered_cells(&project, 300_000, &[(1, 100)], Some(revealed)),
            overview_cells(&project, 300_000)
        );
        let state = view_state(project, 1, 300_000);
        let cache = std::cell::RefCell::new(None);
        let all = super::cached(&cache, 1, &state, state.timeline_frames, Rc::from([]), None);
        let focused = super::cached(
            &cache,
            1,
            &state,
            state.timeline_frames,
            Rc::from([(1, 100)]),
            None,
        );
        assert!(!Rc::ptr_eq(&all, &focused));
        let with_revealed = super::cached(
            &cache,
            1,
            &state,
            state.timeline_frames,
            Rc::from([(1, 100)]),
            Some(revealed),
        );
        assert!(!Rc::ptr_eq(&focused, &with_revealed));
        assert!(!Rc::ptr_eq(
            &with_revealed,
            &super::cached(
                &cache,
                1,
                &state,
                state.timeline_frames,
                Rc::from([(1, 100)]),
                None
            )
        ));
        let focused = super::cached(
            &cache,
            1,
            &state,
            state.timeline_frames,
            Rc::from([(1, 100)]),
            None,
        );
        assert!(Rc::ptr_eq(
            &focused,
            &super::cached(
                &cache,
                1,
                &state,
                state.timeline_frames,
                Rc::from([(1, 100)]),
                None
            )
        ));
    }

    fn view_state(project: Project, revision: u64, timeline_frames: u64) -> ViewState {
        ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project: Arc::new(project),
            waveforms: Default::default(),
            source_overviews: Default::default(),
            audio_clip_waveforms: std::collections::BTreeMap::new(),
            midi_preview: Arc::new(Default::default()),
            gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
            timeline_frames,
            can_undo: false,
            can_redo: false,
            revision,
            unsaved: false,
            document_path: None,
        }
    }

    #[test]
    fn raster_keeps_full_horizon_and_merges_overlapping_clip_kinds() {
        let mut cells = vec![0; OVERVIEW_COLUMNS * OVERVIEW_ROWS];
        mark_interval(&mut cells, 2, 128, 10, 50, AUDIO_CELL);
        mark_interval(&mut cells, 2, 128, 35, 70, MIDI_CELL);
        assert!(cells.contains(&AUDIO_CELL));
        assert!(cells.contains(&MIDI_CELL));
        assert!(cells.contains(&MIXED_CELL));
        assert_eq!(cells[2 * OVERVIEW_COLUMNS], 0);
        assert!(cells.iter().filter(|cell| **cell != 0).count() <= OVERVIEW_COLUMNS);
    }

    #[test]
    fn overview_maps_tempo_converted_midi_and_bottom_track_clips() {
        let project = overview_project();
        let cells = overview_cells(&project, 240_000);
        let bottom = OVERVIEW_ROWS - 1;
        let row = &cells[bottom * OVERVIEW_COLUMNS..(bottom + 1) * OVERVIEW_COLUMNS];
        assert!(row.contains(&AUDIO_CELL));
        assert!(row.contains(&MIDI_CELL));
        assert!(row.contains(&MIXED_CELL));
        assert!(
            cells[..bottom * OVERVIEW_COLUMNS]
                .iter()
                .all(|cell| *cell == 0)
        );
    }

    #[test]
    fn overview_cache_ignores_viewport_pan_but_tracks_revision_and_project_generation() {
        let project = overview_project();
        let first = view_state(project.clone(), 7, 192_000);
        let cache = std::cell::RefCell::new(Some(Rc::new(OverviewCache::for_state(
            3,
            &first,
            first.timeline_frames,
            Rc::from([]),
            None,
        ))));
        assert!(cache.borrow().as_ref().is_some_and(|cache| cache.matches(
            3,
            &first,
            first.timeline_frames,
            &[],
            None
        )));
        // Viewport state is deliberately not part of the cache key.
        let unchanged = view_state(project.clone(), 7, 192_000);
        let first_read =
            super::cached(&cache, 3, &first, first.timeline_frames, Rc::from([]), None);
        let after_pan = super::cached(
            &cache,
            3,
            &unchanged,
            unchanged.timeline_frames,
            Rc::from([]),
            None,
        );
        assert!(Rc::ptr_eq(&first_read, &after_pan));
        let retired = super::cached(&cache, 3, &unchanged, 96_000, Rc::from([]), None);
        assert_eq!(retired.horizon(), 96_000);
        assert!(
            !Rc::ptr_eq(&after_pan, &retired),
            "retiring view extent invalidates the raster even without a project edit"
        );
        let changed_revision = view_state(project.clone(), 8, 192_000);
        let next_revision = super::cached(
            &cache,
            3,
            &changed_revision,
            changed_revision.timeline_frames,
            Rc::from([]),
            None,
        );
        assert!(!Rc::ptr_eq(&first_read, &next_revision));
        let changed_generation = view_state(project, 7, 192_000);
        let next_generation = super::cached(
            &cache,
            4,
            &changed_generation,
            changed_generation.timeline_frames,
            Rc::from([]),
            None,
        );
        assert!(!Rc::ptr_eq(&next_revision, &next_generation));
    }
}
