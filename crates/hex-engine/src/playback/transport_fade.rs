//! Declicked transport stops and resumes for `Playback`.
//!
//! A declicked Pause or Stop does not cut the output at its command frame.
//! The transport keeps rendering for `F = max(1, round(rate * 128 / 48000))`
//! frames (about 2.7 ms) under a linear ramp to zero, and the command applies
//! only at that boundary. The reported pause position is therefore the
//! command frame plus `F`, the audible end. Stop keeps its reset to frame
//! zero; its audible end is reported only by the completed fade outcome
//! (`TransportFadeCompletion`), never as the transport position. The paused
//! or stopped output that
//! follows (release tails, monitoring) ramps back in over `F` frames, as does
//! Play from a paused, nonzero position. Play from frame zero keeps its first
//! transient.
//!
//! A declicked transport Seek while playing fades out the same way at the old
//! position, applies the jump at the fade end and ramps in over `F` frames at
//! the target. The reported position runs on from the command frame through
//! the fade and is the seek target from the fade end onward; the completion
//! reports the old position's audible end.
//!
//! The owner splits a render block at the fade end so the result does not
//! depend on block size. Any other action, including a direct immediate one,
//! completes a pending fade first. `apply_priority_stop` cuts immediately.
//!
//! Gain is applied to the project mix before monitor-only metronome and tuning
//! audition audio. Device and project-output taps observe the pre-fade signal.
//! The state is a few integers: no allocation, locking or ownership transfer.

use crate::{Frame, PlaybackAction, PlaybackActionRejection, PlaybackCommand};

/// Reference ramp length at 48 kHz.
const REFERENCE_FADE_FRAMES: u64 = 128;
const REFERENCE_RATE: u64 = 48_000;

/// Rate-scaled declick length: `round(rate * 128 / 48000)`, at least one frame.
pub(crate) fn fade_frames(rate: u32) -> u32 {
    let scaled = (u64::from(rate) * REFERENCE_FADE_FRAMES + REFERENCE_RATE / 2) / REFERENCE_RATE;
    u32::try_from(scaled.max(1)).unwrap_or(u32::MAX)
}

/// Pause/Stop actions whose audible cut is replaced by a fade.
pub(crate) fn fades_out(action: PlaybackAction) -> bool {
    matches!(
        action,
        PlaybackAction::Transport(PlaybackCommand::Pause | PlaybackCommand::Stop)
            | PlaybackAction::Session(
                crate::SessionPlaybackCommand::Pause | crate::SessionPlaybackCommand::Stop
            )
    )
}

/// Transport seeks, which fade out, jump and ramp back in while playing.
pub(crate) fn seeks(action: PlaybackAction) -> bool {
    matches!(action, PlaybackAction::Transport(PlaybackCommand::Seek(_)))
}

/// Pause actions, whose paused output ramps back in after the fade.
pub(crate) fn pauses(action: PlaybackAction) -> bool {
    matches!(
        action,
        PlaybackAction::Transport(PlaybackCommand::Pause)
            | PlaybackAction::Session(crate::SessionPlaybackCommand::Pause)
    )
}

/// Play actions that resume an existing transport position.
pub(crate) fn resumes(action: PlaybackAction) -> bool {
    matches!(
        action,
        PlaybackAction::Transport(PlaybackCommand::Play)
            | PlaybackAction::Session(crate::SessionPlaybackCommand::Play)
    )
}

/// Compact deferred action; `Playback` is moved by value through publication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pending {
    Pause,
    Stop,
    SessionPause,
    SessionStop,
    Seek(u64),
}

impl Pending {
    const fn from_action(action: PlaybackAction) -> Option<Self> {
        match action {
            PlaybackAction::Transport(PlaybackCommand::Pause) => Some(Self::Pause),
            PlaybackAction::Transport(PlaybackCommand::Stop) => Some(Self::Stop),
            PlaybackAction::Session(crate::SessionPlaybackCommand::Pause) => {
                Some(Self::SessionPause)
            }
            PlaybackAction::Session(crate::SessionPlaybackCommand::Stop) => Some(Self::SessionStop),
            PlaybackAction::Transport(PlaybackCommand::Seek(frame)) => Some(Self::Seek(frame)),
            _ => None,
        }
    }

