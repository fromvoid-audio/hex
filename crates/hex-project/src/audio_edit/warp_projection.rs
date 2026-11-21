//! Pure canonical warp-coordinate projection shared by preparation and visual previews.
//! Callers use validated canonical metadata; local arithmetic checks do not
//! replace complete project validation.
use super::{AudioClipDomain, WarpMarker, WarpSettings};
use crate::{BeatTime, Invalid};

pub(crate) fn validate_warp_markers(
    markers: &[WarpMarker],
    source_start: u64,
    source_end: u64,
) -> Result<(), Invalid> {
    if !(2..=super::MAX_AUDIO_POINTS).contains(&markers.len())
        || markers.first().is_none_or(|marker| {
            marker.source_frame != source_start || marker.beat != BeatTime::ZERO
        })
        || markers
            .last()
            .is_none_or(|marker| marker.source_frame != source_end || marker.beat == BeatTime::ZERO)
        || markers.windows(2).any(|pair| {
            pair[0].source_frame >= pair[1].source_frame || pair[0].beat >= pair[1].beat
        })
    {
        return Err(Invalid(
            "warp markers need increasing source/beat positions and explicit region endpoints",
        ));
    }
    Ok(())
}

impl WarpSettings {
    /// Add an interior anchor at its current mapped beat, preserving the map
    /// on both sides. Returns false when the source point is already pinned.
    pub fn insert_marker(&mut self, source_frame: u64) -> Result<bool, Invalid> {
        let index = match self
            .markers
            .binary_search_by_key(&source_frame, |m| m.source_frame)
        {
            Ok(_) => return Ok(false),
            Err(index) if index > 0 && index < self.markers.len() => index,
            Err(_) => return Err(Invalid("warp markers must be inside the source region")),
        };
        if self.markers.len() >= super::MAX_AUDIO_POINTS {
            return Err(Invalid("warp is limited to 1024 markers"));
        }
        let beat = self.beat_for_source_frame(source_frame)?;
        if !(self.markers[index - 1].beat < beat && beat < self.markers[index].beat) {
            return Err(Invalid("warp marker is too close to a neighboring beat"));
        }
        self.markers
            .try_reserve(1)
            .map_err(|_| Invalid("could not allocate a warp marker"))?;
        self.markers
            .insert(index, super::WarpMarker { source_frame, beat });
        Ok(true)
    }

    /// Move the authored beat of a pinned source frame between its neighbors.
    /// The endpoints of the region stay fixed.
    pub fn move_marker(&mut self, source_frame: u64, beat: BeatTime) -> Result<bool, Invalid> {
        let index = self.interior_marker_index(source_frame)?;
        let beat = self.marker_target_beat(source_frame, beat)?;
        let changed = self.markers[index].beat != beat;
        self.markers[index].beat = beat;
        Ok(changed)
    }

    /// Resolve a drag for an existing or prospective interior source anchor.
    /// Pointer previews and the committed move share the same neighboring bounds.
    pub fn marker_target_beat(
        &self,
        source_frame: u64,
        beat: BeatTime,
    ) -> Result<BeatTime, Invalid> {
        let (index, next) = match self
            .markers
            .binary_search_by_key(&source_frame, |m| m.source_frame)
        {
            Ok(index) => (index, index + 1),
            Err(index) => (index, index),
        };
        let left = index
            .checked_sub(1)
            .and_then(|index| self.markers.get(index));
        let right = self.markers.get(next);
        let (Some(left), Some(right)) = (left, right) else {
            return Err(Invalid("warp markers must be inside the source region"));
        };
        let first = left
            .beat
            .raw()
            .checked_add(1)
            .ok_or(Invalid("warp marker has no room to move"))?;
        let last = right
            .beat
            .raw()
            .checked_sub(1)
            .filter(|last| *last >= first)
            .ok_or(Invalid("warp marker has no room to move"))?;
        Ok(BeatTime::from_raw(beat.raw().clamp(first, last)))
    }

    pub fn remove_marker(&mut self, source_frame: u64) -> Result<(), Invalid> {
        let index = self.interior_marker_index(source_frame)?;
        self.markers.remove(index);
        Ok(())
    }

    fn interior_marker_index(&self, source_frame: u64) -> Result<usize, Invalid> {
        self.markers
            .binary_search_by_key(&source_frame, |m| m.source_frame)
            .ok()
            .filter(|index| *index > 0 && *index + 1 < self.markers.len())
            .ok_or(Invalid(
                "only interior warp markers can be moved or removed",
            ))
    }

