//! Control-side, finite Arrangement range fragments; no PCM or OS clipboard.
use crate::{
    Asset, AssetId, BeatTime, Clip, ClipId, Created, Invalid, MAX_CLIPS, MAX_NOTES, MAX_TRACKS,
    MidiClip, MidiNote, MidiSource, NoteId, Project, TrackId,
};
use std::sync::Arc;
mod insert;
mod midi_volume;
pub(crate) use insert::insert;
pub(crate) use insert::insert_on_source_tracks;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeRange {
    tracks: Vec<TrackId>,
    start: u64,
    end: u64,
}
impl TimeRange {
    pub fn new(tracks: Vec<TrackId>, start: u64, end: u64) -> Result<Self, Invalid> {
        if tracks.is_empty() || tracks.len() > MAX_TRACKS || start >= end {
            return Err(Invalid(
                "time selection needs tracks and a positive interval",
            ));
        }
        for (index, track) in tracks.iter().enumerate() {
            if tracks[..index].contains(track) {
                return Err(Invalid("time selection contains duplicate tracks"));
            }
        }
        Ok(Self { tracks, start, end })
    }
    pub fn tracks(&self) -> &[TrackId] {
        &self.tracks
    }
    pub fn start(&self) -> u64 {
        self.start
    }
    pub fn end(&self) -> u64 {
        self.end
    }
    pub fn duration(&self) -> u64 {
        self.end - self.start
    }
    pub(crate) fn validate(&self, project: &Project) -> Result<usize, Invalid> {
        let first = project
            .tracks()
            .iter()
            .position(|track| Some(&track.id()) == self.tracks.first())
            .ok_or(Invalid("time selection track no longer exists"))?;
        let end = first
            .checked_add(self.tracks.len())
            .ok_or(Invalid("track range overflow"))?;
        let rows = project
            .tracks()
            .get(first..end)
            .ok_or(Invalid("time selection tracks no longer exist"))?;
        if !rows
            .iter()
            .map(|track| track.id())
            .eq(self.tracks.iter().copied())
        {
            return Err(Invalid("time selection track order changed"));
        }
        Ok(first)
    }
}

/// Opaque, bounded selected playback metadata. Scope this to one open project.
/// Partial MIDI is finite one-shot playback; gates crossing a cut retrigger.
#[derive(Debug, Clone)]
pub struct RangeClipboard {
    sample_rate: u32,
    duration: u64,
    rows: usize,
    source_tracks: Vec<TrackId>,
    assets: Vec<(AssetId, Asset)>,
    audio: Vec<(usize, Clip)>,
    midi: Vec<MidiFragment>,
}
impl RangeClipboard {
    pub fn duration(&self) -> u64 {
        self.duration
    }
    pub fn track_count(&self) -> usize {
        self.rows
    }
}
#[derive(Debug, Clone)]
struct MidiFragment {
    active: bool,
    volume: midi_volume::FrameVolume,
    name: String,
    flavor: crate::ClipFlavor,
    row: usize,
    start: u64,
    duration: u64,
    notes: Vec<FrameNote>,
}
#[derive(Debug, Clone)]
struct FrameNote {
    active: bool,
    start: u64,
    duration: u64,
    pitch: u8,
    channel: u8,
    velocity: u8,
}

fn reserve<T>(items: &mut Vec<T>, count: usize) -> Result<(), Invalid> {
    items
        .try_reserve(count)
        .map_err(|_| Invalid("could not reserve time selection storage"))
}

