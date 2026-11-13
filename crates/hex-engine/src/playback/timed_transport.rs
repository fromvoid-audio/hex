//! Bounded immediate transport replay within a resident output buffer.
use super::{Playback, PlaybackAction, PlaybackActionError, PlaybackCommand, ShiftedForeign};
use crate::{Frame, Invalid, MediaPages};

/// Maximum commands admitted in one timed render call.
pub const MAX_TIMED_TRANSPORT_COMMANDS: usize = 64;

/// A transport command applied before the sample at `offset` in this buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimedTransportCommand {
    pub offset: usize,
    pub command: PlaybackCommand,
}

/// Invalid schedules are refused before any output or playback state changes.
/// Later failures retain earlier actions and skip later actions. The resident
/// renderer preserves completed samples and zeros the remaining output; the
/// foreign renderer rejects its staged callback and zeros the entire output.
#[derive(Debug, PartialEq, Eq)]
pub enum TimedTransportError {
    TooManyCommands,
    InvalidOffset {
        index: usize,
    },
    Action {
        index: usize,
        error: PlaybackActionError,
    },
    Render {
        offset: usize,
        error: Invalid,
    },
}

impl Playback {
    /// Render resident audio while applying immediate commands at sample offsets.
    /// Offsets are nondecreasing and may equal `output.len()`; equal offsets are
    /// applied in input order. A command at the end takes effect after rendering,
    /// including for an empty output. A schedule is limited to
    /// [`MAX_TIMED_TRANSPORT_COMMANDS`]. This uses the same immediate transport
    /// semantics as [`Playback::try_apply_action`], not the callback's declicked
    /// command policy. It does not supply streamed pages or foreign processors.
    ///
    /// Schedule validation leaves output and playback untouched. A later render
    /// or action error is not transactional: earlier samples/actions are kept,
    /// the failing span and suffix are zeroed, and later commands are skipped.
    pub fn render_with_timed_transport(
        &mut self,
        output: &mut [Frame],
        commands: &[TimedTransportCommand],
    ) -> Result<(), TimedTransportError> {
        validate(output.len(), commands)?;
        self.render_timed_validated(output, commands, |playback, part, _| playback.render(part))
    }

    /// Render with borrowed stream pages and immediate timed transport actions.
    /// The page provider is borrowed only for this call. Page misses use the
    /// existing renderer's silence/request behavior; this does not perform
    /// native stream admission, preflight, or page protection. Schedule and
    /// partial-failure semantics match [`Playback::render_with_timed_transport`].
    pub fn render_with_timed_transport_and_pages<P: MediaPages>(
        &mut self,
        output: &mut [Frame],
        commands: &[TimedTransportCommand],
        pages: &mut P,
    ) -> Result<(), TimedTransportError> {
        validate(output.len(), commands)?;
        self.render_timed_validated(output, commands, |playback, part, _| {
            playback.render_with_pages(part, pages)
        })
    }

    /// Render one resident foreign-processor callback with timed transport.
    /// Processor offsets remain relative to the complete output buffer, even
    /// when commands split it. Endpoint ownership and commit remain with the
    /// caller. Schedule refusal leaves output and endpoints untouched; a later
    /// failure rejects the staged foreign callback and clears the whole output.
    /// Earlier transport state changes are not rolled back. Pages, signal taps,
    /// callback admission and declicked command policy are not supplied here.
    pub fn render_with_timed_transport_and_foreign_processors(
        &mut self,
        output: &mut [Frame],
        commands: &[TimedTransportCommand],
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), TimedTransportError> {
        self.render_timed_foreign(output, commands, None, foreign)
    }

    /// Render one timed foreign callback while borrowing streamed pages.
    /// Source/page readiness and endpoint custody remain with the caller.
    /// Foreign offsets and failure semantics match
    /// [`Playback::render_with_timed_transport_and_foreign_processors`];
    /// page misses retain the existing renderer's silence/request behavior.
    pub fn render_with_timed_transport_pages_and_foreign_processors(
        &mut self,
        output: &mut [Frame],
        commands: &[TimedTransportCommand],
        pages: &mut dyn MediaPages,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), TimedTransportError> {
        self.render_timed_foreign(output, commands, Some(pages), foreign)
    }

    fn render_timed_foreign(
        &mut self,
        output: &mut [Frame],
        commands: &[TimedTransportCommand],
        mut pages: Option<&mut dyn MediaPages>,
        foreign: &mut dyn hex_lattice::ForeignProcessors,
    ) -> Result<(), TimedTransportError> {
        validate(output.len(), commands)?;
        let result = self.render_timed_validated(output, commands, |playback, part, offset| {
            let mut shifted = ShiftedForeign {
                inner: foreign,
                offset,
            };
            playback.render_with_foreign_processors(
                part,
                pages
                    .as_mut()
                    .map(|pages| &mut **pages as &mut dyn MediaPages),
                None,
                None,
                &mut shifted,
            )
        });
        if result.is_err() {
            // Revocation is idempotent; the inner renderer may already have
            // rejected, or may fail after a successfully rendered part.
            foreign.reject_callback();
            output.fill([0.0; 2]);
        }
        result
    }

    fn render_timed_validated(
        &mut self,
        output: &mut [Frame],
        commands: &[TimedTransportCommand],
        mut render: impl FnMut(&mut Self, &mut [Frame], usize) -> Result<(), Invalid>,
    ) -> Result<(), TimedTransportError> {
        let mut cursor = 0;
        for (index, command) in commands.iter().enumerate() {
            if cursor < command.offset {
                if let Err(error) = render(self, &mut output[cursor..command.offset], cursor) {
                    output[cursor..].fill([0.0; 2]);
                    return Err(TimedTransportError::Render {
                        offset: cursor,
                        error,
                    });
                }
                cursor = command.offset;
            }
            if let Err(error) = self.try_apply_action(PlaybackAction::Transport(command.command)) {
                output[cursor..].fill([0.0; 2]);
                return Err(TimedTransportError::Action { index, error });
            }
        }
        if cursor < output.len()
            && let Err(error) = render(self, &mut output[cursor..], cursor)
        {
            output[cursor..].fill([0.0; 2]);
            return Err(TimedTransportError::Render {
                offset: cursor,
                error,
            });
        }
        Ok(())
    }
}

fn validate(length: usize, commands: &[TimedTransportCommand]) -> Result<(), TimedTransportError> {
    if commands.len() > MAX_TIMED_TRANSPORT_COMMANDS {
        return Err(TimedTransportError::TooManyCommands);
    }
    let mut previous = 0;
    for (index, command) in commands.iter().enumerate() {
        if command.offset < previous || command.offset > length {
            return Err(TimedTransportError::InvalidOffset { index });
        }
        previous = command.offset;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
