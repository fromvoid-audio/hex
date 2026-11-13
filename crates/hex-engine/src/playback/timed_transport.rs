//! Bounded immediate transport replay within a resident output buffer.
use super::{Playback, PlaybackAction, PlaybackActionError, PlaybackCommand};
use crate::{Frame, Invalid};

/// Maximum commands admitted in one timed render call.
pub const MAX_TIMED_TRANSPORT_COMMANDS: usize = 64;

/// A transport command applied before the sample at `offset` in this buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimedTransportCommand {
    pub offset: usize,
    pub command: PlaybackCommand,
}

/// Invalid schedules are refused before any output or playback state changes.
/// Later failures retain earlier actions and completed samples; the remaining
/// output from the failed span is zeroed and no later action is applied.
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
        if commands.len() > MAX_TIMED_TRANSPORT_COMMANDS {
            return Err(TimedTransportError::TooManyCommands);
        }
        let mut previous = 0;
        for (index, command) in commands.iter().enumerate() {
            if command.offset < previous || command.offset > output.len() {
                return Err(TimedTransportError::InvalidOffset { index });
            }
            previous = command.offset;
        }
        let mut cursor = 0;
        for (index, command) in commands.iter().enumerate() {
            if cursor < command.offset {
                if let Err(error) = self.render(&mut output[cursor..command.offset]) {
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
            && let Err(error) = self.render(&mut output[cursor..])
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

#[cfg(test)]
mod tests;
