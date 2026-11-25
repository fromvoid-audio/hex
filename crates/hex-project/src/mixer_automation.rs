//! Canonical Arrangement mixer automation lanes for track balance/pan, mute,
//! and return send levels, following the bounded `GainLane` ownership contract.
use crate::{AutomationInterpolation, BeatTime, Created, Invalid, Project, ReturnId, TrackId};

pub const MAX_MIXER_AUTOMATION_POINTS_PER_LANE: usize = 1_024;
pub const MAX_MIXER_AUTOMATION_POINTS: usize = 65_536;

/// Canonical mixer parameter targeted by a track-owned automation lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MixerLaneTarget {
    /// Track stereo balance in `-1.0..=1.0`.
    Balance,
    /// Track mute automation in `0.0..=1.0` (`>= 0.5` mutes output).
    Mute,
    /// Track post-fader return send level in `0.0..=2.0`.
    Send(ReturnId),
}

/// One validated breakpoint in a track mixer automation lane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MixerAutomationPoint {
    pub beat: BeatTime,
    pub value: f32,
    pub outgoing: AutomationInterpolation,
}

/// Canonical ordered breakpoint lane for a single track mixer parameter.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MixerAutomationLane {
    pub(crate) points: Vec<MixerAutomationPoint>,
}

impl MixerAutomationLane {
    pub fn new(
        target: MixerLaneTarget,
        points: Vec<MixerAutomationPoint>,
    ) -> Result<Self, Invalid> {
        validate_mixer_points(target, &points)?;
        Ok(Self { points })
    }

    pub fn points(&self) -> &[MixerAutomationPoint] {
        &self.points
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn validate(&self, target: MixerLaneTarget) -> Result<(), Invalid> {
        validate_mixer_points(target, &self.points)
    }

    /// Evaluate a validated mixer lane at canonical Arrangement beat time.
    /// Returns `None` when empty (the static track control owns playback).
    pub fn value_at(&self, beat: BeatTime) -> Option<f32> {
        let first = self.points.first()?;
        let next = self.points.partition_point(|point| point.beat <= beat);
        if next == 0 {
            return Some(first.value);
        }
        let left = &self.points[next - 1];
        if beat == left.beat {
            return Some(left.value);
        }
        let Some(right) = self.points.get(next) else {
            return Some(left.value);
        };
        if left.outgoing == AutomationInterpolation::Step {
            return Some(left.value);
        }
        let span = right.beat.raw().checked_sub(left.beat.raw())?;
        if span == 0 {
            return Some(left.value);
        }
        let elapsed = beat.raw().checked_sub(left.beat.raw())?;
        let fraction = elapsed as f64 / span as f64;
        Some(
            left.outgoing
                .interpolate(f64::from(left.value), f64::from(right.value), fraction)
                as f32,
        )
    }
}

/// Snapshot transport for one track's return-send automation lane.
#[derive(Clone, Debug, PartialEq)]
pub struct SendAutomationSnapshot {
    pub return_track: u64,
    pub points: Vec<MixerAutomationPoint>,
}

/// Snapshot transport for one track's balance, mute, and send automation lanes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackMixerAutomationSnapshot {
    pub track: u64,
    pub balance: Vec<MixerAutomationPoint>,
    pub mute: Vec<MixerAutomationPoint>,
    pub sends: Vec<SendAutomationSnapshot>,
}

pub fn validate_mixer_points(
    target: MixerLaneTarget,
    points: &[MixerAutomationPoint],
) -> Result<(), Invalid> {
    if points.len() > MAX_MIXER_AUTOMATION_POINTS_PER_LANE {
        return Err(Invalid("mixer automation lane point limit reached"));
    }
    let (min, max, err) = match target {
        MixerLaneTarget::Balance => (
            -1.0_f32,
            1.0_f32,
            "balance automation value must be finite and within -1..1",
        ),
        MixerLaneTarget::Mute => (
            0.0_f32,
            1.0_f32,
            "mute automation value must be finite and within 0..1",
        ),
        MixerLaneTarget::Send(_) => (
            0.0_f32,
            2.0_f32,
            "send automation value must be finite and within 0..2",
        ),
    };
    for (index, point) in points.iter().enumerate() {
        if !point.value.is_finite() || !(min..=max).contains(&point.value) {
            return Err(Invalid(err));
        }
        if index > 0 && points[index - 1].beat >= point.beat {
            return Err(Invalid("mixer automation beats must strictly increase"));
        }
    }
    Ok(())
}

