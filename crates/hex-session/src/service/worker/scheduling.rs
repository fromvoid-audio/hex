//! Worker request deferral, recovery classification, and bounded export-event flushing.
use super::*;

/// Control lanes that still need worker polls (#4574): a resident seek
/// awaiting its result or completion delivery, or an admitted export.
pub(super) fn control_lanes_pending(
    seek: Option<&resident_seek::Lane>,
    export: &ExportLane,
) -> bool {
    seek.is_some_and(resident_seek::Lane::pending) || export.pending()
}

pub(super) fn is_track_switch(command: &Command) -> bool {
    matches!(
        command,
        Command::Mute { .. }
            | Command::ToggleMute(_)
            | Command::ToggleTracksMute(_)
            | Command::ToggleSolo { .. }
            | Command::ToggleSoloTracks { .. }
    )
}

pub(super) fn is_section_copy(command: &Command) -> bool {
    matches!(
        command,
        Command::DuplicateArrangementSection(_)
            | Command::DuplicateArrangementSectionAt { .. }
            | Command::AppendArrangementSectionCopy { .. }
            | Command::ReplaceArrangementSection { copy: true, .. }
            | Command::PasteArrangementSection { .. }
    )
}

pub(in crate::service) fn defers_behind_resident_publication(request: &Request) -> bool {
    match request {
        Request::MoveExistingClips { .. }
        | Request::LoadTrackCatalogDevice { .. }
        | Request::AddNativeDevice { .. }
        | Request::InsertEffectBusDevice { .. }
        | Request::InsertBrowserInstrumentTrack { .. }
        | Request::DropBrowserDeviceOnArrangement { .. }
        | Request::DropNativeDevice { .. }
        | Request::ReplaceNativeDevice { .. }
        | Request::MoveTrackDevices { .. }
        | Request::ReorderDevices { .. }
        | Request::ReparentTracks { .. }
        | Request::DeviceClipboard {
            action:
                DeviceClipboardAction::Cut { .. }
                | DeviceClipboardAction::Paste { .. }
                | DeviceClipboardAction::Duplicate { .. },
            ..
        }
        | Request::TrackClipboard {
            action:
                TrackClipboardAction::Cut(_)
                | TrackClipboardAction::Paste { .. }
                | TrackClipboardAction::Duplicate { .. },
            ..
        }
        | Request::Undo
        | Request::Redo
        | Request::SetDeviceParameter { .. } => true,
        Request::DeviceGesture(gesture) => matches!(
            **gesture,
            GestureRequest::Change { .. }
                | GestureRequest::ChangePair { .. }
                | GestureRequest::ChangeSnapshot { .. }
                | GestureRequest::ChangeEqCreation { .. }
                | GestureRequest::Commit { .. }
                | GestureRequest::CommitPair { .. }
                | GestureRequest::CommitSnapshot { .. }
                | GestureRequest::CommitEqCreation { .. }
                | GestureRequest::Cancel { .. }
        ),
        Request::Edit(command) => {
            matches!(
                command,
                Command::SetLoopRegion { .. }
                    | Command::ToggleLoopRegion
                    | Command::ToggleDeviceBypass { .. }
                    | Command::SetDeviceBypass { .. }
                    | Command::ReplaceDeviceState { .. }
            ) || is_track_switch(command)
                || resident_audio::device_insert::graph_edit_operation(command).is_some()
                || resident_audio::device_insert::resident_graph_edit_operation(command, false)
                    .is_some()
        }
        _ => false,
    }
}

/// Buffers worker-produced `ExportEvent`s when the capacity-1 `export_events`
/// channel already holds an unread event, flushing FIFO on subsequent polls
/// without blocking the worker loop or dropping terminal completions/errors.
#[derive(Default)]
pub(super) struct PendingExportEvents {
    queued: std::collections::VecDeque<ExportEvent>,
}

impl PendingExportEvents {
    pub(super) fn is_empty(&self) -> bool {
        self.queued.is_empty()
    }

    pub(super) fn push_and_flush(&mut self, event: ExportEvent, sender: &SyncSender<ExportEvent>) {
        self.queued.push_back(event);
        self.flush(sender);
    }

    pub(super) fn flush(&mut self, sender: &SyncSender<ExportEvent>) {
        while let Some(event) = self.queued.pop_front() {
            match sender.try_send(event) {
                Ok(()) => {}
                Err(TrySendError::Full(event)) => {
                    self.queued.push_front(event);
                    break;
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.queued.clear();
                    break;
                }
            }
        }
    }
}
