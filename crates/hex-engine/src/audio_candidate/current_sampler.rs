//! Synchronous borrowing facade for the actual native CURRENT source root.
use super::*;
impl RuntimeOwner {
    pub(super) fn sampler_console_available(&self) -> bool {
        self.sampler_sources.is_some()
            && self.playback.current_sampler_console_available()
            && self.playback.session_capability_status()
                == crate::SessionCapabilityStatus::Available
    }
    pub(super) fn apply_action(
        &mut self,
        action: crate::PlaybackAction,
    ) -> Result<(), crate::PlaybackActionError> {
        if self.sampler_sources.is_some() {
            use crate::{PlaybackAction::Transport, PlaybackCommand};
            match action {
                Transport(
                    PlaybackCommand::Stop | PlaybackCommand::PlayFrom(0) | PlaybackCommand::Seek(0),
                ) => {}
                Transport(PlaybackCommand::Play) if self.playback.position() == 0 => {}
                crate::PlaybackAction::SetMetronome(_) => {}
                crate::PlaybackAction::Session(
                    crate::SessionPlaybackCommand::LaunchClip { .. }
                    | crate::SessionPlaybackCommand::LaunchScene { .. }
                    | crate::SessionPlaybackCommand::StopTrack { .. }
                    | crate::SessionPlaybackCommand::StopAllClips { .. }
                    | crate::SessionPlaybackCommand::BackToArrangement { .. }
                    | crate::SessionPlaybackCommand::CancelPending { .. },
                )
                | crate::PlaybackAction::LaunchAndPlay(
                    crate::SessionPlaybackCommand::LaunchClip { .. }
                    | crate::SessionPlaybackCommand::LaunchScene { .. },
                ) if self.sampler_console_available() => {}
                _ => {
                    return Err(crate::PlaybackActionError::Session(
                        crate::SessionPlaybackError::InvalidCommand,
                    ));
                }
            }
        }
        self.playback.try_apply_action(action)
    }
    pub(super) fn render_with_signal_observers(
        &mut self,
        output: &mut [crate::Frame],
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        project: Option<&mut crate::ProjectOutputTap<'_, '_>>,
    ) -> Result<(), crate::Invalid> {
        if let Some((sources, _)) = self.sampler_sources.as_ref() {
            self.playback
                .render_current_sampler::<dyn crate::MediaPages>(
                    output, None, observer, project, sources,
                )
        } else {
            self.playback
                .render_with_signal_observers(output, observer, project)
        }
    }
    pub(super) fn render_with_pages_and_signal_observers<P: crate::MediaPages + ?Sized>(
        &mut self,
        output: &mut [crate::Frame],
        pages: &mut P,
        observer: Option<&mut crate::NodeSignalObserver<'_>>,
        project: Option<&mut crate::ProjectOutputTap<'_, '_>>,
    ) -> Result<(), crate::Invalid> {
        if let Some((sources, _)) = self.sampler_sources.as_ref() {
            self.playback
                .render_current_sampler(output, Some(pages), observer, project, sources)
        } else {
            self.playback
                .render_with_pages_and_signal_observers(output, pages, observer, project)
        }
    }
}
