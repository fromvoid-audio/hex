use crate::{
    Asset, AssetId, Clip, ClipFade, ClipId, IDENTITY_CONVERSION, Invalid, LoopRegion, MAX_ASSETS,
    MAX_CLIPS, MAX_NAME_BYTES, MAX_TRACKS, MeterPoint, NoteId, Project, TempoPoint, Track,
    TrackContent, TrackId, TrimEdge, converted_frame_count, validate_conversion_metadata,
};
use std::path::PathBuf;

/// Validated edits on the control plane. One successful command is one undo step.
#[derive(Debug, Clone)]
pub enum Command {
    AddTrack {
        name: String,
    },
    /// Atomically create one track, its source asset and a full-length one-shot clip.
    ImportAudioTrack {
        name: String,
        path: PathBuf,
        sample_rate: u32,
        frames: u64,
    },
    /// Atomically import original source metadata and project-rate prepared PCM.
    ImportPreparedAudioTrack {
        name: String,
        path: PathBuf,
        source_sample_rate: u32,
        source_frames: u64,
        playback_frames: u64,
        conversion_version: u32,
    },
    /// Atomically import prepared media and place a full-length clip on an existing hybrid track.
    ImportPreparedAudioClip {
        track: TrackId,
        start: u64,
        path: PathBuf,
        source_sample_rate: u32,
        source_frames: u64,
        playback_frames: u64,
        conversion_version: u32,
    },
    /// Atomically add prepared media and place a one-shot source in a Session cell.
    ImportPreparedSessionAudioClip {
        track: TrackId,
        scene: crate::SceneId,
        name: String,
        path: PathBuf,
        source_sample_rate: u32,
        source_frames: u64,
        playback_frames: u64,
        conversion_version: u32,
        source_start: u64,
        source_end: u64,
    },
    RemoveTrack(TrackId),
    RenameTrack {
        track: TrackId,
        name: String,
    },
    Gain {
        track: TrackId,
        gain: f32,
    },
    /// Set original Hex stereo balance; this is not a measured Live pan law.
    Balance {
        track: TrackId,
        balance: f32,
    },
    MasterGain {
        gain: f32,
    },
    SetTempoPoint {
        beat: crate::BeatTime,
        tempo: crate::Tempo,
    },
    SetMeterPoint {
        beat: crate::BeatTime,
        meter: crate::Meter,
    },
    Mute {
        track: TrackId,
        muted: bool,
    },
    /// Toggle track solo. Exclusive solo clears other solos only when enabling.
    ToggleSolo {
        track: TrackId,
        exclusive: bool,
    },
    SetLoopRegion {
        start: u64,
        end: u64,
        enabled: bool,
    },
    ToggleLoopRegion,
    ClearLoopRegion,
    /// Add source metadata for PCM that is already at the project rate.
    AddAsset {
        path: PathBuf,
        sample_rate: u32,
        frames: u64,
    },
    RemoveAsset(AssetId),
    PlaceClip {
        track: TrackId,
        asset: AssetId,
        start: u64,
        duration: u64,
        source_offset: u64,
        source_length: u64,
        looping: bool,
    },
    /// Place a new clip after the selected clip using its same media interval.
    DuplicateClip(ClipId),
    /// Split a clip at an absolute timeline frame.
    SplitClip {
        clip: ClipId,
        frame: u64,
    },
    /// Trim an absolute timeline edge, clamping to the nearest valid clip region.
    TrimClipEdge {
        clip: ClipId,
        edge: TrimEdge,
        frame: u64,
    },
    MoveClip {
        clip: ClipId,
        track: TrackId,
        start: u64,
    },
    /// Explicit source interval; changing the source interval resets loop phase to zero.
    TrimClip {
        clip: ClipId,
        start: u64,
        duration: u64,
        source_offset: u64,
        source_length: u64,
    },
    /// Set linear fade lengths and reanchor them to the clip's current duration.
    SetClipFades {
        clip: ClipId,
        fade_in: u64,
        fade_out: u64,
    },
    RemoveClip(ClipId),
    /// Relative commands are resolved against canonical state, so queued repeats
    /// do not overwrite one another using an outdated UI snapshot.
    NudgeClip {
        clip: ClipId,
        frames: i64,
    },
    TrimClipEndBy {
        clip: ClipId,
        frames: i64,
    },
    GainBy {
        track: TrackId,
        amount: f32,
    },
    BalanceBy {
        track: TrackId,
        amount: f32,
    },
    MasterGainBy {
        amount: f32,
    },
    ToggleMute(TrackId),
    AddMidiTrack {
        name: String,
    },
    AddScene {
        name: String,
    },
    /// Copy a scene immediately after it with fresh IDs for copied MIDI content.
    DuplicateScene(crate::SceneId),
    /// Reorder one scene before a stable target, or append it when `before` is `None`.
    MoveScene {
        scene: crate::SceneId,
        before: Option<crate::SceneId>,
    },
    RenameScene {
        scene: crate::SceneId,
        name: String,
    },
    RemoveScene(crate::SceneId),
    CreateSessionMidiClip {
        track: TrackId,
        scene: crate::SceneId,
        name: String,
        source_length: crate::BeatTime,
    },
    CreateSessionAudioClip {
        track: TrackId,
        scene: crate::SceneId,
        name: String,
        asset: AssetId,
        source_start: u64,
        source_end: u64,
    },
    CopyArrangementAudioClipToSession {
        source: ClipId,
        scene: crate::SceneId,
    },
    CopyArrangementMidiToSession {
        source: ClipId,
        scene: crate::SceneId,
    },
    MoveSessionCell {
        from_track: TrackId,
        from_scene: crate::SceneId,
        to_track: TrackId,
        to_scene: crate::SceneId,
    },
    DuplicateSessionMidiCell {
        track: TrackId,
        from_scene: crate::SceneId,
        to_scene: crate::SceneId,
    },
    DuplicateSessionAudioCell {
        track: TrackId,
        from_scene: crate::SceneId,
        to_scene: crate::SceneId,
    },
    SetSessionAudioClipSource {
        clip: ClipId,
        asset: AssetId,
        source_start: u64,
        source_end: u64,
    },
    ClearSessionCell {
        track: TrackId,
        scene: crate::SceneId,
    },
    /// Explicitly replaces any existing cell content with a stop marker.
    SetSessionStopCell {
        track: TrackId,
        scene: crate::SceneId,
    },
    SetSessionMidiClipSource {
        clip: ClipId,
        source_length: crate::BeatTime,
        source_start: crate::BeatTime,
        loop_start: crate::BeatTime,
        loop_end: crate::BeatTime,
        looping: bool,
    },
    RenameSessionMidiClip {
        clip: ClipId,
        name: String,
    },
    PlaceMidiClip {
        track: TrackId,
        start: crate::BeatTime,
        length: crate::BeatTime,
    },
    /// Atomically replace the finite source and arrangement region for a MIDI clip.
    SetMidiClipSource {
        clip: ClipId,
        source_length: crate::BeatTime,
        source_start: crate::BeatTime,
        loop_start: crate::BeatTime,
        loop_end: crate::BeatTime,
        looping: bool,
        length: crate::BeatTime,
    },
    AddMidiNote {
        clip: ClipId,
        start: crate::BeatTime,
        duration: crate::BeatTime,
        pitch: u8,
        channel: u8,
        velocity: u8,
    },
    /// Atomically append several notes to one MIDI clip as one history step.
    /// Identical musical values remain distinct notes, as with `AddMidiNote`.
    AddMidiNotes {
        clip: ClipId,
        notes: Vec<MidiNoteInsert>,
    },
    /// Atomically remove and insert notes in one MIDI clip as one history step.
    EditMidiNotes {
        clip: ClipId,
        remove: Vec<NoteId>,
        insert: Vec<MidiNoteInsert>,
    },
    UpdateMidiNote {
        note: NoteId,
        start: crate::BeatTime,
        duration: crate::BeatTime,
        pitch: u8,
        channel: u8,
        velocity: u8,
    },
    /// Atomically update several notes in one MIDI clip as one history step.
    UpdateMidiNotes {
        clip: ClipId,
        notes: Vec<MidiNoteUpdate>,
    },
    RemoveMidiNote(NoteId),
    /// Atomically remove several notes in one MIDI clip as one history step.
    RemoveMidiNotes {
        clip: ClipId,
        notes: Vec<NoteId>,
    },
    MoveMidiClip {
        clip: ClipId,
        track: TrackId,
        start: crate::BeatTime,
    },
    RemoveMidiClip(ClipId),
    DuplicateMidiClip(ClipId),
}

