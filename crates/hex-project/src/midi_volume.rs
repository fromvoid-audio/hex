//! Clip-owned, source-beat volume curves. Note/source identity is independent of
//! this immutable control metadata; replacing a curve must not restart a voice.
//! A separate clip scalar multiplies the curve without changing its points.
//! Rendering projects these values through the existing source/tempo contracts.
use crate::{BeatTime, Invalid, MidiSourcePlayback};
use std::sync::Arc;

pub const MAX_MIDI_VOLUME_POINTS_PER_CLIP: usize = 1_024;
pub const MAX_MIDI_VOLUME_POINTS: usize = 65_536;

/// Finite linear amplitude in 0..=4. Negative zero is canonicalized so equality
/// is exact and suitable for history and current-format snapshots.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MidiClipGain(f64);
impl Eq for MidiClipGain {}
impl MidiClipGain {
    pub const UNITY: Self = Self(1.0);
    pub fn new(gain: f64) -> Result<Self, Invalid> {
        if !gain.is_finite() || !(0.0..=4.0).contains(&gain) {
            return Err(Invalid("clip volume must be finite and within 0..4"));
        }
        Ok(Self(if gain == 0.0 { 0.0 } else { gain }))
    }
    pub const fn linear(self) -> f64 {
        self.0
    }
}

/// A unique source-beat boundary. Interpolation approaches `incoming`; exactly
/// at the boundary the value becomes `value`. Ordinary authored points use the
/// same gain for both. Separate limits preserve loop/split/overwrite jumps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiClipVolumePoint {
    pub beat: BeatTime,
    pub incoming: MidiClipGain,
    pub value: MidiClipGain,
}
impl MidiClipVolumePoint {
    pub const fn linear(beat: BeatTime, gain: MidiClipGain) -> Self {
        Self {
            beat,
            incoming: gain,
            value: gain,
        }
    }
}

