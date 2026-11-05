//! One immutable selection-wide property intent. Target membership is validated
//! before mutation; each member reuses canonical scalar semantics. Geometry-changing
//! properties must not run overwrite against another row member midway through a
//! batch. Arrangement rows are therefore detached once, each selected member is
//! edited in isolation, and complete restored rows undergo final validation. No
//! partial history, intermediate cuts, source transplantation or DSP work occurs.
use crate::{
    BeatsPreserve, BeatsTail, ClipFlavor, ClipId, Command, Created, Invalid, MAX_CLIPS, NoteId,
    Project, Tempo, TrackContent, WarpMode,
};
use std::collections::BTreeSet;

/// A clip-wide value applied to one frozen Arrangement selection. Audio-only
/// values explicitly leave selected MIDI clips unchanged; zero eligible clips refuse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipEdit {
    Active {
        active: bool,
    },
    Gain {
        millidb: i32,
    },
    Pitch {
        cents: i16,
    },
    /// Relative controls preserve each member's own value. Gain overflow or an
    /// out-of-range result refuses the whole edit, matching scalar gain edits.
    GainBy {
        millidb: i32,
    },
    /// Pitch follows the scalar control's saturating -4800..4800-cent limit.
    PitchBy {
        cents: i16,
    },
    Musical {
        enabled: bool,
    },
    SampleBpm {
        tempo: Tempo,
    },
    WarpEnabled {
        enabled: bool,
    },
    /// Choosing an algorithm enables warp, retaining each member's own markers.
    WarpMode {
        mode: WarpMode,
    },
    /// Relative controls retain each algorithm and markers; as with the
    /// existing SetAudioClipWarp(Some) edit, a successful adjustment enables warp.
    WarpAdjust {
        adjustment: WarpAdjustment,
    },
    Looping {
        looping: bool,
    },
    Flavor {
        flavor: ClipFlavor,
    },
    Rename {
        name: String,
    },
    Delete,
}

/// Relative algorithm controls retain every member's other settings and markers.
/// A member with an incompatible algorithm refuses the complete selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WarpAdjustment {
    Grain(i16),
    Fluctuation(i8),
    Seed,
    Preserve,
    Tail(BeatsTail),
    Decay(i8),
    Formants(i8),
    Resolution(i16),
}
impl WarpAdjustment {
    fn apply(self, mode: &mut WarpMode) -> Result<(), Invalid> {
        match (self, mode) {
            (
                Self::Grain(delta),
                WarpMode::Tones { grain_ms } | WarpMode::Texture { grain_ms, .. },
            ) => *grain_ms = (i32::from(*grain_ms) + i32::from(delta)).clamp(8, 200) as u16,
            (Self::Fluctuation(delta), WarpMode::Texture { fluctuation, .. }) => {
                *fluctuation = (i16::from(*fluctuation) + i16::from(delta)).clamp(0, 100) as u8
            }
            (Self::Seed, WarpMode::Texture { seed, .. }) => *seed = seed.wrapping_add(1),
            (Self::Preserve, WarpMode::Beats { preserve, .. }) => {
                *preserve = match *preserve {
                    BeatsPreserve::Transients => BeatsPreserve::Division(
                        crate::BeatTime::from_raw(crate::BeatTime::ONE.raw() / 4),
                    ),
                    BeatsPreserve::Division(beat)
                        if beat.raw() < crate::BeatTime::ONE.raw() * 2 =>
                    {
                        BeatsPreserve::Division(crate::BeatTime::from_raw(beat.raw() * 2))
                    }
                    _ => BeatsPreserve::Transients,
                }
            }
            (Self::Tail(selected), WarpMode::Beats { tail, .. }) => *tail = selected,
            (Self::Decay(delta), WarpMode::Beats { decay, .. }) => {
                *decay = (i16::from(*decay) + i16::from(delta)).clamp(0, 100) as u8
            }
            (Self::Formants(delta), WarpMode::ComplexPro { formants, .. }) => {
                *formants = (i16::from(*formants) + i16::from(delta)).clamp(0, 100) as u8
            }
            (Self::Resolution(delta), WarpMode::ComplexPro { envelope, .. }) => {
                *envelope = (i32::from(*envelope) + i32::from(delta)).clamp(32, 512) as u16
            }
            _ => {
                return Err(Invalid(
                    "warp adjustment is unavailable for a selected clip algorithm",
                ));
            }
        }
        Ok(())
    }
}