impl Project {
    pub fn has_mixer_automation(&self) -> bool {
        self.tracks.iter().any(|track| track.has_mixer_automation())
    }

    pub(crate) fn mixer_automation_point_count(&self) -> Result<usize, Invalid> {
        let mut total = 0_usize;
        for track in &self.tracks {
            total = total
                .checked_add(track.balance_lane.points.len())
                .and_then(|n| n.checked_add(track.mute_lane.points.len()))
                .ok_or(Invalid("mixer automation point count overflow"))?;
            for (_, lane) in &track.send_lanes {
                total = total
                    .checked_add(lane.points.len())
                    .ok_or(Invalid("mixer automation point count overflow"))?;
            }
        }
        Ok(total)
    }

    pub(crate) fn set_track_mixer_automation(
        &mut self,
        track: TrackId,
        target: MixerLaneTarget,
        points: Vec<MixerAutomationPoint>,
    ) -> Result<Created, Invalid> {
        validate_mixer_points(target, &points)?;
        if let MixerLaneTarget::Send(return_id) = target
            && !self.returns.iter().any(|bus| bus.id == return_id)
        {
            return Err(Invalid("return track no longer exists"));
        }
        let owner = self.track(track).ok_or(Invalid("track no longer exists"))?;
        let old_len = match target {
            MixerLaneTarget::Balance => owner.balance_lane.points.len(),
            MixerLaneTarget::Mute => owner.mute_lane.points.len(),
            MixerLaneTarget::Send(return_id) => owner
                .send_lane(return_id)
                .map_or(0, |lane| lane.points.len()),
        };
        let total = self
            .mixer_automation_point_count()?
            .saturating_sub(old_len)
            .checked_add(points.len())
            .ok_or(Invalid("mixer automation point count overflow"))?;
        if total > MAX_MIXER_AUTOMATION_POINTS {
            return Err(Invalid("project mixer automation point limit reached"));
        }
        let lane = MixerAutomationLane { points };
        let owner = self.track_mut(track)?;
        match target {
            MixerLaneTarget::Balance => owner.balance_lane = lane,
            MixerLaneTarget::Mute => owner.mute_lane = lane,
            MixerLaneTarget::Send(return_id) => {
                match owner
                    .send_lanes
                    .binary_search_by_key(&return_id, |(id, _)| *id)
                {
                    Ok(index) => {
                        if lane.is_empty() {
                            owner.send_lanes.remove(index);
                        } else {
                            owner.send_lanes[index].1 = lane;
                        }
                    }
                    Err(index) => {
                        if !lane.is_empty() {
                            owner.send_lanes.insert(index, (return_id, lane));
                        }
                    }
                }
            }
        }
        Ok(Created::None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Created, CurveTension, Editor};

    fn q(beats: u64) -> BeatTime {
        BeatTime::from_quarters(beats).unwrap()
    }

    #[test]
    fn curve_tension_validation_and_monotonic_shaping() {
        assert!(CurveTension::from_millis(-1001).is_err());
        assert!(CurveTension::from_millis(1001).is_err());
        assert!(CurveTension::from_f32(f32::NAN).is_err());
        assert!(CurveTension::from_f32(1.5).is_err());

        let linear = CurveTension::LINEAR;
        let convex = CurveTension::from_millis(600).unwrap();
        let concave = CurveTension::from_millis(-600).unwrap();

        assert!((linear.shape(0.5) - 0.5).abs() < 1e-12);
        assert!(convex.shape(0.5) < 0.5);
        assert!(concave.shape(0.5) > 0.5);
        assert_eq!(convex.shape(0.0), 0.0);
        assert_eq!(convex.shape(1.0), 1.0);
        assert!(convex.shape(0.25) < convex.shape(0.5));
        assert!(convex.shape(0.5) < convex.shape(0.75));
    }

    #[test]
    fn mixer_lanes_validate_evaluate_and_undo_redo() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack {
                name: "synth".into(),
            })
            .unwrap()
        else {
            panic!("expected track");
        };
        let Created::ReturnTrack(ret) = editor
            .execute(Command::AddReturnTrack {
                name: "reverb".into(),
            })
            .unwrap()
        else {
            panic!("expected return");
        };

