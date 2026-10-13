//! One canonical Arrangement-timeline absolute gain lane per track.
use crate::{BeatTime, Created, Invalid, Project, TrackId};

pub const MAX_GAIN_POINTS_PER_TRACK: usize = 1_024;
pub const MAX_GAIN_POINTS: usize = 65_536;
pub const AUTOMATION_EDIT_UNSUPPORTED: &str = "time transforms require automation support";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GainInterpolation {
    Step,
    Linear,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GainPoint {
    pub beat: BeatTime,
    pub gain: f32,
    pub outgoing: GainInterpolation,
}

pub fn validate_gain_envelope(points: &[GainPoint]) -> Result<(), Invalid> {
    if points.len() > MAX_GAIN_POINTS_PER_TRACK {
        return Err(Invalid("track gain envelope point limit reached"));
    }
    for (index, point) in points.iter().enumerate() {
        if !point.gain.is_finite() || !(0.0..=2.0).contains(&point.gain) {
            return Err(Invalid("automation gain must be finite and within 0..2"));
        }
        if index > 0 && points[index - 1].beat >= point.beat {
            return Err(Invalid("gain envelope beats must strictly increase"));
        }
    }
    Ok(())
}
impl Project {
    pub fn has_gain_automation(&self) -> bool {
        self.tracks()
            .iter()
            .any(|track| !track.gain_envelope().is_empty())
    }
    pub(crate) fn refuse_automation_transform(&self, tracks: &[TrackId]) -> Result<(), Invalid> {
        for &id in tracks {
            let track = self.track(id).ok_or(Invalid("track no longer exists"))?;
            if !track.gain_envelope().is_empty() {
                return Err(Invalid(AUTOMATION_EDIT_UNSUPPORTED));
            }
        }
        Ok(())
    }
    pub(crate) fn replace_gain_envelope(
        &mut self,
        track: TrackId,
        points: Vec<GainPoint>,
    ) -> Result<Created, Invalid> {
        self.track(track).ok_or(Invalid("track no longer exists"))?;
        validate_gain_envelope(&points)?;
        let other = self
            .tracks()
            .iter()
            .filter(|owner| owner.id() != track)
            .try_fold(0_usize, |count, owner| {
                count
                    .checked_add(owner.gain_envelope().len())
                    .ok_or(Invalid("gain point count overflow"))
            })?;
        if other
            .checked_add(points.len())
            .is_none_or(|count| count > MAX_GAIN_POINTS)
        {
            return Err(Invalid("project gain envelope point limit reached"));
        }
        self.track_mut(track)?.gain_envelope = points;
        Ok(Created::None)
    }
    pub(crate) fn upsert_gain_point(
        &mut self,
        track: TrackId,
        point: GainPoint,
    ) -> Result<Created, Invalid> {
        let mut points = self
            .track(track)
            .ok_or(Invalid("track no longer exists"))?
            .gain_envelope
            .clone();
        match points.binary_search_by_key(&point.beat, |point| point.beat) {
            Ok(index) => points[index] = point,
            Err(index) => {
                if points.len() == MAX_GAIN_POINTS_PER_TRACK {
                    return Err(Invalid("track gain envelope point limit reached"));
                }
                points.insert(index, point);
            }
        }
        self.replace_gain_envelope(track, points)
    }
    pub(crate) fn move_gain_point(
        &mut self,
        track: TrackId,
        from: BeatTime,
        to: BeatTime,
    ) -> Result<Created, Invalid> {
        let mut points = self
            .track(track)
            .ok_or(Invalid("track no longer exists"))?
            .gain_envelope
            .clone();
        let index = points
            .binary_search_by_key(&from, |point| point.beat)
            .map_err(|_| Invalid("gain point no longer exists"))?;
        if from != to && points.binary_search_by_key(&to, |point| point.beat).is_ok() {
            return Err(Invalid("gain point destination is occupied"));
        }
        let mut point = points.remove(index);
        point.beat = to;
        let index = points
            .binary_search_by_key(&to, |point| point.beat)
            .unwrap_err();
        points.insert(index, point);
        self.replace_gain_envelope(track, points)
    }
    pub(crate) fn remove_gain_point(
        &mut self,
        track: TrackId,
        beat: BeatTime,
    ) -> Result<Created, Invalid> {
        let mut points = self
            .track(track)
            .ok_or(Invalid("track no longer exists"))?
            .gain_envelope
            .clone();
        let index = points
            .binary_search_by_key(&beat, |point| point.beat)
            .map_err(|_| Invalid("gain point no longer exists"))?;
        points.remove(index);
        self.replace_gain_envelope(track, points)
    }
}
