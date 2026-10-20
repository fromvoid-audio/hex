//! Compact, revision-cached Arrangement overview raster and viewport navigation.
use super::{EditorView, viewport::Viewport};
use crate::theme;
use gpui::{
    Bounds, Div, MouseButton, MouseDownEvent, Window, canvas, div, fill, point, prelude::*, px,
    rgb, size,
};
#[cfg(test)]
use hex_project::Project;
use hex_session::snapshot_exports::{SnapshotProjectRead, SnapshotRead};
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
}

impl OverviewCache {
    pub(super) fn for_read(generation: u64, snapshot: SnapshotRead<'_>) -> Self {
        Self {
            generation,
            revision: snapshot.revision(),
            horizon: snapshot.timeline_frames().max(1),
            track_count: snapshot.model().tracks().len(),
            cells: overview_cells_read(snapshot.model(), snapshot.timeline_frames().max(1)),
        }
    }
    fn matches_facts(&self, generation: u64, revision: u64, horizon: u64, tracks: usize) -> bool {
        self.generation == generation
            && self.revision == revision
            && self.horizon == horizon.max(1)
            && self.track_count == tracks
    }
    #[cfg(test)]
    fn for_state(generation: u64, state: &hex_session::service::ViewState) -> Self {
        Self {
            generation,
            revision: state.revision,
            horizon: state.timeline_frames.max(1),
            track_count: state.project.tracks().len(),
            cells: overview_cells(&state.project, state.timeline_frames.max(1)),
        }
    }
    #[cfg(test)]
    fn matches(&self, generation: u64, state: &hex_session::service::ViewState) -> bool {
        self.matches_facts(
            generation,
            state.revision,
            state.timeline_frames,
            state.project.tracks().len(),
        )
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

fn raster(
    track_count: usize,
    horizon: u64,
    intervals: impl Iterator<Item = (usize, u64, u64, u8)>,
) -> Vec<u8> {
    let mut cells = vec![0; OVERVIEW_CELLS];
    if track_count == 0 || horizon == 0 {
        return cells;
    }
    for (track_index, start, end, kind) in intervals {
        let row = track_index
            .saturating_mul(OVERVIEW_ROWS)
            .checked_div(track_count)
            .unwrap_or(0)
            .min(OVERVIEW_ROWS - 1);
        mark_interval(&mut cells, row, horizon, start, end, kind);
    }
    cells
}
fn overview_cells_read(project: SnapshotProjectRead<'_>, horizon: u64) -> Vec<u8> {
    raster(
        project.tracks().len(),
        horizon,
        project.tracks().enumerate().flat_map(|(index, track)| {
            track
                .clips()
                .filter_map(move |clip| {
                    clip.start()
                        .checked_add(clip.duration())
                        .map(|end| (index, clip.start(), end, AUDIO_CELL))
                })
                .chain(track.midi_clips().filter_map(move |clip| {
                    let end = clip
                        .start()
                        .raw()
                        .checked_add(clip.arrangement_length().raw())?;
                    let tempo = project.tempo();
                    Some((
                        index,
                        tempo.beat_to_frame(clip.start()).ok()?,
                        tempo
                            .beat_to_frame(hex_project::BeatTime::from_raw(end))
                            .ok()?,
                        MIDI_CELL,
                    ))
                }))
        }),
    )
}
#[cfg(test)]
fn overview_cells(project: &Project, horizon: u64) -> Vec<u8> {
    raster(
        project.tracks().len(),
        horizon,
        project
            .tracks()
            .iter()
            .enumerate()
            .flat_map(|(index, track)| {
                track
                    .clips()
                    .iter()
                    .filter_map(move |clip| {
                        clip.start()
                            .checked_add(clip.duration())
                            .map(|end| (index, clip.start(), end, AUDIO_CELL))
                    })
                    .chain(track.midi_clips().iter().filter_map(move |clip| {
                        let end = clip.start().raw().checked_add(clip.length().raw())?;
                        let tempo = project.tempo_map();
                        Some((
                            index,
                            tempo.beat_to_frame(clip.start()).ok()?,
                            tempo
                                .beat_to_frame(hex_project::BeatTime::from_raw(end))
                                .ok()?,
                            MIDI_CELL,
                        ))
                    }))
            }),
    )
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
        let Ok((horizon, current)) = state.read_with(|snapshot| {
            (
                snapshot.timeline_frames(),
                self.visible_viewport(snapshot, snapshot.timeline_frames()),
            )
        }) else {
            return;
        };
        if !fraction.is_finite() || horizon == 0 {
            return;
        }
        let length = current.length.max(1).min(horizon);
        let center = (f64::from(fraction.clamp(0.0, 1.0)) * horizon as f64)
            .round()
            .clamp(0.0, horizon as f64) as u64;
        let next = Viewport {
            start: center.saturating_sub(length / 2),
            length,
        }
        .clamp(horizon);
        self.viewport = (next != Viewport::fit(horizon)).then_some(next);
    }
}

pub(super) fn cached(
    cache: &RefCell<Option<Rc<OverviewCache>>>,
    generation: u64,
    snapshot: SnapshotRead<'_>,
) -> Rc<OverviewCache> {
    cached_facts(
        cache,
        generation,
        snapshot.revision(),
        snapshot.timeline_frames(),
        snapshot.model().tracks().len(),
        || OverviewCache::for_read(generation, snapshot),
    )
}
fn cached_facts(
    cache: &RefCell<Option<Rc<OverviewCache>>>,
    generation: u64,
    revision: u64,
    horizon: u64,
    tracks: usize,
    build: impl FnOnce() -> OverviewCache,
) -> Rc<OverviewCache> {
    let mut cache = cache.borrow_mut();
    if let Some(current) = cache
        .as_ref()
        .filter(|current| current.matches_facts(generation, revision, horizon, tracks))
    {
        return Rc::clone(current);
    }
    let next = Rc::new(build());
    *cache = Some(Rc::clone(&next));
    next
}
#[cfg(test)]
fn cached_state(
    cache: &RefCell<Option<Rc<OverviewCache>>>,
    generation: u64,
    state: &hex_session::service::ViewState,
) -> Rc<OverviewCache> {
    cached_facts(
        cache,
        generation,
        state.revision,
        state.timeline_frames,
        state.project.tracks().len(),
        || OverviewCache::for_state(generation, state),
    )
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

    fn view_state(project: Project, revision: u64, timeline_frames: u64) -> ViewState {
        ViewState {
            canonical_audio_source: None,
            comparison_audio: None,
            project: Arc::new(project),
            waveforms: Default::default(),
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
        let cache = std::cell::RefCell::new(Some(Rc::new(OverviewCache::for_state(3, &first))));
        assert!(
            cache
                .borrow()
                .as_ref()
                .is_some_and(|cache| cache.matches(3, &first))
        );
        // Viewport state is deliberately not part of the cache key.
        let unchanged = view_state(project.clone(), 7, 192_000);
        let first_read = super::cached_state(&cache, 3, &first);
        let after_pan = super::cached_state(&cache, 3, &unchanged);
        assert!(Rc::ptr_eq(&first_read, &after_pan));
        let changed_revision = view_state(project.clone(), 8, 192_000);
        let next_revision = super::cached_state(&cache, 3, &changed_revision);
        assert!(!Rc::ptr_eq(&first_read, &next_revision));
        let changed_generation = view_state(project, 7, 192_000);
        let next_generation = super::cached_state(&cache, 4, &changed_generation);
        assert!(!Rc::ptr_eq(&next_revision, &next_generation));
    }
}