impl Project {
    /// Capture selected audible fragments on a control worker, not the UI/audio thread.
    pub fn capture_time_range(&self, range: &TimeRange) -> Result<RangeClipboard, Invalid> {
        let first = range.validate(self)?;
        self.refuse_automation_transform(range.tracks())?;
        let mut result = RangeClipboard {
            sample_rate: self.sample_rate(),
            duration: range.duration(),
            rows: range.tracks.len(),
            source_tracks: range.tracks.clone(),
            assets: Vec::new(),
            audio: Vec::new(),
            midi: Vec::new(),
        };
        let mut expanded = 0_usize;
        for (row, track) in self.tracks()[first..first + range.tracks.len()]
            .iter()
            .enumerate()
        {
            for clip in track.clips() {
                let clip_end = clip
                    .start()
                    .checked_add(clip.duration())
                    .ok_or(Invalid("clip end overflow"))?;
                let start = range.start.max(clip.start());
                let end = range.end.min(clip_end);
                if start >= end {
                    continue;
                }
                let mut fragment = clip.clone();
                if start > fragment.start() {
                    fragment = crate::edit::split_clip_at_tempo(
                        &fragment,
                        start,
                        fragment.id(),
                        self.tempo_map(),
                    )?
                    .1;
                }
                if end < clip_end {
                    fragment = crate::edit::split_clip_at_tempo(
                        &fragment,
                        end,
                        fragment.id(),
                        self.tempo_map(),
                    )?
                    .0;
                }
                fragment.start = start - range.start;
                for id in clip.referenced_assets() {
                    if !result.assets.iter().any(|(asset, _)| *asset == id) {
                        let asset = self
                            .asset(id)
                            .ok_or(Invalid("selected media no longer exists"))?;
                        reserve(&mut result.assets, 1)?;
                        result.assets.push((id, asset.clone()));
                    }
                }
                reserve(&mut result.audio, 1)?;
                result.audio.push((row, fragment));
            }
            for clip in track.midi_clips() {
                let clip_end = clip
                    .start()
                    .raw()
                    .checked_add(clip.length().raw())
                    .ok_or(Invalid("midi clip end overflow"))?;
                let clip_start_frame = self.tempo_map().beat_to_frame(clip.start())?;
                let clip_end_frame = self
                    .tempo_map()
                    .beat_to_frame(BeatTime::from_raw(clip_end))?;
                let start = range.start.max(clip_start_frame);
                let end = range.end.min(clip_end_frame);
                if start >= end {
                    continue;
                }
                let mut notes = Vec::new();
                let playback = clip.source_playback()?;
                for note in clip.notes() {
                    let occurrences = playback.note_occurrences(note.start(), note.duration())?;
                    let count = usize::try_from(occurrences.count())
                        .map_err(|_| Invalid("midi range expansion overflow"))?;
                    expanded = expanded
                        .checked_add(count)
                        .ok_or(Invalid("midi range expansion overflow"))?;
                    if expanded > MAX_NOTES {
                        return Err(Invalid("midi range expansion exceeds note limit"));
                    }
                    for occurrence in occurrences.iter() {
                        let onset = clip
                            .start()
                            .raw()
                            .checked_add(occurrence.timeline_start().raw())
                            .ok_or(Invalid("note onset overflow"))?;
                        let gate_end = onset
                            .checked_add(occurrence.gate_duration().raw())
                            .ok_or(Invalid("note gate overflow"))?
                            .min(clip_end);
                        let note_start = self
                            .tempo_map()
                            .beat_to_frame(BeatTime::from_raw(onset))?
                            .max(start);
                        let note_end = self
                            .tempo_map()
                            .beat_to_frame(BeatTime::from_raw(gate_end))?
                            .min(end);
                        if note_start < note_end {
                            reserve(&mut notes, 1)?;
                            notes.push(FrameNote {
                                active: note.active(),
                                start: note_start - start,
                                duration: note_end - note_start,
                                pitch: note.pitch(),
                                channel: note.channel(),
                                velocity: note.velocity(),
                            });
                        }
                    }
                }
                reserve(&mut result.midi, 1)?;
                result.midi.push(MidiFragment {
                    active: clip.active,
                    volume: midi_volume::FrameVolume::capture(clip, self.tempo_map(), start, end)?,
                    name: clip.name().into(),
                    flavor: clip.flavor,
                    row,
                    start: start - range.start,
                    duration: end - start,
                    notes,
                });
            }
        }
        if result.audio.len().saturating_add(result.midi.len()) > MAX_CLIPS {
            return Err(Invalid("time selection exceeds clip limit"));
        }
        Ok(result)
    }
}

pub(crate) fn clear(project: &mut Project, range: &TimeRange) -> Result<Created, Invalid> {
    range.validate(project)?;
    clear_rows(project, &range.tracks, range.start, range.end)?;
    Ok(Created::None)
}

fn clear_rows(
    project: &mut Project,
    rows: &[TrackId],
    start: u64,
    end: u64,
) -> Result<(), Invalid> {
    project.refuse_automation_transform(rows)?;
    let mut cuts = Vec::new();
    reserve(&mut cuts, rows.len())?;
    cuts.extend(rows.iter().map(|track| (*track, start, end)));
    crate::clip_group::place_batch(project, Vec::new(), &cuts)?;
    Ok(())
}

pub(crate) fn paste(
    project: &mut Project,
    clipboard: &RangeClipboard,
    track: TrackId,
    frame: u64,
) -> Result<Created, Invalid> {
    let rows = adjacent_rows(project, clipboard, track, "paste")?;
    paste_rows(project, clipboard, &rows, frame)
}

