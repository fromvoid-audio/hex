//! Song-wide insertion, removal and copy-insertion planned from one original state.
//! Growth adds empty time, shrink/delete removes a half-open interval and closes
//! time, and duplicate inserts a copy after its source. Leading-edge resize keeps
//! marker start fixed and changes the prefix; trailing-edge resize changes the
//! suffix. No operation stretches samples. Musical loop bounds and stopped cursor
//! retain their beat positions, as for reorder. Empty time holds the preceding
//! tempo/meter/gain until the original right-hand state resumes. Every fragment
//! retains original processing/source custody and one Editor transaction owns
//! publication/history or complete refusal.
use super::*;
use crate::{GainInterpolation, GainLane, GainPoint, MeterMap, TempoMap, TempoPoint, TrimEdge};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy)]
enum Transform {
    Insert {
        at: BeatTime,
        end: BeatTime,
        length: u64,
    },
    Delete {
        start: BeatTime,
        end: BeatTime,
    },
    Duplicate {
        start: BeatTime,
        end: BeatTime,
    },
}
impl Transform {
    fn start(self) -> BeatTime {
        match self {
            Self::Insert { at, .. } => at,
            Self::Delete { start, .. } | Self::Duplicate { start, .. } => start,
        }
    }
    fn end(self) -> BeatTime {
        match self {
            Self::Insert { end, .. } => end,
            Self::Delete { end, .. } | Self::Duplicate { end, .. } => end,
        }
    }
    fn length(self) -> u64 {
        match self {
            Self::Insert { length, .. } => length,
            _ => self.end().raw() - self.start().raw(),
        }
    }
    fn map_position(self, beat: BeatTime) -> Result<BeatTime, Invalid> {
        let b = beat.raw();
        let s = self.start().raw();
        let e = self.end().raw();
        let len = self.length();
        let mapped = match self {
            Self::Insert { .. } if b >= s => b
                .checked_add(len)
                .ok_or(Invalid("section insertion position overflow"))?,
            Self::Insert { .. } => b,
            Self::Delete { .. } if b >= e => b
                .checked_sub(len)
                .ok_or(Invalid("section deletion position underflow"))?,
            Self::Delete { .. } if b > s => s,
            Self::Delete { .. } => b,
            Self::Duplicate { .. } if b >= e => b
                .checked_add(len)
                .ok_or(Invalid("section duplication position overflow"))?,
            Self::Duplicate { .. } => b,
        };
        Ok(BeatTime::from_raw(mapped))
    }
    fn frame_map(self, frame: u64, start: u64, end: u64) -> Result<Option<u64>, Invalid> {
        let len = end
            .checked_sub(start)
            .filter(|length| *length > 0)
            .ok_or(Invalid(
                "section interval must contain at least one audio frame",
            ))?;
        match self {
            Self::Insert { .. } if frame >= start => Ok(Some(
                frame
                    .checked_add(len)
                    .ok_or(Invalid("section insertion frame overflow"))?,
            )),
            Self::Insert { .. } => Ok(Some(frame)),
            Self::Delete { .. } if start <= frame && frame < end => Ok(None),
            Self::Delete { .. } if frame >= end => Ok(Some(
                frame
                    .checked_sub(len)
                    .ok_or(Invalid("section deletion frame underflow"))?,
            )),
            Self::Delete { .. } => Ok(Some(frame)),
            Self::Duplicate { .. } if frame >= end => Ok(Some(
                frame
                    .checked_add(len)
                    .ok_or(Invalid("section duplication frame overflow"))?,
            )),
            Self::Duplicate { .. } => Ok(Some(frame)),
        }
    }
}

fn section_index(project: &Project, id: SectionId) -> Result<usize, Invalid> {
    project
        .arrangement_sections
        .iter()
        .position(|section| section.id == id)
        .ok_or(Invalid("arrangement section no longer exists"))
}

pub(super) fn resize(
    project: &mut Project,
    section: SectionId,
    edge: TrimEdge,
    length: BeatTime,
) -> Result<Created, Invalid> {
    let index = section_index(project, section)?;
    let old = &project.arrangement_sections[index];
    let old_len = old.end.raw() - old.start.raw();
    if length == BeatTime::ZERO {
        return Err(Invalid("arrangement section length must be positive"));
    }
    let start = old.start;
    let end = BeatTime::from_raw(
        old.start
            .raw()
            .checked_add(length.raw())
            .ok_or(Invalid("arrangement section end overflow"))?,
    );
    let target_length = end.raw() - start.raw();
    if target_length == old_len {
        return Ok(Created::None);
    }
    let transform = resize_transform(old, edge, target_length, old_len, end)?;
    apply_transform(
        project,
        transform,
        TransformPurpose::Resize(section, start, end),
    )?;
    Ok(Created::None)
}

pub(super) fn delete(project: &mut Project, section: SectionId) -> Result<Created, Invalid> {
    let index = section_index(project, section)?;
    let marker = project.arrangement_sections[index].clone();
    apply_transform(
        project,
        Transform::Delete {
            start: marker.start,
            end: marker.end,
        },
        TransformPurpose::Delete(section),
    )?;
    Ok(Created::None)
}