    /// Local beat for an absolute project-rate source frame. The region's
    /// endpoint markers define the bounds; no PCM access or analysis occurs.
    pub fn beat_for_source_frame(&self, source: u64) -> Result<BeatTime, Invalid> {
        let markers = &self.markers;
        if markers.len() < 2 {
            return Err(Invalid("warp needs two endpoints"));
        }
        let index = markers
            .partition_point(|m| m.source_frame <= source)
            .saturating_sub(1)
            .min(markers.len() - 2);
        let a = markers[index];
        let b = markers[index + 1];
        let delta = source
            .checked_sub(a.source_frame)
            .ok_or(Invalid("source precedes warp marker"))?;
        let frames = b
            .source_frame
            .checked_sub(a.source_frame)
            .filter(|v| *v > 0)
            .ok_or(Invalid("warp source markers must increase"))?;
        let beats = b
            .beat
            .raw()
            .checked_sub(a.beat.raw())
            .filter(|v| *v > 0)
            .ok_or(Invalid("warp beat markers must increase"))?;
        let value =
            u128::from(delta) * u128::from(beats) / u128::from(frames) + u128::from(a.beat.raw());
        Ok(BeatTime::from_raw(
            u64::try_from(value).map_err(|_| Invalid("source beat projection overflow"))?,
        ))
    }
}
impl AudioClipDomain {
    /// Source-relative, unwrapped Q32 frame position at a local beat. Loops
    /// retain their cycle count; callers wrap only when indexing resident peaks
    /// or PCM. This is the preparation mapping law, independent of project tempo.
    pub fn warp_source_position_q32(&self, beat: u128) -> Result<i128, Invalid> {
        let domain = self;
        let warp = domain
            .warp
            .as_ref()
            .ok_or(Invalid("warp settings missing"))?;
        if warp.markers.len() < 2 {
            return Err(Invalid("warp needs two endpoints"));
        }
        let unit = warp
            .markers
            .last()
            .ok_or(Invalid("warp endpoint missing"))?
            .beat
            .raw();
        if unit == 0 {
            return Err(Invalid("warp beat span must be positive"));
        }
        let (cycle, local) = if domain.looping {
            (beat / u128::from(unit), (beat % u128::from(unit)) as u64)
        } else {
            (
                0,
                u64::try_from(beat).map_err(|_| Invalid("warp beat position overflow"))?,
            )
        };
        let index = warp
            .markers
            .partition_point(|m| m.beat.raw() <= local)
            .saturating_sub(1)
            .min(warp.markers.len() - 2);
        let a = warp.markers[index];
        let b = warp.markers[index + 1];
        let marker_offset = a
            .source_frame
            .checked_sub(domain.source_offset)
            .ok_or(Invalid("warp marker precedes source region"))?;
        let base = cycle
            .checked_mul(u128::from(domain.source_length))
            .and_then(|n| n.checked_add(u128::from(marker_offset)))
            .and_then(|n| n.checked_mul(1_u128 << 32))
            .ok_or(Invalid("warp source cycle overflow"))?;
        let beat_delta = local
            .checked_sub(a.beat.raw())
            .ok_or(Invalid("warp beat precedes marker"))?;
        let source_delta = b
            .source_frame
            .checked_sub(a.source_frame)
            .filter(|v| *v > 0)
            .ok_or(Invalid("warp source markers must increase"))?;
        let marker_span = b
            .beat
            .raw()
            .checked_sub(a.beat.raw())
            .filter(|v| *v > 0)
            .ok_or(Invalid("warp beat markers must increase"))?;
        let delta = u128::from(beat_delta)
            .checked_mul(u128::from(source_delta))
            .and_then(|n| n.checked_mul(1_u128 << 32))
            .ok_or(Invalid("warp source projection overflow"))?
            / u128::from(marker_span);
        i128::try_from(
            base.checked_add(delta)
                .ok_or(Invalid("warp source position overflow"))?,
        )
        .map_err(|_| Invalid("warp source position overflow"))
    }

    /// Project the authored loop phase into the same marker clock.
    pub fn warp_phase_beat(&self) -> Result<u64, Invalid> {
        self.warp
            .as_ref()
            .ok_or(Invalid("warp settings missing"))?
            .beat_for_source_frame(
                self.source_offset
                    .checked_add(self.source_phase)
                    .ok_or(Invalid("warp phase overflow"))?,
            )
            .map(BeatTime::raw)
    }
}

#[cfg(test)]
mod marker_tests {
    use super::*;
    use crate::{WarpMarker, WarpMode};
    #[test]
    fn pins_retime_only_between_fixed_neighbors() {
        let mut warp = WarpSettings {
            mode: WarpMode::RePitch,
            markers: vec![
                WarpMarker {
                    source_frame: 20,
                    beat: BeatTime::ZERO,
                },
                WarpMarker {
                    source_frame: 120,
                    beat: BeatTime::ONE,
                },
            ],
        };
        let original = warp.clone();
        assert!(warp.insert_marker(70).unwrap());
        assert_eq!(warp.markers[1].beat, BeatTime::from_ratio(1, 2).unwrap());
        assert!(!warp.insert_marker(70).unwrap());
        assert!(warp.insert_marker(2).is_err());
        let target = BeatTime::from_ratio(3, 4).unwrap();
        assert!(warp.move_marker(70, target).unwrap());
        assert_eq!(warp.beat_for_source_frame(70).unwrap(), target);
        assert_eq!(warp.markers[0], original.markers[0]);
        assert_eq!(warp.markers[2], original.markers[1]);
        assert!(warp.move_marker(20, target).is_err());
        assert!(warp.remove_marker(120).is_err());
        warp.remove_marker(70).unwrap();
        assert_eq!(warp, original);
    }
}
