//! Pure canonical warp-coordinate projection shared by preparation and visual previews.
//! Callers use validated canonical metadata; local arithmetic checks do not
//! replace complete project validation.
use super::{AudioClipDomain, WarpSettings};
use crate::{BeatTime, Invalid};

impl WarpSettings {
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