pub(super) fn duplicate(project: &mut Project, section: SectionId) -> Result<Created, Invalid> {
    let index = section_index(project, section)?;
    let marker = project.arrangement_sections[index].clone();
    if project.arrangement_sections.len() == MAX_ARRANGEMENT_SECTIONS {
        return Err(Invalid("arrangement section limit reached"));
    }
    let mut copy = marker.clone();
    copy.color = super::color::duplicate_color(project, section);
    copy.id = SectionId(0);
    copy.start = marker.end;
    copy.end = BeatTime::from_raw(
        marker
            .end
            .raw()
            .checked_add(marker.end.raw() - marker.start.raw())
            .ok_or(Invalid("duplicated section end overflow"))?,
    );
    let copy_id = SectionId(project.next_id);
    apply_transform(
        project,
        Transform::Duplicate {
            start: marker.start,
            end: marker.end,
        },
        TransformPurpose::Duplicate(section, copy),
    )?;
    Ok(Created::ArrangementSection(copy_id))
}

/// Both established transforms run only on the Editor's isolated candidate;
/// a failed insertion cannot publish the intermediate adjacent copy or history.
pub(super) fn duplicate_at(
    project: &mut Project,
    section: SectionId,
    target: SectionId,
    after: bool,
) -> Result<Created, Invalid> {
    section_index(project, target)?;
    let Created::ArrangementSection(copy) = duplicate(project, section)? else {
        return Err(Invalid("section duplication produced no identity"));
    };
    if after {
        super::reorder::reorder_after(project, copy, target)?;
    } else {
        super::reorder::reorder(project, copy, target)?;
    }
    Ok(Created::ArrangementSection(copy))
}

pub(super) fn preview_duplicate_at(
    project: &Project,
    section: SectionId,
    target: SectionId,
    after: bool,
) -> Result<SectionSplicePreview, Invalid> {
    section_index(project, target)?;
    let layout = preview_duplicate(project, section)?;
    let copy = layout
        .sections
        .iter()
        .find(|s| s.id == SectionId(0))
        .ok_or(Invalid("section copy preview is missing"))?;
    let target = layout
        .sections
        .iter()
        .find(|s| s.id == target)
        .ok_or(Invalid("arrangement destination no longer exists"))?;
    let insertion = if after { target.end } else { target.start };
    let reordered =
        super::preview_reorder_at(copy, insertion, &layout.sections, &layout.tempo_map)?;
    Ok(SectionSplicePreview {
        sections: reordered.sections,
        tempo_map: reordered.tempo_map,
    })
}