/// Declared target semantics, independent of UI selection/focus. This exhaustive
/// classification forces every new command to choose its owning scope. A property
/// may be widened by the caller; indexed source edits and geometry cannot be
/// copied to other sources merely because their clips are selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipCommandScope {
    Property {
        clip: ClipId,
        edit: ClipEdit,
    },
    /// Typed source-coordinate edits retain their explicit source owner.
    FocusedSource {
        clip: ClipId,
    },
    /// Whole-source snapshots cannot be broadcast to different retained roots;
    /// a caller explicitly opts into its focused source when multi-selected.
    FocusedSnapshot {
        clip: ClipId,
    },
    FocusedNote {
        note: NoteId,
    },
    Geometry {
        clip: ClipId,
    },
    Explicit,
    Unrelated,
}

impl ClipEdit {
    fn audio_only(&self) -> bool {
        !matches!(
            self,
            Self::Active { .. } | Self::Flavor { .. } | Self::Rename { .. } | Self::Delete
        )
    }
    pub(crate) fn command(
        &self,
        project: &Project,
        clip: ClipId,
        midi: bool,
    ) -> Result<Command, Invalid> {
        Ok(match self {
            Self::Active { active } => Command::SetClipActive {
                clip,
                active: *active,
            },
            Self::GainBy { millidb } => Command::SetAudioClipGain {
                clip,
                millidb: project
                    .clip(clip)
                    .ok_or(Invalid("selected audio clip no longer exists"))?
                    .gain()
                    .millidb()
                    .checked_add(*millidb)
                    .ok_or(Invalid("relative clip gain exceeds numeric range"))?,
            },
            Self::PitchBy { cents } => Command::SetAudioClipPitch {
                clip,
                cents: project
                    .clip(clip)
                    .ok_or(Invalid("selected audio clip no longer exists"))?
                    .audio_edit()
                    .map_or(0, |edit| edit.domain.pitch_cents)
                    .saturating_add(*cents)
                    .clamp(-4_800, 4_800),
            },
            Self::Gain { millidb } => Command::SetAudioClipGain {
                clip,
                millidb: *millidb,
            },
            Self::Pitch { cents } => Command::SetAudioClipPitch {
                clip,
                cents: *cents,
            },
            Self::Musical { enabled } => Command::SetAudioClipMusical {
                clip,
                enabled: *enabled,
            },
            Self::SampleBpm { tempo } => Command::SetAudioClipSampleBpm {
                clip,
                tempo: *tempo,
            },
            Self::WarpEnabled { enabled: false } => Command::SetAudioClipWarp {
                clip,
                settings: None,
            },
            Self::WarpEnabled { enabled: true } | Self::WarpMode { .. } => {
                let source = project
                    .clip(clip)
                    .ok_or(Invalid("selected audio clip no longer exists"))?;
                let mut settings = match source
                    .audio_edit()
                    .and_then(|edit| edit.domain.warp.as_ref())
                {
                    Some(settings) => settings.clone(),
                    None => crate::default_audio_warp(
                        source,
                        project.tempo_map(),
                        WarpMode::Beats {
                            preserve: BeatsPreserve::Transients,
                            tail: BeatsTail::Forward,
                            decay: 100,
                        },
                    )?,
                };
                if let Self::WarpMode { mode } = self {
                    mode.validate()?;
                    if std::mem::discriminant(mode) != std::mem::discriminant(&settings.mode) {
                        settings.mode = *mode;
                    }
                }
                Command::SetAudioClipWarp {
                    clip,
                    settings: Some(settings),
                }
            }
            Self::WarpAdjust { adjustment } => {
                let source = project
                    .clip(clip)
                    .ok_or(Invalid("selected audio clip no longer exists"))?;
                let mut settings = source
                    .audio_edit()
                    .and_then(|edit| edit.domain.warp.as_ref())
                    .ok_or(Invalid("enable warp to edit its algorithm controls"))?
                    .clone();
                adjustment.apply(&mut settings.mode)?;
                Command::SetAudioClipWarp {
                    clip,
                    settings: Some(settings),
                }
            }
            Self::Looping { looping } => Command::SetClipLooping {
                clip,
                looping: *looping,
            },
            Self::Flavor { flavor } => Command::SetClipFlavor {
                clip,
                flavor: *flavor,
            },
            Self::Rename { name } => {
                crate::edit::name_valid(name)?;
                Command::RenameClip {
                    clip,
                    name: name.clone(),
                }
            }
            Self::Delete if midi => Command::RemoveMidiClip(clip),
            Self::Delete => Command::RemoveClip(clip),
        })
    }
}