        // Invalid values rejected cleanly without mutating state.
        assert!(
            editor
                .execute(Command::SetTrackMixerAutomation {
                    track,
                    target: MixerLaneTarget::Balance,
                    points: vec![MixerAutomationPoint {
                        beat: q(0),
                        value: 1.5,
                        outgoing: AutomationInterpolation::Linear,
                    }],
                })
                .is_err()
        );
        assert!(
            editor
                .execute(Command::SetTrackMixerAutomation {
                    track,
                    target: MixerLaneTarget::Mute,
                    points: vec![MixerAutomationPoint {
                        beat: q(0),
                        value: -0.1,
                        outgoing: AutomationInterpolation::Step,
                    }],
                })
                .is_err()
        );

        let curve = AutomationInterpolation::Curve(CurveTension::from_millis(500).unwrap());
        editor
            .execute(Command::SetTrackMixerAutomation {
                track,
                target: MixerLaneTarget::Balance,
                points: vec![
                    MixerAutomationPoint {
                        beat: q(0),
                        value: -1.0,
                        outgoing: curve,
                    },
                    MixerAutomationPoint {
                        beat: q(4),
                        value: 1.0,
                        outgoing: AutomationInterpolation::Linear,
                    },
                ],
            })
            .unwrap();
        editor
            .execute(Command::SetTrackMixerAutomation {
                track,
                target: MixerLaneTarget::Mute,
                points: vec![
                    MixerAutomationPoint {
                        beat: q(0),
                        value: 0.0,
                        outgoing: AutomationInterpolation::Step,
                    },
                    MixerAutomationPoint {
                        beat: q(2),
                        value: 1.0,
                        outgoing: AutomationInterpolation::Step,
                    },
                ],
            })
            .unwrap();
        editor
            .execute(Command::SetTrackMixerAutomation {
                track,
                target: MixerLaneTarget::Send(ret),
                points: vec![
                    MixerAutomationPoint {
                        beat: q(0),
                        value: 0.0,
                        outgoing: AutomationInterpolation::Linear,
                    },
                    MixerAutomationPoint {
                        beat: q(4),
                        value: 1.0,
                        outgoing: AutomationInterpolation::Step,
                    },
                ],
            })
            .unwrap();

        let t = editor.project().track(track).unwrap();
        // Convex curve from -1 to +1 at midpoint 0.5 must be < 0.0.
        let mid_pan = t.balance_lane().value_at(q(2)).unwrap();
        assert!(mid_pan < 0.0 && mid_pan > -1.0, "mid_pan={mid_pan}");
        assert_eq!(t.mute_lane().value_at(q(1)), Some(0.0));
        assert_eq!(t.mute_lane().value_at(q(2)), Some(1.0));
        assert!((t.send_lane(ret).unwrap().value_at(q(2)).unwrap() - 0.5).abs() < 1e-6);

        // Snapshot round-trip preserves all mixer lanes.
        let restored = Project::from_snapshot(editor.project().snapshot()).unwrap();
        assert_eq!(&restored, editor.project());

        // Undo & Redo preserve send lane cleanly.
        assert!(editor.undo());
        assert!(
            editor
                .project()
                .track(track)
                .unwrap()
                .send_lane(ret)
                .is_none()
        );
        assert!(editor.redo());
        assert!(
            editor
                .project()
                .track(track)
                .unwrap()
                .send_lane(ret)
                .is_some()
        );

        // Removing the return track prunes its send lane.
        editor.execute(Command::RemoveReturnTrack(ret)).unwrap();
        assert!(
            editor
                .project()
                .track(track)
                .unwrap()
                .send_lanes()
                .is_empty()
        );
    }
}
