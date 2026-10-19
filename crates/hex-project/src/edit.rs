use crate::{
    Asset, AssetId, Clip, ClipFade, ClipId, DeviceId, IDENTITY_CONVERSION, Invalid, LoopRegion,
    MAX_ASSETS, MAX_CLIPS, MAX_NAME_BYTES, MAX_TRACKS, MeterPoint, NoteId, Project, ReturnId,
    TempoPoint, Track, TrackContent, TrackId, TrimEdge, converted_frame_count,
    validate_conversion_metadata,
};
use std::{collections::HashMap, path::PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackDeviceSlot {
    Instrument,
    Effect,
}

/// Validated edits on the control plane. One successful command is one undo step.
#[derive(Debug, Clone)]
pub enum Command {
    /// Replace validated project tuning and audition settings in one history step.
    SetTuning(hex_tuning::ProjectTuning),
    /// Event IDs come from the clip/revision-scoped editable_audio_events view.
    EditAudioEvents {
        clip: ClipId,
        operation: crate::AudioEventEdit,
    },
    /// Rename any Arrangement or Session clip by stable ID.
    RenameClip {
        clip: ClipId,
        name: String,
    },
    SetTrackColor {
        track: TrackId,
        color: crate::ElementColor,
    },
    SetClipFlavor {
        clip: ClipId,
        flavor: crate::ClipFlavor,
    },
    /// Replace audio with an empty editable MIDI source, retaining clip identity.
    ConvertAudioClipToMidi(ClipId),
    /// Adopt a rendered project-rate WAV for a MIDI clip, retaining identity.
    ConvertMidiClipToAudio {
        clip: ClipId,
        path: PathBuf,
        frames: u64,
    },
    /// One prepared history operation creates a normal hybrid track with a device.
    InsertTrackWithDevice {
        name: String,
        before_track: Option<TrackId>,
        slot: TrackDeviceSlot,
        state: hex_lattice_contract::NativeDeviceState,
    },
    /// Atomically splice a bounded touch span, preserving the underlying base curve.
    SpliceGainTouch {
        track: TrackId,
        expected_epoch: u64,
        span: crate::GainTouchSpan,
    },
    /// One completed take, including loop passes, commits as one history edit.
    SpliceGainTouchBatch {
        track: TrackId,
        expected_epoch: u64,
        spans: Vec<crate::GainTouchSpan>,
    },
    ReplaceTrackGainEnvelope {
        track: TrackId,
        points: Vec<crate::GainPoint>,
    },
    UpsertTrackGainPoint {
        track: TrackId,
        point: crate::GainPoint,
    },
    MoveTrackGainPoint {
        track: TrackId,
        from: crate::BeatTime,
        to: crate::BeatTime,
    },
    RemoveTrackGainPoint {
        track: TrackId,
        beat: crate::BeatTime,
    },
    /// Clear base and recorded overlays, retaining the recording epoch high-water mark.
    ClearTrackGainEnvelope {
        track: TrackId,
    },
    /// Validate proposed metadata, then refuse non-legacy adoption until playback support.
    SetRouting(Vec<crate::RoutingEdge>),
    /// Add/change a post-fader stereo feed to a track input; zero removes it.
    SetAudioFeed {
        source: crate::RoutingOwner,
        destination: TrackId,
        gain: f32,
    },
    /// Select authored MIDI from another track; None restores local MIDI.
    SetMidiSource {
        destination: TrackId,
        source: Option<TrackId>,
    },
    /// Adopt or change a bounded additive stereo Return feed; zero removes it.
    SetReturnFeed {
        source: ReturnId,
        destination: ReturnId,
        gain: f32,
    },
    InsertDevice {
        track: TrackId,
        before: Option<DeviceId>,
        state: hex_lattice_contract::NativeDeviceState,
    },
    /// Fill a removed native instrument slot with a new stable device identity.
    InsertInstrument {
        track: TrackId,
        state: hex_lattice_contract::NativeDeviceState,
    },
    RemoveInstrument {
        track: TrackId,
    },
    RemoveDevice(DeviceId),
    MoveDevice {
        device: DeviceId,
        track: TrackId,
        before: Option<DeviceId>,
    },
    /// Move any effect or the optional instrument in the canonical ordered
    /// chain. Effects and an instrument can cross tracks when the destination
    /// has capacity and, for an instrument, an empty slot. `before` may identify
    /// either node class on the destination; `None` appends.
    MoveTrackDevice {
        device: DeviceId,
        track: TrackId,
        before: Option<DeviceId>,
    },
    ReplaceDeviceState {
        device: DeviceId,
        state: hex_lattice_contract::NativeDeviceState,
    },
    /// Atomically replace opaque device state and reserve fresh identities.
    /// Expected source and allocator cursor prevent stale UI edits overwriting
    /// another edit; history preserves the allocator high-water mark on undo.
    ReplaceDeviceStateWithIdentityReservation {
        device: DeviceId,
        expected_state: hex_lattice_contract::NativeDeviceState,
        state: hex_lattice_contract::NativeDeviceState,
        expected_next_id: u64,
        reserved_ids: u64,
    },
    SetDeviceBypass {
        device: DeviceId,
        bypassed: bool,
    },
    AddReturnTrack {
        name: String,
    },
    RemoveReturnTrack(ReturnId),
    RenameReturnTrack {
        return_track: ReturnId,
        name: String,
    },
    ReturnGain {
        return_track: ReturnId,
        gain: f32,
    },
    ReturnMute {
        return_track: ReturnId,
        muted: bool,
    },
    /// Zero removes the sparse send; positive levels use the linear 0..2 domain.
    SetReturnSend {
        track: TrackId,
        return_track: ReturnId,
        gain: f32,
    },
    InsertReturnDevice {
        return_track: ReturnId,
        before: Option<DeviceId>,
        state: hex_lattice_contract::NativeDeviceState,
    },
    MoveReturnDevice {
        device: DeviceId,
        before: Option<DeviceId>,
    },
    DeleteTimeRange {
        range: crate::TimeRange,
    },
    PasteTimeRange {
        clipboard: std::sync::Arc<crate::RangeClipboard>,
        track: TrackId,
        frame: u64,
    },
    /// Paste complete clips, retaining relative timing and stable multi-track owners.
    PasteClips {
        clipboard: std::sync::Arc<crate::ClipClipboard>,
        track: TrackId,
        frame: u64,
        insert: bool,
    },
    /// Keyboard paste that preserves original track IDs for multi-row clipboards.
    PasteTimeRangeOnSourceTracks {
        clipboard: std::sync::Arc<crate::RangeClipboard>,
        track: TrackId,
        frame: u64,
    },
    InsertTimeRange {
        clipboard: std::sync::Arc<crate::RangeClipboard>,
        track: TrackId,
        frame: u64,
    },
    /// Keyboard ripple paste that preserves original track IDs for multi-row clipboards.
    InsertTimeRangeOnSourceTracks {
        clipboard: std::sync::Arc<crate::RangeClipboard>,
        track: TrackId,
        frame: u64,
    },
    MoveTimeRange {
        range: crate::TimeRange,
        track: TrackId,
        frame: u64,
    },
    /// Copy a selected time rectangle to a destination in one undoable edit.
    CopyTimeRangeTo {
        range: crate::TimeRange,
        track: TrackId,
        frame: u64,
    },
    AddTrack {
        name: String,
    },
    /// Insert an empty hybrid track before a stable anchor, or append with None.
    InsertTrack {
        name: String,
        before_track: Option<TrackId>,
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
    /// Atomically insert a new hybrid track before a stable track id (or at the
    /// end when `before_track` is `None`) and place the prepared audio at `start`.
    ImportPreparedAudioTrackAt {
        name: String,
        before_track: Option<TrackId>,
        start: u64,
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
    /// Import prepared audio and cut its destination interval from both clip
    /// collections on the target track as one drop-specific edit.
    ImportPreparedAudioClipWithOverwrite {
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
    /// Remove explicit tracks through the existing cleanup as one history edit.
    RemoveTracks(Vec<TrackId>),
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
    /// Split every Arrangement clip crossing one absolute frame on the given
    /// tracks as one atomic edit.
    SplitTracksAtFrame {
        tracks: Vec<TrackId>,
        frame: u64,
    },
    /// Trim an absolute timeline edge, clamping to the nearest valid clip region.
    TrimClipEdge {
        clip: ClipId,
        edge: TrimEdge,
        frame: u64,
    },
    /// Resize an Arrangement audio clip with its lower fade endpoint as one edit.
    TrimClipFadeEdge {
        clip: ClipId,
        edge: TrimEdge,
        frame: u64,
    },
    MoveClip {
        clip: ClipId,
        track: TrackId,
        start: u64,
    },
    /// Drop-specific move that carves the destination frame interval out of
    /// both Arrangement clip collections before placing this audio clip.
    MoveClipWithOverwrite {
        clip: ClipId,
        track: TrackId,
        start: u64,
    },
    /// Move/copy one selected Arrangement group with a single prepared history edit.
    MoveClipsWithOverwrite {
        clips: Vec<ClipId>,
        anchor: ClipId,
        track: TrackId,
        position: crate::ClipGroupPosition,
        copy: bool,
    },
    /// Atomically insert a hybrid track before a stable anchor/end and move an
    /// existing audio clip there, preserving its ID and source metadata.
    MoveClipToNewTrack {
        clip: ClipId,
        name: String,
        before_track: Option<TrackId>,
        start: u64,
    },
    /// Copy an Arrangement audio clip to a destination, replacing overlap atomically.
    CopyClipTo {
        clip: ClipId,
        track: TrackId,
        start: u64,
    },
    /// Copy an Arrangement audio clip onto a newly inserted hybrid track.
    CopyClipToNewTrack {
        clip: ClipId,
        name: String,
        before_track: Option<TrackId>,
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
    /// Reanchor a complete fade envelope to the current Arrangement clip.
    SetClipFadeEnvelope {
        clip: ClipId,
        fade: ClipFade,
    },
    /// Set Arrangement or Session audio clip gain, in -60_000..=24_000 millidecibels.
    SetAudioClipGain {
        clip: ClipId,
        millidb: i32,
    },
    /// Integer cents; resampling is root-anchored and preserves placement duration.
    SetAudioClipPitch {
        clip: ClipId,
        cents: i16,
    },
    SetAudioClipMusical {
        clip: ClipId,
        enabled: bool,
    },
    /// Change the declared original sample tempo while Warp is active.
    SetAudioClipSampleBpm {
        clip: ClipId,
        tempo: crate::Tempo,
    },
    /// Affine retiming of the retained source map; opposite visible edge stays fixed.
    StretchAudioClipEdge {
        clip: ClipId,
        edge: TrimEdge,
        frame: u64,
    },
    SetAudioClipWarp {
        clip: ClipId,
        settings: Option<crate::WarpSettings>,
    },
    SetAudioClipSourceGain {
        clip: ClipId,
        points: Vec<crate::SourceGainPoint>,
    },
    /// Enable or disable the audio clip's source loop; disabling resets its phase.
    SetClipLooping {
        clip: ClipId,
        looping: bool,
    },
    /// Set a looping audio clip's source-relative play position.
    SetClipSourcePhase {
        clip: ClipId,
        phase: u64,
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
    /// Paint an empty MIDI clip, cutting existing audio/MIDI at its boundaries.
    PlaceMidiClipWithOverwrite {
        track: TrackId,
        start: crate::BeatTime,
        length: crate::BeatTime,
    },
    /// Resize a MIDI Arrangement edge without deleting authored source notes.
    TrimMidiClipEdge {
        clip: ClipId,
        edge: TrimEdge,
        beat: crate::BeatTime,
    },
    /// Split a finite MIDI Arrangement clip at an absolute musical beat.
    SplitMidiClip {
        clip: ClipId,
        beat: crate::BeatTime,
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
    /// Drop-specific MIDI move that carves the destination beat interval out
    /// of both Arrangement clip collections before placing this MIDI clip.
    MoveMidiClipWithOverwrite {
        clip: ClipId,
        track: TrackId,
        start: crate::BeatTime,
    },
    /// Atomically insert a hybrid track before a stable anchor/end and move an
    /// existing MIDI clip there, preserving its ID and note/source data.
    MoveMidiClipToNewTrack {
        clip: ClipId,
        name: String,
        before_track: Option<TrackId>,
        start: crate::BeatTime,
    },
    /// Copy an Arrangement MIDI clip to a destination, replacing overlap atomically.
    CopyMidiClipTo {
        clip: ClipId,
        track: TrackId,
        start: crate::BeatTime,
    },
    /// Copy an Arrangement MIDI clip onto a newly inserted hybrid track.
    CopyMidiClipToNewTrack {
        clip: ClipId,
        name: String,
        before_track: Option<TrackId>,
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
    ReturnTrack(ReturnId),
    Device(DeviceId),
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

pub(crate) fn insert_track_before(
    project: &mut Project,
    name: String,
    before_track: Option<TrackId>,
) -> Result<TrackId, Invalid> {
    name_valid(&name)?;
    if project.tracks.len() >= MAX_TRACKS {
        return Err(Invalid("track limit reached"));
    }
    let index = match before_track {
        Some(track) => project
            .tracks
            .iter()
            .position(|candidate| candidate.id == track)
            .ok_or(Invalid("track insertion anchor no longer exists"))?,
        None => project.tracks.len(),
    };
    let instrument_state = crate::default_instrument_state()?;
    let (first_id, instrument_id) = project.allocate_id_range(2)?;
    let id = TrackId(first_id);
    project.tracks.insert(
        index,
        Track {
            color: crate::ElementColor::default(),
            gain_lane: crate::GainLane::default(),
            id,
            name,
            gain: 1.0,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: Some(crate::Device {
                id: DeviceId(instrument_id),
                state: instrument_state,
                bypassed: false,
            }),
            instrument_position: Some(0),
            content: TrackContent::default(),
            sends: Vec::new(),
        },
    );
    Ok(id)
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
    if clip.edit.is_some() {
        return Err(Invalid("musical audio trim requires the project tempo map"));
    }
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
        clip.fade.reanchor(duration)?
    } else {
        clip.fade
    };
    Ok(Clip {
        name: clip.name.clone(),
        flavor: clip.flavor,
        gain: clip.gain,
        edit: clip.edit.clone(),
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

pub(crate) fn split_clip_at(
    original: &Clip,
    frame: u64,
    right_id: ClipId,
) -> Result<(Clip, Clip), Invalid> {
    if original.edit.is_some() {
        return Err(Invalid(
            "musical audio split requires the project tempo map",
        ));
    }
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
    let mut left = original.clone();
    left.duration = left_length;
    if !left.looping {
        left.source_length = left_length;
        left.source_phase = 0;
    }
    let right = Clip {
        name: original.name.clone(),
        flavor: original.flavor,
        id: right_id,
        asset: original.asset,
        start: frame,
        duration: right_duration,
        source_offset: right_source_offset,
        source_length: right_source_length,
        looping: original.looping,
        source_phase: right_phase,
        gain: original.gain,
        edit: None,
        fade: right_fade,
    };
    Ok((left, right))
}

pub(crate) fn split_clip_at_tempo(
    original: &Clip,
    frame: u64,
    right_id: ClipId,
    tempo: &crate::TempoMap,
) -> Result<(Clip, Clip), Invalid> {
    let Some(edit) = &original.edit else {
        return split_clip_at(original, frame, right_id);
    };
    let end = original
        .start
        .checked_add(original.duration)
        .ok_or(Invalid("audio split end overflow"))?;
    if frame <= original.start || frame >= end {
        return Err(Invalid("split cursor must be inside the clip"));
    }
    let cut = edit.crop_at_frame(original, frame, tempo)?;
    if cut <= edit.view_start || cut >= edit.view_end {
        return Err(Invalid("split has no positive musical crop"));
    }
    let mut left = original.clone();
    let mut right = original.clone();
    left.audio_edit_mut()
        .ok_or(Invalid("audio domain missing"))?
        .view_end = cut;
    let view = right
        .audio_edit_mut()
        .ok_or(Invalid("audio domain missing"))?;
    view.view_start = cut;
    if view.musical_start.is_some() {
        view.musical_start = Some(tempo.frame_to_beat(frame)?);
    }
    right.id = right_id;
    right.start = frame;
    crate::audio_edit::project_clip(&mut left, tempo)?;
    crate::audio_edit::project_clip(&mut right, tempo)?;
    Ok((left, right))
}

struct AudioOverwriteFragments {
    left: Option<Clip>,
    right: Option<Clip>,
}

pub(crate) fn overwrite_audio_interval(
    project: &mut Project,
    track: TrackId,
    start: u64,
    end: u64,
    excluded: ClipId,
) -> Result<(), Invalid> {
    overwrite_audio_interval_excluding(project, track, start, end, |id| id == excluded)
}

pub(crate) fn overwrite_audio_interval_excluding(
    project: &mut Project,
    track: TrackId,
    start: u64,
    end: u64,
    excluded: impl Fn(ClipId) -> bool,
) -> Result<(), Invalid> {
    if start >= end {
        return Err(Invalid("overwrite interval must be positive"));
    }
    let source_clips = project
        .track(track)
        .ok_or(Invalid("destination track no longer exists"))?
        .clips();
    let mut snapshot = Vec::new();
    snapshot
        .try_reserve_exact(source_clips.len())
        .map_err(|_| Invalid("could not reserve overwrite planning storage"))?;
    snapshot.extend(source_clips.iter().cloned());
    let mut fragments = Vec::new();
    fragments
        .try_reserve_exact(snapshot.len())
        .map_err(|_| Invalid("could not reserve overwrite fragment storage"))?;
    for victim in snapshot {
        if excluded(victim.id) {
            continue;
        }
        let victim_end = victim
            .start
            .checked_add(victim.duration)
            .ok_or(Invalid("timeline end overflow"))?;
        if victim.start >= end || start >= victim_end {
            continue;
        }
        let keeps_left = victim.start < start;
        let keeps_right = victim_end > end;
        let left = keeps_left
            .then(|| split_clip_at_tempo(&victim, start, victim.id, project.tempo_map()))
            .transpose()?
            .map(|parts| parts.0);
        let right_id = if keeps_left && keeps_right {
            ClipId(0)
        } else {
            victim.id
        };
        let right = keeps_right
            .then(|| split_clip_at_tempo(&victim, end, right_id, project.tempo_map()))
            .transpose()?
            .map(|parts| parts.1);
        fragments.push((victim.id, AudioOverwriteFragments { left, right }));
    }
    if fragments.is_empty() {
        return Ok(());
    }

    let total_clips = project.tracks.iter().try_fold(0_usize, |total, item| {
        total
            .checked_add(item.clips().len())
            .and_then(|value| value.checked_add(item.midi_clips().len()))
            .ok_or(Invalid("clip count overflow"))
    })?;
    let kept_fragments = fragments.iter().try_fold(0_usize, |total, (_, plan)| {
        total
            .checked_add(usize::from(plan.left.is_some()))
            .and_then(|value| value.checked_add(usize::from(plan.right.is_some())))
            .ok_or(Invalid("clip count overflow"))
    })?;
    let final_count = total_clips
        .checked_sub(fragments.len())
        .and_then(|value| value.checked_add(kept_fragments))
        .ok_or(Invalid("clip count overflow"))?;
    if final_count > MAX_CLIPS {
        return Err(Invalid("clip limit reached"));
    }
    for (_, plan) in &mut fragments {
        if let Some(right) = plan.right.as_mut().filter(|right| right.id == ClipId(0)) {
            right.id = ClipId(project.allocate_id()?);
        }
    }
    let fragment_count = fragments.len();
    let mut by_id = HashMap::new();
    by_id
        .try_reserve(fragments.len())
        .map_err(|_| Invalid("could not reserve overwrite lookup storage"))?;
    for (id, plan) in fragments {
        by_id.insert(id, plan);
    }
    let old_len = project
        .track(track)
        .ok_or(Invalid("destination track no longer exists"))?
        .clips()
        .len();
    let new_len = old_len
        .checked_sub(fragment_count)
        .and_then(|value| value.checked_add(kept_fragments))
        .ok_or(Invalid("clip count overflow"))?;
    let mut replacement = Vec::new();
    replacement
        .try_reserve_exact(new_len)
        .map_err(|_| Invalid("could not reserve overwritten clip storage"))?;
    let old = std::mem::take(project.audio_clips_mut(track)?);
    for clip in old {
        if let Some(plan) = by_id.remove(&clip.id) {
            if let Some(left) = plan.left {
                replacement.push(left);
            }
            if let Some(right) = plan.right {
                replacement.push(right);
            }
        } else {
            replacement.push(clip);
        }
    }
    *project.audio_clips_mut(track)? = replacement;
    Ok(())
}

pub(crate) fn overwrite_audio_in_beats(
    project: &mut Project,
    track: TrackId,
    start: crate::BeatTime,
    end: crate::BeatTime,
    excluded: ClipId,
) -> Result<(), Invalid> {
    overwrite_audio_in_beats_excluding(project, track, start, end, |id| id == excluded)
}

pub(crate) fn overwrite_audio_in_beats_excluding(
    project: &mut Project,
    track: TrackId,
    start: crate::BeatTime,
    end: crate::BeatTime,
    excluded: impl Fn(ClipId) -> bool,
) -> Result<(), Invalid> {
    let start_frame = project.tempo_map().beat_to_frame(start)?;
    let end_frame = project.tempo_map().beat_to_frame(end)?;
    if start_frame < end_frame {
        overwrite_audio_interval_excluding(project, track, start_frame, end_frame, excluded)?;
    }
    Ok(())
}
impl Project {
    /// Called only on an isolated candidate by Editor; never on an audio thread.
    pub(crate) fn apply(&mut self, command: Command) -> Result<Created, Invalid> {
        match command {
            Command::SetTuning(tuning) => {
                self.tuning = tuning;
                Ok(Created::None)
            }
            Command::EditAudioEvents { clip, operation } => {
                crate::audio_events::apply(self, clip, operation)?;
                Ok(Created::None)
            }
            Command::ConvertAudioClipToMidi(clip) => {
                crate::clip_conversion::audio_to_midi(self, clip)
            }
            Command::ConvertMidiClipToAudio { clip, path, frames } => {
                crate::clip_conversion::midi_to_audio(self, clip, path, frames)
            }
            Command::SpliceGainTouchBatch {
                track,
                expected_epoch,
                spans,
            } => {
                if spans.is_empty() || spans.len() > crate::MAX_GAIN_TOUCH_SPANS_PER_TRACK {
                    return Err(Invalid("gain touch take span count is outside its bound"));
                }
                if self
                    .track(track)
                    .ok_or(Invalid("track no longer exists"))?
                    .gain_lane
                    .recording_epoch
                    != expected_epoch
                {
                    return Err(Invalid("gain touch replacement epoch is stale"));
                }
                let mut epoch = expected_epoch;
                for span in &spans {
                    span.validate()?;
                    if span.context.recording_epoch <= epoch {
                        return Err(Invalid("gain touch take epochs must strictly increase"));
                    }
                    if span.context.sample_rate != self.sample_rate {
                        return Err(Invalid("gain touch sample rate differs from project"));
                    }
                    epoch = span.context.recording_epoch;
                }
                // Project::apply runs only on Editor's isolated candidate. A
                // later refusal discards every earlier pass and all its IDs.
                let mut epoch = expected_epoch;
                for span in spans {
                    let next = span.context.recording_epoch;
                    self.apply(Command::SpliceGainTouch {
                        track,
                        expected_epoch: epoch,
                        span,
                    })?;
                    epoch = next;
                }
                Ok(Created::None)
            }
            Command::SpliceGainTouch {
                track,
                expected_epoch,
                span,
            } => {
                let owner = self.track(track).ok_or(Invalid("track no longer exists"))?;
                if span.context.sample_rate != self.sample_rate {
                    return Err(Invalid("gain touch sample rate differs from project"));
                }
                let proposed_units = owner.gain_lane.touch_units(expected_epoch, &span)?;
                let other = self
                    .tracks()
                    .iter()
                    .filter(|owner| owner.id() != track)
                    .try_fold(0_usize, |count, owner| {
                        count
                            .checked_add(owner.gain_lane.units()?)
                            .ok_or(Invalid("gain lane count overflow"))
                    })?;
                if other
                    .checked_add(proposed_units)
                    .is_none_or(|count| count > crate::MAX_GAIN_POINTS)
                {
                    return Err(Invalid("project gain lane limit reached"));
                }
                let proposed = owner.gain_lane.with_touch(expected_epoch, span)?;
                self.track_mut(track)?.gain_lane = proposed;
                Ok(Created::None)
            }
            Command::ReplaceTrackGainEnvelope { track, points } => {
                self.replace_gain_envelope(track, points)
            }
            Command::UpsertTrackGainPoint { track, point } => self.upsert_gain_point(track, point),
            Command::MoveTrackGainPoint { track, from, to } => {
                self.move_gain_point(track, from, to)
            }
            Command::RemoveTrackGainPoint { track, beat } => self.remove_gain_point(track, beat),
            Command::ClearTrackGainEnvelope { track } => {
                let lane = &mut self.track_mut(track)?.gain_lane;
                lane.base.clear();
                lane.spans.clear();
                Ok(Created::None)
            }
            Command::SetTrackColor { track, color } => {
                self.track_mut(track)?.color = color;
                Ok(Created::None)
            }
            Command::SetClipFlavor { clip, flavor } => self.set_clip_flavor(clip, flavor),
            Command::SetRouting(edges) => self.adopt_routing(edges),
            Command::SetAudioFeed {
                source,
                destination,
                gain,
            } => self.set_audio_feed(source, destination, gain),
            Command::SetMidiSource {
                destination,
                source,
            } => self.set_midi_source(destination, source),
            Command::SetReturnFeed {
                source,
                destination,
                gain,
            } => self.set_return_feed(source, destination, gain),
            device_command @ (Command::InsertDevice { .. }
            | Command::InsertInstrument { .. }
            | Command::RemoveInstrument { .. }
            | Command::RemoveDevice(_)
            | Command::MoveDevice { .. }
            | Command::MoveTrackDevice { .. }
            | Command::ReplaceDeviceState { .. }
            | Command::ReplaceDeviceStateWithIdentityReservation { .. }
            | Command::SetDeviceBypass { .. }) => crate::device::apply(self, device_command),
            return_command @ (Command::AddReturnTrack { .. }
            | Command::RemoveReturnTrack(_)
            | Command::RenameReturnTrack { .. }
            | Command::ReturnGain { .. }
            | Command::ReturnMute { .. }
            | Command::SetReturnSend { .. }
            | Command::InsertReturnDevice { .. }
            | Command::MoveReturnDevice { .. }) => crate::returns::apply(self, return_command),
            Command::DeleteTimeRange { range } => crate::time_range::clear(self, &range),
            Command::PasteTimeRange {
                clipboard,
                track,
                frame,
            } => crate::time_range::paste(self, &clipboard, track, frame),
            Command::PasteTimeRangeOnSourceTracks {
                clipboard,
                track,
                frame,
            } => crate::time_range::paste_on_source_tracks(self, &clipboard, track, frame),
            Command::InsertTimeRange {
                clipboard,
                track,
                frame,
            } => crate::time_range::insert(self, &clipboard, track, frame),
            Command::InsertTimeRangeOnSourceTracks {
                clipboard,
                track,
                frame,
            } => crate::time_range::insert_on_source_tracks(self, &clipboard, track, frame),
            Command::MoveTimeRange {
                range,
                track,
                frame,
            } => crate::time_range::move_range(self, &range, track, frame),
            Command::CopyTimeRangeTo {
                range,
                track,
                frame,
            } => crate::time_range::copy_range(self, &range, track, frame),
            midi_command @ (Command::AddMidiTrack { .. }
            | Command::PlaceMidiClip { .. }
            | Command::PlaceMidiClipWithOverwrite { .. }
            | Command::SetMidiClipSource { .. }
            | Command::TrimMidiClipEdge { .. }
            | Command::SplitMidiClip { .. }
            | Command::AddMidiNote { .. }
            | Command::AddMidiNotes { .. }
            | Command::EditMidiNotes { .. }
            | Command::UpdateMidiNote { .. }
            | Command::UpdateMidiNotes { .. }
            | Command::RemoveMidiNote(_)
            | Command::RemoveMidiNotes { .. }
            | Command::MoveMidiClip { .. }
            | Command::MoveMidiClipWithOverwrite { .. }
            | Command::MoveMidiClipToNewTrack { .. }
            | Command::CopyMidiClipTo { .. }
            | Command::RemoveMidiClip(_)
            | Command::DuplicateMidiClip(_)) => crate::midi::apply(self, midi_command),
            Command::CopyMidiClipToNewTrack {
                clip,
                name,
                before_track,
                start,
            } => {
                let track = insert_track_before(self, name, before_track)?;
                crate::clip_copy::midi_to(self, clip, track, start)?;
                Ok(Created::Track(track))
            }
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
                let gain = original.gain;
                let flavor = original.flavor;
                let audio_edit = original.edit.clone();
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
                    self.clip_mut(duplicate_id)?.gain = gain;
                    self.clip_mut(duplicate_id)?.flavor = flavor;
                    let mut duplicated = self
                        .clip(duplicate_id)
                        .cloned()
                        .ok_or(Invalid("duplicate missing"))?;
                    duplicated.edit = audio_edit;
                    crate::audio_edit::move_clip(&mut duplicated, start, self.tempo_map())?;
                    *self.clip_mut(duplicate_id)? = duplicated;
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
                if self
                    .tracks
                    .iter()
                    .map(|track| track.clips().len() + track.midi_clips().len())
                    .sum::<usize>()
                    >= MAX_CLIPS
                {
                    return Err(Invalid("clip limit reached"));
                }
                self.audio_clips_mut(self.tracks[track_index].id)?
                    .try_reserve(1)
                    .map_err(|_| Invalid("could not reserve clip storage"))?;
                let right_id = ClipId(self.allocate_id()?);
                let (left, right) =
                    split_clip_at_tempo(&original, frame, right_id, self.tempo_map())?;
                self.audio_clips_mut(self.tracks[track_index].id)?[clip_index] = left;
                self.audio_clips_mut(self.tracks[track_index].id)?
                    .insert(clip_index + 1, right);
                Ok(Created::Clip(right_id))
            }
            Command::SplitTracksAtFrame { tracks, frame } => {
                crate::split_tracks::apply(self, &tracks, frame)
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
                if original.looping || original.edit.is_some() {
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
            Command::InsertTrackWithDevice {
                name,
                before_track,
                slot,
                state,
            } => {
                let track = insert_track_before(self, name, before_track)?;
                match slot {
                    TrackDeviceSlot::Instrument => {
                        let device = self
                            .track(track)
                            .and_then(|track| track.instrument())
                            .ok_or(Invalid("new track has no default instrument"))?
                            .id();
                        self.apply(Command::ReplaceDeviceState { device, state })?;
                    }
                    TrackDeviceSlot::Effect => {
                        self.apply(Command::InsertDevice {
                            track,
                            before: None,
                            state,
                        })?;
                    }
                }
                Ok(Created::Track(track))
            }
            Command::AddTrack { name } => {
                Ok(Created::Track(insert_track_before(self, name, None)?))
            }
            Command::InsertTrack { name, before_track } => Ok(Created::Track(insert_track_before(
                self,
                name,
                before_track,
            )?)),
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
            } => self.apply(Command::ImportPreparedAudioTrackAt {
                name,
                before_track: None,
                start: 0,
                path,
                source_sample_rate,
                source_frames,
                playback_frames,
                conversion_version,
            }),
            Command::ImportPreparedAudioTrackAt {
                name,
                before_track,
                start,
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
                let track = insert_track_before(self, name, before_track)?;
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
            Command::ImportPreparedAudioClipWithOverwrite {
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
                let end = start
                    .checked_add(playback_frames)
                    .ok_or(Invalid("timeline end overflow"))?;
                let asset = self.add_prepared_asset(
                    path,
                    source_sample_rate,
                    source_frames,
                    playback_frames,
                    conversion_version,
                )?;
                // Clip IDs are nonzero, so zero is a safe sentinel before the
                // incoming clip has been allocated. Carve first so a full
                // project can still replace existing material at MAX_CLIPS.
                overwrite_audio_interval(self, track, start, end, ClipId(0))?;
                crate::midi::overwrite_midi_in_frames(self, track, start, end, ClipId(0))?;
                let clip = match self.apply(Command::PlaceClip {
                    track,
                    asset,
                    start,
                    duration: playback_frames,
                    source_offset: 0,
                    source_length: playback_frames,
                    looping: false,
                })? {
                    Created::Clip(clip) => clip,
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
            Command::RemoveTracks(ids) => {
                if ids.is_empty() || ids.len() > crate::MAX_TRACKS {
                    return Err(Invalid("track removal needs a bounded selection"));
                }
                for (index, id) in ids.iter().enumerate() {
                    if ids[..index].contains(id) || self.track(*id).is_none() {
                        return Err(Invalid("track removal selection is stale or duplicated"));
                    }
                }
                for id in ids {
                    self.apply(Command::RemoveTrack(id))?;
                }
                Ok(Created::None)
            }
            Command::RemoveTrack(id) => {
                let index = self
                    .tracks
                    .iter()
                    .position(|t| t.id == id)
                    .ok_or(Invalid("track no longer exists"))?;
                self.tracks.remove(index);
                self.audio_feeds.retain(|r| {
                    r.source() != crate::RoutingOwner::Track(id) && r.destination() != id
                });
                self.midi_routes
                    .retain(|r| r.source() != id && r.destination() != id);
                self.session.cells.retain(|cell| cell.track != id);
                Ok(Created::None)
            }
            Command::RenameClip { clip, name } => crate::clip_name::rename(self, clip, name),
            Command::RenameTrack { track, name } => {
                name_valid(&name)?;
                self.track_mut(track)?.name = name;
                Ok(Created::None)
            }
            Command::Gain { track, gain } => {
                if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
                    return Err(Invalid("gain must be finite and within 0..2"));
                }
                if !self
                    .track(track)
                    .ok_or(Invalid("track no longer exists"))?
                    .gain_lane()
                    .base()
                    .is_empty()
                    || !self
                        .track(track)
                        .ok_or(Invalid("track no longer exists"))?
                        .gain_lane
                        .spans
                        .is_empty()
                {
                    return Err(Invalid("track gain is owned by automation"));
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
                let mut candidate = self.clone();
                candidate.tempo_map = candidate
                    .tempo_map
                    .replace_point(TempoPoint::new(beat, tempo))?;
                let map = candidate.tempo_map.clone();
                for track in &mut candidate.tracks {
                    for clip in &mut track.content.audio_clips {
                        crate::audio_edit::project_clip(clip, &map)?;
                    }
                }
                crate::Project::from_snapshot(candidate.snapshot())?;
                *self = candidate;
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
                if self.device_uses_asset(id) {
                    return Err(Invalid("media is still referenced by a device"));
                }
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
                    name: "clip".into(),
                    flavor: crate::ClipFlavor::default(),
                    id,
                    asset,
                    start,
                    duration,
                    source_offset,
                    source_length,
                    looping,
                    source_phase: 0,
                    fade,
                    gain: crate::ClipGain::UNITY,
                    edit: None,
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
                let mut moved = self
                    .clip(clip)
                    .cloned()
                    .ok_or(Invalid("audio clip is missing"))?;
                crate::audio_edit::move_clip(&mut moved, start, self.tempo_map())?;
                self.audio_clips_mut(source_id)?.remove(index);
                self.audio_clips_mut(track)?.push(moved);
                Ok(Created::None)
            }
            Command::MoveClipsWithOverwrite {
                clips,
                anchor,
                track,
                position,
                copy,
            } => crate::clip_group::apply(self, clips, anchor, track, position, copy),
            Command::MoveClipWithOverwrite { clip, track, start } => {
                let mut incoming = self
                    .clip(clip)
                    .cloned()
                    .ok_or(Invalid("clip no longer exists"))?;
                let source_track = self
                    .tracks
                    .iter()
                    .find(|candidate| candidate.clips().iter().any(|item| item.id == clip))
                    .map(|candidate| candidate.id)
                    .ok_or(Invalid("clip track missing"))?;
                if source_track == track && incoming.start == start {
                    return Ok(Created::None);
                }
                crate::audio_edit::move_clip(&mut incoming, start, self.tempo_map())?;
                let end = start
                    .checked_add(incoming.duration)
                    .ok_or(Invalid("timeline end overflow"))?;
                self.track(track)
                    .ok_or(Invalid("destination track no longer exists"))?;
                overwrite_audio_interval(self, track, start, end, clip)?;
                crate::midi::overwrite_midi_in_frames(self, track, start, end, clip)?;
                self.apply(Command::MoveClip { clip, track, start })
            }
            Command::MoveClipToNewTrack {
                clip,
                name,
                before_track,
                start,
            } => {
                let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                start
                    .checked_add(original.duration)
                    .ok_or(Invalid("timeline end overflow"))?;
                let track = insert_track_before(self, name, before_track)?;
                self.apply(Command::MoveClip { clip, track, start })?;
                Ok(Created::Track(track))
            }
            Command::PasteClips {
                clipboard,
                track,
                frame,
                insert,
            } => crate::clip_clipboard::paste(self, &clipboard, track, frame, insert),
            Command::CopyClipTo { clip, track, start } => {
                crate::clip_copy::audio_to(self, clip, track, start)
            }
            Command::CopyClipToNewTrack {
                clip,
                name,
                before_track,
                start,
            } => {
                let track = insert_track_before(self, name, before_track)?;
                crate::clip_copy::audio_to(self, clip, track, start)?;
                Ok(Created::Track(track))
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
                if original.edit.is_some() {
                    return crate::audio_edit::reset_region(
                        self,
                        clip,
                        start,
                        duration,
                        source_offset,
                        source_length,
                    );
                }
                let old_fade = original.fade;
                let old_source_offset = original.source_offset;
                let old_source_length = original.source_length;
                let fade = old_fade.reanchor(duration)?;
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
                let trimmed = if original.edit.is_some() {
                    crate::audio_edit::trim_view(original, edge, frame, self.tempo_map())?
                } else {
                    preview_edge_trim(original, edge, frame, asset_frames)?
                };
                *self.clip_mut(clip)? = trimmed;
                Ok(Created::None)
            }
            Command::TrimClipFadeEdge { clip, edge, frame } => {
                let original = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                let asset_frames = self
                    .asset(original.asset)
                    .ok_or(Invalid("media no longer exists"))?
                    .playback_frames;
                let trimmed =
                    original.preview_fade_edge_trim(edge, frame, asset_frames, self.tempo_map())?;
                *self.clip_mut(clip)? = trimmed;
                Ok(Created::None)
            }
            command @ (Command::SetAudioClipPitch { .. }
            | Command::SetAudioClipMusical { .. }
            | Command::SetAudioClipWarp { .. }
            | Command::SetAudioClipSampleBpm { .. }
            | Command::StretchAudioClipEdge { .. }
            | Command::SetAudioClipSourceGain { .. }) => crate::audio_edit::apply(self, command),
            Command::SetAudioClipGain { clip, millidb } => {
                let gain = crate::ClipGain::new(millidb)?;
                if self.clip(clip).is_some() {
                    self.clip_mut(clip)?.gain = gain;
                } else {
                    let target = self
                        .session
                        .cells
                        .iter_mut()
                        .find_map(|cell| {
                            if let crate::SessionCellContent::Audio(audio) = &mut cell.content {
                                (audio.id == clip).then_some(audio)
                            } else {
                                None
                            }
                        })
                        .ok_or(Invalid("audio clip no longer exists"))?;
                    target.gain = gain;
                }
                Ok(Created::None)
            }
            Command::SetClipFadeEnvelope { clip, fade } => {
                let target = self.clip(clip).ok_or(Invalid("clip no longer exists"))?;
                if fade.span() != target.duration || fade.offset() != 0 {
                    return Err(Invalid("edited fade must match the current clip"));
                }
                if target.edit.is_some() {
                    return crate::audio_edit::set_fade_envelope(self, clip, fade);
                }
                self.clip_mut(clip)?.fade = fade;
                Ok(Created::None)
            }
            Command::SetClipFades {
                clip,
                fade_in,
                fade_out,
            } => {
                if self.clip(clip).is_some_and(|clip| clip.edit.is_some()) {
                    return crate::audio_edit::set_fades(self, clip, fade_in, fade_out);
                }
                let duration = self
                    .clip(clip)
                    .ok_or(Invalid("clip no longer exists"))?
                    .duration;
                let fade = ClipFade::new(duration, 0, fade_in, fade_out)?;
                self.clip_mut(clip)?.fade = fade;
                Ok(Created::None)
            }
            Command::SetClipLooping { clip, looping } => {
                if self.clip(clip).is_some_and(|clip| clip.edit.is_some()) {
                    return crate::audio_edit::source_policy(self, clip, Some(looping), None);
                }
                let target = self.clip_mut(clip)?;
                if target.looping != looping {
                    target.looping = looping;
                    if !looping {
                        target.source_phase = 0;
                    }
                }
                Ok(Created::None)
            }
            Command::SetClipSourcePhase { clip, phase } => {
                if self.clip(clip).is_some_and(|clip| clip.edit.is_some()) {
                    return crate::audio_edit::source_policy(self, clip, None, Some(phase));
                }
                let target = self.clip_mut(clip)?;
                if !target.looping {
                    return Err(Invalid("source phase can only be set on a looping clip"));
                }
                if phase >= target.source_length {
                    return Err(Invalid("source phase must be inside the source interval"));
                }
                target.source_phase = phase;
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

#[cfg(test)]
mod overwrite_tests {
    use super::{Command, Created, insert_track_before};
    use crate::{Clip, ClipFade, MAX_CLIPS, Project};
    use std::path::PathBuf;

    #[test]
    fn audio_overwrite_clip_limit_failure_preserves_candidate_exactly() {
        let mut project = Project::new(48_000).expect("project");
        let target = insert_track_before(&mut project, "target".into(), None).expect("target");
        let source = insert_track_before(&mut project, "source".into(), None).expect("source");
        let asset = project
            .add_prepared_asset(PathBuf::from("fixture.wav"), 48_000, 128, 128, 0)
            .expect("asset");
        let _victim = match project
            .apply(Command::PlaceClip {
                track: target,
                asset,
                start: 0,
                duration: 10,
                source_offset: 0,
                source_length: 10,
                looping: true,
            })
            .expect("victim")
        {
            Created::Clip(clip) => clip,
            other => panic!("unexpected victim: {other:?}"),
        };
        let incoming = match project
            .apply(Command::PlaceClip {
                track: source,
                asset,
                start: 0,
                duration: 2,
                source_offset: 20,
                source_length: 2,
                looping: false,
            })
            .expect("incoming")
        {
            Created::Clip(clip) => clip,
            other => panic!("unexpected incoming: {other:?}"),
        };
        let filler_count = MAX_CLIPS - 2;
        project
            .audio_clips_mut(target)
            .expect("target content")
            .try_reserve_exact(filler_count)
            .expect("filler capacity");
        for index in 0..filler_count {
            let id = crate::ClipId(project.allocate_id().expect("filler id"));
            let start = 1_000 + (index as u64 * 2);
            project
                .audio_clips_mut(target)
                .expect("target content")
                .push(Clip {
                    name: "clip".into(),
                    flavor: crate::ClipFlavor::default(),
                    id,
                    asset,
                    start,
                    duration: 1,
                    source_offset: 0,
                    source_length: 1,
                    looping: false,
                    source_phase: 0,
                    fade: ClipFade::neutral(1).expect("neutral fade"),
                    gain: crate::ClipGain::UNITY,
                    edit: None,
                });
        }
        let before = project.clone();
        assert!(
            project
                .apply(Command::MoveClipWithOverwrite {
                    clip: incoming,
                    track: target,
                    start: 4,
                })
                .is_err()
        );
        assert_eq!(project, before);
    }

    #[test]
    fn prepared_audio_overwrite_can_replace_fully_covered_clip_at_clip_limit() {
        let mut project = Project::new(48_000).expect("project");
        let target = insert_track_before(&mut project, "target".into(), None).expect("target");
        let asset = project
            .add_prepared_asset(PathBuf::from("fixture.wav"), 48_000, 128, 128, 0)
            .expect("asset");
        let victim = match project
            .apply(Command::PlaceClip {
                track: target,
                asset,
                start: 0,
                duration: 4,
                source_offset: 0,
                source_length: 4,
                looping: false,
            })
            .expect("victim")
        {
            Created::Clip(clip) => clip,
            other => panic!("unexpected victim: {other:?}"),
        };
        let filler_count = MAX_CLIPS - 1;
        project
            .audio_clips_mut(target)
            .expect("target content")
            .try_reserve_exact(filler_count)
            .expect("filler capacity");
        for index in 0..filler_count {
            let id = crate::ClipId(project.allocate_id().expect("filler id"));
            let start = 1_000 + (index as u64 * 2);
            project
                .audio_clips_mut(target)
                .expect("target content")
                .push(Clip {
                    name: "clip".into(),
                    flavor: crate::ClipFlavor::default(),
                    id,
                    asset,
                    start,
                    duration: 1,
                    source_offset: 0,
                    source_length: 1,
                    looping: false,
                    source_phase: 0,
                    fade: ClipFade::neutral(1).expect("neutral fade"),
                    gain: crate::ClipGain::UNITY,
                    edit: None,
                });
        }
        assert_eq!(
            project.track(target).expect("target").clips().len(),
            MAX_CLIPS
        );
        let created = project
            .apply(Command::ImportPreparedAudioClipWithOverwrite {
                track: target,
                start: 0,
                path: PathBuf::from("replacement.wav"),
                source_sample_rate: 48_000,
                source_frames: 4,
                playback_frames: 4,
                conversion_version: 0,
            })
            .expect("full-cover replacement at the clip limit");
        let Created::Imported { clip, .. } = created else {
            panic!("expected imported clip, got {created:?}");
        };
        let clips = project.track(target).expect("target").clips();
        assert_eq!(clips.len(), MAX_CLIPS);
        assert!(!clips.iter().any(|candidate| candidate.id() == victim));
        assert!(clips.iter().any(|candidate| candidate.id() == clip));
    }
}

#[cfg(test)]
#[path = "track_remove_tests.rs"]
mod track_remove_tests;