fn row(project: &mut Project, index: usize) -> Result<&mut TrackContent, Invalid> {
    project
        .tracks
        .get_mut(index)
        .map(|track| &mut track.content)
        .ok_or(Invalid("clip edit track changed"))
}

pub(crate) fn apply(
    project: &mut Project,
    clips: Vec<ClipId>,
    edit: ClipEdit,
) -> Result<Created, Invalid> {
    if clips.is_empty() || clips.len() > MAX_CLIPS {
        return Err(Invalid("clip selection must contain 1..16384 clips"));
    }
    let selected: BTreeSet<_> = clips.iter().copied().collect();
    if selected.len() != clips.len() {
        return Err(Invalid("clip selection contains duplicate ids"));
    }
    let mut found = 0_usize;
    let mut audio = 0_usize;
    for track in project.tracks() {
        audio += track
            .clips()
            .iter()
            .filter(|clip| selected.contains(&clip.id()))
            .count();
        found += track
            .clips()
            .iter()
            .filter(|clip| selected.contains(&clip.id()))
            .count();
        found += track
            .midi_clips()
            .iter()
            .filter(|clip| selected.contains(&clip.id()))
            .count();
    }
    if found != clips.len() {
        return Err(Invalid("selected arrangement clip no longer exists"));
    }
    if edit.audio_only() && audio == 0 {
        return Err(Invalid("clip property requires a selected audio clip"));
    }

    let mut candidate = project.clone();
    let mut rows = Vec::new();
    rows.try_reserve_exact(candidate.tracks.len())
        .map_err(|_| Invalid("could not reserve clip edit rows"))?;
    for track in &mut candidate.tracks {
        rows.push(std::mem::take(&mut track.content));
    }
    for (index, original) in rows.iter_mut().enumerate() {
        for source in &mut original.audio_clips {
            if !selected.contains(&source.id()) {
                continue;
            }
            row(&mut candidate, index)?
                .audio_clips
                .try_reserve(1)
                .map_err(|_| Invalid("could not reserve clip edit member"))?;
            row(&mut candidate, index)?.audio_clips.push(source.clone());
            let command = edit.command(&candidate, source.id(), false)?;
            candidate.apply(command)?;
            let active = &mut row(&mut candidate, index)?.audio_clips;
            if matches!(edit, ClipEdit::Delete) {
                if !active.is_empty() {
                    return Err(Invalid("clip deletion retained an unexpected member"));
                }
            } else {
                let changed = active.pop().ok_or(Invalid("clip edit lost its member"))?;
                if !active.is_empty() || changed.id() != source.id() {
                    return Err(Invalid("clip edit changed member identity"));
                }
                *source = changed;
            }
        }
        if !edit.audio_only() {
            for source in &mut original.midi_clips {
                if !selected.contains(&source.id()) {
                    continue;
                }
                row(&mut candidate, index)?
                    .midi_clips
                    .try_reserve(1)
                    .map_err(|_| Invalid("could not reserve clip edit member"))?;
                row(&mut candidate, index)?.midi_clips.push(source.clone());
                let command = edit.command(&candidate, source.id(), true)?;
                candidate.apply(command)?;
                let active = &mut row(&mut candidate, index)?.midi_clips;
                if matches!(edit, ClipEdit::Delete) {
                    if !active.is_empty() {
                        return Err(Invalid("clip deletion retained an unexpected member"));
                    }
                } else {
                    let changed = active.pop().ok_or(Invalid("clip edit lost its member"))?;
                    if !active.is_empty() || changed.id() != source.id() {
                        return Err(Invalid("clip edit changed member identity"));
                    }
                    *source = changed;
                }
            }
        }
        if matches!(edit, ClipEdit::Delete) {
            original
                .audio_clips
                .retain(|clip| !selected.contains(&clip.id()));
            original
                .midi_clips
                .retain(|clip| !selected.contains(&clip.id()));
        }
    }
    for (track, original) in candidate.tracks.iter_mut().zip(rows) {
        track.content = original;
    }
    if candidate.aliases.sources.is_empty() {
        Project::from_snapshot(candidate.snapshot())?;
    }
    *project = candidate;
    Ok(Created::None)
}