#[derive(Clone)]
enum TransformPurpose {
    Delete(SectionId),
    Resize(SectionId, BeatTime, BeatTime),
    Duplicate(SectionId, ArrangementSection),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionSplicePreview {
    pub(super) sections: Vec<ArrangementSection>,
    pub(super) tempo_map: TempoMap,
}
impl SectionSplicePreview {
    pub fn sections(&self) -> &[ArrangementSection] {
        &self.sections
    }
    pub fn tempo_map(&self) -> &TempoMap {
        &self.tempo_map
    }
}

pub(super) fn preview_resize(
    project: &Project,
    section: SectionId,
    edge: TrimEdge,
    length: BeatTime,
) -> Result<SectionSplicePreview, Invalid> {
    preview_resize_parts(
        &project.arrangement_sections,
        &project.tempo_map,
        section,
        edge,
        length,
    )
}

pub(super) fn preview_resize_parts(
    source: &[ArrangementSection],
    tempo: &TempoMap,
    section: SectionId,
    edge: TrimEdge,
    length: BeatTime,
) -> Result<SectionSplicePreview, Invalid> {
    let marker = source
        .iter()
        .find(|s| s.id == section)
        .ok_or(Invalid("arrangement section no longer exists"))?;
    if length == BeatTime::ZERO {
        return Err(Invalid("arrangement section length must be positive"));
    }
    let end = BeatTime::from_raw(
        marker
            .start
            .raw()
            .checked_add(length.raw())
            .ok_or(Invalid("arrangement section end overflow"))?,
    );
    let old_len = marker.end.raw() - marker.start.raw();
    if length.raw() == old_len {
        let mut sections = Vec::new();
        sections
            .try_reserve_exact(source.len())
            .map_err(|_| Invalid("could not reserve section marker preview"))?;
        sections.extend(source.iter().cloned());
        return Ok(SectionSplicePreview {
            sections,
            tempo_map: tempo.clone(),
        });
    }
    let transform = resize_transform(marker, edge, length.raw(), old_len, end)?;
    layout_preview(
        source,
        tempo,
        transform,
        TransformPurpose::Resize(section, marker.start, end),
    )
}

pub(super) fn preview_delete(
    project: &Project,
    section: SectionId,
) -> Result<SectionSplicePreview, Invalid> {
    let marker = &project.arrangement_sections[section_index(project, section)?];
    layout_preview(
        &project.arrangement_sections,
        &project.tempo_map,
        Transform::Delete {
            start: marker.start,
            end: marker.end,
        },
        TransformPurpose::Delete(section),
    )
}

pub(super) fn preview_duplicate(
    project: &Project,
    section: SectionId,
) -> Result<SectionSplicePreview, Invalid> {
    let marker = &project.arrangement_sections[section_index(project, section)?];
    if project.arrangement_sections.len() == MAX_ARRANGEMENT_SECTIONS {
        return Err(Invalid("arrangement section limit reached"));
    }
    let mut copy = marker.clone();
    copy.color = super::color::duplicate_color(project, section);
    copy.id = SectionId(0);
    copy.start = marker.end;
    copy.end = BeatTime::from_raw(
        marker
            .end
            .raw()
            .checked_add(marker.end.raw() - marker.start.raw())
            .ok_or(Invalid("duplicated section end overflow"))?,
    );
    layout_preview(
        &project.arrangement_sections,
        &project.tempo_map,
        Transform::Duplicate {
            start: marker.start,
            end: marker.end,
        },
        TransformPurpose::Duplicate(section, copy),
    )
}

fn layout_preview(
    source: &[ArrangementSection],
    tempo: &TempoMap,
    transform: Transform,
    purpose: TransformPurpose,
) -> Result<SectionSplicePreview, Invalid> {
    let mut sections = Vec::new();
    let capacity = source
        .len()
        .checked_add(1)
        .ok_or(Invalid("section marker preview count overflow"))?;
    sections
        .try_reserve_exact(capacity)
        .map_err(|_| Invalid("could not reserve section marker preview"))?;
    for marker in source {
        match &purpose {
            TransformPurpose::Delete(id) if marker.id == *id => continue,
            TransformPurpose::Resize(id, start, end) if marker.id == *id => {
                let mut adjusted = marker.clone();
                adjusted.start = *start;
                adjusted.end = *end;
                sections.push(adjusted);
                continue;
            }
            TransformPurpose::Duplicate(source, _) if marker.id == *source => {
                sections.push(marker.clone());
                continue;
            }
            _ => {}
        }
        let mut adjusted = marker.clone();
        adjusted.start = transform.map_position(marker.start)?;
        adjusted.end = BeatTime::from_raw(
            adjusted
                .start
                .raw()
                .checked_add(marker.end.raw() - marker.start.raw())
                .ok_or(Invalid("section marker end overflow"))?,
        );
        if adjusted.start < adjusted.end {
            sections.push(adjusted);
        }
    }
    if let TransformPurpose::Duplicate(_, marker) = purpose {
        sections.push(marker);
    }
    sections.sort_by_key(|marker| marker.start);
    let tempo_map = transformed_tempo(tempo, transform)?;
    super::validate(&sections, &tempo_map)?;
    Ok(SectionSplicePreview {
        sections,
        tempo_map,
    })
}

fn resize_transform(
    marker: &ArrangementSection,
    edge: TrimEdge,
    target_length: u64,
    old_length: u64,
    new_end: BeatTime,
) -> Result<Transform, Invalid> {
    let delta = target_length.abs_diff(old_length);
    if target_length > old_length {
        let at = match edge {
            TrimEdge::Left => marker.start,
            TrimEdge::Right => marker.end,
        };
        Ok(Transform::Insert {
            at,
            end: BeatTime::from_raw(
                at.raw()
                    .checked_add(delta)
                    .ok_or(Invalid("section extension overflow"))?,
            ),
            length: delta,
        })
    } else {
        let (start, end) = match edge {
            TrimEdge::Left => (
                marker.start,
                BeatTime::from_raw(
                    marker
                        .start
                        .raw()
                        .checked_add(delta)
                        .ok_or(Invalid("section prefix removal overflow"))?,
                ),
            ),
            TrimEdge::Right => (new_end, marker.end),
        };
        Ok(Transform::Delete { start, end })
    }
}

fn apply_transform(
    project: &mut Project,
    transform: Transform,
    purpose: TransformPurpose,
) -> Result<(), Invalid> {
    if project.has_device_automation() {
        return Err(Invalid(crate::AUTOMATION_EDIT_UNSUPPORTED));
    }
    let mut next_id = project.next_id;
    let purpose = match purpose {
        TransformPurpose::Duplicate(source, mut marker) => {
            marker.id = SectionId(super::reorder::take_id(&mut next_id)?);
            TransformPurpose::Duplicate(source, marker)
        }
        other => other,
    };
    let old_tempo = std::sync::Arc::new(project.tempo_map.clone());
    let start_frame = old_tempo.beat_to_frame(transform.start())?;
    let layout = layout_preview(
        &project.arrangement_sections,
        &project.tempo_map,
        transform,
        purpose.clone(),
    )?;
    let new_tempo = layout.tempo_map;
    let end_frame = if matches!(transform, Transform::Insert { .. }) {
        start_frame
            .checked_add(
                new_tempo
                    .beat_to_frame(transform.end())?
                    .checked_sub(new_tempo.beat_to_frame(transform.start())?)
                    .ok_or(Invalid("inserted time frame underflow"))?,
            )
            .ok_or(Invalid("inserted time frame overflow"))?
    } else {
        old_tempo.beat_to_frame(transform.end())?
    };
    let frame_length = end_frame
        .checked_sub(start_frame)
        .filter(|n| *n > 0)
        .ok_or(Invalid(
            "section interval must contain at least one audio frame",
        ))?;
    let new_meter = transformed_meter(&project.meter_map, transform)?;
    let loop_region = transformed_loop(project.loop_region, &old_tempo, &new_tempo)?;
    let mut clip_count = project
        .session
        .cells
        .iter()
        .filter(|cell| !matches!(cell.content, crate::SessionCellContent::Stop))
        .count();
    let mut note_count = project
        .session
        .cells
        .iter()
        .filter_map(|cell| match &cell.content {
            crate::SessionCellContent::Midi(clip) => Some(clip.source().notes().len()),
            _ => None,
        })
        .try_fold(0_usize, |total, notes| total.checked_add(notes))
        .ok_or(Invalid("session midi note count overflow"))?;
    let mut tracks = Vec::new();
    let mut gain_units = 0_usize;
    tracks
        .try_reserve_exact(project.tracks.len())
        .map_err(|_| Invalid("could not reserve section splice track plan"))?;
    for source in &project.tracks {
        let mut track = source.clone();
        track.gain_lane = transform_gain_lane(&source.gain_lane, source.gain, transform)?;
        gain_units = gain_units
            .checked_add(track.gain_lane.units()?)
            .filter(|units| *units <= crate::MAX_GAIN_POINTS)
            .ok_or(Invalid("project gain envelope point limit reached"))?;
        let mut audio = Vec::new();
        for clip in source.clips() {
            let fragments = transform_audio(
                clip,
                transform,
                &FrameSplice {
                    start: start_frame,
                    end: end_frame,
                    length: frame_length,
                },
                &old_tempo,
                &new_tempo,
                &mut next_id,
            )?;
            clip_count = clip_count
                .checked_add(fragments.len())
                .filter(|count| *count <= crate::MAX_CLIPS)
                .ok_or(Invalid("clip limit reached"))?;
            audio
                .try_reserve(fragments.len())
                .map_err(|_| Invalid("could not reserve section audio result"))?;
            audio.extend(fragments);
        }
        audio.sort_by_key(|clip| clip.start());
        track.content.audio_clips = audio;
        let mut midi = Vec::new();
        for clip in source.midi_clips() {
            let fragments = transform_midi(clip, transform, &mut next_id, &mut note_count)?;
            clip_count = clip_count
                .checked_add(fragments.len())
                .filter(|count| *count <= crate::MAX_CLIPS)
                .ok_or(Invalid("clip limit reached"))?;
            midi.try_reserve(fragments.len())
                .map_err(|_| Invalid("could not reserve section midi result"))?;
            midi.extend(fragments);
        }
        midi.sort_by_key(|clip| clip.start());
        track.content.midi_clips = midi;
        tracks.push(track);
    }
    project.next_id = next_id;
    project.tracks = tracks;
    project.tempo_map = new_tempo;
    project.meter_map = new_meter;
    project.loop_region = loop_region;
    project.arrangement_sections = layout.sections;
    Ok(())
}

/// Fence the moved music's tempo. Authored points always stay; a seam that
/// repeats the tempo already in effect there changes no clock, and inserting
/// it anyway piles same-tempo points up with every structural section edit.
fn seam(points: &mut BTreeMap<u64, crate::Tempo>, beat: u64, tempo: crate::Tempo) {
    let prevailing = points.range(..=beat).next_back().map(|(_, tempo)| *tempo);
    if points.contains_key(&beat) || prevailing != Some(tempo) {
        points.insert(beat, tempo);
    }
}
fn transformed_tempo(old: &TempoMap, transform: Transform) -> Result<TempoMap, Invalid> {
    // Duplicating music cannot change a single global tempo. Preserve its exact
    // clock, including fractional BPMs, rather than adding same-tempo seams.
    if matches!(transform, Transform::Duplicate { .. }) && old.points().len() == 1 {
        return Ok(old.clone());
    }
    let start = transform.start();
    let end = transform.end();
    let length = transform.length();
    let mut points = BTreeMap::<u64, crate::Tempo>::new();
    match transform {
        Transform::Insert { at, length, .. } => {
            let prior = old.points()[old
                .points()
                .partition_point(|point| point.beat() < at)
                .saturating_sub(1)]
            .tempo();
            for point in old.points() {
                if point.beat() < at {
                    points.insert(point.beat().raw(), point.tempo());
                } else {
                    points.insert(
                        point
                            .beat()
                            .raw()
                            .checked_add(length)
                            .ok_or(Invalid("section tempo point overflow"))?,
                        point.tempo(),
                    );
                }
            }
            seam(&mut points, at.raw(), prior);
            seam(&mut points, end.raw(), old.tempo_at(at));
        }
        Transform::Delete { .. } => {
            for point in old.points() {
                if point.beat() < start {
                    points.insert(point.beat().raw(), point.tempo());
                } else if point.beat() >= end {
                    points.insert(point.beat().raw() - length, point.tempo());
                }
            }
            seam(&mut points, start.raw(), old.tempo_at(end));
        }
        Transform::Duplicate { .. } => {
            for point in old.points() {
                let beat = point.beat();
                if beat < end {
                    points.insert(beat.raw(), point.tempo());
                }
                if start <= beat && beat < end {
                    let copied = beat
                        .raw()
                        .checked_add(length)
                        .ok_or(Invalid("section tempo point overflow"))?;
                    points.insert(copied, point.tempo());
                }
                if beat >= end {
                    let shifted = beat
                        .raw()
                        .checked_add(length)
                        .ok_or(Invalid("section tempo point overflow"))?;
                    points.insert(shifted, point.tempo());
                }
            }
            seam(&mut points, end.raw(), old.tempo_at(start));
            seam(
                &mut points,
                end.raw()
                    .checked_add(length)
                    .ok_or(Invalid("section tempo end overflow"))?,
                old.tempo_at(end),
            );
        }
    }
    points
        .entry(0)
        .or_insert_with(|| old.tempo_at(BeatTime::ZERO));
    let result = points
        .into_iter()
        .map(|(beat, tempo)| TempoPoint::new(BeatTime::from_raw(beat), tempo))
        .collect();
    TempoMap::new(old.sample_rate(), result)
}

fn transformed_meter(old: &MeterMap, transform: Transform) -> Result<MeterMap, Invalid> {
    let start = transform.start();
    let end = transform.end();
    let length = transform.length();
    let mut result = meter::MeterEdit::default();
    match transform {
        Transform::Insert { at, .. } => {
            let prior = old.points()[old
                .points()
                .partition_point(|point| point.beat() < at)
                .saturating_sub(1)]
            .meter();
            for point in old.points() {
                result.original(transform.map_position(point.beat())?, *point);
            }
            result.seam(at, prior);
            result.seam(end, old.meter_at(at));
        }
        Transform::Delete { .. } => {
            for point in old.points() {
                if point.beat() < start || point.beat() >= end {
                    result.original(transform.map_position(point.beat())?, *point);
                }
            }
            result.seam(start, old.meter_at(end));
        }
        Transform::Duplicate { .. } => {
            for point in old.points() {
                let beat = point.beat();
                if beat < end {
                    result.original(beat, *point);
                }
                if beat >= start {
                    result.original(
                        BeatTime::from_raw(
                            beat.raw()
                                .checked_add(length)
                                .ok_or(Invalid("section meter point overflow"))?,
                        ),
                        *point,
                    );
                }
            }
            result.seam(end, old.meter_at(start));
            result.seam(
                BeatTime::from_raw(
                    end.raw()
                        .checked_add(length)
                        .ok_or(Invalid("section meter end overflow"))?,
                ),
                old.meter_at(end),
            );
        }
    }
    result.finish(old.meter_at(BeatTime::ZERO))
}

fn transformed_loop(
    region: Option<crate::LoopRegion>,
    old: &TempoMap,
    new: &TempoMap,
) -> Result<Option<crate::LoopRegion>, Invalid> {
    let Some(region) = region else {
        return Ok(None);
    };
    let start = old.frame_to_beat(region.start())?;
    let end = old.frame_to_beat(region.end())?;
    Ok(Some(crate::LoopRegion::new(
        new.beat_to_frame(start)?,
        new.beat_to_frame(end)?,
        region.enabled(),
    )?))
}

/// Insert uses a synthetic blank end; only removal/copy ends are source fences.
struct FrameSplice {
    start: u64,
    end: u64,
    length: u64,
}
fn transform_audio(
    original: &crate::Clip,
    transform: Transform,
    frames: &FrameSplice,
    old: &std::sync::Arc<TempoMap>,
    new: &TempoMap,
    next_id: &mut u64,
) -> Result<Vec<crate::Clip>, Invalid> {
    let clip_end = original
        .start()
        .checked_add(original.duration())
        .ok_or(Invalid("audio section clip end overflow"))?;
    // A musical clip whose beat edge lies on a fence may sit one residual
    // sample beside it. Treat that edge as on the fence rather than cutting a
    // one-sample fragment whose beat and frame land on opposite sides.
    let fence = |frame: u64| {
        let Some(edit) = original
            .audio_edit()
            .filter(|edit| edit.live_musical_start().is_some())
        else {
            return frame;
        };
        let nominal = |actual: u64, offset: i8| i128::from(actual) - i128::from(offset);
        if i128::from(frame) == nominal(original.start(), edit.placement_start_offset) {
            original.start()
        } else if i128::from(frame) == nominal(clip_end, edit.placement_end_offset) {
            clip_end
        } else {
            frame
        }
    };
    let (start_frame, end_frame, frame_length) =
        (fence(frames.start), fence(frames.end), frames.length);
    let mut cuts = vec![original.start(), clip_end];
    let splice_frames = if matches!(transform, Transform::Insert { .. }) {
        vec![start_frame]
    } else {
        vec![start_frame, end_frame]
    };
    cuts.extend(
        splice_frames
            .into_iter()
            .filter(|frame| original.start() < *frame && *frame < clip_end),
    );
    cuts.sort_unstable();
    cuts.dedup();
    let mut root = original.clone();
    if let Some(edit) = root.audio_edit_mut() {
        edit.freeze_processing(std::sync::Arc::clone(old))?;
    }
    let mut fragments = Vec::new();
    fragments
        .try_reserve_exact(cuts.len())
        .map_err(|_| Invalid("could not reserve section audio fragments"))?;
    let mut kept_original_id = false;
    for pair in cuts.windows(2) {
        let source_start = pair[0];
        let source_end = pair[1];
        let is_duplicate_source = matches!(transform, Transform::Duplicate { .. })
            && start_frame <= source_start
            && source_end <= end_frame;
        let removed = matches!(transform, Transform::Delete { .. })
            && start_frame <= source_start
            && source_end <= end_frame;
        if removed {
            continue;
        }
        let retained = crate::edit::retained_audio_fragment(
            &root,
            source_start,
            source_end,
            original.id(),
            old,
        )?;
        let destinations = if is_duplicate_source {
            let shifted = source_start
                .checked_add(frame_length)
                .ok_or(Invalid("section audio duplicate overflow"))?;
            vec![Some(source_start), Some(shifted)]
        } else {
            // A snapped edge moves with its authored fence and lands on the
            // mapped fence itself, as its beat does, dropping the residual.
            let authored = if source_start == start_frame {
                frames.start
            } else if source_start == end_frame {
                frames.end
            } else {
                source_start
            };
            vec![transform.frame_map(authored, frames.start, frames.end)?]
        };
        for (destination_index, destination) in destinations.into_iter().enumerate() {
            let Some(destination) = destination else {
                continue;
            };
            let is_copy = is_duplicate_source && destination_index == 1;
            let id = if !is_copy && !kept_original_id {
                kept_original_id = true;
                original.id()
            } else {
                crate::ClipId(super::reorder::take_id(next_id)?)
            };
            let mut fragment = retained.clone();
            fragment.id = id;
            let mut destination = destination;
            if let Some(edit) = fragment.audio_edit_mut()
                && let Some(musical_start) = edit.live_musical_start()
            {
                let source_beat = if source_start == start_frame {
                    transform.start()
                } else if !matches!(transform, Transform::Insert { .. })
                    && source_start == end_frame
                {
                    transform.end()
                } else {
                    musical_start
                };
                // A duplicated source fragment stays in place by frame. Its beat
                // may sit on the end fence (a residual sample inside it), where
                // map_position would move only the beat, so keep it too.
                let destination_beat = if is_copy {
                    BeatTime::from_raw(
                        source_beat
                            .raw()
                            .checked_add(transform.length())
                            .ok_or(Invalid("section audio timing overflow"))?,
                    )
                } else if is_duplicate_source {
                    source_beat
                } else {
                    transform.map_position(source_beat)?
                };
                edit.timing = crate::AudioClipTiming::Musical(destination_beat);
                destination =
                    edit.retain_moved_frame_span(destination, source_end - source_start, new)?;
            }
            fragment.start = destination;
            crate::audio_edit::project_clip(&mut fragment, new)?;
            if fragment.start() != destination || fragment.duration() != source_end - source_start {
                return Err(Invalid("section splice changed retained audio sample span"));
            }
            fragments.push(fragment);
            if fragments.len() > crate::MAX_CLIPS {
                return Err(Invalid("clip limit reached"));
            }
        }
    }
    Ok(fragments)
}

fn transform_midi(
    original: &crate::MidiClip,
    transform: Transform,
    next_id: &mut u64,
    total_notes: &mut usize,
) -> Result<Vec<crate::MidiClip>, Invalid> {
    let clip_start = original.start().raw();
    let clip_end = clip_start
        .checked_add(original.length().raw())
        .ok_or(Invalid("midi section clip end overflow"))?;
    if let Transform::Duplicate { start, end } = transform {
        let length = end.raw() - start.raw();
        let mut result = transform_midi(
            original,
            Transform::Insert {
                at: end,
                end: BeatTime::from_raw(
                    end.raw()
                        .checked_add(length)
                        .ok_or(Invalid("duplicate time overflow"))?,
                ),
                length,
            },
            next_id,
            total_notes,
        )?;
        let a = clip_start.max(start.raw());
        let b = clip_end.min(end.raw());
        if a < b {
            let mut copy = if a == clip_start {
                let mut prefix = original.clone();
                prefix.length = BeatTime::from_raw(b - a);
                prefix
            } else {
                if crate::midi::overwrite_occurrence_count(original)? > crate::MAX_NOTES {
                    return Err(Invalid("midi section copy expansion exceeds note limit"));
                }
                crate::midi::materialize_overwrite_ranges(original, &[(a, b)])?
                    .pop()
                    .ok_or(Invalid("midi section copy produced no range"))?
            };
            copy.id = crate::ClipId(super::reorder::take_id(next_id)?);
            copy.start = BeatTime::from_raw(
                a.checked_add(length)
                    .ok_or(Invalid("midi section copy placement overflow"))?,
            );
            assign_note_ids(&mut copy, next_id)?;
            add_notes(total_notes, copy.notes().len())?;
            result
                .try_reserve(1)
                .map_err(|_| Invalid("could not reserve section midi copy"))?;
            result.push(copy);
            result.sort_by_key(|c| c.start());
        }
        return Ok(result);
    }
    let start = transform.start().raw();
    let end = transform.end().raw();
    let mut cuts = vec![clip_start, clip_end];
    if clip_start < start && start < clip_end {
        cuts.push(start);
    }
    if matches!(transform, Transform::Delete { .. }) && clip_start < end && end < clip_end {
        cuts.push(end);
    }
    cuts.sort_unstable();
    cuts.dedup();
    if cuts.len() > 2 && crate::midi::overwrite_occurrence_count(original)? > crate::MAX_NOTES {
        return Err(Invalid("midi section splice expansion exceeds note limit"));
    }
    let mut result = Vec::new();
    result
        .try_reserve_exact(cuts.len() - 1)
        .map_err(|_| Invalid("could not reserve section midi fragments"))?;
    let mut kept_original_id = false;
    for pair in cuts.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if matches!(transform, Transform::Delete { .. }) && start <= a && b <= end {
            continue;
        }
        let mut part = if a == clip_start {
            let mut prefix = original.clone();
            prefix.length = BeatTime::from_raw(b - a);
            prefix
        } else {
            crate::midi::materialize_overwrite_ranges(original, &[(a, b)])?
                .pop()
                .ok_or(Invalid("midi section splice produced no range"))?
        };
        if kept_original_id {
            part.id = crate::ClipId(super::reorder::take_id(next_id)?);
        } else {
            part.id = original.id();
            kept_original_id = true;
        }
        if a != clip_start {
            assign_note_ids(&mut part, next_id)?;
        }
        part.start = transform.map_position(BeatTime::from_raw(a))?;
        add_notes(total_notes, part.notes().len())?;
        result.push(part);
    }
    Ok(result)
}