/// Complete canonical field replacement for one existing MIDI note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiNoteUpdate {
    pub note: NoteId,
    pub start: crate::BeatTime,
    pub duration: crate::BeatTime,
    pub pitch: u8,
    pub channel: u8,
    pub velocity: u8,
}

/// Musical fields for one newly inserted MIDI note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiNoteInsert {
    pub start: crate::BeatTime,
    pub duration: crate::BeatTime,
    pub pitch: u8,
    pub channel: u8,
    pub velocity: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Created {
    Track(TrackId),
    Asset(AssetId),
    Clip(ClipId),
    Note(NoteId),
    Scene(crate::SceneId),
    SessionClip(ClipId),
    /// Inclusive contiguous note-ID range created in `clip`; select IDs in the
    /// current clip whose values fall within this range.
    Notes {
        clip: ClipId,
        first: NoteId,
        last: NoteId,
    },
    Imported {
        track: TrackId,
        asset: AssetId,
        clip: ClipId,
    },
    None,
}

pub(crate) fn name_valid(name: &str) -> Result<(), Invalid> {
    if name.trim().is_empty() || name.len() > MAX_NAME_BYTES || name.chars().any(char::is_control) {
        Err(Invalid(
            "name must be nonempty, without control characters, and at most 1024 bytes",
        ))
    } else {
        Ok(())
    }
}
pub(crate) fn interval_valid(
    start: u64,
    duration: u64,
    offset: u64,
    length: u64,
    asset: &Asset,
) -> Result<(), Invalid> {
    if duration == 0 || length == 0 {
        return Err(Invalid("clip and source lengths must be positive"));
    }
    start
        .checked_add(duration)
        .ok_or(Invalid("timeline end overflow"))?;
    let end = offset
        .checked_add(length)
        .ok_or(Invalid("source end overflow"))?;
    if end > asset.playback_frames {
        return Err(Invalid("clip source interval exceeds media"));
    }
    Ok(())
}