impl Command {
    pub fn clip_scope(&self) -> ClipCommandScope {
        match self {
            Self::SetClipActive { clip, active } => ClipCommandScope::Property {
                clip: *clip,
                edit: ClipEdit::Active { active: *active },
            },
            Self::SetAudioClipGain { clip, millidb } => ClipCommandScope::Property {
                clip: *clip,
                edit: ClipEdit::Gain { millidb: *millidb },
            },
            Self::SetAudioClipPitch { clip, cents } => ClipCommandScope::Property {
                clip: *clip,
                edit: ClipEdit::Pitch { cents: *cents },
            },
            Self::SetAudioClipMusical { clip, enabled } => ClipCommandScope::Property {
                clip: *clip,
                edit: ClipEdit::Musical { enabled: *enabled },
            },
            Self::SetAudioClipSampleBpm { clip, tempo } => ClipCommandScope::Property {
                clip: *clip,
                edit: ClipEdit::SampleBpm { tempo: *tempo },
            },
            Self::SetClipLooping { clip, looping } => ClipCommandScope::Property {
                clip: *clip,
                edit: ClipEdit::Looping { looping: *looping },
            },
            Self::SetClipFlavor { clip, flavor } => ClipCommandScope::Property {
                clip: *clip,
                edit: ClipEdit::Flavor { flavor: *flavor },
            },
            Self::RenameClip { clip, name } => ClipCommandScope::Property {
                clip: *clip,
                edit: ClipEdit::Rename { name: name.clone() },
            },
            Self::RenameSessionMidiClip { clip, name } => ClipCommandScope::Property {
                clip: *clip,
                edit: ClipEdit::Rename { name: name.clone() },
            },
            Self::RemoveClip(clip) => ClipCommandScope::Property {
                clip: *clip,
                edit: ClipEdit::Delete,
            },
            Self::RemoveMidiClip(clip) => ClipCommandScope::Property {
                clip: *clip,
                edit: ClipEdit::Delete,
            },
            Self::SetClipFades { clip, .. }
            | Self::SetClipFadeEnvelope { clip, .. }
            | Self::SetAudioClipWarp { clip, .. }
            | Self::SetAudioClipSourceGain { clip, .. }
            | Self::SetClipSourcePhase { clip, .. } => {
                ClipCommandScope::FocusedSnapshot { clip: *clip }
            }
            Self::EditAudioEvents { clip, .. }
            | Self::CreateAudioEventFades { clip, .. }
            | Self::SetSessionAudioClipLoop { clip, .. }
            | Self::SetSessionAudioClipSource { clip, .. }
            | Self::SetSessionMidiClipSource { clip, .. }
            | Self::SetMidiClipSource { clip, .. }
            | Self::ReplaceMidiClipVolume { clip, .. }
            | Self::SetMidiClipGain { clip, .. }
            | Self::AddMidiNote { clip, .. }
            | Self::AddMidiNotes { clip, .. }
            | Self::SetMidiNotesActive { clip, .. }
            | Self::EditMidiNotes { clip, .. }
            | Self::EditMidiNotesWithSourceExtent { clip, .. }
            | Self::DuplicateMidiSourceRange { clip, .. }
            | Self::UpdateMidiNotes { clip, .. }
            | Self::UpdateMidiNotesWithSourceExtent { clip, .. }
            | Self::UpdateMidiNotesWithSourcePrefix { clip, .. }
            | Self::RemoveMidiNotes { clip, .. } => ClipCommandScope::FocusedSource { clip: *clip },
            Self::ConvertMidiClipToAudio { clip, .. }
            | Self::SplitClip { clip, .. }
            | Self::TrimClipEdge { clip, .. }
            | Self::TrimClipFadeEdge { clip, .. }
            | Self::MoveClip { clip, .. }
            | Self::MoveClipWithOverwrite { clip, .. }
            | Self::MoveClipToNewTrack { clip, .. }
            | Self::CopyClipTo { clip, .. }
            | Self::CopyClipToNewTrack { clip, .. }
            | Self::TrimClip { clip, .. }
            | Self::StretchAudioClipEdge { clip, .. }
            | Self::ResizeAudioClipView { clip, .. }
            | Self::ResizeMidiClipView { clip, .. }
            | Self::NudgeClip { clip, .. }
            | Self::TrimClipEndBy { clip, .. }
            | Self::TrimMidiClipEdge { clip, .. }
            | Self::StretchMidiClipEdge { clip, .. }
            | Self::SplitMidiClip { clip, .. }
            | Self::MoveMidiClip { clip, .. }
            | Self::MoveMidiClipWithOverwrite { clip, .. }
            | Self::MoveMidiClipToNewTrack { clip, .. }
            | Self::CopyMidiClipTo { clip, .. }
            | Self::CopyMidiClipToNewTrack { clip, .. } => {
                ClipCommandScope::Geometry { clip: *clip }
            }
            Self::DuplicateClip(clip) => ClipCommandScope::Geometry { clip: *clip },
            Self::DuplicateMidiClip(clip) => ClipCommandScope::Geometry { clip: *clip },
            Self::ConvertAudioClipToMidi(clip) => ClipCommandScope::Geometry { clip: *clip },
            Self::CopyArrangementAudioClipToSession { source, .. }
            | Self::CopyArrangementAudioClipToNewSessionTrack { source, .. } => {
                ClipCommandScope::Geometry { clip: *source }
            }
            Self::CopyArrangementMidiToSession { source, .. }
            | Self::CopyArrangementMidiToNewSessionTrack { source, .. } => {
                ClipCommandScope::Geometry { clip: *source }
            }
            Self::UpdateMidiNote { note, .. } | Self::RemoveMidiNote(note) => {
                ClipCommandScope::FocusedNote { note: *note }
            }
            Self::SetTimeRangeActive { .. }
            | Self::EditClips { .. }
            | Self::DeleteTimeRange { .. }
            | Self::PasteTimeRange { .. }
            | Self::PasteClips { .. }
            | Self::PasteTimeRangeOnSourceTracks { .. }
            | Self::InsertTimeRange { .. }
            | Self::InsertTimeRangeOnSourceTracks { .. }
            | Self::MoveTimeRange { .. }
            | Self::CopyTimeRangeTo { .. }
            | Self::SplitTracksAtFrame { .. }
            | Self::SplitTimeRange { .. }
            | Self::CreateArrangementFades { .. }
            | Self::MoveClipsWithOverwrite { .. }
            | Self::GroupAudioEventRange { .. }
            | Self::GroupTimeRange(_)
            | Self::GroupClips(_)
            | Self::DuplicateClips(_)
            | Self::DuplicateAliasClips(_)
            | Self::DuplicateAliasClipsAt { .. }
            | Self::DuplicateAliasClipToNewTrack { .. }
            | Self::MakeClipsUnique(_)
            | Self::ApplyClipSettingsToAliases(_)
            | Self::DuplicateSessionCell { .. }
            | Self::ReverseClips(_) => ClipCommandScope::Explicit,
            Self::AddArrangementSection { .. }
            | Self::SetArrangementSectionRange { .. }
            | Self::MoveArrangementSectionMarker { .. }
            | Self::ResizeArrangementSectionMarker { .. }
            | Self::RenameArrangementSection { .. }
            | Self::SetArrangementSectionColor { .. }
            | Self::RemoveArrangementSection(..)
            | Self::ReorderSectionBefore { .. }
            | Self::ReorderSectionAfter { .. }
            | Self::ResizeArrangementSection { .. }
            | Self::ResizeArrangementSectionEdge { .. }
            | Self::ResizeArrangementSectionBoundary { .. }
            | Self::DeleteArrangementSection(_)
            | Self::DuplicateArrangementSection(_)
            | Self::DuplicateArrangementSectionAt { .. }
            | Self::SplitArrangementSection { .. }
            | Self::MergeArrangementSections(_)
            | Self::SetTuning(..)
            | Self::SetTrackColor { .. }
            | Self::InsertTrackWithDevice { .. }
            | Self::InsertDefaultTrackWithDevice { .. }
            | Self::InsertDefaultTrackWithDeviceAndGain { .. }
            | Self::SpliceGainTouch { .. }
            | Self::SpliceGainTouchBatch { .. }
            | Self::ReplaceTrackGainEnvelope { .. }
            | Self::UpsertTrackGainPoint { .. }
            | Self::MoveTrackGainPoint { .. }
            | Self::RemoveTrackGainPoint { .. }
            | Self::ClearTrackGainEnvelope { .. }
            | Self::SetRouting(..)
            | Self::SetAudioFeed { .. }
            | Self::SetSidechain { .. }
            | Self::SetMidiSource { .. }
            | Self::SetReturnFeed { .. }
            | Self::InsertMainDevice { .. }
            | Self::MoveMainDevice { .. }
            | Self::InsertDevice { .. }
            | Self::InsertInstrument { .. }
            | Self::LoadInstrument { .. }
            | Self::RemoveInstrument { .. }
            | Self::RemoveDevice(..)
            | Self::MoveDevice { .. }
            | Self::MoveTrackDevice { .. }
            | Self::ReplaceDeviceAutomationTarget { .. }
            | Self::ReplaceDeviceAutomation { .. }
            | Self::ManualDeviceEdit { .. }
            | Self::SetDeviceAutomationReadSelection { .. }
            | Self::ReenableDeviceAutomation
            | Self::ReplaceDeviceState { .. }
            | Self::ReplaceDeviceStateWithIdentityReservation { .. }
            | Self::SetDeviceBypass { .. }
            | Self::ToggleDeviceBypass { .. }
            | Self::CompareDevice { .. }
            | Self::ReplaceDeviceComparison { .. }
            | Self::AddReturnTrack { .. }
            | Self::AddReturnTrackWithGain { .. }
            | Self::RemoveReturnTrack(..)
            | Self::RenameReturnTrack { .. }
            | Self::ReturnGain { .. }
            | Self::ReturnMute { .. }
            | Self::SetReturnSend { .. }
            | Self::InsertReturnDevice { .. }
            | Self::MoveReturnDevice { .. }
            | Self::GroupTracks { .. }
            | Self::UngroupTrack(..)
            | Self::ReparentTracks { .. }
            | Self::DuplicateTracks { .. }
            | Self::AddTrack { .. }
            | Self::InsertTrack { .. }
            | Self::InsertDefaultTrack { .. }
            | Self::InsertDefaultTrackWithGain { .. }
            | Self::MoveTrackBefore { .. }
            | Self::MoveTracksBefore { .. }
            | Self::PasteTracks { .. }
            | Self::PasteDevices { .. }
            | Self::RemoveDevices { .. }
            | Self::ImportAudioTrack { .. }
            | Self::CommitArrangementBounce { .. }
            | Self::ImportPreparedAudioTrack { .. }
            | Self::ImportPreparedAudioTrackAt { .. }
            | Self::ImportPreparedAudioClip { .. }
            | Self::ImportPreparedAudioClipWithOverwrite { .. }
            | Self::ImportPreparedSessionAudioClip { .. }
            | Self::ImportExistingAudio { .. }
            | Self::RemoveTrack(..)
            | Self::RemoveTracks(..)
            | Self::SetTrackTags { .. }
            | Self::RenameTrack { .. }
            | Self::Gain { .. }
            | Self::SetTrackGains { .. }
            | Self::Balance { .. }
            | Self::MasterGain { .. }
            | Self::SetProjectTempo { .. }
            | Self::SetTempoPoint { .. }
            | Self::SetMeterPoint { .. }
            | Self::Mute { .. }
            | Self::ToggleSolo { .. }
            | Self::ToggleSoloTracks { .. }
            | Self::SetLoopRegion { .. }
            | Self::ToggleLoopRegion
            | Self::ClearLoopRegion
            | Self::AddAsset { .. }
            | Self::ImportDeviceAsset { .. }
            | Self::ImportDeviceAssetIntoEmptyInstrument { .. }
            | Self::ImportDeviceAssetReplacingInstrument { .. }
            | Self::RemoveAsset(..)
            | Self::RelinkAssets(..)
            | Self::PlaceClip { .. }
            | Self::GainBy { .. }
            | Self::BalanceBy { .. }
            | Self::MasterGainBy { .. }
            | Self::ToggleMute(..)
            | Self::ToggleTracksMute(..)
            | Self::AddMidiTrack { .. }
            | Self::AddScene { .. }
            | Self::InsertScene { .. }
            | Self::DuplicateScene(..)
            | Self::MoveScene { .. }
            | Self::RenameScene { .. }
            | Self::RemoveScene(..)
            | Self::CreateSessionMidiClip { .. }
            | Self::CreateSessionAudioClip { .. }
            | Self::MoveSessionCell { .. }
            | Self::DuplicateSessionMidiCell { .. }
            | Self::DuplicateSessionAudioCell { .. }
            | Self::ClearSessionCell { .. }
            | Self::SetSessionStopCell { .. }
            | Self::PlaceMidiClip { .. }
            | Self::BindTrackSource { .. }
            | Self::SetNativeSourceReceiver { .. }
            | Self::CreateNativeSource { .. }
            | Self::RenameNativeSource { .. }
            | Self::RemoveNativeSource(_)
            | Self::PlaceMidiClipWithOverwrite { .. } => ClipCommandScope::Unrelated,
        }
    }
}