/// An empty curve is unity. Copies share immutable storage; editing one clip
/// replaces its curve without changing another clip or its MIDI source.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MidiClipVolume {
    points: Option<Arc<[MidiClipVolumePoint]>>,
    clip_gain: crate::ClipGain,
}
impl MidiClipVolume {
    pub fn new(points: Vec<MidiClipVolumePoint>, source_length: BeatTime) -> Result<Self, Invalid> {
        Self::validate_points(&points, source_length)?;
        Ok(Self {
            points: (!points.is_empty()).then(|| points.into()),
            clip_gain: crate::ClipGain::UNITY,
        })
    }
    /// Independent clip amplitude; this never changes authored curve points.
    pub fn clip_gain(&self) -> crate::ClipGain {
        self.clip_gain
    }
    /// Replaces the clip amplitude without mutating the source-beat curve.
    pub fn with_clip_gain(mut self, gain: crate::ClipGain) -> Self {
        self.clip_gain = gain;
        self
    }
    /// True only when neither the clip scalar nor its curve changes audio.
    pub fn is_unity(&self) -> bool {
        self.clip_gain == crate::ClipGain::UNITY && self.points.is_none()
    }
    fn validate_points(
        points: &[MidiClipVolumePoint],
        source_length: BeatTime,
    ) -> Result<(), Invalid> {
        if points.len() > MAX_MIDI_VOLUME_POINTS_PER_CLIP {
            return Err(Invalid("clip volume point limit reached"));
        }
        if source_length == BeatTime::ZERO {
            return Err(Invalid("midi source length must be positive"));
        }
        for (index, point) in points.iter().enumerate() {
            if point.beat > source_length {
                return Err(Invalid("clip volume point exceeds source length"));
            }
            if index > 0 && points[index - 1].beat >= point.beat {
                return Err(Invalid("clip volume beats must strictly increase"));
            }
        }
        Ok(())
    }
    pub fn validate_source_length(&self, length: BeatTime) -> Result<(), Invalid> {
        Self::validate_points(self.points(), length)
    }
    pub fn points(&self) -> &[MidiClipVolumePoint] {
        self.points.as_deref().unwrap_or(&[])
    }
    /// Identity for prepared-cache invalidation; note identity remains separate.
    pub fn same_revision(&self, other: &Self) -> bool {
        if self.clip_gain != other.clip_gain {
            return false;
        }
        match (&self.points, &other.points) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
    pub fn retained_control_metadata_bytes(&self) -> Option<usize> {
        if self.points.is_none() {
            return Some(0);
        }
        std::mem::size_of_val(self.points())
            .checked_add(2usize.checked_mul(std::mem::size_of::<usize>())?)?
            .checked_add(
                std::mem::align_of::<usize>()
                    .max(std::mem::align_of::<MidiClipVolumePoint>())
                    .checked_sub(1)?,
            )
    }
    /// Held endpoints, linear interiors, and right-continuous boundary values.
    pub fn gain_at(&self, beat: BeatTime) -> MidiClipGain {
        self.sample(beat, false)
    }
    /// The left limit is needed when materializing a half-open source interval.
    pub fn gain_before(&self, beat: BeatTime) -> MidiClipGain {
        self.sample(beat, true)
    }
    fn sample(&self, beat: BeatTime, before: bool) -> MidiClipGain {
        let points = self.points();
        let index = points.partition_point(|point| point.beat < beat);
        if let Some(point) = points.get(index)
            && point.beat == beat
        {
            return if before { point.incoming } else { point.value };
        }
        let Some(previous) = index.checked_sub(1).and_then(|i| points.get(i)) else {
            return points.first().map_or(MidiClipGain::UNITY, |p| p.incoming);
        };
        let Some(next) = points.get(index) else {
            return previous.value;
        };
        if previous.value == next.incoming {
            return previous.value;
        }
        let fraction = (beat.raw() - previous.beat.raw()) as f64
            / (next.beat.raw() - previous.beat.raw()) as f64;
        MidiClipGain(previous.value.0 + (next.incoming.0 - previous.value.0) * fraction)
    }
    /// Materialize a finite elapsed interval into a new one-shot source. This
    /// control-side projection preserves loop seam jumps and bounds generated
    /// points before allocating/enumerating repeated source boundaries. Authored
    /// points remain editable metadata even when their gains are equal.
    pub fn materialized(
        &self,
        playback: MidiSourcePlayback,
        start: BeatTime,
        end: BeatTime,
    ) -> Result<Self, Invalid> {
        let length = end
            .raw()
            .checked_sub(start.raw())
            .filter(|n| *n > 0)
            .ok_or(Invalid("clip volume interval must be positive"))?;
        self.validate_source_length(playback.source_length())?;
        if end > playback.arrangement_duration() {
            return Err(Invalid("clip volume interval exceeds finite playback"));
        }
        if self.points().is_empty() {
            return Ok(Self::default().with_clip_gain(self.clip_gain));
        }
        if let Some(forward) = playback.unreversed() {
            // Enumerate the same bounded forward interval once, then reflect
            // its complete curve. Reconstructing a forward law at the reversed
            // initial phase would walk the wrong authored points and seams.
            let duration = playback.arrangement_duration().raw();
            let reflected_start = BeatTime::from_raw(duration - end.raw());
            let reflected_end = BeatTime::from_raw(duration - start.raw());
            let mut projected = self.materialized(forward, reflected_start, reflected_end)?;
            if let Some(points) = &mut projected.points {
                let points = Arc::make_mut(points);
                points.reverse();
                for point in points.iter_mut() {
                    point.beat = BeatTime::from_raw(length - point.beat.raw());
                    std::mem::swap(&mut point.incoming, &mut point.value);
                }
                if let Some(first) = points.first_mut() {
                    first.incoming = first.value;
                }
                if let Some(last) = points.last_mut() {
                    // The forward materializer normalizes its initial incoming
                    // limit; recover that limit for the reflected end point.
                    last.value = self.gain_at_elapsed(forward, reflected_start, true);
                }
            }
            return Ok(projected);
        }
        let mut boundaries = Vec::new();
        boundaries
            .try_reserve_exact(MAX_MIDI_VOLUME_POINTS_PER_CLIP)
            .map_err(|_| Invalid("could not reserve clip volume projection"))?;
        boundaries.push(start.raw());
        boundaries.push(end.raw());
        let mut add = |raw| -> Result<(), Invalid> {
            if start.raw() < raw
                && raw < end.raw()
                && let Err(index) = boundaries.binary_search(&raw)
            {
                if boundaries.len() == MAX_MIDI_VOLUME_POINTS_PER_CLIP {
                    return Err(Invalid("materialized clip volume point limit reached"));
                }
                boundaries.insert(index, raw);
            }
            Ok(())
        };
        // Rebase on the actual phase so a very late retained interval never
        // enumerates its discarded loop prefix. The existing projection owns
        // first-pass and repeated-onset behavior.
        if let Some(phase) = playback.source_phase_at_elapsed(start) {
            let projected = MidiSourcePlayback::new_with_initial_end(
                playback.source_length(),
                phase,
                playback.loop_range(),
                BeatTime::from_raw(length),
                playback.initial_end(),
            )?;
            for point in self.points() {
                if point.beat == playback.source_length() {
                    continue;
                }
                let plan = projected.note_occurrences(point.beat, BeatTime::from_raw(1))?;
                for occurrence in plan.iter() {
                    add(start
                        .raw()
                        .checked_add(occurrence.timeline_start().raw())
                        .ok_or(Invalid("clip volume projection overflow"))?)?;
                }
            }
        }
        if let Some((loop_start, loop_end)) = playback.loop_range() {
            let first = loop_end.raw() - playback.source_start().raw();
            let period = loop_end.raw() - loop_start.raw();
            let absolute_start = playback
                .playback_offset()
                .raw()
                .checked_add(start.raw())
                .ok_or(Invalid("clip volume window overflow"))?;
            let skipped = absolute_start.saturating_sub(first) / period;
            let mut seam = first
                .checked_add(
                    skipped
                        .checked_mul(period)
                        .ok_or(Invalid("clip volume loop projection overflow"))?,
                )
                .ok_or(Invalid("clip volume loop projection overflow"))?;
            let absolute_end = playback
                .playback_offset()
                .raw()
                .checked_add(end.raw())
                .ok_or(Invalid("clip volume window overflow"))?;
            while seam < absolute_end {
                if let Some(local) = seam.checked_sub(playback.playback_offset().raw()) {
                    add(local)?;
                }
                let Some(next) = seam.checked_add(period) else {
                    break;
                };
                seam = next;
            }
        } else {
            if let Some(local) = (playback.initial_end().raw() - playback.source_start().raw())
                .checked_sub(playback.playback_offset().raw())
            {
                add(local)?;
            }
        }
        boundaries.sort_unstable();
        boundaries.dedup();
        let mut points = Vec::new();
        points
            .try_reserve_exact(boundaries.len())
            .map_err(|_| Invalid("could not reserve materialized clip volume"))?;
        for raw in boundaries {
            let beat = BeatTime::from_raw(raw);
            let value = self.gain_at_elapsed(playback, beat, false);
            points.push(MidiClipVolumePoint {
                beat: BeatTime::from_raw(raw - start.raw()),
                incoming: if raw == start.raw() {
                    value
                } else {
                    self.gain_at_elapsed(playback, beat, true)
                },
                value,
            });
        }
        Self::new(points, BeatTime::from_raw(length)).map(|v| v.with_clip_gain(self.clip_gain))
    }
    fn gain_at_elapsed(
        &self,
        playback: MidiSourcePlayback,
        elapsed: BeatTime,
        before: bool,
    ) -> MidiClipGain {
        let absolute = playback.playback_offset().raw().checked_add(elapsed.raw());
        if before && elapsed != BeatTime::ZERO {
            if let Some((start, end)) = playback.loop_range() {
                let first = end.raw() - playback.source_start().raw();
                if absolute.is_some_and(|absolute| {
                    absolute >= first && (absolute - first) % (end.raw() - start.raw()) == 0
                }) {
                    return self.gain_before(end);
                }
            } else if absolute == Some(playback.initial_end().raw() - playback.source_start().raw())
            {
                return self.gain_before(playback.initial_end());
            }
        }
        match playback.source_phase_at_elapsed(elapsed) {
            Some(phase) if before => self.gain_before(phase),
            Some(phase) => self.gain_at(phase),
            None => self.gain_at(playback.initial_end()),
        }
    }
    /// Copy a source-local half-open interval immediately after itself, keeping
    /// the original curve on both sides of the overwritten destination. Boundary
    /// limits make that replacement exact even when either boundary is a jump.
    pub fn duplicated_range(
        &self,
        start: BeatTime,
        end: BeatTime,
        source_length: BeatTime,
    ) -> Result<Self, Invalid> {
        let delta = end
            .raw()
            .checked_sub(start.raw())
            .filter(|n| *n > 0)
            .ok_or(Invalid("clip volume range must be positive"))?;
        let destination_end = end
            .raw()
            .checked_add(delta)
            .ok_or(Invalid("clip volume range destination overflow"))?;
        if destination_end > source_length.raw() {
            return Err(Invalid("clip volume range exceeds source length"));
        }
        if self.points().is_empty() {
            return Ok(Self::default().with_clip_gain(self.clip_gain));
        }
        let mut points = Vec::new();
        points
            .try_reserve_exact(MAX_MIDI_VOLUME_POINTS_PER_CLIP)
            .map_err(|_| Invalid("could not reserve duplicated clip volume"))?;
        let mut push = |point| -> Result<(), Invalid> {
            if points.len() == MAX_MIDI_VOLUME_POINTS_PER_CLIP {
                return Err(Invalid("duplicated clip volume point limit reached"));
            }
            points.push(point);
            Ok(())
        };
        for &point in self.points().iter().filter(|point| point.beat < end) {
            push(point)?;
        }
        push(MidiClipVolumePoint {
            beat: end,
            incoming: self.gain_before(end),
            value: self.gain_at(start),
        })?;
        for &point in self
            .points()
            .iter()
            .filter(|point| start < point.beat && point.beat < end)
        {
            push(MidiClipVolumePoint {
                beat: BeatTime::from_raw(
                    point
                        .beat
                        .raw()
                        .checked_add(delta)
                        .ok_or(Invalid("clip volume point overflow"))?,
                ),
                ..point
            })?;
        }
        push(MidiClipVolumePoint {
            beat: BeatTime::from_raw(destination_end),
            incoming: self.gain_before(end),
            value: self.gain_at(BeatTime::from_raw(destination_end)),
        })?;
        for &point in self
            .points()
            .iter()
            .filter(|point| point.beat.raw() > destination_end)
        {
            push(point)?;
        }
        Self::new(points, source_length).map(|v| v.with_clip_gain(self.clip_gain))
    }
    /// Prepending silence moves authored source coordinates without changing
    /// their values or sharing mutable storage with another clip.
    pub fn shifted(&self, prefix: BeatTime, length: BeatTime) -> Result<Self, Invalid> {
        if prefix == BeatTime::ZERO {
            self.validate_source_length(length)?;
            return Ok(self.clone());
        }
        let mut points = Vec::new();
        points
            .try_reserve_exact(self.points().len())
            .map_err(|_| Invalid("could not reserve clip volume points"))?;
        for point in self.points() {
            points.push(MidiClipVolumePoint {
                beat: BeatTime::from_raw(
                    point
                        .beat
                        .raw()
                        .checked_add(prefix.raw())
                        .ok_or(Invalid("clip volume beat overflow"))?,
                ),
                ..*point
            });
        }
        Self::new(points, length).map(|v| v.with_clip_gain(self.clip_gain))
    }
}

impl crate::Project {
    pub fn midi_clip_volume(&self, clip: crate::ClipId) -> Option<&MidiClipVolume> {
        self.midi_clip(clip)
            .map(|clip| clip.volume())
            .or_else(|| self.session_midi_clip(clip).map(|clip| clip.volume()))
    }
    pub(crate) fn validate_midi_volumes(&self) -> Result<(), Invalid> {
        let mut count = 0_usize;
        let arrangement = self
            .tracks()
            .iter()
            .flat_map(|track| track.midi_clips())
            .map(|clip| (clip.volume(), clip.source_length()));
        let console = self
            .session
            .cells
            .iter()
            .filter_map(|cell| match cell.content() {
                crate::SessionCellContent::Midi(clip) => {
                    Some((clip.volume(), clip.source().source_length()))
                }
                _ => None,
            });
        for (volume, length) in arrangement.chain(console) {
            volume.validate_source_length(length)?;
            count = count
                .checked_add(volume.points().len())
                .filter(|n| *n <= MAX_MIDI_VOLUME_POINTS)
                .ok_or(Invalid("project clip volume point limit reached"))?;
        }
        Ok(())
    }
    pub(crate) fn replace_midi_clip_volume(
        &mut self,
        clip: crate::ClipId,
        volume: MidiClipVolume,
    ) -> Result<crate::Created, Invalid> {
        let gain = self
            .midi_clip_volume(clip)
            .ok_or(Invalid("midi clip no longer exists"))?
            .clip_gain();
        let volume = volume.with_clip_gain(gain);
        self.replace_midi_clip_volume_and_gain(clip, volume)
    }

