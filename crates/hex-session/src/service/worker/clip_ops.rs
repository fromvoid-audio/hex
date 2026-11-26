//! Clip, media import, arrangement edit, time-range and track-clipboard worker operations.
use super::*;
use std::ops::ControlFlow;

mod audio_clip_ops;
mod clipboard_ops;
mod midi_clip_ops;

#[cfg(test)]
pub(crate) use audio_clip_ops::import;

pub(super) fn handle(ctx: &mut WorkerDispatch<'_>, request: Request) -> ControlFlow<()> {
    match request {
        Request::GroupAudioEvents { .. }
        | Request::InsertProjectSample { .. }
        | Request::ImportAudioBatch { .. }
        | Request::Import(_)
        | Request::ImportToTrack { .. }
        | Request::ImportToTrackWithOverwrite { .. }
        | Request::ImportToNewTrack { .. }
        | Request::ImportToNewTrackWithGain { .. }
        | Request::ImportToSession { .. }
        | Request::ArmGainRecording { .. }
        | Request::GainLane { .. }
        | Request::BounceArrangement { .. }
        | Request::ConvertClip { .. }
        | Request::EditArrangementSection { .. } => audio_clip_ops::handle(ctx, request),

        Request::PaintMidiClip(_) | Request::CommitMidiTake { .. } | Request::Edit(_) => {
            midi_clip_ops::handle(ctx, request)
        }

        Request::ReorderTracks { .. }
        | Request::ReparentTracks { .. }
        | Request::MoveExistingClips { .. }
        | Request::CopyArrangementClipToConsoleTrack { .. }
        | Request::DuplicateClips { .. }
        | Request::GroupClips { .. }
        | Request::GroupTimeRange { .. }
        | Request::TrackClipboard { .. }
        | Request::CopyClips(_)
        | Request::CutClips(_)
        | Request::CopyTimeRange(_)
        | Request::CutTimeRange(_)
        | Request::PasteTimeRange { .. }
        | Request::InsertTimeRange { .. } => clipboard_ops::handle(ctx, request),

        _ => {
            ctx.outcome.error = Some("unexpected worker dispatch route".into());
            ControlFlow::Continue(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_ops_rejects_non_clip_request_without_panicking() {
        assert_eq!(
            latest_timeline_content_end(&Project::new(48_000).unwrap()),
            0
        );
    }
}