fn assign_note_ids(clip: &mut crate::MidiClip, next_id: &mut u64) -> Result<(), Invalid> {
    for note in &mut std::sync::Arc::make_mut(&mut clip.source).notes {
        note.id = crate::NoteId(super::reorder::take_id(next_id)?);
    }
    Ok(())
}
fn add_notes(total: &mut usize, count: usize) -> Result<(), Invalid> {
    *total = total
        .checked_add(count)
        .filter(|n| *n <= crate::MAX_NOTES)
        .ok_or(Invalid("midi note limit reached"))?;
    Ok(())
}
fn transform_gain_lane(
    lane: &GainLane,
    fallback: f32,
    transform: Transform,
) -> Result<GainLane, Invalid> {
    let mut points: Vec<GainPoint> = Vec::new();
    let s = transform.start();
    let e = transform.end();
    let len = transform.length();
    if !lane.base().is_empty()
        || (matches!(transform, Transform::Insert { .. }) && !lane.spans().is_empty())
    {
        let mut beats: BTreeSet<_> = lane.base().iter().map(|p| p.beat).collect();
        beats.insert(BeatTime::ZERO);
        if matches!(transform, Transform::Insert { .. }) {
            beats.insert(s);
        } else {
            beats.extend([s, e]);
        }
        let mut transformed = Vec::<GainPoint>::new();
        let reserve_count = beats
            .len()
            .checked_mul(2)
            .and_then(|n| n.checked_add(2))
            .ok_or(Invalid("section gain point count overflow"))?;
        transformed
            .try_reserve_exact(reserve_count)
            .map_err(|_| Invalid("could not reserve section gain points"))?;
        for beat in beats {
            let (gain, outgoing) = super::reorder::base_value(lane.base(), fallback, beat, false);
            let incoming = super::reorder::base_value(lane.base(), fallback, beat, true).0;
            let original = GainPoint {
                incoming_gain: Some(incoming),
                beat,
                gain,
                outgoing,
            };
            match transform {
                Transform::Insert { .. } => {
                    let mapped = if beat < s {
                        beat
                    } else {
                        BeatTime::from_raw(
                            beat.raw()
                                .checked_add(len)
                                .ok_or(Invalid("gain point placement overflow"))?,
                        )
                    };
                    transformed.push(GainPoint {
                        beat: mapped,
                        ..original
                    });
                }
                Transform::Delete { .. } => {
                    if beat < s {
                        transformed.push(original);
                    } else if beat >= e {
                        transformed.push(GainPoint {
                            beat: BeatTime::from_raw(beat.raw() - len),
                            ..original
                        });
                    }
                }
                Transform::Duplicate { .. } => {
                    if beat < e {
                        transformed.push(original);
                    }
                    if s <= beat && beat < e {
                        transformed.push(GainPoint {
                            beat: BeatTime::from_raw(
                                beat.raw()
                                    .checked_add(len)
                                    .ok_or(Invalid("gain duplicate point overflow"))?,
                            ),
                            ..original
                        });
                    }
                    if beat >= e {
                        transformed.push(GainPoint {
                            beat: BeatTime::from_raw(
                                beat.raw()
                                    .checked_add(len)
                                    .ok_or(Invalid("gain suffix point overflow"))?,
                            ),
                            ..original
                        });
                    }
                }
            }
        }
        if matches!(transform, Transform::Insert { .. }) {
            let base_left = super::reorder::base_value(lane.base(), fallback, s, true).0;
            let before = BeatTime::from_raw(s.raw().saturating_sub(1));
            let held = lane.touch_gain_at(before).unwrap_or(base_left);
            transformed.push(GainPoint {
                beat: s,
                gain: held,
                outgoing: GainInterpolation::Step,
                incoming_gain: Some(base_left),
            });
            let (gain, outgoing) = super::reorder::base_value(lane.base(), fallback, s, false);
            transformed.push(GainPoint {
                beat: e,
                gain,
                outgoing,
                incoming_gain: Some(held),
            });
        } else if matches!(transform, Transform::Delete { .. }) {
            let (gain, outgoing) = super::reorder::base_value(lane.base(), fallback, e, false);
            let incoming = super::reorder::base_value(lane.base(), fallback, s, true).0;
            transformed.push(GainPoint {
                beat: s,
                gain,
                outgoing,
                incoming_gain: Some(incoming),
            });
        } else if matches!(transform, Transform::Duplicate { .. }) {
            let (gain, outgoing) = super::reorder::base_value(lane.base(), fallback, s, false);
            let incoming = super::reorder::base_value(lane.base(), fallback, e, true).0;
            transformed.push(GainPoint {
                beat: e,
                gain,
                outgoing,
                incoming_gain: Some(incoming),
            });
            let (gain, outgoing) = super::reorder::base_value(lane.base(), fallback, e, false);
            let incoming = super::reorder::base_value(lane.base(), fallback, e, true).0;
            transformed.push(GainPoint {
                beat: BeatTime::from_raw(
                    e.raw()
                        .checked_add(len)
                        .ok_or(Invalid("gain restore point overflow"))?,
                ),
                gain,
                outgoing,
                incoming_gain: Some(incoming),
            });
        }
        transformed.sort_by_key(|point| point.beat);
        points
            .try_reserve_exact(transformed.len())
            .map_err(|_| Invalid("could not reserve section gain result"))?;
        for point in transformed {
            if let Some(previous) = points.last_mut()
                && previous.beat == point.beat
            {
                // Synthetic seam points are appended after transformed source
                // points and deliberately replace their values at collisions.
                *previous = point;
            } else {
                points.push(point);
            }
        }
    }
    let mut spans = Vec::new();
    for span in lane.spans() {
        let mut cuts = vec![span.start, span.end];
        let splice_cuts = if matches!(transform, Transform::Insert { .. }) {
            vec![s]
        } else {
            vec![s, e]
        };
        cuts.extend(
            splice_cuts
                .into_iter()
                .filter(|beat| span.start < *beat && *beat < span.end),
        );
        cuts.sort();
        cuts.dedup();
        for pair in cuts.windows(2) {
            let a = pair[0];
            let b = pair[1];
            let inside = s <= a && b <= e;
            if matches!(transform, Transform::Delete { .. }) && inside {
                continue;
            }
            let fragment = span.fragment(a, b)?;
            let destinations = if matches!(transform, Transform::Duplicate { .. }) && inside {
                vec![
                    a,
                    BeatTime::from_raw(
                        a.raw()
                            .checked_add(len)
                            .ok_or(Invalid("touch duplicate start overflow"))?,
                    ),
                ]
            } else {
                vec![transform.map_position(a)?]
            };
            for destination in destinations {
                let delta = i128::from(destination.raw()) - i128::from(fragment.start.raw());
                let mut moved = fragment.clone();
                moved.start = destination;
                moved.end = BeatTime::from_raw(
                    destination
                        .raw()
                        .checked_add(b.raw() - a.raw())
                        .ok_or(Invalid("touch span placement overflow"))?,
                );
                for step in &mut moved.steps {
                    step.beat = BeatTime::from_raw(
                        u64::try_from(i128::from(step.beat.raw()) + delta)
                            .map_err(|_| Invalid("touch step placement overflow"))?,
                    );
                }
                spans
                    .try_reserve(1)
                    .map_err(|_| Invalid("could not reserve section gain touch spans"))?;
                spans.push(moved);
                if spans.len() > crate::MAX_GAIN_TOUCH_SPANS_PER_TRACK {
                    return Err(Invalid("gain touch span limit reached"));
                }
            }
        }
    }
    spans.sort_by_key(|span| span.start);
    GainLane::new(points, spans, lane.recording_epoch())
}

