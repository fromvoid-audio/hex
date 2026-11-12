//! Frozen control-side gain cohorts. Same finite dB delta means one linear
//! multiplier applied to each original value, then independently clamped to 0..=2.
//! Reversals always use the originals; zero members stay silent. A silent anchor
//! is intentionally a single-track interaction and cannot form a relative cohort.
//! This bounded ephemeral value is never part of a saved project or callback.
use crate::{Command, Created, Invalid, MAX_TRACKS, Project, Track, TrackId};

const _: () = assert!(MAX_TRACKS <= u16::MAX as usize);

/// Immutable, validated starting gains, sorted by stable track identity.
/// Fixed inline metadata keeps the keyed control coalescer Copy and bounded;
/// target lookup does not traverse project content or allocate during repaint.
#[derive(Clone, Copy, PartialEq)]
pub struct RelativeTrackGains {
    anchor: TrackId,
    anchor_gain: f32,
    len: u16,
    ids: [u64; MAX_TRACKS],
    starts: [f32; MAX_TRACKS],
}
impl std::fmt::Debug for RelativeTrackGains {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelativeTrackGains")
            .field("anchor", &self.anchor)
            .field("anchor_gain", &self.anchor_gain)
            .field("members", &self.len)
            .finish_non_exhaustive()
    }
}
impl RelativeTrackGains {
    /// Capture existing selected tracks; authored gain automation is ineligible.
    /// Missing/duplicate/oversized membership refuses before any model change.
    pub fn capture(
        project: &Project,
        anchor: TrackId,
        selected: &[TrackId],
    ) -> Result<Self, Invalid> {
        if selected.is_empty() || selected.len() > MAX_TRACKS {
            return Err(Invalid("selected gain cohort is empty or too large"));
        }
        let anchor_track = project
            .track(anchor)
            .ok_or(Invalid("track no longer exists"))?;
        if !eligible(anchor_track) || anchor_track.gain() <= 0.0 {
            return Err(Invalid(
                "relative gain needs an unautomated positive anchor",
            ));
        }
        let mut selected = selected.to_vec();
        selected.sort_unstable();
        if selected.windows(2).any(|pair| pair.first() == pair.get(1))
            || !selected.contains(&anchor)
        {
            return Err(Invalid(
                "selected gain cohort has duplicate tracks or no anchor",
            ));
        }
        let mut result = Self {
            anchor,
            anchor_gain: anchor_track.gain(),
            len: 0,
            ids: [0; MAX_TRACKS],
            starts: [0.0; MAX_TRACKS],
        };
        for id in selected {
            let track = project.track(id).ok_or(Invalid("track no longer exists"))?;
            if !eligible(track) {
                continue;
            }
            let index = usize::from(result.len);
            *result
                .ids
                .get_mut(index)
                .ok_or(Invalid("selected gain cohort is too large"))? = id.value();
            *result
                .starts
                .get_mut(index)
                .ok_or(Invalid("selected gain cohort is too large"))? = track.gain();
            result.len = result
                .len
                .checked_add(1)
                .ok_or(Invalid("selected gain cohort is too large"))?;
        }
        Ok(result)
    }
    pub fn anchor(&self) -> TrackId {
        self.anchor
    }
    pub fn anchor_gain(&self) -> f32 {
        self.anchor_gain
    }
    pub fn len(&self) -> usize {
        usize::from(self.len)
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn entries(&self) -> impl Iterator<Item = (TrackId, f32)> + '_ {
        self.ids
            .iter()
            .zip(&self.starts)
            .take(self.len())
            .map(|(&id, &gain)| (TrackId(id), gain))
    }
    pub fn starting_gain(&self, track: TrackId) -> Option<f32> {
        let index = self
            .ids
            .get(..self.len())?
            .binary_search(&track.value())
            .ok()?;
        self.starts.get(index).copied()
    }
    /// Reject stale or newly automated members together. Unrelated names/order
    /// and other tracks' edits do not change the captured gain identities.
    pub fn validate_current(&self, project: &Project) -> Result<(), Invalid> {
        for (id, gain) in self.entries() {
            let track = project
                .track(id)
                .ok_or(Invalid("selected gain track no longer exists"))?;
            if !eligible(track) || track.gain() != gain {
                return Err(Invalid("selected track gains changed during the gesture"));
            }
        }
        Ok(())
    }
    pub fn gain_for(&self, track: TrackId, anchor_gain: f32) -> Result<Option<f32>, Invalid> {
        validate_gain(anchor_gain)?;
        Ok(self
            .starting_gain(track)
            .map(|start| self.scale(start, anchor_gain)))
    }
    pub fn values(&self, anchor_gain: f32) -> Result<Vec<(TrackId, f32)>, Invalid> {
        validate_gain(anchor_gain)?;
        Ok(self
            .entries()
            .map(|(id, start)| (id, self.scale(start, anchor_gain)))
            .collect())
    }
    pub fn command(&self, project: &Project, anchor_gain: f32) -> Result<Command, Invalid> {
        self.validate_current(project)?;
        Ok(Command::SetTrackGains {
            gains: self.values(anchor_gain)?,
        })
    }
    fn scale(&self, start: f32, target: f32) -> f32 {
        // f64 keeps the ratio finite even for a positive f32 subnormal anchor.
        // Clamp before narrowing so a loud member cannot overflow to infinity.
        (f64::from(start) * (f64::from(target) / f64::from(self.anchor_gain))).clamp(0.0, 2.0)
            as f32
    }
}

pub(crate) fn eligible(track: &Track) -> bool {
    track.gain_envelope().is_empty() && track.gain_lane().spans().is_empty()
}
fn validate_gain(gain: f32) -> Result<(), Invalid> {
    if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
        return Err(Invalid("gain must be finite and within 0..2"));
    }
    Ok(())
}
/// Validate every destination before any write. After validation the write pass
/// cannot fail and never allocates. Same seam serves ordinary single-track gain.
pub(crate) fn apply(project: &mut Project, gains: &[(TrackId, f32)]) -> Result<Created, Invalid> {
    if gains.is_empty() || gains.len() > MAX_TRACKS {
        return Err(Invalid("selected gain cohort is empty or too large"));
    }
    for (index, &(id, gain)) in gains.iter().enumerate() {
        validate_gain(gain)?;
        if gains.iter().take(index).any(|(prior, _)| *prior == id) {
            return Err(Invalid("selected gain cohort has duplicate tracks"));
        }
        let track = project.track(id).ok_or(Invalid("track no longer exists"))?;
        if !eligible(track) {
            return Err(Invalid("track gain is owned by automation"));
        }
    }
    for target in &mut project.tracks {
        if let Some((_, gain)) = gains.iter().find(|(id, _)| *id == target.id) {
            target.gain = *gain;
        }
    }
    Ok(Created::None)
}

#[cfg(test)]
mod tests;
