//! Visible Arrangement note intervals for one MIDI clip, derived from the
//! canonical playback occurrence plan (`MidiSourcePlayback::note_occurrences`),
//! the same plan MIDI playback compiles. Display-only: nothing here edits or
//! schedules notes, and no sampling is applied, so every visible loop repeat
//! draws its notes where playback sounds them. The resting clip, its
//! time-move fragments, drag/drop ghosts and the edge-trim preview all draw
//! through [`visible_occurrences`]; a trim supplies its previewed playback.
//!
//! Work is bounded by the visible window: each note locates its first visible
//! occurrence by binary search over the constant-time plan, then walks only
//! the visible repeats. `MAX_VISIBLE_OCCURRENCES` matches the project-wide
//! occurrence admission limit, so an admitted clip is never truncated. Notes
//! of one pitch that touch within one display pixel are merged, which bounds
//! painted quads by visible pixels and pitches rather than by repeats.
//! `VisibleNotesCache` reuses a clip's result while its project snapshot and
//! visible window are unchanged, so resting repaints skip the walk.
//!
//! Resting looped clips tile instead of walking repeats: when every repeat
//! has the same on-screen width (a looping, forward, unwindowed clip with no
//! tempo point strictly inside it, and a period of 2 to `MAX_TILE_PIXELS`
//! device pixels), [`LoopTiles`] holds one tile of consecutive periods (at
//! least `MIN_TILE_SPAN_PIXELS` wide) and the start of each visible tile.
//! [`paint_tiles`] rasterizes that tile once into a sprite and paints it per
//! visible tile inside the body's content mask, which
//! reproduces playback's cuts at the clip start (offset) and end (partial
//! last repeat). The first pass before the loop repeats stays per-occurrence.
//! Other clips (tempo change inside, reversed, content-windowed, sub-2px or
//! very wide periods) draw every visible occurrence instead.
use super::midi_thumbnail::{PitchFit, note_rows};
use hex_project::{BeatTime, ClipId, Invalid, MidiClip, MidiNoteOccurrencePlan, Project, TempoMap};
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

/// Per-clip visible-occurrence walk bound (the admission limit).
pub(super) const MAX_VISIBLE_OCCURRENCES: usize = 65_536;
/// Cached windows per clip: the resting body plus the few gesture fragments
/// (time-move residuals, a moved copy, a drop ghost) one frame draws.
const WINDOWS_PER_CLIP: usize = 4;
/// Widest tiled period, in device pixels; wider periods draw per occurrence.
const MAX_TILE_PIXELS: f64 = 2048.0;
/// Narrowest tiled period, in device pixels: below it same-pitch merging
/// already bounds the walk and a sprite per repeat would cost more.
const MIN_TILE_PIXELS: f64 = 2.0;
/// Tile image bytes held at once; past it repeats paint as quads.
const TILE_BYTE_BUDGET: usize = 64 << 20;
/// Narrow periods pack several repeats into one tile at least this wide, in
/// device pixels, so sprites scale with visible pixels rather than repeats.
const MIN_TILE_SPAN_PIXELS: f64 = 256.0;

pub(super) struct VisibleNotes {
    /// `(frame_start, frame_gate_end, pitch)`, sorted by pitch then onset.
    /// With `tiles`, only the occurrences outside the repeated loop.
    pub(super) intervals: Vec<(u64, u64, u8)>,
    /// Row fit over every pitch that sounds anywhere in the clip, so the
    /// vertical zoom does not change while scrolling.
    pub(super) fit: PitchFit,
    /// Repeats drawn as one tiled period, when every repeat shares a shape.
    pub(super) tiles: Option<LoopTiles>,
}

/// One tile of `repeats` consecutive loop periods and the visible tiles.
pub(super) struct LoopTiles {
    /// `(start, gate_end, pitch)` in frames from a tile's start.
    pub(super) notes: Vec<(f64, f64, u8)>,
    /// One repeat's length in frames.
    pub(super) period: f64,
    /// Repeats per tile.
    pub(super) repeats: u32,
    /// Start frame of each tile intersecting the visible clip window. A tile
    /// may start before the clip (offset) or end past it (last repeat).
    pub(super) blocks: Vec<f64>,
}

/// Frames covered by one device pixel of a `pixel_width`-wide body showing
/// `span` frames: the gap below which same-pitch notes merge into one quad.
pub(super) fn merge_frames(span: u64, pixel_width: f32, scale_factor: f32) -> u64 {
    let physical = f64::from(pixel_width * scale_factor.max(1.0));
    if physical >= 1.0 {
        (span as f64 / physical) as u64
    } else {
        span
    }
}

/// Row fit over the pitches that sound under the clip's canonical playback,
/// shared by the resting body and its gesture previews.
pub(super) fn sounding_fit(clip: &MidiClip) -> PitchFit {
    let Ok(source) = clip.source_playback() else {
        return PitchFit::from_pitches([]);
    };
    PitchFit::from_pitches(clip.notes().iter().filter_map(|note| {
        source
            .note_occurrences(note.start(), note.duration())
            .is_ok_and(|plan| plan.count() > 0)
            .then_some(note.pitch())
    }))
}

/// Exact note intervals intersecting `[visible_start, visible_end)` frames.
/// `merge_frames` (one display pixel in frames; `0` disables merging) joins
/// same-pitch intervals separated by less than that gap.
pub(super) fn visible_notes(
    project: &Project,
    clip: &MidiClip,
    visible_start: u64,
    visible_end: u64,
    merge_frames: u64,
) -> Result<VisibleNotes, Invalid> {
    walk(
        project,
        clip,
        visible_start,
        visible_end,
        merge_frames,
        None,
    )
}

/// [`visible_notes`], except that repeats of a tileable loop are returned as
/// [`LoopTiles`] instead of one interval per occurrence.
pub(super) fn visible_notes_tiled(
    project: &Project,
    clip: &MidiClip,
    visible_start: u64,
    visible_end: u64,
    merge_frames: u64,
) -> Result<VisibleNotes, Invalid> {
    let tiles = loop_tiles(
        project.tempo_map(),
        clip,
        visible_start,
        visible_end,
        merge_frames,
    )?;
    walk(
        project,
        clip,
        visible_start,
        visible_end,
        merge_frames,
        tiles,
    )
}