struct IsolatedResize {
    section: SectionId,
    start: BeatTime,
    end: BeatTime,
    temporary_start: BeatTime,
    temporary_end: BeatTime,
    delete: Transform,
    insert: Transform,
}
fn isolated_plan(
    project: &Project,
    section: SectionId,
    edge: TrimEdge,
    boundary: BeatTime,
) -> Result<Option<IsolatedResize>, Invalid> {
    let index = section_index(project, section)?;
    let old = &project.arrangement_sections[index];
    let (start, end, original) = match edge {
        TrimEdge::Left => (boundary, old.end, old.start),
        TrimEdge::Right => (old.start, boundary, old.end),
    };
    if start >= end {
        return Err(Invalid("section edge must leave a positive range"));
    }
    if project
        .arrangement_sections
        .iter()
        .any(|s| s.id != section && start < s.end && s.start < end)
    {
        return Err(Invalid(
            "section edge cannot overlap another section; drag the shared seam to resize both",
        ));
    }
    if original == boundary {
        return Ok(None);
    }
    let lo = original.min(boundary);
    let hi = original.max(boundary);
    let delete = Transform::Delete { start: lo, end: hi };
    Ok(Some(IsolatedResize {
        section,
        start,
        end,
        temporary_start: delete.map_position(old.start)?,
        temporary_end: delete.map_position(old.end)?,
        delete,
        insert: Transform::Insert {
            at: lo,
            end: hi,
            length: hi.raw() - lo.raw(),
        },
    }))
}

