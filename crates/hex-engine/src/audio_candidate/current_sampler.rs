//! Synchronous borrowing facade for the actual native CURRENT source root.
use super::*;
impl RuntimeOwner {
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