/// Keyboard clipboard paste keeps multi-track rows attached to their source IDs.
/// A single-row clipboard remains free to paste at the requested current track.
pub(crate) fn paste_on_source_tracks(
    project: &mut Project,
    clipboard: &RangeClipboard,
    track: TrackId,
    frame: u64,
) -> Result<Created, Invalid> {
    let rows = source_rows_or_single_target(project, clipboard, track, "paste")?;
    paste_rows(project, clipboard, &rows, frame)
}

pub(crate) fn adjacent_rows(
    project: &Project,
    clipboard: &RangeClipboard,
    track: TrackId,
    operation: &str,
) -> Result<Vec<TrackId>, Invalid> {
    let first = project
        .tracks()
        .iter()
        .position(|item| item.id() == track)
        .ok_or(Invalid(if operation == "insert" {
            "insert track no longer exists"
        } else {
            "paste track no longer exists"
        }))?;
    let last = first
        .checked_add(clipboard.rows)
        .ok_or(Invalid(if operation == "insert" {
            "insert track range overflow"
        } else {
            "paste track range overflow"
        }))?;
    project
        .tracks()
        .get(first..last)
        .ok_or(Invalid(if operation == "insert" {
            "insert needs more existing tracks"
        } else {
            "paste needs more existing tracks"
        }))
        .map(|rows| rows.iter().map(|item| item.id()).collect())
}

pub(crate) fn source_rows_or_single_target(
    project: &Project,
    clipboard: &RangeClipboard,
    track: TrackId,
    operation: &str,
) -> Result<Vec<TrackId>, Invalid> {
    if clipboard.rows == 1 {
        return adjacent_rows(project, clipboard, track, operation);
    }
    if clipboard.source_tracks.len() != clipboard.rows {
        return Err(Invalid("copied source track mapping is invalid"));
    }
    for source in &clipboard.source_tracks {
        if project.track(*source).is_none() {
            return Err(Invalid("copied source track no longer exists"));
        }
    }
    Ok(clipboard.source_tracks.clone())
}

pub(crate) fn paste_rows(
    project: &mut Project,
    clipboard: &RangeClipboard,
    rows: &[TrackId],
    frame: u64,
) -> Result<Created, Invalid> {
    paste_rows_with_source(project, clipboard, rows, frame, None)
}

