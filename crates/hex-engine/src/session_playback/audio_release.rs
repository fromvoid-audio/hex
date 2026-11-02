//! Bounded fade-out of the Session audio clip a route has just left.
//!
//! A launch, stop, scene replace or Back to Arrangement that moves a playing
//! route away from a Session audio clip does not cut that clip. The route
//! keeps rendering the outgoing clip on its own launch clock for
//! `F = transport_fade::fade_frames(rate)` frames (128 at 48 kHz, the shared
//! transport declick length) under the linear gain `(F - elapsed) / F`, where
//! `elapsed` counts frames from the transition frame. The first release frame
//! therefore has full gain and the frame at `F` is silent, so the largest step
//! a full-scale DC clip produces is `1 / F`. The incoming owner starts at full
//! level, keeping its first transient as Play from frame zero does. Gain is a
//! per-frame function of the absolute performance frame, so output does not
//! depend on render block size and no extra block split is needed.
//!
//! Custody: each route holds at most one release in its `Copy` route record,
//! so command and render transactions stage it in the existing route scratch
//! and commit or discard it with the route. No allocation, lock or drop occurs
//! on the audio thread. A release is a current-route contribution of its
//! track: it renders through the same source stage, mix and page provider as
//! the owner it replaces, for resident and streamed clips alike, and stream
//! demand covers its pages. Stop and seek clear it with the other release
//! history. A revision handoff keeps it only while the new revision still has
//! the same clip on the same track; otherwise it ends with the replaced source.
//!
//! Capacity: an active release counts one entry against the shared Session
//! release capacity, and a pending transition away from Session audio reserves
//! one. An owner that leaves at its own launch frame never sounded and needs
//! no release, so commands applied together between two blocks always
//! compose. A transition that would begin a second release while the route's
//! previous release is still sounding (the clip that replaced it played for
//! fewer than `F` frames) is refused with `ReleaseBankFull` when it is
//! admitted, never cut at render time.

use super::{RouteOwner, SessionPlayback, SessionPlaybackError, TrackRoute};
use hex_project::ClipId;

impl SessionPlayback {
    /// The shared transport declick length at this revision's rate.
    pub(super) fn audio_release_fade(&self) -> u32 {
        crate::playback::transport_fade::fade_frames(self.revision.arrangement().sample_rate())
    }
}

/// One outgoing Session audio clip fading on its original launch clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AudioRouteRelease {
    pub(super) clip: ClipId,
    pub(super) launch_frame: u64,
    start_frame: u64,
    end_frame: u64,
}

impl AudioRouteRelease {
    /// Whether this release still sounds at or after `frame`.
    pub(super) fn active_at(self, frame: u64) -> bool {
        frame < self.end_frame
    }

    pub(super) fn end_frame(self) -> u64 {
        self.end_frame
    }

    /// Linear fade gain at absolute performance `frame`; zero outside the
    /// release window.
    pub(super) fn gain_at(self, frame: u64) -> f32 {
        fade_gain(self.start_frame, self.end_frame, frame)
    }
}

/// Starts the route's release when `previous` was a Session audio owner that
/// the route has just left at `frame`. `fade` is the shared declick length.
pub(super) fn begin(
    route: &mut TrackRoute,
    previous: RouteOwner,
    frame: u64,
    fade: u32,
) -> Result<(), SessionPlaybackError> {
    let RouteOwner::SessionAudio { clip, launch_frame } = previous else {
        return Ok(());
    };
    // An owner launched at this frame has not produced a sample yet, so it
    // leaves without a release; same-frame command sequences stay admitted.
    if route.owner == previous || launch_frame >= frame {
        return Ok(());
    }
    if route
        .audio_release
        .is_some_and(|release| release.active_at(frame))
    {
        return Err(SessionPlaybackError::ReleaseBankFull);
    }
    route.audio_release = Some(AudioRouteRelease {
        clip,
        launch_frame,
        start_frame: frame,
        end_frame: frame
            .checked_add(u64::from(fade.max(1)))
            .ok_or(SessionPlaybackError::ArithmeticOverflow)?,
    });
    Ok(())
}

/// Whether a release beginning at `frame` would replace one still sounding.
pub(super) fn conflicts(route: &TrackRoute, frame: u64) -> bool {
    matches!(route.owner, RouteOwner::SessionAudio { launch_frame, .. } if launch_frame < frame)
        && route
            .audio_release
            .is_some_and(|release| release.active_at(frame))
}

/// Release entries this route charges against the shared capacity at
/// `frame`: its sounding release plus a reservation for a pending transition
/// away from Session audio.
pub(super) fn charged(route: &TrackRoute, frame: u64) -> usize {
    usize::from(
        route
            .audio_release
            .is_some_and(|release| release.active_at(frame)),
    ) + usize::from(
        matches!(route.owner, RouteOwner::SessionAudio { .. }) && route.pending.is_some(),
    )
}

/// `(end - frame) / (end - start)` inside `start..end`, zero outside.
fn fade_gain(start_frame: u64, end_frame: u64, frame: u64) -> f32 {
    if frame < start_frame || frame >= end_frame {
        return 0.0;
    }
    ((end_frame - frame) as f64 / (end_frame - start_frame) as f64) as f32
}

#[cfg(test)]
mod tests {
    use super::fade_gain;

    #[test]
    fn gain_ramps_linearly_from_full_to_silent_over_fade() {
        let gains: Vec<f32> = (99..106).map(|frame| fade_gain(100, 104, frame)).collect();
        assert_eq!(gains, [0.0, 1.0, 0.75, 0.5, 0.25, 0.0, 0.0]);
    }
}