    const fn action(self) -> PlaybackAction {
        match self {
            Self::Pause => PlaybackAction::Transport(PlaybackCommand::Pause),
            Self::Stop => PlaybackAction::Transport(PlaybackCommand::Stop),
            Self::SessionPause => PlaybackAction::Session(crate::SessionPlaybackCommand::Pause),
            Self::SessionStop => PlaybackAction::Session(crate::SessionPlaybackCommand::Stop),
            Self::Seek(frame) => PlaybackAction::Transport(PlaybackCommand::Seek(frame)),
        }
    }
}

/// Outcome of a deferred Pause/Stop applied at its fade end, or completed
/// early by a later action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportFadeCompletion {
    /// The deferred Pause/Stop that was applied.
    pub action: PlaybackAction,
    /// Transport frame at which the fade ended: the command frame plus `F`,
    /// or earlier when another action completed the fade first. After Stop
    /// this differs from the reset transport position.
    pub audible_end_frame: u64,
    /// False when the action was refused at the fade end and Playback fell
    /// back to Stop (see `take_transport_fade_failure`).
    pub applied: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct TransportFade {
    /// The deferred Pause/Stop, applied once `out_elapsed` reaches `F`.
    pending: Option<Pending>,
    out_elapsed: u32,
    /// Frames left in the ramp up from silence.
    in_left: u32,
    /// A deferred action that could not apply at its fade end.
    failure: Option<PlaybackActionRejection>,
    /// The latest completed deferred action, until taken.
    completion: Option<TransportFadeCompletion>,
}

impl TransportFade {
    pub(crate) fn pending(&self) -> Option<PlaybackAction> {
        self.pending.map(Pending::action)
    }

    /// Defer a Pause/Stop (`fades_out`) or Seek (`seeks`); any other action
    /// defers nothing.
    pub(crate) fn begin_out(&mut self, action: PlaybackAction) {
        self.pending = Pending::from_action(action);
        self.out_elapsed = 0;
    }

    pub(crate) fn begin_in(&mut self, frames: u32) {
        self.in_left = frames;
    }

    /// Remove the pending action; the caller applies it immediately.
    pub(crate) fn take_pending(&mut self) -> Option<PlaybackAction> {
        self.out_elapsed = 0;
        self.pending.take().map(Pending::action)
    }

    pub(crate) fn clear(&mut self) {
        *self = Self {
            failure: self.failure,
            completion: self.completion,
            ..Self::default()
        };
    }

    pub(crate) fn record_completion(&mut self, completion: TransportFadeCompletion) {
        self.completion = Some(completion);
    }

    pub(crate) fn take_completion(&mut self) -> Option<TransportFadeCompletion> {
        self.completion.take()
    }

    pub(crate) fn record_failure(&mut self, rejection: PlaybackActionRejection) {
        self.failure = Some(rejection);
    }

    pub(crate) fn take_failure(&mut self) -> Option<PlaybackActionRejection> {
        self.failure.take()
    }

    /// Frames to render before the next boundary at which state changes.
    pub(crate) fn part_len(&self, remaining: usize, frames: u32) -> usize {
        if self.pending.is_some() {
            let left = frames.saturating_sub(self.out_elapsed).max(1);
            remaining.min(usize::try_from(left).unwrap_or(usize::MAX))
        } else {
            remaining
        }
    }

    /// Apply this part's gain and advance both ramps. Returns true when the
    /// pending action's fade has completed and it must now be applied.
    pub(crate) fn apply(&mut self, output: &mut [Frame], frames: u32) -> bool {
        if self.pending.is_none() && self.in_left == 0 {
            return false;
        }
        let total = frames.max(1);
        let scale = 1.0 / total as f32;
        for frame in output.iter_mut() {
            let mut gain = 1.0;
            if self.pending.is_some() {
                // The owner splits at the fade end; a longer part stays silent.
                self.out_elapsed = self.out_elapsed.saturating_add(1).min(total);
                gain *= (total - self.out_elapsed) as f32 * scale;
            }
            if self.in_left > 0 {
                gain *= (total.saturating_sub(self.in_left) + 1).min(total) as f32 * scale;
                self.in_left -= 1;
            }
            frame[0] *= gain;
            frame[1] *= gain;
        }
        self.pending.is_some() && self.out_elapsed >= total
    }
}

#[cfg(test)]
mod tests;