fn walk(
    project: &Project,
    clip: &MidiClip,
    visible_start: u64,
    visible_end: u64,
    merge_frames: u64,
    tiles: Option<LoopTiles>,
) -> Result<VisibleNotes, Invalid> {
    let source = clip.source_playback()?;
    let mut plans = Vec::with_capacity(clip.notes().len());
    for note in clip.notes() {
        plans.push((
            source.note_occurrences(note.start(), note.duration())?,
            note.pitch(),
        ));
    }
    let fit = PitchFit::from_pitches(
        plans
            .iter()
            .filter(|(plan, _)| plan.count() > 0)
            .map(|&(_, pitch)| pitch),
    );
    Ok(VisibleNotes {
        intervals: visible_occurrences(
            project.tempo_map(),
            clip.start(),
            plans,
            visible_start,
            visible_end,
            merge_frames,
            // Tiled repeats are drawn by the tile; only the first pass remains.
            tiles.is_some(),
        )?,
        fit,
        tiles,
    })
}

/// Visible `(frame_start, frame_gate_end, pitch)` intervals of `plans`, whose
/// occurrences are relative to `timeline_start`, sorted by pitch then onset.
/// Refuses rather than truncates past `MAX_VISIBLE_OCCURRENCES`. With
/// `first_pass_only`, only each plan's first-pass (iteration 0) occurrence.
pub(super) fn visible_occurrences(
    tempo: &TempoMap,
    timeline_start: BeatTime,
    plans: impl IntoIterator<Item = (MidiNoteOccurrencePlan, u8)>,
    visible_start: u64,
    visible_end: u64,
    merge_frames: u64,
    first_pass_only: bool,
) -> Result<Vec<(u64, u64, u8)>, Invalid> {
    let clip_start = timeline_start.raw();
    // Clip-local beat window, widened by one raw beat for frame rounding; the
    // exact frame cull below decides membership.
    let local = |frame: u64| -> Result<u64, Invalid> {
        Ok(tempo.frame_to_beat(frame)?.raw().saturating_sub(clip_start))
    };
    let window_start = local(visible_start)?.saturating_sub(1);
    let window_end = local(visible_end)?.saturating_add(1);
    let frame = |local_beat: u64| -> Result<u64, Invalid> {
        let beat = clip_start
            .checked_add(local_beat)
            .ok_or(Invalid("midi note beat overflow"))?;
        tempo.beat_to_frame(BeatTime::from_raw(beat))
    };
    let mut intervals = Vec::new();
    for (plan, pitch) in plans {
        let count = if first_pass_only {
            u64::from(plan.iter().next().is_some_and(|hit| hit.iteration() == 0))
        } else {
            plan.count()
        };
        if count == 0 {
            continue;
        }
        // Occurrences of one note never overlap and ascend in timeline order
        // (also when reversed), so the first one ending after the window
        // start is a partition point. `nth` is constant-time on the plan.
        let end_of = |ordinal: u64| -> Option<u64> {
            let hit = plan.iter().nth(usize::try_from(ordinal).ok()?)?;
            hit.timeline_start()
                .raw()
                .checked_add(hit.gate_duration().raw())
        };
        let (mut low, mut high) = (0_u64, count);
        while low < high {
            let middle = low + (high - low) / 2;
            if end_of(middle).is_some_and(|end| end <= window_start) {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let skip = usize::try_from(low).map_err(|_| Invalid("midi occurrence index overflow"))?;
        let take = usize::try_from(count - low).unwrap_or(usize::MAX);
        for hit in plan.iter().skip(skip).take(take) {
            let start = hit.timeline_start().raw();
            if start >= window_end {
                break;
            }
            if intervals.len() >= MAX_VISIBLE_OCCURRENCES {
                return Err(Invalid(
                    "visible midi occurrences exceed the admission limit",
                ));
            }
            let end = start
                .checked_add(hit.gate_duration().raw())
                .ok_or(Invalid("midi note gate overflow"))?;
            let (start, end) = (frame(start)?, frame(end)?);
            if start < visible_end && end > visible_start && end > start {
                intervals.push((start, end, pitch));
            }
        }
    }
    intervals.sort_unstable_by_key(|&(start, end, pitch)| (pitch, start, end));
    if merge_frames > 0 {
        let mut merged: Vec<(u64, u64, u8)> = Vec::with_capacity(intervals.len());
        for (start, end, pitch) in intervals {
            match merged.last_mut() {
                Some(last) if last.2 == pitch && start < last.1.saturating_add(merge_frames) => {
                    last.1 = last.1.max(end);
                }
                _ => merged.push((start, end, pitch)),
            }
        }
        intervals = merged;
    }
    Ok(intervals)
}

/// Repeats of `clip` as one tiled period, or `None` when repeats may differ
/// on screen or a tile would not pay off (see the module docs).
fn loop_tiles(
    tempo: &TempoMap,
    clip: &MidiClip,
    visible_start: u64,
    visible_end: u64,
    merge_frames: u64,
) -> Result<Option<LoopTiles>, Invalid> {
    let source = clip.source_playback()?;
    if merge_frames == 0
        || !clip.looping()
        || clip.content_view().is_some()
        || source.playback_reversed()
    {
        return Ok(None);
    }
    let (loop_start, loop_end) = (clip.loop_start().raw(), clip.loop_end().raw());
    let (start, length) = (clip.start().raw(), clip.length().raw());
    let (Some(period), Some(first_pass), Some(end)) = (
        loop_end
            .checked_sub(loop_start)
            .filter(|period| *period > 0),
        loop_end
            .checked_sub(clip.source_start().raw())
            .filter(|pass| *pass > 0),
        start.checked_add(length).filter(|_| length > 0),
    ) else {
        return Ok(None);
    };
    // One tempo across the clip makes frames linear in beats, so every
    // repeat has the same width; a step inside it would stretch repeats.
    if tempo
        .points()
        .iter()
        .any(|point| (start + 1..end).contains(&point.beat().raw()))
    {
        return Ok(None);
    }
    let start_frame = tempo.beat_to_frame(clip.start())?;
    let end_frame = tempo.beat_to_frame(BeatTime::from_raw(end))?;
    let frames_per_beat = end_frame.saturating_sub(start_frame) as f64 / length as f64;
    let period_frames = period as f64 * frames_per_beat;
    let period_pixels = period_frames / merge_frames as f64;
    if !(MIN_TILE_PIXELS..=MAX_TILE_PIXELS).contains(&period_pixels) {
        return Ok(None);
    }
    let repeats = (MIN_TILE_SPAN_PIXELS / period_pixels).ceil().max(1.0);
    // Repeat `i` starts at clip-local beat `first_pass + i * period - offset`
    // (`MidiSourcePlayback::note_occurrences`' first repeat and spacing).
    let origin = first_pass as f64 - clip.playback_offset().raw() as f64;
    let from = start_frame.max(visible_start);
    let to = end_frame.min(visible_end);
    let mut blocks = Vec::new();
    if from < to {
        let beat_at = |frame: u64| (frame - start_frame) as f64 / frames_per_beat;
        let first = ((beat_at(from) - origin) / period as f64).floor().max(0.0);
        let last = ((beat_at(to) - origin) / period as f64).ceil();
        // Bounded by the visible width over `MIN_TILE_PIXELS` per repeat.
        let mut index = first;
        while index < last {
            let local = origin + index * period as f64;
            if local >= length as f64 {
                break;
            }
            blocks.push(start_frame as f64 + local * frames_per_beat);
            index += repeats;
        }
    }
    let one: Vec<_> = clip
        .notes()
        .iter()
        .filter(|note| (loop_start..loop_end).contains(&note.start().raw()))
        .map(|note| {
            let delta = (note.start().raw() - loop_start) as f64;
            let gate = note.duration().raw().min(loop_end - note.start().raw()) as f64;
            (
                delta * frames_per_beat,
                (delta + gate) * frames_per_beat,
                note.pitch(),
            )
        })
        .collect();
    // At most `MIN_TILE_SPAN_PIXELS / MIN_TILE_PIXELS` repeats per tile.
    let repeats = repeats as u32;
    let notes = (0..repeats)
        .flat_map(|repeat| {
            let shift = f64::from(repeat) * period_frames;
            one.iter()
                .map(move |&(start, end, pitch)| (start + shift, end + shift, pitch))
        })
        .collect();
    Ok(Some(LoopTiles {
        notes,
        period: period_frames,
        repeats,
        blocks,
    }))
}

/// Visible frames and merge gap one cached result was computed for.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Window {
    start: u64,
    end: u64,
    merge_frames: u64,
    tiled: bool,
}

/// Recent visible notes per clip for one immutable project snapshot.
///
/// Holding the snapshot `Arc` keeps cached clips immutable and rules out a
/// reused address matching a different project. Each clip keeps at most
/// `WINDOWS_PER_CLIP` windows, most recent first, so a gesture preview does
/// not evict the resting body; entries are bounded by the snapshot's clip
/// count and are all dropped when a different snapshot is drawn. A refused
/// walk is cached as `None` so an over-limit clip is not re-walked every frame.
#[derive(Default)]
pub(super) struct VisibleNotesCache {
    project: Option<Arc<Project>>,
    entries: HashMap<ClipId, Vec<CachedWindow>>,
}

/// One cached window and its walk; `None` when the walk was refused.
type CachedWindow = (Window, Option<Rc<VisibleNotes>>);

impl VisibleNotesCache {
    /// `visible_notes` for `clip`, reused when the snapshot and window match.
    pub(super) fn get(
        &mut self,
        project: &Arc<Project>,
        clip: &MidiClip,
        visible_start: u64,
        visible_end: u64,
        merge_frames: u64,
    ) -> Option<Rc<VisibleNotes>> {
        self.lookup(
            project,
            clip,
            visible_start,
            visible_end,
            merge_frames,
            false,
        )
    }

    /// `visible_notes_tiled` for the resting body, reused when the snapshot
    /// and window match.
    pub(super) fn get_tiled(
        &mut self,
        project: &Arc<Project>,
        clip: &MidiClip,
        visible_start: u64,
        visible_end: u64,
        merge_frames: u64,
    ) -> Option<Rc<VisibleNotes>> {
        self.lookup(
            project,
            clip,
            visible_start,
            visible_end,
            merge_frames,
            true,
        )
    }

    fn lookup(
        &mut self,
        project: &Arc<Project>,
        clip: &MidiClip,
        visible_start: u64,
        visible_end: u64,
        merge_frames: u64,
        tiled: bool,
    ) -> Option<Rc<VisibleNotes>> {
        if !self
            .project
            .as_ref()
            .is_some_and(|cached| Arc::ptr_eq(cached, project))
        {
            self.entries.clear();
            self.project = Some(Arc::clone(project));
        }
        let window = Window {
            start: visible_start,
            end: visible_end,
            merge_frames,
            tiled,
        };
        let windows = self.entries.entry(clip.id()).or_default();
        if let Some(index) = windows.iter().position(|(cached, _)| *cached == window) {
            let hit = windows.remove(index);
            let notes = hit.1.clone();
            windows.insert(0, hit);
            return notes;
        }
        let walk = if tiled {
            visible_notes_tiled
        } else {
            visible_notes
        };
        let notes = walk(project, clip, visible_start, visible_end, merge_frames)
            .ok()
            .map(Rc::new);
        windows.truncate(WINDOWS_PER_CLIP - 1);
        windows.insert(0, (window, notes.clone()));
        notes
    }
}

impl super::EditorView {
    /// Shapes (fractions of `[start, end)`) and row fit of the canonical
    /// `clip`'s notes in those frames, through the per-clip cache. A
    /// re-entrant build computes without the cache; a refused walk draws none.
    pub(super) fn loop_note_shapes(
        &self,
        project: &Arc<Project>,
        clip: &MidiClip,
        start: u64,
        end: u64,
        merge_frames: u64,
    ) -> (Vec<(f32, f32, u8)>, PitchFit) {
        let visible = match self.midi_visible_notes.try_borrow_mut() {
            Ok(mut cache) => cache.get(project, clip, start, end, merge_frames),
            Err(_) => visible_notes(project, clip, start, end, merge_frames)
                .ok()
                .map(Rc::new),
        };
        let Some(visible) = visible else {
            return (Vec::new(), sounding_fit(clip));
        };
        let shapes = visible
            .intervals
            .iter()
            .filter_map(|&(note_start, note_end, pitch)| {
                super::midi_thumbnail::note_shape(note_start, note_end, pitch, start, end)
            })
            .collect();
        (shapes, visible.fit)
    }
}

/// One rasterized tile and the result it was drawn from.
struct TileImage {
    notes: Rc<VisibleNotes>,
    size: (u32, u32),
    scale: u32,
    image: Arc<gpui::RenderImage>,
}

/// Rasterized loop tiles per clip for one project snapshot.
///
/// Sprite atlas custody: a tile is replaced only when its clip's resting
/// body is rebuilt (new notes, window, height or scale), and every tile is
/// retired when a different snapshot paints; both rebuild the cached track
/// rows that replayed the old sprite. Retired images are released from the
/// atlas at the next tile paint, after their last frame. At most one tile
/// per clip and `TILE_BYTE_BUDGET` bytes are held; past the budget repeats
/// paint as quads.
#[derive(Default)]
pub(super) struct TileImages {
    project: Option<Arc<Project>>,
    entries: HashMap<ClipId, TileImage>,
    bytes: usize,
    retired: Vec<Arc<gpui::RenderImage>>,
    /// Sprites painted, for rendered tests.
    #[cfg(test)]
    pub(super) sprites: usize,
}

impl TileImages {
    fn retire(&mut self, tile: TileImage) {
        self.bytes = self
            .bytes
            .saturating_sub(tile.size.0 as usize * tile.size.1 as usize * 4);
        self.retired.push(tile.image);
    }
}

/// Device-pixel note rectangles `(left, top, right, bottom)` of one tile,
/// relative to the repeat's start, with the resting body's row fit, minimum
/// width (half a percent of the visible span) and pixel snapping.
fn tile_rects(
    tiles: &LoopTiles,
    fit: PitchFit,
    span: f64,
    body: (f64, f32),
    scale: f32,
) -> Vec<(i64, i64, i64, i64)> {
    let (width, height) = body;
    let per_frame = width / span;
    tiles
        .notes
        .iter()
        .filter_map(|&(start, end, pitch)| {
            let (top, rows) = note_rows(fit, pitch, height)?;
            let left = start * per_frame;
            let right = left + ((end - start) / span).max(0.005) * width;
            let (left, right) = (left.round() as i64, right.round() as i64);
            let y = (f64::from(top) * f64::from(scale)).round() as i64;
            let bottom = ((f64::from(top + rows) * f64::from(scale)).round() as i64).max(y + 1);
            (right > left).then_some((left, y, right, bottom))
        })
        .collect()
}

/// Paint `notes.tiles` into `bounds`, the visible clip body showing frames
/// `[visible_start, visible_end)`: one sprite per visible tile, or the same
/// rectangles as quads when the tile budget is spent.
pub(super) fn paint_tiles(
    window: &mut gpui::Window,
    images: &RefCell<TileImages>,
    project: &Arc<Project>,
    clip: ClipId,
    notes: &Rc<VisibleNotes>,
    bounds: gpui::Bounds<gpui::Pixels>,
    visible: (u64, u64),
) {
    let Some(tiles) = notes.tiles.as_ref() else {
        return;
    };
    let Ok(mut images) = images.try_borrow_mut() else {
        return;
    };
    for image in images.retired.drain(..) {
        // Already out of every scene; a missing atlas entry is harmless.
        let _ = window.drop_image(image);
    }
    if !images
        .project
        .as_ref()
        .is_some_and(|cached| Arc::ptr_eq(cached, project))
    {
        for (_, tile) in std::mem::take(&mut images.entries) {
            images.retire(tile);
        }
        images.project = Some(Arc::clone(project));
    }
    let scale = window.scale_factor();
    let span = visible.1.saturating_sub(visible.0) as f64;
    let body_width = f64::from(f32::from(bounds.size.width) * scale);
    let body_height = f32::from(bounds.size.height);
    if span <= 0.0 || body_width <= 0.0 || body_height <= 0.0 || scale <= 0.0 {
        return;
    }
    let per_frame = body_width / span;
    let width = (tiles.period * f64::from(tiles.repeats) * per_frame).ceil() as u32 + 1;
    let height = (body_height * scale).ceil() as u32;
    let rects = tile_rects(tiles, notes.fit, span, (body_width, body_height), scale);
    let reusable = images.entries.get(&clip).is_some_and(|tile| {
        Rc::ptr_eq(&tile.notes, notes)
            && tile.size == (width, height)
            && tile.scale == scale.to_bits()
    });
    if !reusable {
        if let Some(old) = images.entries.remove(&clip) {
            images.retire(old);
        }
        let bytes = width as usize * height as usize * 4;
        if width <= 2 * MAX_TILE_PIXELS as u32 && images.bytes + bytes <= TILE_BYTE_BUDGET {
            let mut buffer = image::RgbaImage::new(width, height);
            for &(left, top, right, bottom) in &rects {
                for y in top.max(0)..bottom.min(i64::from(height)) {
                    for x in left.max(0)..right.min(i64::from(width)) {
                        buffer.put_pixel(x as u32, y as u32, image::Rgba([0, 0, 0, 255]));
                    }
                }
            }
            let image = Arc::new(gpui::RenderImage::new([image::Frame::new(buffer)]));
            images.bytes += bytes;
            images.entries.insert(
                clip,
                TileImage {
                    notes: Rc::clone(notes),
                    size: (width, height),
                    scale: scale.to_bits(),
                    image,
                },
            );
        }
    }
    let image = images
        .entries
        .get(&clip)
        .map(|tile| Arc::clone(&tile.image));
    let left = f64::from(f32::from(bounds.left()) * scale);
    let top = f32::from(bounds.top());
    window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
        for &block in &tiles.blocks {
            let origin = (left + (block - visible.0 as f64) * per_frame).floor();
            match &image {
                Some(image) => {
                    let tile = gpui::Bounds::new(
                        gpui::point(gpui::px(origin as f32 / scale), gpui::px(top)),
                        gpui::size(
                            gpui::px(width as f32 / scale),
                            gpui::px(height as f32 / scale),
                        ),
                    );
                    if window
                        .paint_image(tile, Default::default(), Arc::clone(image), 0, false)
                        .is_ok()
                    {
                        #[cfg(test)]
                        {
                            images.sprites += 1;
                        }
                    }
                }
                None => {
                    for &(x0, y0, x1, y1) in &rects {
                        let rect = gpui::Bounds::new(
                            gpui::point(
                                gpui::px((origin + x0 as f64) as f32 / scale),
                                gpui::px(top + y0 as f32 / scale),
                            ),
                            gpui::size(
                                gpui::px((x1 - x0) as f32 / scale),
                                gpui::px((y1 - y0) as f32 / scale),
                            ),
                        );
                        window.paint_quad(gpui::fill(rect, gpui::rgb(0x000000)));
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Tempo};

    fn beats(quarters: u64) -> BeatTime {
        BeatTime::from_quarters(quarters).unwrap()
    }

    /// Loop start differs from the clip start, the source has a start offset,
    /// a tempo change lands mid-clip and the last repeat is partial.
    fn looped_clip() -> (Editor, hex_project::ClipId) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 64).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "loop".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: beats(2),
                length: beats(4),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        for (start, pitch) in [(BeatTime::ZERO, 60), (beats(1), 64), (beats(2), 67)] {
            editor
                .execute(Command::AddMidiNote {
                    clip,
                    start,
                    duration: BeatTime::from_raw(BeatTime::ONE.raw() / 2),
                    pitch,
                    channel: 0,
                    velocity: 100,
                })
                .unwrap();
        }
        // Source starts one beat in, loops [1, 3) and runs 40 repeats plus a
        // half-period tail: 1 + 2 * 40 + 1 beats.
        editor
            .execute(Command::SetMidiClipSource {
                clip,
                source_length: beats(4),
                source_start: beats(1),
                loop_start: beats(1),
                loop_end: beats(3),
                looping: true,
                length: beats(82),
            })
            .unwrap();
        editor
            .execute(Command::SetTempoPoint {
                beat: beats(30),
                tempo: Tempo::new(90_000_000).unwrap(),
            })
            .unwrap();
        (editor, clip)
    }

    /// Exactly what playback compiles: every occurrence of every note.
    fn playback(project: &Project, clip: &MidiClip) -> Vec<(u64, u64, u8)> {
        let source = clip.source_playback().unwrap();
        let tempo = project.tempo_map();
        let frame = |beat: u64| {
            tempo
                .beat_to_frame(BeatTime::from_raw(clip.start().raw() + beat))
                .unwrap()
        };
        let mut all: Vec<_> = clip
            .notes()
            .iter()
            .flat_map(|note| {
                let plan = source
                    .note_occurrences(note.start(), note.duration())
                    .unwrap();
                plan.iter()
                    .map(|hit| {
                        let start = hit.timeline_start().raw();
                        (
                            frame(start),
                            frame(start + hit.gate_duration().raw()),
                            note.pitch(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        all.sort_unstable_by_key(|&(start, end, pitch)| (pitch, start, end));
        all
    }

    #[test]
    fn every_visible_repeat_draws_its_notes_where_playback_sounds_them() {
        let (editor, id) = looped_clip();
        let project = editor.project();
        let clip = project.midi_clip(id).unwrap();
        let expected = playback(project, clip);
        // Two notes inside the loop over 40+ repeats: far more than the old
        // 32-sample engine preview.
        assert!(expected.len() > 64, "{}", expected.len());
        let tempo = project.tempo_map();
        let start = tempo.beat_to_frame(clip.start()).unwrap();
        let end = tempo
            .beat_to_frame(BeatTime::from_raw(clip.start().raw() + beats(82).raw()))
            .unwrap();
        let whole = visible_notes(project, clip, start, end, 0).unwrap();
        assert_eq!(whole.intervals, expected);
        // A window across the tempo change and into the partial last repeat
        // keeps exactly the intersecting playback occurrences.
        let (from, to) = (
            tempo.beat_to_frame(beats(25)).unwrap(),
            tempo.beat_to_frame(beats(84)).unwrap(),
        );
        let window = visible_notes(project, clip, from, to.min(end), 0).unwrap();
        let culled: Vec<_> = expected
            .iter()
            .copied()
            .filter(|&(s, e, _)| s < to.min(end) && e > from)
            .collect();
        assert_eq!(window.intervals, culled);
        // The note before the source start never sounds and does not widen the fit.
        assert_eq!(whole.fit, PitchFit::from_pitches([64, 67]));
    }

    #[test]
    fn same_pitch_notes_within_one_pixel_merge_into_one_quad() {
        let (editor, id) = looped_clip();
        let project = editor.project();
        let clip = project.midi_clip(id).unwrap();
        let tempo = project.tempo_map();
        let start = tempo.beat_to_frame(clip.start()).unwrap();
        let end = tempo
            .beat_to_frame(BeatTime::from_raw(clip.start().raw() + beats(82).raw()))
            .unwrap();
        let exact = visible_notes(project, clip, start, end, 0).unwrap();
        let coarse = visible_notes(project, clip, start, end, end - start).unwrap();
        assert!(coarse.intervals.len() < exact.intervals.len());
        assert_eq!(coarse.intervals.len(), 2, "one span per sounding pitch");
    }

    #[test]
    fn cache_reuses_only_the_same_snapshot_and_window() {
        let (mut editor, id) = looped_clip();
        let project = Arc::new(editor.project().clone());
        let clip = project.midi_clip(id).unwrap();
        let mut cache = VisibleNotesCache::default();
        let first = cache.get_tiled(&project, clip, 0, 400_000, 100).unwrap();
        let again = cache.get_tiled(&project, clip, 0, 400_000, 100).unwrap();
        assert!(
            Rc::ptr_eq(&first, &again),
            "resting repaint reuses the walk"
        );
        let scrolled = cache
            .get_tiled(&project, clip, 50_000, 450_000, 100)
            .unwrap();
        assert!(!Rc::ptr_eq(&first, &scrolled));
        assert_eq!(
            scrolled.intervals,
            visible_notes(&project, clip, 50_000, 450_000, 100)
                .unwrap()
                .intervals
        );
        // An edit publishes a new snapshot; the old result must not be reused.
        editor
            .execute(Command::SetTempoPoint {
                beat: beats(4),
                tempo: Tempo::new(60_000_000).unwrap(),
            })
            .unwrap();
        let edited = Arc::new(editor.project().clone());
        let clip = edited.midi_clip(id).unwrap();
        let fresh = cache
            .get_tiled(&edited, clip, 50_000, 450_000, 100)
            .unwrap();
        assert_eq!(
            fresh.intervals,
            visible_notes(&edited, clip, 50_000, 450_000, 100)
                .unwrap()
                .intervals
        );
        assert_ne!(fresh.intervals, scrolled.intervals);
    }

    /// Publish `project` (or keep the current one) with a timeline and
    /// viewport showing `length` frames from `start`; the fixture timeline is
    /// shorter than the looped clip.
    #[cfg(feature = "ui-test-support")]
    fn publish(
        visual: &mut gpui::VisualTestContext,
        view: &gpui::Entity<crate::editor::EditorView>,
        project: Option<Project>,
        start: u64,
        length: u64,
    ) {
        view.update(visual, |view, cx| {
            let old = view.state.take().unwrap();
            let (project, midi_preview) = match project {
                Some(project) => {
                    let (_, preview) = hex_engine::compile::project_with_midi_preview(
                        &project,
                        &Default::default(),
                    )
                    .unwrap();
                    (Arc::new(project), Arc::new(preview))
                }
                None => (Arc::clone(&old.project), Arc::clone(&old.midi_preview)),
            };
            view.state = Some(Arc::new(hex_session::service::ViewState {
                canonical_audio_source: None,
                comparison_audio: None,
                project,
                waveforms: Default::default(),
                source_overviews: Default::default(),
                audio_clip_waveforms: std::collections::BTreeMap::new(),
                midi_preview,
                gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
                timeline_frames: start + length,
                can_undo: false,
                can_redo: false,
                revision: old.revision + 1,
                unsaved: false,
                document_path: None,
            }));
            view.viewport = Some(crate::editor::viewport::Viewport { start, length });
            cx.notify();
        });
        visual.run_until_parked();
    }

    /// Rendered regression for the user report: the resting Arrangement clip
    /// paints every repeat's notes at playback positions and no count badge.
    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn rendered_long_loop_paints_every_repeat_without_a_sample_badge(
        cx: &mut gpui::TestAppContext,
    ) {
        let (mut editor, id) = looped_clip();
        let project = editor.project().clone();
        let clip = project.midi_clip(id).unwrap().clone();
        let expected = playback(&project, &clip);
        let tempo = project.tempo_map();
        let start = tempo.beat_to_frame(clip.start()).unwrap();
        let end = tempo
            .beat_to_frame(BeatTime::from_raw(clip.start().raw() + beats(82).raw()))
            .unwrap();
        let (_, preview) =
            hex_engine::compile::project_with_midi_preview(&project, &Default::default()).unwrap();
        let (handle, view) =
            crate::editor::clip_drag_ui_test::editor_window(cx, project, Arc::new(preview));
        let mut visual = gpui::VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(gpui::size(gpui::px(2400.0), gpui::px(800.0)));
        // The fixture timeline is shorter than this clip; publish a snapshot
        // whose timeline shows the whole loop.
        let install = |visual: &mut gpui::VisualTestContext, project: Option<Project>| {
            publish(visual, &view, project, start, end - start)
        };
        install(&mut visual, None);
        let body = visual
            .debug_bounds(Box::leak(
                format!("clip-notes-{}", id.value()).into_boxed_str(),
            ))
            .expect("note body");
        let notes = crate::editor::midi_thumbnail::rendered_notes(&mut visual, id, false);
        assert_eq!(
            notes.len(),
            expected.len(),
            "every repeat is drawn, not a 32-note sample"
        );
        let (left, width) = (f32::from(body.left()), f32::from(body.size.width));
        for (onset, _, _) in &expected {
            let x = left + (onset - start) as f32 / (end - start) as f32 * width;
            assert!(
                notes
                    .iter()
                    .any(|(bounds, _)| (f32::from(bounds.left()) - x).abs() <= 1.5),
                "a note at playback frame {onset} (x {x})"
            );
        }
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("midi-preview-count-{}", id.value()).into_boxed_str(),
                ))
                .is_none(),
            "no sample-count badge"
        );
        // A resting repaint reuses the cached walk; an edit that leaves the
        // visible window unchanged publishes a new snapshot and must repaint
        // from it, not from the cached notes.
        install(&mut visual, None);
        assert_eq!(
            crate::editor::midi_thumbnail::rendered_notes(&mut visual, id, false).len(),
            expected.len()
        );
        editor
            .execute(Command::AddMidiNote {
                clip: id,
                start: BeatTime::from_raw(BeatTime::ONE.raw() * 3 / 2),
                duration: BeatTime::from_raw(BeatTime::ONE.raw() / 4),
                pitch: 72,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
        let edited = editor.project().clone();
        let added = playback(&edited, edited.midi_clip(id).unwrap());
        assert!(added.len() > expected.len() + 32, "{}", added.len());
        install(&mut visual, Some(edited));
        assert_eq!(
            crate::editor::midi_thumbnail::rendered_notes(&mut visual, id, false).len(),
            added.len(),
            "the edit's notes paint at every repeat"
        );
    }

    /// Constant tempo; the source starts before the loop (a first pass with
    /// an intro note), the clip is left-trimmed into its first pass (an
    /// offset), the loop starts after the clip start and the last repeat is
    /// partial.
    fn tiled_clip() -> (Editor, hex_project::ClipId) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 64).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "tiles".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: beats(2),
                length: beats(4),
            })
            .unwrap()
        else {
            panic!("clip")
        };
        for (start, pitch) in [(BeatTime::ZERO, 60), (beats(1), 64), (beats(2), 67)] {
            editor
                .execute(Command::AddMidiNote {
                    clip,
                    start,
                    duration: BeatTime::from_raw(BeatTime::ONE.raw() / 2),
                    pitch,
                    channel: 0,
                    velocity: 100,
                })
                .unwrap();
        }
        editor
            .execute(Command::SetMidiClipSource {
                clip,
                source_length: beats(4),
                source_start: BeatTime::ZERO,
                loop_start: beats(1),
                loop_end: beats(3),
                looping: true,
                length: beats(82),
            })
            .unwrap();
        editor
            .execute(Command::TrimMidiClipEdge {
                clip,
                edge: hex_project::TrimEdge::Left,
                beat: BeatTime::from_raw(beats(2).raw() + BeatTime::ONE.raw() / 4),
            })
            .unwrap();
        (editor, clip)
    }

    /// `(frame_start, frame_gate_end, pitch)`.
    type Interval = (u64, u64, u8);

    /// Playback occurrences split into the first pass and the repeats.
    fn playback_passes(project: &Project, clip: &MidiClip) -> (Vec<Interval>, Vec<Interval>) {
        let source = clip.source_playback().unwrap();
        let tempo = project.tempo_map();
        let frame = |beat: u64| {
            tempo
                .beat_to_frame(BeatTime::from_raw(clip.start().raw() + beat))
                .unwrap()
        };
        let (mut first, mut repeats) = (Vec::new(), Vec::new());
        for note in clip.notes() {
            let plan = source
                .note_occurrences(note.start(), note.duration())
                .unwrap();
            for hit in plan.iter() {
                let start = hit.timeline_start().raw();
                let interval = (
                    frame(start),
                    frame(start + hit.gate_duration().raw()),
                    note.pitch(),
                );
                if hit.iteration() == 0 {
                    first.push(interval);
                } else {
                    repeats.push(interval);
                }
            }
        }
        first.sort_unstable_by_key(|&(start, end, pitch)| (pitch, start, end));
        repeats.sort_unstable_by_key(|&(start, end, pitch)| (pitch, start, end));
        (first, repeats)
    }

    /// Each tile note at each block, cut to the clip and window as the body's
    /// content mask cuts it, sorted like `visible_notes`.
    fn tiled_occurrences(
        tiles: &LoopTiles,
        clip: (u64, u64),
        window: (u64, u64),
    ) -> Vec<(f64, f64, u8)> {
        let (from, to) = (clip.0.max(window.0) as f64, clip.1.min(window.1) as f64);
        let mut all: Vec<_> = tiles
            .blocks
            .iter()
            .flat_map(|block| {
                tiles.notes.iter().filter_map(move |&(start, end, pitch)| {
                    let (start, end) = (block + start, block + end);
                    // A repeat cut at the clip start keeps its cut start.
                    (start < to && end > from && start < clip.1 as f64).then_some((
                        start.max(clip.0 as f64),
                        end.min(clip.1 as f64),
                        pitch,
                    ))
                })
            })
            .collect();
        all.sort_unstable_by(|a, b| (a.2, a.0).partial_cmp(&(b.2, b.0)).unwrap());
        all
    }

    fn assert_tiles_match(
        tiled: &[(f64, f64, u8)],
        expected: &[(u64, u64, u8)],
        window: (u64, u64),
    ) {
        let expected: Vec<_> = expected
            .iter()
            .filter(|&&(start, end, _)| start < window.1 && end > window.0)
            .collect();
        assert_eq!(tiled.len(), expected.len());
        for (tile, &&(start, end, pitch)) in tiled.iter().zip(&expected) {
            assert_eq!(tile.2, pitch);
            assert!((tile.0 - start as f64).abs() <= 1.0, "{tile:?} vs {start}");
            assert!((tile.1 - end as f64).abs() <= 1.0, "{tile:?} vs {end}");
        }
    }

    #[test]
    fn tiled_repeats_land_exactly_where_playback_sounds_them() {
        let (editor, id) = tiled_clip();
        let project = editor.project();
        let clip = project.midi_clip(id).unwrap();
        assert!(clip.playback_offset() > BeatTime::ZERO, "fixture is offset");
        let tempo = project.tempo_map();
        let start = tempo.beat_to_frame(clip.start()).unwrap();
        let end = tempo
            .beat_to_frame(BeatTime::from_raw(clip.start().raw() + clip.length().raw()))
            .unwrap();
        let (first, repeats) = playback_passes(project, clip);
        assert!(repeats.len() > 64 && !first.is_empty());
        // 10 px per two-beat repeat (26 repeats per 256 px tile), then 4 px (64
        // per tile); the whole clip, a window cutting repeats at both ends,
        // and the partial last repeat.
        for (merge, per_tile) in [(4_800, 26), (12_000, 64)] {
            for window in [
                (start, end),
                (start + 600_000, start + 1_500_123),
                (end - 30_000, end),
            ] {
                let notes = visible_notes_tiled(project, clip, window.0, window.1, merge).unwrap();
                let tiles = notes.tiles.as_ref().expect("constant tempo tiles");
                assert_eq!(tiles.repeats, per_tile);
                assert!((tiles.period - 48_000.0).abs() < 1e-6);
                assert_tiles_match(
                    &tiled_occurrences(tiles, (start, end), window),
                    &repeats,
                    window,
                );
                let intro: Vec<_> = first
                    .iter()
                    .copied()
                    .filter(|&(s, e, _)| s < window.1 && e > window.0)
                    .collect();
                assert_eq!(
                    notes.intervals, intro,
                    "the first pass stays per occurrence"
                );
            }
        }
        // Unmerged walks stay exact per occurrence.
        assert!(
            visible_notes_tiled(project, clip, start, end, 0)
                .unwrap()
                .tiles
                .is_none()
        );
    }

    #[test]
    fn a_tempo_change_inside_the_clip_draws_every_occurrence() {
        let (editor, id) = looped_clip();
        let project = editor.project();
        let clip = project.midi_clip(id).unwrap();
        let tempo = project.tempo_map();
        let start = tempo.beat_to_frame(clip.start()).unwrap();
        let end = tempo
            .beat_to_frame(BeatTime::from_raw(clip.start().raw() + beats(82).raw()))
            .unwrap();
        let notes = visible_notes_tiled(project, clip, start, end, 4_800).unwrap();
        assert!(
            notes.tiles.is_none(),
            "repeats differ in width across the tempo step"
        );
        assert_eq!(
            notes.intervals,
            visible_notes(project, clip, start, end, 4_800)
                .unwrap()
                .intervals
        );
    }

    /// Rendered: a tileable long loop paints one sprite per visible tile
    /// and only its first pass as quads.
    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn rendered_tiled_loop_paints_one_sprite_per_visible_tile(cx: &mut gpui::TestAppContext) {
        let (editor, id) = tiled_clip();
        let project = editor.project().clone();
        let clip = project.midi_clip(id).unwrap().clone();
        let tempo = project.tempo_map();
        let start = tempo.beat_to_frame(clip.start()).unwrap();
        let end = tempo
            .beat_to_frame(BeatTime::from_raw(clip.start().raw() + clip.length().raw()))
            .unwrap();
        let (first, _) = playback_passes(&project, &clip);
        let (_, preview) =
            hex_engine::compile::project_with_midi_preview(&project, &Default::default()).unwrap();
        let (handle, view) =
            crate::editor::clip_drag_ui_test::editor_window(cx, project, Arc::new(preview));
        let mut visual = gpui::VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(gpui::size(gpui::px(2400.0), gpui::px(800.0)));
        publish(&mut visual, &view, None, start, end - start);
        let painted = |visual: &mut gpui::VisualTestContext| {
            view.read_with(visual, |view, _| view.midi_tile_images.borrow().sprites)
        };
        // One more completed rebuild of the resting row at the same window.
        let before = painted(&mut visual);
        publish(&mut visual, &view, None, start, end - start);
        let sprites = painted(&mut visual) - before;
        let blocks = view.read_with(&visual, |view, _| {
            let cache = view.midi_visible_notes.borrow();
            cache
                .entries
                .get(&id)
                .and_then(|windows| windows.first())
                .and_then(|(_, notes)| notes.clone())
                .and_then(|notes| {
                    notes
                        .tiles
                        .as_ref()
                        .map(|tiles| (tiles.blocks.len(), tiles.repeats as usize))
                })
                .expect("tiled resting clip")
        });
        let (blocks, per_tile) = blocks;
        // 40+ visible repeats paint as far fewer sprites.
        assert!(
            per_tile > 1 && blocks * per_tile >= 40,
            "{blocks} x {per_tile}"
        );
        assert_eq!(sprites, blocks, "one sprite per visible tile");
        assert_eq!(
            crate::editor::midi_thumbnail::rendered_notes(&mut visual, id, false).len(),
            first.len(),
            "only the first pass paints as quads"
        );
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("clip-note-tiles-{}", id.value()).into_boxed_str(),
                ))
                .is_some()
        );
    }

    /// #4137: dragging a long loop's right edge previews the shortened clip
    /// with every visible repeat's notes where playback sounds them, not the
    /// 32 engine samples.
    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn rendered_long_loop_trim_preview_paints_every_visible_repeat(cx: &mut gpui::TestAppContext) {
        use gpui::{Modifiers, MouseButton, point, px};
        let (editor, id) = looped_clip();
        let project = editor.project().clone();
        let clip = project.midi_clip(id).unwrap().clone();
        let tempo = project.tempo_map().clone();
        let start = tempo.beat_to_frame(clip.start()).unwrap();
        let end = tempo
            .beat_to_frame(BeatTime::from_raw(clip.start().raw() + beats(82).raw()))
            .unwrap();
        let (_, preview) =
            hex_engine::compile::project_with_midi_preview(&project, &Default::default()).unwrap();
        let (handle, view) =
            crate::editor::clip_drag_ui_test::editor_window(cx, project.clone(), Arc::new(preview));
        let mut visual = gpui::VisualTestContext::from_window(handle, cx);
        visual.simulate_resize(gpui::size(gpui::px(2400.0), gpui::px(800.0)));
        // Leave room right of the clip for its edge handle.
        publish(&mut visual, &view, None, start, (end - start) / 10 * 11);
        let edge = visual
            .debug_bounds(Box::leak(
                format!("midi-clip-trim-right-{}", id.value()).into_boxed_str(),
            ))
            .expect("right MIDI trim edge");
        let down = crate::editor::clip_drag_ui_test::trim_grab_point(edge);
        visual.simulate_mouse_move(down, None, Modifiers::default());
        visual.run_until_parked();
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(down.x - px(12.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        visual.simulate_mouse_move(
            point(down.x - px(400.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        let trim_end = view.read_with(&visual, |view, _| {
            let drag = view
                .active_midi_trim_drag
                .as_ref()
                .expect("right MIDI trim");
            let preview = drag.preview.borrow().expect("trim preview");
            assert_eq!(
                preview.start(),
                clip.start(),
                "a right trim keeps the start"
            );
            BeatTime::from_raw(preview.start().raw() + preview.length().raw())
        });
        let trim_end = tempo.beat_to_frame(trim_end).unwrap();
        assert!(start < trim_end && trim_end < end, "{trim_end}");
        let expected: Vec<_> = playback(&project, &clip)
            .into_iter()
            .filter(|&(onset, _, _)| onset < trim_end)
            .collect();
        assert!(expected.len() > 32, "{}", expected.len());
        let body = visual
            .debug_bounds("midi-trim-preview-notes")
            .expect("trim preview note layer");
        let notes = crate::editor::midi_thumbnail::rendered_note_layer(
            &mut visual,
            "midi-trim-preview-notes",
        );
        assert_eq!(
            notes.len(),
            expected.len(),
            "every visible repeat is drawn, not a 32-note sample"
        );
        let (left, width) = (f32::from(body.left()), f32::from(body.size.width));
        for (onset, _, _) in &expected {
            let x = left + (onset - start) as f32 / (trim_end - start) as f32 * width;
            assert!(
                notes
                    .iter()
                    .any(|(bounds, _)| (f32::from(bounds.left()) - x).abs() <= 1.5),
                "a trim preview note at playback frame {onset} (x {x})"
            );
        }
    }
}