pub(crate) fn preview_edge_trim(
    clip: &Clip,
    edge: TrimEdge,
    frame: u64,
    asset_playback_frames: u64,
) -> Result<Clip, Invalid> {
    if clip.duration == 0 || clip.source_length == 0 {
        return Err(Invalid("clip and source lengths must be positive"));
    }
    let timeline_end = clip
        .start
        .checked_add(clip.duration)
        .ok_or(Invalid("timeline end overflow"))?;
    let source_end = clip
        .source_offset
        .checked_add(clip.source_length)
        .ok_or(Invalid("source end overflow"))?;
    if source_end > asset_playback_frames {
        return Err(Invalid("clip source interval exceeds media"));
    }
    if (clip.looping && clip.source_phase >= clip.source_length)
        || (!clip.looping && clip.source_phase != 0)
    {
        return Err(Invalid("clip source phase is outside its source interval"));
    }

    let end = timeline_end;
    let (start, duration, source_offset, source_length, source_phase) = match (clip.looping, edge) {
        (true, TrimEdge::Left) => {
            let new_start = frame.clamp(0, end - 1);
            let phase = if new_start >= clip.start {
                (u128::from(clip.source_phase)
                    + u128::from(new_start - clip.start) % u128::from(clip.source_length))
                    % u128::from(clip.source_length)
            } else {
                let delta = u128::from(clip.start - new_start) % u128::from(clip.source_length);
                (u128::from(clip.source_phase) + u128::from(clip.source_length) - delta)
                    % u128::from(clip.source_length)
            } as u64;
            (
                new_start,
                end - new_start,
                clip.source_offset,
                clip.source_length,
                phase,
            )
        }
        (true, TrimEdge::Right) => {
            let new_end = frame.clamp(clip.start + 1, u64::MAX);
            (
                clip.start,
                new_end - clip.start,
                clip.source_offset,
                clip.source_length,
                clip.source_phase,
            )
        }
        (false, TrimEdge::Left) => {
            let min_start = clip.start.saturating_sub(clip.source_offset);
            let max_advance = clip
                .duration
                .saturating_sub(1)
                .min(clip.source_length.saturating_sub(1));
            let max_start = clip
                .start
                .checked_add(max_advance)
                .ok_or(Invalid("timeline end overflow"))?;
            let new_start = frame.clamp(min_start, max_start);
            if new_start >= clip.start {
                let advance = new_start - clip.start;
                (
                    new_start,
                    clip.duration - advance,
                    clip.source_offset + advance,
                    clip.source_length - advance,
                    0,
                )
            } else {
                let extension = clip.start - new_start;
                (
                    new_start,
                    clip.duration
                        .checked_add(extension)
                        .ok_or(Invalid("timeline end overflow"))?,
                    clip.source_offset - extension,
                    clip.source_length
                        .checked_add(extension)
                        .ok_or(Invalid("source length overflow"))?,
                    0,
                )
            }
        }
        (false, TrimEdge::Right) => {
            let source_end = clip
                .source_offset
                .checked_add(clip.source_length)
                .ok_or(Invalid("source end overflow"))?;
            let media_remaining = asset_playback_frames - source_end;
            let timeline_remaining = u64::MAX - end;
            let extension = media_remaining.min(timeline_remaining);
            let minimum_reduction = clip
                .duration
                .saturating_sub(1)
                .min(clip.source_length.saturating_sub(1));
            let minimum_end = end - minimum_reduction;
            let maximum_end = end + extension;
            let new_end = frame.clamp(minimum_end, maximum_end);
            if new_end >= end {
                let extension = new_end - end;
                (
                    clip.start,
                    clip.duration + extension,
                    clip.source_offset,
                    clip.source_length + extension,
                    0,
                )
            } else {
                let reduction = end - new_end;
                (
                    clip.start,
                    clip.duration - reduction,
                    clip.source_offset,
                    clip.source_length - reduction,
                    0,
                )
            }
        }
    };

    let changed = start != clip.start
        || duration != clip.duration
        || source_offset != clip.source_offset
        || source_length != clip.source_length
        || source_phase != clip.source_phase;
    let fade = if changed {
        ClipFade::new(
            duration,
            0,
            clip.fade.fade_in().min(duration),
            clip.fade.fade_out().min(duration),
        )?
    } else {
        clip.fade
    };
    Ok(Clip {
        id: clip.id,
        asset: clip.asset,
        start,
        duration,
        source_offset,
        source_length,
        looping: clip.looping,
        source_phase,
        fade,
    })
}
impl Project {
    /// Called only on an isolated candidate by Editor; never on an audio thread.
    pub(crate) fn apply(&mut self, command: Command) -> Result<Created, Invalid> {
        match command {
            midi_command @ (Command::AddMidiTrack { .. }
            | Command::PlaceMidiClip { .. }
            | Command::SetMidiClipSource { .. }
            | Command::AddMidiNote { .. }
            | Command::AddMidiNotes { .. }
            | Command::EditMidiNotes { .. }
            | Command::UpdateMidiNote { .. }
            | Command::UpdateMidiNotes { .. }
            | Command::RemoveMidiNote(_)
            | Command::RemoveMidiNotes { .. }
            | Command::MoveMidiClip { .. }
            | Command::RemoveMidiClip(_)
            | Command::DuplicateMidiClip(_)) => crate::midi::apply(self, midi_command),
            session_command @ (Command::AddScene { .. }
            | Command::DuplicateScene(_)
            | Command::MoveScene { .. }
            | Command::RenameScene { .. }
            | Command::RemoveScene(_)
            | Command::CreateSessionMidiClip { .. }
            | Command::CreateSessionAudioClip { .. }
            | Command::CopyArrangementAudioClipToSession { .. }
            | Command::CopyArrangementMidiToSession { .. }
            | Command::MoveSessionCell { .. }
            | Command::DuplicateSessionMidiCell { .. }
            | Command::DuplicateSessionAudioCell { .. }
            | Command::SetSessionAudioClipSource { .. }
            | Command::ClearSessionCell { .. }
            | Command::SetSessionStopCell { .. }
            | Command::SetSessionMidiClipSource { .. }
            | Command::RenameSessionMidiClip { .. }) => {
                crate::session::apply(self, session_command)
            }
            Command::DuplicateClip(clip) => {
                let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                let start = original
                    .start
                    .checked_add(original.duration)
                    .ok_or(Invalid("timeline end overflow"))?;
                let track = self
                    .tracks
                    .iter()
                    .find(|track| track.clips().iter().any(|item| item.id == clip))
                    .ok_or(Invalid("clip track missing"))?
                    .id;
                let fade = original.fade;
                let source_phase = original.source_phase;
                let duplicate = self.apply(Command::PlaceClip {
                    track,
                    asset: original.asset,
                    start,
                    duration: original.duration,
                    source_offset: original.source_offset,
                    source_length: original.source_length,
                    looping: original.looping,
                })?;
                if let Created::Clip(duplicate_id) = duplicate {
                    self.clip_mut(duplicate_id)?.fade = fade;
                    self.clip_mut(duplicate_id)?.source_phase = source_phase;
                }
                Ok(duplicate)
            }
            Command::SplitClip { clip, frame } => {
                let (track_index, clip_index) = self
                    .tracks
                    .iter()
                    .enumerate()
                    .find_map(|(track_index, track)| {
                        track
                            .clips()
                            .iter()
                            .position(|item| item.id == clip)
                            .map(|clip_index| (track_index, clip_index))
                    })
                    .ok_or(Invalid("clip no longer exists"))?;
                let original = self.tracks[track_index].clips()[clip_index].clone();
                let end = original
                    .start
                    .checked_add(original.duration)
                    .ok_or(Invalid("timeline end overflow"))?;
                if frame <= original.start || frame >= end {
                    return Err(Invalid("split cursor must be inside the clip"));
                }
                let left_length = frame - original.start;
                if !original.looping && left_length >= original.source_length {
                    return Err(Invalid("split cursor is beyond the one-shot source"));
                }
                if self
                    .tracks
                    .iter()
                    .map(|track| track.clips().len() + track.midi_clips().len())
                    .sum::<usize>()
                    >= MAX_CLIPS
                {
                    return Err(Invalid("clip limit reached"));
                }
                let (right_source_offset, right_source_length, right_phase) = if original.looping {
                    (
                        original.source_offset,
                        original.source_length,
                        ((u128::from(original.source_phase) + u128::from(left_length))
                            % u128::from(original.source_length)) as u64,
                    )
                } else {
                    (
                        original
                            .source_offset
                            .checked_add(left_length)
                            .ok_or(Invalid("source offset overflow"))?,
                        original
                            .source_length
                            .checked_sub(left_length)
                            .ok_or(Invalid("source length underflow"))?,
                        0,
                    )
                };
                let right_duration = end - frame;
                let right_fade = original.fade.with_offset(
                    original
                        .fade
                        .offset()
                        .checked_add(left_length)
                        .ok_or(Invalid("fade offset overflow"))?,
                    right_duration,
                )?;
                self.audio_clips_mut(self.tracks[track_index].id)?
                    .try_reserve(1)
                    .map_err(|_| Invalid("could not reserve clip storage"))?;
                let right_id = ClipId(self.allocate_id()?);
                let right = Clip {
                    id: right_id,
                    asset: original.asset,
                    start: frame,
                    duration: right_duration,
                    source_offset: right_source_offset,
                    source_length: right_source_length,
                    looping: original.looping,
                    source_phase: right_phase,
                    fade: right_fade,
                };
                let left = &mut self.audio_clips_mut(self.tracks[track_index].id)?[clip_index];
                left.duration = left_length;
                if !left.looping {
                    left.source_length = left_length;
                    left.source_phase = 0;
                }
                self.audio_clips_mut(self.tracks[track_index].id)?
                    .insert(clip_index + 1, right);
                Ok(Created::Clip(right_id))
            }
            Command::NudgeClip { clip, frames } => {
                let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                let start = if frames < 0 {
                    original.start.saturating_sub(frames.unsigned_abs())
                } else {
                    original
                        .start
                        .checked_add(frames as u64)
                        .ok_or(Invalid("timeline start overflow"))?
                };
                let track = self
                    .tracks
                    .iter()
                    .find(|track| track.clips().iter().any(|item| item.id == clip))
                    .ok_or(Invalid("clip track missing"))?
                    .id;
                self.apply(Command::MoveClip { clip, track, start })
            }
            Command::TrimClipEndBy { clip, frames } => {
                let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                if original.looping {
                    let end = original
                        .start
                        .checked_add(original.duration)
                        .ok_or(Invalid("timeline end overflow"))?;
                    let desired = if frames < 0 {
                        end.saturating_sub(frames.unsigned_abs())
                    } else {
                        end.saturating_add(frames as u64)
                    };
                    return self.apply(Command::TrimClipEdge {
                        clip,
                        edge: TrimEdge::Right,
                        frame: desired,
                    });
                }
                let adjust = |value: u64| {
                    if frames < 0 {
                        value.checked_sub(frames.unsigned_abs())
                    } else {
                        value.checked_add(frames as u64)
                    }
                };
                let duration = adjust(original.duration).ok_or(Invalid("trim length overflow"))?;
                let source_length =
                    adjust(original.source_length).ok_or(Invalid("trim source length overflow"))?;
                self.apply(Command::TrimClip {
                    clip,
                    start: original.start,
                    duration,
                    source_offset: original.source_offset,
                    source_length,
                })
            }
            Command::GainBy { track, amount } => {
                if !amount.is_finite() {
                    return Err(Invalid("gain adjustment must be finite"));
                }
                let gain = (self
                    .track(track)
                    .ok_or(Invalid("track no longer exists"))?
                    .gain
                    + amount)
                    .clamp(0.0, 2.0);
                self.apply(Command::Gain { track, gain })
            }
            Command::BalanceBy { track, amount } => {
                if !amount.is_finite() {
                    return Err(Invalid("balance adjustment must be finite"));
                }
                let balance = (self
                    .track(track)
                    .ok_or(Invalid("track no longer exists"))?
                    .balance
                    + amount)
                    .clamp(-1.0, 1.0);
                self.apply(Command::Balance { track, balance })
            }
            Command::MasterGainBy { amount } => {
                if !amount.is_finite() {
                    return Err(Invalid("master gain adjustment must be finite"));
                }
                let gain = (self.master_gain + amount).clamp(0.0, 2.0);
                self.apply(Command::MasterGain { gain })
            }
            Command::ToggleMute(track) => {
                let muted = !self
                    .track(track)
                    .ok_or(Invalid("track no longer exists"))?
                    .muted;
                self.apply(Command::Mute { track, muted })
            }
            Command::AddTrack { name } => {
                name_valid(&name)?;
                if self.tracks.len() >= MAX_TRACKS {
                    return Err(Invalid("track limit reached"));
                }
                let id = TrackId(self.allocate_id()?);
                self.tracks.push(Track {
                    id,
                    name,
                    gain: 1.0,
                    balance: 0.0,
                    muted: false,
                    soloed: false,
                    content: TrackContent::default(),
                });
                Ok(Created::Track(id))
            }
            Command::ImportAudioTrack {
                name,
                path,
                sample_rate,
                frames,
            } => self.apply(Command::ImportPreparedAudioTrack {
                name,
                path,
                source_sample_rate: sample_rate,
                source_frames: frames,
                playback_frames: frames,
                conversion_version: IDENTITY_CONVERSION,
            }),
            Command::ImportPreparedAudioTrack {
                name,
                path,
                source_sample_rate,
                source_frames,
                playback_frames,
                conversion_version,
            } => {
                validate_conversion_metadata(
                    source_frames,
                    source_sample_rate,
                    playback_frames,
                    self.sample_rate,
                    conversion_version,
                )?;
                let track = match self.apply(Command::AddTrack { name })? {
                    Created::Track(id) => id,
                    _ => return Err(Invalid("audio import did not create its track")),
                };
                let asset = self.add_prepared_asset(
                    path,
                    source_sample_rate,
                    source_frames,
                    playback_frames,
                    conversion_version,
                )?;
                let clip = match self.apply(Command::PlaceClip {
                    track,
                    asset,
                    start: 0,
                    duration: playback_frames,
                    source_offset: 0,
                    source_length: playback_frames,
                    looping: false,
                })? {
                    Created::Clip(id) => id,
                    _ => return Err(Invalid("audio import did not create its clip")),
                };
                Ok(Created::Imported { track, asset, clip })
            }
            Command::ImportPreparedAudioClip {
                track,
                start,
                path,
                source_sample_rate,
                source_frames,
                playback_frames,
                conversion_version,
            } => {
                self.track(track).ok_or(Invalid("track no longer exists"))?;
                validate_conversion_metadata(
                    source_frames,
                    source_sample_rate,
                    playback_frames,
                    self.sample_rate,
                    conversion_version,
                )?;
                let asset = self.add_prepared_asset(
                    path,
                    source_sample_rate,
                    source_frames,
                    playback_frames,
                    conversion_version,
                )?;
                let clip = match self.apply(Command::PlaceClip {
                    track,
                    asset,
                    start,
                    duration: playback_frames,
                    source_offset: 0,
                    source_length: playback_frames,
                    looping: false,
                })? {
                    Created::Clip(id) => id,
                    _ => return Err(Invalid("audio import did not create its clip")),
                };
                Ok(Created::Imported { track, asset, clip })
            }
            Command::ImportPreparedSessionAudioClip {
                track,
                scene,
                name,
                path,
                source_sample_rate,
                source_frames,
                playback_frames,
                conversion_version,
                source_start,
                source_end,
            } => {
                validate_conversion_metadata(
                    source_frames,
                    source_sample_rate,
                    playback_frames,
                    self.sample_rate,
                    conversion_version,
                )?;
                let asset = self.add_prepared_asset(
                    path,
                    source_sample_rate,
                    source_frames,
                    playback_frames,
                    conversion_version,
                )?;
                self.apply(Command::CreateSessionAudioClip {
                    track,
                    scene,
                    name,
                    asset,
                    source_start,
                    source_end,
                })
            }
            Command::RemoveTrack(id) => {
                let index = self
                    .tracks
                    .iter()
                    .position(|t| t.id == id)
                    .ok_or(Invalid("track no longer exists"))?;
                self.tracks.remove(index);
                self.session.cells.retain(|cell| cell.track != id);
                Ok(Created::None)
            }
            Command::RenameTrack { track, name } => {
                name_valid(&name)?;
                self.track_mut(track)?.name = name;
                Ok(Created::None)
            }
            Command::Gain { track, gain } => {
                if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
                    return Err(Invalid("gain must be finite and within 0..2"));
                }
                self.track_mut(track)?.gain = gain;
                Ok(Created::None)
            }
            Command::Balance { track, balance } => {
                if !balance.is_finite() || !(-1.0..=1.0).contains(&balance) {
                    return Err(Invalid("balance must be finite and within -1..1"));
                }
                self.track_mut(track)?.balance = balance;
                Ok(Created::None)
            }
            Command::MasterGain { gain } => {
                if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
                    return Err(Invalid("master gain must be finite and within 0..2"));
                }
                self.master_gain = gain;
                Ok(Created::None)
            }
            Command::SetTempoPoint { beat, tempo } => {
                self.tempo_map = self.tempo_map.replace_point(TempoPoint::new(beat, tempo))?;
                Ok(Created::None)
            }
            Command::SetMeterPoint { beat, meter } => {
                self.meter_map = self.meter_map.replace_point(MeterPoint::new(beat, meter))?;
                Ok(Created::None)
            }
            Command::Mute { track, muted } => {
                self.track_mut(track)?.muted = muted;
                Ok(Created::None)
            }
            Command::ToggleSolo { track, exclusive } => {
                let currently_soloed = self
                    .track(track)
                    .ok_or(Invalid("track no longer exists"))?
                    .soloed;
                if currently_soloed {
                    self.track_mut(track)?.soloed = false;
                } else {
                    if exclusive {
                        for item in &mut self.tracks {
                            item.soloed = false;
                        }
                    }
                    self.track_mut(track)?.soloed = true;
                }
                Ok(Created::None)
            }
            Command::SetLoopRegion {
                start,
                end,
                enabled,
            } => {
                self.loop_region = Some(LoopRegion::new(start, end, enabled)?);
                Ok(Created::None)
            }
            Command::ToggleLoopRegion => {
                let region = self.loop_region.ok_or(Invalid("no loop region is set"))?;
                self.loop_region = Some(LoopRegion::new(
                    region.start(),
                    region.end(),
                    !region.enabled(),
                )?);
                Ok(Created::None)
            }
            Command::ClearLoopRegion => {
                self.loop_region = None;
                Ok(Created::None)
            }
            Command::AddAsset {
                path,
                sample_rate,
                frames,
            } => {
                if sample_rate != self.sample_rate {
                    return Err(Invalid(
                        "media rate differs; prepared conversion is required",
                    ));
                }
                let frames = converted_frame_count(frames, sample_rate, self.sample_rate)?;
                self.add_prepared_asset(path, sample_rate, frames, frames, IDENTITY_CONVERSION)
                    .map(Created::Asset)
            }
            Command::RemoveAsset(id) => {
                if self
                    .tracks
                    .iter()
                    .flat_map(Track::clips)
                    .any(|c| c.asset == id)
                    || self.session.cells.iter().any(|cell| matches!(&cell.content, crate::SessionCellContent::Audio(clip) if clip.asset == id))
                {
                    return Err(Invalid("media is still referenced by a clip"));
                }
                self.assets
                    .remove(&id)
                    .ok_or(Invalid("media no longer exists"))?;
                Ok(Created::None)
            }
            Command::PlaceClip {
                track,
                asset,
                start,
                duration,
                source_offset,
                source_length,
                looping,
            } => {
                self.track(track).ok_or(Invalid("track no longer exists"))?;
                let media = self.asset(asset).ok_or(Invalid("media no longer exists"))?;
                interval_valid(start, duration, source_offset, source_length, media)?;
                if self
                    .tracks
                    .iter()
                    .map(|t| t.clips().len() + t.midi_clips().len())
                    .sum::<usize>()
                    >= MAX_CLIPS
                {
                    return Err(Invalid("clip limit reached"));
                }
                let id = ClipId(self.allocate_id()?);
                let fade = ClipFade::neutral(duration)?;
                self.audio_clips_mut(track)?.push(Clip {
                    id,
                    asset,
                    start,
                    duration,
                    source_offset,
                    source_length,
                    looping,
                    source_phase: 0,
                    fade,
                });
                Ok(Created::Clip(id))
            }
            Command::MoveClip { clip, track, start } => {
                let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                start
                    .checked_add(original.duration)
                    .ok_or(Invalid("timeline end overflow"))?;
                self.track(track)
                    .ok_or(Invalid("destination track no longer exists"))?;
                let source = self
                    .tracks
                    .iter()
                    .position(|t| t.clips().iter().any(|c| c.id == clip))
                    .ok_or(Invalid("clip no longer exists"))?;
                if self.tracks[source].id == track && original.start == start {
                    return Ok(Created::None);
                }
                let index = self.tracks[source]
                    .clips()
                    .iter()
                    .position(|c| c.id == clip)
                    .ok_or(Invalid("clip no longer exists"))?;
                let source_id = self.tracks[source].id;
                let mut moved = self.audio_clips_mut(source_id)?.remove(index);
                moved.start = start;
                self.audio_clips_mut(track)?.push(moved);
                Ok(Created::None)
            }
            Command::TrimClip {
                clip,
                start,
                duration,
                source_offset,
                source_length,
            } => {
                let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                let media = self
                    .asset(original.asset)
                    .ok_or(Invalid("media no longer exists"))?;
                interval_valid(start, duration, source_offset, source_length, media)?;
                let old_fade = original.fade;
                let old_source_offset = original.source_offset;
                let old_source_length = original.source_length;
                let fade = ClipFade::new(
                    duration,
                    0,
                    old_fade.fade_in().min(duration),
                    old_fade.fade_out().min(duration),
                )?;
                let target = self.clip_mut(clip)?;
                target.start = start;
                target.duration = duration;
                target.source_offset = source_offset;
                target.source_length = source_length;
                if source_offset != old_source_offset || source_length != old_source_length {
                    target.source_phase = 0;
                }
                target.fade = fade;
                Ok(Created::None)
            }
            Command::TrimClipEdge { clip, edge, frame } => {
                let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                let asset_frames = self
                    .asset(original.asset)
                    .ok_or(Invalid("media no longer exists"))?
                    .playback_frames;
                let trimmed = preview_edge_trim(original, edge, frame, asset_frames)?;
                *self.clip_mut(clip)? = trimmed;
                Ok(Created::None)
            }
            Command::SetClipFades {
                clip,
                fade_in,
                fade_out,
            } => {
                let duration = self
                    .clip(clip)
                    .ok_or(Invalid("clip no longer exists"))?
                    .duration;
                let fade = ClipFade::new(duration, 0, fade_in, fade_out)?;
                self.clip_mut(clip)?.fade = fade;
                Ok(Created::None)
            }
            Command::RemoveClip(id) => {
                for track in &mut self.tracks {
                    if let Some(index) = track.clips().iter().position(|c| c.id == id) {
                        track.content.audio_clips.remove(index);
                        return Ok(Created::None);
                    }
                }
                Err(Invalid("clip no longer exists"))
            }
        }
    }
}

impl Project {
    fn add_prepared_asset(
        &mut self,
        path: PathBuf,
        source_sample_rate: u32,
        source_frames: u64,
        playback_frames: u64,
        conversion_version: u32,
    ) -> Result<AssetId, Invalid> {
        if path.as_os_str().is_empty() {
            return Err(Invalid("media needs a path and frames"));
        }
        validate_conversion_metadata(
            source_frames,
            source_sample_rate,
            playback_frames,
            self.sample_rate,
            conversion_version,
        )?;
        if self.assets.len() >= MAX_ASSETS {
            return Err(Invalid("asset limit reached"));
        }
        let id = AssetId(self.allocate_id()?);
        self.assets.insert(
            id,
            Asset {
                id,
                path,
                source_sample_rate,
                source_frames,
                playback_frames,
                conversion_version,
            },
        );
        Ok(id)
    }
}