fn paste_rows_with_source(
    project: &mut Project,
    clipboard: &RangeClipboard,
    rows: &[TrackId],
    frame: u64,
    source: Option<&TimeRange>,
) -> Result<Created, Invalid> {
    if project.sample_rate() != clipboard.sample_rate {
        return Err(Invalid("time clipboard belongs to another sample rate"));
    }
    for (id, expected) in &clipboard.assets {
        if project.asset(*id) != Some(expected) {
            return Err(Invalid("copied media no longer matches the project"));
        }
    }
    if rows.len() != clipboard.rows || rows.iter().any(|track| project.track(*track).is_none()) {
        return Err(Invalid("paste destination track mapping is invalid"));
    }
    let end = frame
        .checked_add(clipboard.duration)
        .ok_or(Invalid("paste end overflow"))?;
    // Admit the complete rectangle, including empty cells, in one final-row plan.
    project.refuse_automation_transform(rows)?;
    let mut cuts = Vec::new();
    reserve(&mut cuts, rows.len())?;
    cuts.extend(rows.iter().map(|track| (*track, frame, end)));
    if let Some(source) = source {
        project.refuse_automation_transform(&source.tracks)?;
        reserve(&mut cuts, source.tracks.len())?;
        cuts.extend(
            source
                .tracks
                .iter()
                .map(|track| (*track, source.start, source.end)),
        );
    }
    let mut incoming = Vec::new();
    reserve(
        &mut incoming,
        clipboard.audio.len().saturating_add(clipboard.midi.len()),
    )?;
    for (row, original) in &clipboard.audio {
        let destination = *rows
            .get(*row)
            .ok_or(Invalid("copied track offset is invalid"))?;
        let mut clip = original.clone();
        clip.id = ClipId(0);
        clip.start = frame
            .checked_add(original.start())
            .ok_or(Invalid("paste clip start overflow"))?;
        let target_frame = clip.start;
        crate::audio_edit::move_clip(&mut clip, target_frame, project.tempo_map())?;
        incoming.push(crate::ClipGroupPreview {
            track: destination,
            content: crate::ClipGroupContent::Audio(clip),
        });
    }
    for fragment in &clipboard.midi {
        let destination = *rows
            .get(fragment.row)
            .ok_or(Invalid("copied track offset is invalid"))?;
        let start_frame = frame
            .checked_add(fragment.start)
            .ok_or(Invalid("paste midi start overflow"))?;
        let end_frame = start_frame
            .checked_add(fragment.duration)
            .ok_or(Invalid("paste midi end overflow"))?;
        let start = project.tempo_map().frame_to_beat(start_frame)?;
        let end = project.tempo_map().frame_to_beat(end_frame)?;
        let length = end
            .raw()
            .checked_sub(start.raw())
            .filter(|n| *n > 0)
            .map(BeatTime::from_raw)
            .ok_or(Invalid("pasted midi interval collapsed"))?;
        let mut notes = Vec::new();
        reserve(&mut notes, fragment.notes.len())?;
        for original in &fragment.notes {
            let onset_frame = start_frame
                .checked_add(original.start)
                .ok_or(Invalid("paste note onset overflow"))?;
            let gate_frame = onset_frame
                .checked_add(original.duration)
                .ok_or(Invalid("paste note gate overflow"))?;
            let onset = project.tempo_map().frame_to_beat(onset_frame)?;
            let gate = project.tempo_map().frame_to_beat(gate_frame)?;
            let duration = gate
                .raw()
                .checked_sub(onset.raw())
                .filter(|n| *n > 0)
                .map(BeatTime::from_raw)
                .ok_or(Invalid("pasted note gate collapsed"))?;
            let note_start = onset
                .raw()
                .checked_sub(start.raw())
                .map(BeatTime::from_raw)
                .ok_or(Invalid("pasted note is before its clip"))?;
            notes.push(MidiNote {
                id: NoteId(0),
                active: original.active,
                start: note_start,
                duration,
                pitch: original.pitch,
                channel: original.channel,
                velocity: original.velocity,
            });
        }
        let source = MidiSource {
            content: std::sync::Arc::new(crate::MidiContent {
                source_length: length,
                notes,
            }),
            one_shot_boundary: crate::MidiOneShotBoundary::SourceEnd,
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: length,
            looping: false,
        };
        let clip = MidiClip {
            content_view: None,
            active: fragment.active,
            playback_offset: crate::BeatTime::ZERO,
            volume: fragment
                .volume
                .paste(project.tempo_map(), start_frame, end_frame)?,
            name: fragment.name.clone(),
            flavor: fragment.flavor,
            id: ClipId(0),
            start,
            length,
            source: Arc::new(source),
        };
        incoming.push(crate::ClipGroupPreview {
            track: destination,
            content: crate::ClipGroupContent::Midi(clip),
        });
    }
    crate::clip_group::place_batch(project, incoming, &cuts)?;
    // Global note/ID/source validation includes the independent Session inventory.
    Project::from_snapshot(project.snapshot())?;
    Ok(Created::None)
}

pub(crate) fn move_range(
    project: &mut Project,
    range: &TimeRange,
    track: TrackId,
    frame: u64,
) -> Result<Created, Invalid> {
    range.validate(project)?;
    if range.tracks.first() == Some(&track) && range.start == frame {
        return Ok(Created::None);
    }
    let clipboard = project.capture_time_range(range)?;
    let rows = adjacent_rows(project, &clipboard, track, "paste")?;
    paste_rows_with_source(project, &clipboard, &rows, frame, Some(range))
}

pub(crate) fn copy_range(
    project: &mut Project,
    range: &TimeRange,
    track: TrackId,
    frame: u64,
) -> Result<Created, Invalid> {
    range.validate(project)?;
    if range.tracks.first() == Some(&track) && range.start == frame {
        return Ok(Created::None);
    }
    let clipboard = project.capture_time_range(range)?;
    paste(project, &clipboard, track, frame)
}

/// Reuse canonical ripple insertion for a complete clip clipboard's projected span.
pub(crate) fn insert_empty(
    project: &mut Project,
    tracks: Vec<TrackId>,
    frame: u64,
    duration: u64,
) -> Result<Created, Invalid> {
    let first = tracks
        .first()
        .copied()
        .ok_or(Invalid("clip clipboard has no source tracks"))?;
    let clipboard = RangeClipboard {
        sample_rate: project.sample_rate(),
        duration,
        rows: tracks.len(),
        source_tracks: tracks,
        assets: Vec::new(),
        audio: Vec::new(),
        midi: Vec::new(),
    };
    insert_on_source_tracks(project, &clipboard, first, frame)
}