/// One candidate transaction: clear crossed edge time, then restore its duration.
/// Only the selected marker changes; all other markers return to their original
/// positions. Existing splice laws retain media processing, MIDI and automation.
pub(super) fn resize_isolated(
    project: &mut Project,
    section: SectionId,
    edge: TrimEdge,
    boundary: BeatTime,
) -> Result<Created, Invalid> {
    let Some(plan) = isolated_plan(project, section, edge, boundary)? else {
        return Ok(Created::None);
    };
    apply_transform(
        project,
        plan.delete,
        TransformPurpose::Resize(plan.section, plan.temporary_start, plan.temporary_end),
    )?;
    apply_transform(
        project,
        plan.insert,
        TransformPurpose::Resize(plan.section, plan.start, plan.end),
    )?;
    Ok(Created::None)
}

pub(super) fn preview_isolated(
    project: &Project,
    section: SectionId,
    edge: TrimEdge,
    boundary: BeatTime,
) -> Result<SectionSplicePreview, Invalid> {
    let Some(plan) = isolated_plan(project, section, edge, boundary)? else {
        let old = &project.arrangement_sections[section_index(project, section)?];
        return preview_resize(
            project,
            section,
            edge,
            BeatTime::from_raw(old.end.raw() - old.start.raw()),
        );
    };
    let first = layout_preview(
        &project.arrangement_sections,
        &project.tempo_map,
        plan.delete,
        TransformPurpose::Resize(plan.section, plan.temporary_start, plan.temporary_end),
    )?;
    layout_preview(
        &first.sections,
        &first.tempo_map,
        plan.insert,
        TransformPurpose::Resize(plan.section, plan.start, plan.end),
    )
}