    pub(crate) fn set_midi_clip_gain(
        &mut self,
        clip: crate::ClipId,
        gain: crate::ClipGain,
    ) -> Result<crate::Created, Invalid> {
        let volume = self
            .midi_clip_volume(clip)
            .ok_or(Invalid("midi clip no longer exists"))?
            .clone()
            .with_clip_gain(gain);
        self.replace_midi_clip_volume_and_gain(clip, volume)
    }

    fn replace_midi_clip_volume_and_gain(
        &mut self,
        clip: crate::ClipId,
        volume: MidiClipVolume,
    ) -> Result<crate::Created, Invalid> {
        let source = self
            .midi_source(clip)
            .ok_or(Invalid("midi clip no longer exists"))?;
        volume.validate_source_length(source.source_length())?;
        // This method acts on the existing private transaction candidate. A
        // failed aggregate check never publishes project/audio/history changes.
        if self.midi_clip(clip).is_some() {
            let clips = self.midi_clips_mut(clip)?;
            let owner = clips
                .iter_mut()
                .find(|owner| owner.id() == clip)
                .ok_or(Invalid("midi clip no longer exists"))?;
            owner.volume = volume;
        } else {
            let owner = self
                .session
                .cells
                .iter_mut()
                .find_map(|cell| match &mut cell.content {
                    crate::SessionCellContent::Midi(owner) if owner.id() == clip => Some(owner),
                    _ => None,
                })
                .ok_or(Invalid("midi clip no longer exists"))?;
            owner.volume = volume;
        }
        self.validate_midi_volumes()?;
        Ok(crate::Created::None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn point(raw: u64, incoming: f64, value: f64) -> MidiClipVolumePoint {
        MidiClipVolumePoint {
            beat: BeatTime::from_raw(raw),
            incoming: MidiClipGain::new(incoming).unwrap(),
            value: MidiClipGain::new(value).unwrap(),
        }
    }
    #[test]
    fn scalar_survives_curve_projection_and_empty_curve_operations() {
        let gain = crate::ClipGain::new(-12_000).unwrap();
        let length = BeatTime::from_raw(8);
        let curve = MidiClipVolume::new(vec![point(0, 0.0, 0.0), point(8, 1.0, 1.0)], length)
            .unwrap()
            .with_clip_gain(gain);
        let shifted = curve
            .shifted(BeatTime::from_raw(2), BeatTime::from_raw(10))
            .unwrap();
        assert_eq!(shifted.clip_gain(), gain);
        let copied = curve
            .duplicated_range(BeatTime::from_raw(2), BeatTime::from_raw(4), length)
            .unwrap();
        assert_eq!(copied.clip_gain(), gain);
        let empty = MidiClipVolume::default().with_clip_gain(gain);
        let playback = MidiSourcePlayback::one_shot(length).unwrap();
        assert_eq!(
            empty
                .materialized(playback, BeatTime::ZERO, length)
                .unwrap()
                .clip_gain(),
            gain
        );
        assert_eq!(
            empty
                .duplicated_range(BeatTime::from_raw(2), BeatTime::from_raw(4), length)
                .unwrap()
                .clip_gain(),
            gain
        );
        assert!(!empty.is_unity());
        assert!(!empty.same_revision(&MidiClipVolume::default()));
    }
    #[test]
    fn fractional_ramp_and_jump_keep_both_boundary_limits_and_shared_copy() {
        let curve = MidiClipVolume::new(
            vec![point(2, 0.0, 0.0), point(5, 1.0, 0.5), point(8, 0.5, 0.5)],
            BeatTime::from_raw(8),
        )
        .unwrap();
        assert_eq!(curve.gain_at(BeatTime::ZERO).linear(), 0.0);
        assert_eq!(curve.gain_at(BeatTime::from_raw(3)).linear(), 1.0 / 3.0);
        assert_eq!(curve.gain_before(BeatTime::from_raw(5)).linear(), 1.0);
        assert_eq!(curve.gain_at(BeatTime::from_raw(5)).linear(), 0.5);
        assert_eq!(curve.gain_at(BeatTime::from_raw(u64::MAX)).linear(), 0.5);
        let copy = curve.clone();
        assert!(curve.same_revision(&copy));
        let shifted = copy
            .shifted(BeatTime::from_raw(7), BeatTime::from_raw(15))
            .unwrap();
        assert_eq!(
            shifted.gain_at(BeatTime::from_raw(10)),
            curve.gain_at(BeatTime::from_raw(3))
        );
        assert!(!curve.same_revision(&shifted));
        assert_eq!(curve.points()[0].beat.raw(), 2);
    }
    #[test]
    fn materialized_constant_curve_retains_authored_points_including_unity() {
        let length = BeatTime::from_raw(6);
        let playback = MidiSourcePlayback::new(length, BeatTime::ZERO, None, length).unwrap();
        for gain in [0.5, 1.0] {
            let curve = MidiClipVolume::new(
                vec![
                    point(1, gain, gain),
                    point(3, gain, gain),
                    point(5, gain, gain),
                ],
                length,
            )
            .unwrap();
            let retained = curve
                .materialized(playback, BeatTime::from_raw(1), BeatTime::from_raw(5))
                .unwrap();
            assert_eq!(
                retained.points(),
                &[
                    point(0, gain, gain),
                    point(2, gain, gain),
                    point(4, gain, gain)
                ]
            );
            let repeated = MidiSourcePlayback::new(
                length,
                BeatTime::ZERO,
                Some((BeatTime::ZERO, length)),
                BeatTime::from_raw(4096),
            )
            .unwrap();
            assert!(
                curve
                    .materialized(repeated, BeatTime::ZERO, BeatTime::from_raw(4096))
                    .is_err()
            );
            assert_eq!(curve.points().len(), 3);
        }
    }
    #[test]
    fn materialized_elapsed_window_retains_loop_seam_limits() {
        let curve = MidiClipVolume::new(
            vec![
                point(0, 0.0, 0.0),
                point(2, 1.0, 1.0),
                point(4, 3.0, 3.0),
                point(6, 0.5, 0.5),
            ],
            BeatTime::from_raw(8),
        )
        .unwrap();
        let original = MidiSourcePlayback::new(
            BeatTime::from_raw(8),
            BeatTime::from_raw(2),
            Some((BeatTime::from_raw(2), BeatTime::from_raw(6))),
            BeatTime::from_raw(20),
        )
        .unwrap();
        let window = MidiSourcePlayback::new(
            BeatTime::from_raw(8),
            BeatTime::from_raw(2),
            original.loop_range(),
            BeatTime::from_raw(8),
        )
        .unwrap()
        .with_playback_offset(BeatTime::from_raw(3))
        .unwrap();
        let expected = curve
            .materialized(original, BeatTime::from_raw(3), BeatTime::from_raw(11))
            .unwrap();
        let actual = curve
            .materialized(window, BeatTime::ZERO, BeatTime::from_raw(8))
            .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(actual.gain_before(BeatTime::from_raw(1)).linear(), 0.5);
        assert_eq!(actual.gain_at(BeatTime::from_raw(1)).linear(), 1.0);
    }
    #[test]
    fn materialized_loop_interval_preserves_ramps_seams_and_late_range_without_prefix_walk() {
        let curve = MidiClipVolume::new(
            vec![
                point(0, 0.0, 0.0),
                point(2, 1.0, 1.0),
                point(4, 3.0, 3.0),
                point(6, 0.5, 0.5),
            ],
            BeatTime::from_raw(8),
        )
        .unwrap();
        let playback = MidiSourcePlayback::new(
            BeatTime::from_raw(8),
            BeatTime::from_raw(2),
            Some((BeatTime::from_raw(2), BeatTime::from_raw(6))),
            BeatTime::from_raw(u64::MAX),
        )
        .unwrap();
        let copy = curve
            .materialized(playback, BeatTime::from_raw(3), BeatTime::from_raw(11))
            .unwrap();
        let expected = [1.75, 1.0, 2.0, 3.0, 1.75, 1.0, 2.0, 3.0, 1.75];
        for (raw, gain) in expected.into_iter().enumerate() {
            assert_eq!(copy.gain_at(BeatTime::from_raw(raw as u64)).linear(), gain);
        }
        assert_eq!(copy.gain_before(BeatTime::from_raw(1)).linear(), 0.5);
        assert_eq!(copy.gain_at(BeatTime::from_raw(1)).linear(), 1.0);
        let late = curve
            .materialized(
                playback,
                BeatTime::from_raw(u64::MAX - 8),
                BeatTime::from_raw(u64::MAX - 1),
            )
            .unwrap();
        assert_eq!(late.gain_at(BeatTime::ZERO).linear(), 1.75);
        assert_eq!(late.gain_at(BeatTime::from_raw(1)).linear(), 1.0);
        let tiny_loop = MidiSourcePlayback::new(
            BeatTime::ONE,
            BeatTime::ZERO,
            Some((BeatTime::ZERO, BeatTime::from_raw(1))),
            BeatTime::ONE,
        )
        .unwrap();
        assert!(
            curve
                .materialized(tiny_loop, BeatTime::ZERO, BeatTime::from_raw(1025))
                .is_err()
        );
    }
    #[test]
    fn reversed_materialization_retains_ramps_authored_jumps_and_loop_seam_limits() {
        let b = BeatTime::from_raw;
        let curve = MidiClipVolume::new(
            vec![
                point(0, 0.0, 0.0),
                point(2, 1.0, 1.0),
                point(4, 3.0, 2.5),
                point(6, 0.5, 0.5),
            ],
            b(8),
        )
        .unwrap();
        let playback = MidiSourcePlayback::new(b(8), b(2), Some((b(2), b(6))), b(8))
            .unwrap()
            .with_content_window(crate::ContentWindow::new(3, 11, true).unwrap())
            .unwrap();
        let reversed = curve.materialized(playback, b(0), b(8)).unwrap();
        // The source ramp descends; both authored and loop discontinuities
        // exchange their incoming/value sides under half-open reflection.
        for (raw, gain) in [1.5, 3.0, 2.0, 0.5, 1.5, 3.0, 2.0, 0.5, 1.5]
            .into_iter()
            .enumerate()
        {
            assert_eq!(reversed.gain_at(b(raw as u64)).linear(), gain);
        }
        assert_eq!(reversed.gain_before(b(1)).linear(), 2.5);
        assert_eq!(reversed.gain_before(b(3)).linear(), 1.0);
        assert_eq!(
            reversed.points().first().unwrap().incoming,
            reversed.points().first().unwrap().value
        );
        let partial = curve.materialized(playback, b(1), b(7)).unwrap();
        assert_eq!(partial.gain_at(b(0)).linear(), 3.0);
        assert_eq!(partial.gain_before(b(2)).linear(), 1.0);
        assert_eq!(partial.gain_at(b(2)).linear(), 0.5);
        assert_eq!(partial.gain_at(b(6)).linear(), 0.5);
        assert_eq!(curve.points()[2], point(4, 3.0, 2.5));
    }
    #[test]
    fn source_range_copy_replaces_only_destination_and_preserves_both_limits() {
        let curve = MidiClipVolume::new(
            vec![point(0, 0.0, 0.0), point(4, 4.0, 4.0), point(8, 0.0, 0.0)],
            BeatTime::from_raw(8),
        )
        .unwrap();
        let copy = curve
            .duplicated_range(
                BeatTime::from_raw(1),
                BeatTime::from_raw(3),
                BeatTime::from_raw(8),
            )
            .unwrap();
        for (raw, gain) in [0.0, 1.0, 2.0, 1.0, 2.0, 3.0, 2.0, 1.0, 0.0]
            .into_iter()
            .enumerate()
        {
            assert_eq!(copy.gain_at(BeatTime::from_raw(raw as u64)).linear(), gain);
        }
        assert_eq!(copy.gain_before(BeatTime::from_raw(3)).linear(), 3.0);
        assert_eq!(curve.gain_at(BeatTime::from_raw(3)).linear(), 3.0);
        assert!(!copy.same_revision(&curve));
    }
    #[test]
    fn malformed_gain_domain_order_capacity_and_shift_are_refused() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.1, 4.1] {
            assert!(MidiClipGain::new(value).is_err());
        }
        assert_eq!(
            MidiClipGain::new(-0.0).unwrap().linear().to_bits(),
            0.0_f64.to_bits()
        );
        let length = BeatTime::from_raw(4);
        for points in [
            vec![point(5, 1.0, 1.0)],
            vec![point(2, 1.0, 1.0); 2],
            vec![point(3, 1.0, 1.0), point(2, 1.0, 1.0)],
            vec![point(0, 1.0, 1.0); MAX_MIDI_VOLUME_POINTS_PER_CLIP + 1],
        ] {
            assert!(MidiClipVolume::new(points, length).is_err());
        }
        assert!(MidiClipVolume::new(Vec::new(), BeatTime::ZERO).is_err());
        let curve = MidiClipVolume::new(vec![point(4, 1.0, 1.0)], length).unwrap();
        assert!(curve.validate_source_length(BeatTime::from_raw(3)).is_err());
        assert!(curve.shifted(BeatTime::from_raw(u64::MAX), length).is_err());
        assert_eq!(
            MidiClipVolume::default().gain_at(length),
            MidiClipGain::UNITY
        );
    }
}
