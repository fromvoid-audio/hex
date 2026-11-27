//! Canonical edit command definitions and exhaustive `Project::apply` dispatch.
mod clip;
mod command;
mod device;
mod track;

pub(crate) use clip::{
    interval_valid, preview_edge_trim, retained_audio_fragment, split_clip_at, split_clip_at_tempo,
};
pub use command::{
    Command, Created, InstrumentAssetOwner, InstrumentLoadDestination, InstrumentLoadPlacement,
    MidiExpressiveNoteInsert, MidiNoteInsert, MidiNoteUpdate, TrackDeviceSlot,
};
pub(crate) use track::{insert_track_before, insert_track_before_with_gain, name_valid, new_track};

use crate::{Invalid, Project};

impl Project {
    pub(crate) fn apply(&mut self, command: Command) -> Result<Created, Invalid> {
        match command {
            source_command @ (Command::CreateNativeSource { .. }
            | Command::BindTrackSource { .. }
            | Command::SetNativeSourceReceiver { .. }
            | Command::RenameNativeSource { .. }
            | Command::RemoveNativeSource(_)) => crate::native_sources::apply(self, source_command),
            command @ (Command::AddArrangementSection { .. }
            | Command::PlaceLinkedScene { .. }
            | Command::UnlinkSectionScene(_)
            | Command::SetArrangementSectionRange { .. }
            | Command::MoveArrangementSectionMarker { .. }
            | Command::ResizeArrangementSectionMarker { .. }
            | Command::RenameArrangementSection { .. }
            | Command::SetArrangementSectionColor { .. }
            | Command::RemoveArrangementSection(_)
            | Command::ReorderSectionBefore { .. }
            | Command::ReorderSectionAfter { .. }
            | Command::ResizeArrangementSection { .. }
            | Command::ResizeArrangementSectionEdge { .. }
            | Command::ResizeArrangementSectionBoundary { .. }
            | Command::DeleteArrangementSection(_)
            | Command::DuplicateArrangementSection(_)
            | Command::DuplicateArrangementSectionAt { .. }
            | Command::AppendArrangementSectionCopy { .. }
            | Command::MoveArrangementSectionBeyondEnd { .. }
            | Command::ReplaceArrangementSection { .. }
            | Command::PasteArrangementSection { .. }
            | Command::SplitArrangementSection { .. }
            | Command::MergeArrangementSections(_)) => {
                crate::arrangement_sections::apply(self, command)
            }
            command @ (Command::AddArrangementLocator { .. }
            | Command::MoveArrangementLocator { .. }
            | Command::RenameArrangementLocator { .. }
            | Command::RemoveArrangementLocator(_)) => {
                crate::arrangement_locators::apply(self, command)
            }
            main_command @ (Command::InsertMainDevice { .. }
            | Command::ReplaceMainDevice { .. }
            | Command::MoveMainDevice { .. }) => crate::main_devices::apply(self, main_command),
            device_command @ (Command::InsertDevice { .. }
            | Command::ReplaceTrackDevice { .. }
            | Command::InsertMidiProcessor { .. }
            | Command::RemoveMidiProcessor { .. }
            | Command::ReplaceTrackMidiProcessor { .. }
            | Command::InsertInstrument { .. }
            | Command::RemoveInstrument { .. }
            | Command::ReplaceTrackInstrument { .. }
            | Command::RemoveDevice(_)
            | Command::MoveDevice { .. }
            | Command::MoveTrackDevice { .. }
            | Command::MoveTrackDevices { .. }
            | Command::ReorderDevices { .. }
            | Command::MoveNativeSourceDevice { .. }
            | Command::ReparentDevice { .. }
            | Command::InsertNestedDevice { .. }
            | Command::ReplaceDeviceState { .. }
            | Command::ReplaceDeviceStateWithIdentityReservation { .. }
            | Command::SetDeviceBypass { .. }
            | Command::ToggleDeviceBypass { .. }
            | Command::CompareDevice { .. }
            | Command::ReplaceDeviceComparison { .. }) => {
                crate::device::apply(self, device_command)
            }
            return_command @ (Command::AddReturnTrack { .. }
            | Command::AddReturnTrackWithGain { .. }
            | Command::RemoveReturnTrack(_)
            | Command::RenameReturnTrack { .. }
            | Command::ReturnGain { .. }
            | Command::ReturnMute { .. }
            | Command::SetReturnSend { .. }
            | Command::InsertReturnDevice { .. }
            | Command::ReplaceReturnDevice { .. }
            | Command::MoveReturnDevice { .. }) => crate::returns::apply(self, return_command),
            midi_command @ (Command::AddMidiTrack { .. }
            | Command::PlaceMidiClip { .. }
            | Command::PlaceMidiClipWithOverwrite { .. }
            | Command::PlaceMidiClipWithNotes { .. }
            | Command::PlaceMidiClipWithExpressiveNotes { .. }
            | Command::ExtendMidiClipWithNotes { .. }
            | Command::ExtendMidiClipWithExpressiveNotes { .. }
            | Command::SetMidiClipSource { .. }
            | Command::ResizeMidiClipView { .. }
            | Command::TrimMidiClipEdge { .. }
            | Command::StretchMidiClipEdge { .. }
            | Command::SplitMidiClip { .. }
            | Command::SetMidiExpressionLane { .. }
            | Command::SetMidiNoteExpression { .. }
            | Command::AddMidiNote { .. }
            | Command::AddMidiNotes { .. }
            | Command::AddMidiNotesWithExpression { .. }
            | Command::EditMidiNotes { .. }
            | Command::EditMidiNotesWithSourceExtent { .. }
            | Command::DuplicateMidiSourceRange { .. }
            | Command::UpdateMidiNote { .. }
            | Command::UpdateMidiNotes { .. }
            | Command::UpdateMidiNotesWithSourceExtent { .. }
            | Command::SetMidiNotesActive { .. }
            | Command::RemoveMidiNote(_)
            | Command::RemoveMidiNotes { .. }
            | Command::MoveMidiClip { .. }
            | Command::MoveMidiClipWithOverwrite { .. }
            | Command::MoveMidiClipToNewTrack { .. }
            | Command::CopyMidiClipTo { .. }
            | Command::RemoveMidiClip(_)
            | Command::DuplicateMidiClip(_)) => crate::midi::apply(self, midi_command),
            session_command @ (Command::AddScene { .. }
            | Command::InsertScene { .. }
            | Command::DuplicateScene(_)
            | Command::MoveScene { .. }
            | Command::RenameScene { .. }
            | Command::SetSceneFollowAction { .. }
            | Command::RemoveScene(_)
            | Command::CreateSessionMidiClip { .. }
            | Command::CreateSessionAudioClip { .. }
            | Command::CopyArrangementAudioClipToSession { .. }
            | Command::CopyArrangementMidiToSession { .. }
            | Command::CopyArrangementAudioClipToNewSessionTrack { .. }
            | Command::CopyArrangementMidiToNewSessionTrack { .. }
            | Command::MoveSessionCell { .. }
            | Command::DuplicateSessionMidiCell { .. }
            | Command::DuplicateSessionAudioCell { .. }
            | Command::SetSessionAudioClipLoop { .. }
            | Command::SetSessionAudioClipSource { .. }
            | Command::ClearSessionCell { .. }
            | Command::SetSessionStopCell { .. }
            | Command::SetSessionMidiClipSource { .. }
            | Command::RenameSessionMidiClip { .. }) => {
                crate::session::apply(self, session_command)
            }
            command @ (Command::SetAudioClipPitch { .. }
            | Command::SetAudioClipMusical { .. }
            | Command::SetAudioClipWarp { .. }
            | Command::SetAudioClipSampleBpm { .. }
            | Command::StretchAudioClipEdge { .. }
            | Command::SetAudioClipSourceGain { .. }) => crate::audio_edit::apply(self, command),
            clip_command @ (Command::DuplicateAliasClips(..)
            | Command::DuplicateAliasClipsAt { .. }
            | Command::DuplicateAliasClipToNewTrack { .. }
            | Command::MakeClipsUnique(..)
            | Command::ApplyClipSettingsToAliases(..)
            | Command::DuplicateSessionCell { .. }
            | Command::CommitArrangementBounce { .. }
            | Command::EditAudioEvents { .. }
            | Command::ConvertAudioClipToMidi(..)
            | Command::ConvertMidiClipToAudio { .. }
            | Command::SetClipActive { .. }
            | Command::SetTimeRangeActive { .. }
            | Command::SetClipFlavor { .. }
            | Command::ReplaceMidiClipVolume { .. }
            | Command::SetMidiClipGain { .. }
            | Command::DeleteTimeRange { .. }
            | Command::PasteTimeRange { .. }
            | Command::PasteTimeRangeOnSourceTracks { .. }
            | Command::InsertTimeRange { .. }
            | Command::InsertTimeRangeOnSourceTracks { .. }
            | Command::MoveTimeRange { .. }
            | Command::CopyTimeRangeTo { .. }
            | Command::CopyMidiClipToNewTrack { .. }
            | Command::DuplicateClips(..)
            | Command::GroupClips(..)
            | Command::GroupTimeRange(..)
            | Command::GroupTimeRangeOrCreate(..)
            | Command::GroupAudioEventRange { .. }
            | Command::ReverseClips(..)
            | Command::DuplicateClip(..)
            | Command::SplitClip { .. }
            | Command::SplitTracksAtFrame { .. }
            | Command::SplitTimeRange { .. }
            | Command::NudgeClip { .. }
            | Command::TrimClipEndBy { .. }
            | Command::ImportAudioTrack { .. }
            | Command::ImportPreparedAudioTrack { .. }
            | Command::ImportPreparedAudioTrackAt { .. }
            | Command::ImportPreparedAudioClip { .. }
            | Command::ImportPreparedAudioClipWithOverwrite { .. }
            | Command::ImportPreparedSessionAudioClip { .. }
            | Command::ImportExistingAudio { .. }
            | Command::EditClips { .. }
            | Command::UpdateMidiNotesWithSourcePrefix { .. }
            | Command::RenameClip { .. }
            | Command::AddAsset { .. }
            | Command::RelinkAssets(..)
            | Command::RemoveAsset(..)
            | Command::PlaceClip { .. }
            | Command::MoveClip { .. }
            | Command::MoveClipsWithOverwrite { .. }
            | Command::MoveClipWithOverwrite { .. }
            | Command::MoveClipToNewTrack { .. }
            | Command::PasteClips { .. }
            | Command::CopyClipTo { .. }
            | Command::CopyClipToNewTrack { .. }
            | Command::TrimClip { .. }
            | Command::TrimClipEdge { .. }
            | Command::TrimClipFadeEdge { .. }
            | Command::ResizeAudioClipView { .. }
            | Command::SetAudioClipGain { .. }
            | Command::CreateArrangementFades { .. }
            | Command::CreateAudioEventFades { .. }
            | Command::SetClipFadeEnvelope { .. }
            | Command::SetClipFades { .. }
            | Command::SetClipLooping { .. }
            | Command::SetClipSourcePhase { .. }
            | Command::RemoveClip(..)) => self.apply_clip_command(clip_command),
            track_command @ (Command::SetTuning(..)
            | Command::SpliceGainTouchBatch { .. }
            | Command::SpliceGainTouch { .. }
            | Command::ReplaceTrackGainEnvelope { .. }
            | Command::UpsertTrackGainPoint { .. }
            | Command::MoveTrackGainPoint { .. }
            | Command::RemoveTrackGainPoint { .. }
            | Command::ClearTrackGainEnvelope { .. }
            | Command::SetTrackMixerAutomation { .. }
            | Command::FreezeTrack { .. }
            | Command::FreezeTrackWithAsset { .. }
            | Command::UnfreezeTrack { .. }
            | Command::SetTrackColor { .. }
            | Command::GainBy { .. }
            | Command::BalanceBy { .. }
            | Command::MasterGainBy { .. }
            | Command::ToggleTracksMute(..)
            | Command::ToggleMute(..)
            | Command::InsertTrackWithDevice { .. }
            | Command::InsertDefaultTrack { .. }
            | Command::InsertDefaultTrackWithGain { .. }
            | Command::InsertDefaultTrackWithDevice { .. }
            | Command::InsertDefaultTrackWithDeviceAndGain { .. }
            | Command::LoadInstrument { .. }
            | Command::AddTrack { .. }
            | Command::InsertTrack { .. }
            | Command::MoveTrackBefore { .. }
            | Command::MoveTracksBefore { .. }
            | Command::GroupTracks { .. }
            | Command::UngroupTrack(..)
            | Command::ReparentTracks { .. }
            | Command::DuplicateTracks { .. }
            | Command::RemoveTracks(..)
            | Command::RemoveTrack(..)
            | Command::SetTrackTags { .. }
            | Command::RenameTrack { .. }
            | Command::Gain { .. }
            | Command::SetTrackGains { .. }
            | Command::Balance { .. }
            | Command::MasterGain { .. }
            | Command::SetProjectTempo { .. }
            | Command::SetTempoPoint { .. }
            | Command::SetMeterPoint { .. }
            | Command::Mute { .. }
            | Command::ToggleSolo { .. }
            | Command::ToggleSoloTracks { .. }
            | Command::SetLoopRegion { .. }
            | Command::ToggleLoopRegion
            | Command::ClearLoopRegion
            | Command::PasteTracks { .. }) => self.apply_track_command(track_command),
            device_command @ (Command::SetRouting(..)
            | Command::SetAudioFeed { .. }
            | Command::SetSidechain { .. }
            | Command::SetMidiSource { .. }
            | Command::AssignTrackAudioInput { .. }
            | Command::SetTrackOutput { .. }
            | Command::SetTrackInputMonitoring { .. }
            | Command::SetReturnFeed { .. }
            | Command::ManualDeviceEdit { .. }
            | Command::SetDeviceAutomationReadSelection { .. }
            | Command::ReenableDeviceAutomation
            | Command::SpliceDeviceAutomationTouch { .. }
            | Command::SpliceVst3AutomationTouchGroup { .. }
            | Command::ReplaceDeviceModulation { .. }
            | Command::ReplaceDeviceAutomationTarget { .. }
            | Command::ReplaceDeviceAutomation { .. }
            | Command::InitializeDevice(..)
            | Command::ImportDeviceAsset { .. }
            | Command::ImportDeviceAssetIntoNewInstrument { .. }
            | Command::ImportDeviceAssetReplacingInstrument { .. }
            | Command::RemoveDevices { .. }
            | Command::PasteDevices { .. }) => self.apply_device_command(device_command),
        }
    }
}

#[cfg(test)]
#[path = "track_remove_tests.rs"]
mod track_remove_tests;
