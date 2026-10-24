pub use allocation_layout::TempoMapOwnedAllocationLayout;
pub use copy_custody::{TempoMapCopyCustody, TempoMapCopyError, TempoMapCopyPhase};
mod allocation_layout;
mod copy_custody;
use super::{BeatTime, MAX_TEMPO_POINTS, Tempo};
use crate::Invalid;

const FRAME_FRACTION_BITS: u32 = 32;
const FRAME_FRACTION_MASK: u128 = (1_u128 << FRAME_FRACTION_BITS) - 1;
const FRAME_HALF: u128 = 1_u128 << (FRAME_FRACTION_BITS - 1);
const FRAME_FACTOR_NUMERATOR: u128 = 60 * 1_000_000;

/// Tempo change at an absolute quarter-note beat position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TempoPoint {
    beat: BeatTime,
    tempo: Tempo,
}

impl TempoPoint {
    pub const fn new(beat: BeatTime, tempo: Tempo) -> Self {
        Self { beat, tempo }
    }

    pub const fn beat(self) -> BeatTime {
        self.beat
    }

    pub const fn tempo(self) -> Tempo {
        self.tempo
    }
}

/// Piecewise-constant tempo map with precomputed fractional-frame Q32 prefixes.
/// Construction and edits belong on the control side. Immutable conversion can
/// support prepared, admitted block/event queries; never invoke it per sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TempoMap {
    sample_rate: u32,
    points: Vec<TempoPoint>,
    frame_prefix_q32: Vec<u128>,
}

impl TempoMap {
    /// First raw Q32.32 value beyond the representable beat domain.
    pub const BEAT_RANGE_SENTINEL: u128 = u64::MAX as u128 + 1;

    pub fn constant(sample_rate: u32, tempo: Tempo) -> Result<Self, Invalid> {
        Self::new(sample_rate, vec![TempoPoint::new(BeatTime::ZERO, tempo)])
    }

    pub fn new(sample_rate: u32, points: Vec<TempoPoint>) -> Result<Self, Invalid> {
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(Invalid("tempo map sample rate must be 8000..192000 hz"));
        }
        if points.is_empty() || points.len() > MAX_TEMPO_POINTS {
            return Err(Invalid("tempo map must contain 1..1024 points"));
        }
        if points[0].beat != BeatTime::ZERO {
            return Err(Invalid("tempo map must begin at beat zero"));
        }
        for pair in points.windows(2) {
            if pair[0].beat >= pair[1].beat {
                return Err(Invalid("tempo points must be strictly increasing"));
            }
        }

        let factor = FRAME_FACTOR_NUMERATOR
            .checked_mul(u128::from(sample_rate))
            .ok_or(Invalid("tempo frame conversion factor overflow"))?;
        let mut frame_prefix_q32 = Vec::with_capacity(points.len());
        frame_prefix_q32.push(0_u128);
        for index in 1..points.len() {
            let previous = points[index - 1];
            let current = points[index];
            let beat_delta = u128::from(current.beat.raw() - previous.beat.raw());
            let segment_frames_q32 = beat_delta
                .checked_mul(factor)
                .ok_or(Invalid("tempo frame prefix overflow"))?
                / u128::from(previous.tempo.micro_bpm());
            let prefix = frame_prefix_q32[index - 1]
                .checked_add(segment_frames_q32)
                .ok_or(Invalid("tempo frame prefix overflow"))?;
            frame_prefix_q32.push(prefix);
        }
        Ok(Self {
            sample_rate,
            points,
            frame_prefix_q32,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Prove identical forward and inverse clock conversions without rewriting
    /// authored points. Identical maps pass directly. Differently segmented
    /// constant maps pass only when every boundary is an exact integral frame
    /// from the origin: Q32 prefixes then telescope exactly, and inverse rounding
    /// cannot select a different subframe marker. Other layouts conservatively
    /// return false. After structural equality, the fallback scans at most
    /// twice MAX_TEMPO_POINTS entries, with no allocation.
    pub fn proves_same_clock(&self, other: &Self) -> bool {
        if self == other {
            return true;
        }
        if self.sample_rate != other.sample_rate {
            return false;
        }
        let Some(first) = self.points.first() else {
            return false;
        };
        let tempo = first.tempo;
        let factor = u128::from(self.sample_rate) * FRAME_FACTOR_NUMERATOR;
        let denominator = u128::from(tempo.micro_bpm()) << FRAME_FRACTION_BITS;
        self.points.iter().chain(&other.points).all(|point| {
            point.tempo == tempo
                && u128::from(point.beat.raw())
                    .checked_mul(factor)
                    .is_some_and(|scaled| scaled % denominator == 0)
        })
    }

    pub fn points(&self) -> &[TempoPoint] {
        &self.points
    }

    pub fn tempo_at(&self, beat: BeatTime) -> Tempo {
        self.points[self.segment_for_beat(beat)].tempo
    }

    /// Convert a beat to the nearest integer frame; exact half-frame ties round later.
    pub fn beat_to_frame(&self, beat: BeatTime) -> Result<u64, Invalid> {
        let fixed = self.frame_q32_at(beat)?;
        let rounded = round_frame_q32(fixed);
        u64::try_from(rounded).map_err(|_| Invalid("beat maps beyond the frame range"))
    }

    /// Convert an integer frame to the nearest Q32.32 beat, ties later. At a
    /// frame shared by quantized tempo boundaries, the latest boundary wins.
    /// The result is checked to map back to the requested frame.
    pub fn frame_to_beat(&self, frame: u64) -> Result<BeatTime, Invalid> {
        let factor = FRAME_FACTOR_NUMERATOR * u128::from(self.sample_rate);
        let target_q32 = u128::from(frame) << FRAME_FRACTION_BITS;

        let mut low = 0_usize;
        let mut high = self.points.len();
        while low < high {
            let middle = low + (high - low) / 2;
            if round_frame_q32(self.frame_prefix_q32[middle]) <= u128::from(frame) {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let segment = low.saturating_sub(1);
        let boundary_frame = round_frame_q32(self.frame_prefix_q32[segment]);
        if boundary_frame == u128::from(frame) {
            return Ok(self.points[segment].beat);
        }
        if boundary_frame > u128::from(frame) {
            return Err(Invalid("frame is outside the tempo map's beat range"));
        }

        let delta_frame_q32 = target_q32
            .checked_sub(self.frame_prefix_q32[segment])
            .ok_or(Invalid("frame precedes its tempo segment"))?;
        let scaled = delta_frame_q32
            .checked_mul(u128::from(self.points[segment].tempo.micro_bpm()))
            .ok_or(Invalid("inverse tempo conversion overflow"))?;
        let raw_delta = round_ratio_later(scaled, factor);
        let raw = u128::from(self.points[segment].beat.raw())
            .checked_add(raw_delta)
            .ok_or(Invalid("inverse beat position overflow"))?;
        let beat = BeatTime::from_raw(
            u64::try_from(raw).map_err(|_| Invalid("frame maps beyond the beat range"))?,
        );
        if self.beat_to_frame(beat)? != frame {
            return Err(Invalid("frame has no representable Q32.32 beat"));
        }
        Ok(beat)
    }

    /// Return the first raw Q32.32 beat whose forward rounded frame is at least
    /// `frame`. If no representable beat reaches that frame, return
    /// [`Self::BEAT_RANGE_SENTINEL`]. Frame zero starts at beat zero.
    ///
    /// This is a lower-bound query, distinct from `frame_to_beat`, which chooses
    /// the latest exact tempo boundary among beats that quantize to the frame.
    /// It searches cached segment prefixes and solves the crossing within one
    /// segment, so the 1,024-point map has O(log n) lookup and the raw beat
    /// domain is never scanned. The query performs no allocation and uses the
    /// same forward Q32 prefixes and half-frame ties-later threshold as the
    /// canonical beat-to-frame conversion.
    pub fn beat_lower_bound_for_frame(&self, frame: u64) -> Result<u128, Invalid> {
        if frame == 0 {
            return Ok(0);
        }
        // Ties round later, so frame_q32 rounds to `frame` at this exact
        // half-frame threshold. Subtracting half avoids duplicating the
        // forward rounding routine or searching the raw beat domain.
        let threshold_q32 = (u128::from(frame) << FRAME_FRACTION_BITS)
            .checked_sub(FRAME_HALF)
            .ok_or(Invalid("tempo lower-bound threshold underflow"))?;
        let final_beat = BeatTime::from_raw(u64::MAX);
        let final_q32 = self.frame_q32_at(final_beat)?;
        if final_q32 < threshold_q32 {
            return Ok(Self::BEAT_RANGE_SENTINEL);
        }

        // Prefixes are nondecreasing (short segments may quantize to zero), so
        // the preceding segment is the first one that can contain the crossing.
        let crossing_end = self
            .frame_prefix_q32
            .partition_point(|prefix| *prefix < threshold_q32);
        let segment = crossing_end
            .saturating_sub(1)
            .min(self.points.len().saturating_sub(1));
        let point = self
            .points
            .get(segment)
            .ok_or(Invalid("tempo map has no points"))?;
        let prefix = *self
            .frame_prefix_q32
            .get(segment)
            .ok_or(Invalid("tempo map has no frame prefixes"))?;
        let required_local_q32 = threshold_q32
            .checked_sub(prefix)
            .ok_or(Invalid("tempo lower bound precedes its segment"))?;
        let factor = FRAME_FACTOR_NUMERATOR
            .checked_mul(u128::from(self.sample_rate))
            .ok_or(Invalid("tempo frame conversion factor overflow"))?;
        let numerator = required_local_q32
            .checked_mul(u128::from(point.tempo.micro_bpm()))
            .ok_or(Invalid("tempo lower-bound ratio overflow"))?;
        // `floor(delta * factor / tempo) >= required` exactly when
        // `delta >= ceil(required * tempo / factor)`.
        let local_raw = ceil_ratio(numerator, factor)?;
        let raw = u128::from(point.beat.raw())
            .checked_add(local_raw)
            .ok_or(Invalid("tempo lower-bound beat overflow"))?;
        Ok(raw.min(Self::BEAT_RANGE_SENTINEL))
    }

    pub(crate) fn replace_point(&self, point: TempoPoint) -> Result<Self, Invalid> {
        let mut points = self.points.clone();
        match points.binary_search_by_key(&point.beat, |existing| existing.beat) {
            Ok(index) => points[index] = point,
            Err(index) => {
                if points.len() >= MAX_TEMPO_POINTS {
                    return Err(Invalid("tempo point limit reached"));
                }
                points.insert(index, point);
            }
        }
        Self::new(self.sample_rate, points)
    }

    fn segment_for_beat(&self, beat: BeatTime) -> usize {
        match self.points.binary_search_by_key(&beat, |point| point.beat) {
            Ok(index) => index,
            Err(0) => 0,
            Err(index) => index - 1,
        }
    }

    /// Fractional project frames with 32 fraction bits, using the same stored
    /// segment prefixes as beat_to_frame. Control-side map preparation can keep
    /// sample phase until its final quantization; this is not a callback clock.
    pub fn beat_to_frame_q32(&self, beat: BeatTime) -> Result<u128, Invalid> {
        self.frame_q32_at(beat)
    }

    fn frame_q32_at(&self, beat: BeatTime) -> Result<u128, Invalid> {
        let index = self.segment_for_beat(beat);
        let point = self.points[index];
        let beat_delta = u128::from(beat.raw() - point.beat.raw());
        let factor = FRAME_FACTOR_NUMERATOR * u128::from(self.sample_rate);
        let local = beat_delta
            .checked_mul(factor)
            .ok_or(Invalid("beat frame conversion overflow"))?
            / u128::from(point.tempo.micro_bpm());
        self.frame_prefix_q32[index]
            .checked_add(local)
            .ok_or(Invalid("beat frame conversion overflow"))
    }
}

fn ceil_ratio(numerator: u128, denominator: u128) -> Result<u128, Invalid> {
    if denominator == 0 {
        return Err(Invalid("tempo lower-bound divisor must be positive"));
    }
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    if remainder == 0 {
        Ok(quotient)
    } else {
        quotient
            .checked_add(1)
            .ok_or(Invalid("tempo lower-bound ratio overflow"))
    }
}

fn round_frame_q32(fixed: u128) -> u128 {
    let whole = fixed >> FRAME_FRACTION_BITS;
    if fixed & FRAME_FRACTION_MASK >= FRAME_HALF {
        whole + 1
    } else {
        whole
    }
}

fn round_ratio_later(numerator: u128, denominator: u128) -> u128 {
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    if remainder >= denominator - remainder {
        quotient + 1
    } else {
        quotient
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FRAME_FRACTION_BITS, TempoMap, TempoPoint, ceil_ratio, round_frame_q32, round_ratio_later,
    };
    use crate::{BeatTime, MAX_TEMPO_POINTS, Tempo};

    fn tempo(micro_bpm: u32) -> Tempo {
        Tempo::new(micro_bpm).expect("test tempo is canonical")
    }

    fn assert_lower_bound(map: &TempoMap, frame: u64, expected: u128) {
        let actual = map.beat_lower_bound_for_frame(frame).unwrap();
        assert_eq!(actual, expected, "frame {frame}");
        if actual == TempoMap::BEAT_RANGE_SENTINEL {
            assert!(map.beat_to_frame(BeatTime::from_raw(u64::MAX)).unwrap() < frame);
        } else {
            let beat = BeatTime::from_raw(actual as u64);
            assert!(map.beat_to_frame(beat).unwrap() >= frame);
            if actual > 0 {
                assert!(
                    map.beat_to_frame(BeatTime::from_raw(actual as u64 - 1))
                        .unwrap()
                        < frame
                );
            }
        }
    }

    #[test]
    fn clock_proof_preserves_fractional_prefixes_and_inverse_marker_rounding() {
        for rate in [44_100, 48_000] {
            for bpm in [120_000_000, 180_000_000] {
                let tempo = Tempo::new(bpm).unwrap();
                let original = TempoMap::constant(rate, tempo).unwrap();
                let split = TempoMap::new(
                    rate,
                    [0, 1, 3, 5]
                        .map(|n| TempoPoint::new(BeatTime::from_quarters(n).unwrap(), tempo))
                        .to_vec(),
                )
                .unwrap();
                assert_ne!(original, split, "authored state is never normalized");
                assert!(original.proves_same_clock(&split));
                assert!(split.proves_same_clock(&original));
                for frame in (0..192_000).step_by(127) {
                    assert_eq!(original.frame_to_beat(frame), split.frame_to_beat(frame));
                }
                for raw in [0, 1, (1 << 32) - 1, 1 << 32, (3 << 32) + 1, u64::MAX] {
                    assert_eq!(
                        original.beat_to_frame(BeatTime::from_raw(raw)),
                        split.beat_to_frame(BeatTime::from_raw(raw))
                    );
                }
                let subframe = TempoMap::new(
                    rate,
                    vec![
                        TempoPoint::new(BeatTime::ZERO, tempo),
                        TempoPoint::new(BeatTime::from_raw(1), tempo),
                    ],
                )
                .unwrap();
                assert_eq!(
                    original.beat_to_frame(BeatTime::from_raw(1)),
                    subframe.beat_to_frame(BeatTime::from_raw(1))
                );
                assert_ne!(original.frame_to_beat(0), subframe.frame_to_beat(0));
                assert!(!original.proves_same_clock(&subframe));
            }
        }
        let tempo = Tempo::new(143_000_000).unwrap();
        let fractional = TempoMap::constant(48_000, tempo).unwrap();
        let split = TempoMap::new(
            48_000,
            vec![
                TempoPoint::new(BeatTime::ZERO, tempo),
                TempoPoint::new(BeatTime::ONE, tempo),
            ],
        )
        .unwrap();
        assert!(
            !fractional.proves_same_clock(&split),
            "fractional prefix rounding must retain the strict proof"
        );
        assert!(!fractional.proves_same_clock(&TempoMap::constant(44_100, tempo).unwrap()));
        assert!(!fractional.proves_same_clock(
            &TempoMap::constant(48_000, Tempo::new(120_000_000).unwrap()).unwrap()
        ));
    }

    #[test]
    fn fixed_point_rounding_moves_exact_half_ties_later() {
        let half = 1_u128 << (FRAME_FRACTION_BITS - 1);
        assert_eq!(round_frame_q32(half - 1), 0);
        assert_eq!(round_frame_q32(half), 1);
        assert_eq!(round_frame_q32((3_u128 << FRAME_FRACTION_BITS) + half), 4);
        assert_eq!(round_ratio_later(5, 2), 3);
        assert_eq!(round_ratio_later(4, 3), 1);
    }

    #[test]
    fn lower_bound_solves_exact_half_frame_and_tempo_boundary_crossings() {
        let constant = TempoMap::constant(8_000, tempo(937_500_000)).unwrap();
        let half_frame = 1_u64 << 22;
        assert_lower_bound(&constant, 0, 0);
        assert_lower_bound(&constant, 1, u128::from(half_frame));
        assert_lower_bound(&constant, 2, u128::from(half_frame * 3));

        let seam = 4_000_u64 * (1_u64 << 32);
        let map = TempoMap::new(
            8_000,
            vec![
                TempoPoint::new(BeatTime::ZERO, tempo(120_000_000)),
                TempoPoint::new(BeatTime::from_raw(seam), tempo(60_000_000)),
            ],
        )
        .unwrap();
        let seam_frame = map.beat_to_frame(BeatTime::from_raw(seam)).unwrap();
        let crossing = map.beat_lower_bound_for_frame(seam_frame).unwrap();
        assert!(crossing <= u128::from(seam));
        assert!(
            map.beat_to_frame(BeatTime::from_raw(crossing as u64))
                .unwrap()
                >= seam_frame
        );
        if crossing > 0 {
            assert!(
                map.beat_to_frame(BeatTime::from_raw(crossing as u64 - 1))
                    .unwrap()
                    < seam_frame
            );
        }

        let exact_half = 1_u64 << 22;
        let shared_frame = TempoMap::new(
            8_000,
            vec![
                TempoPoint::new(BeatTime::ZERO, tempo(937_500_000)),
                TempoPoint::new(BeatTime::from_raw(exact_half), tempo(60_000_000)),
                TempoPoint::new(BeatTime::from_raw(exact_half + 1), tempo(120_000_000)),
            ],
        )
        .unwrap();
        assert_eq!(
            shared_frame
                .beat_to_frame(BeatTime::from_raw(exact_half))
                .unwrap(),
            1
        );
        assert_eq!(
            shared_frame
                .beat_to_frame(BeatTime::from_raw(exact_half + 1))
                .unwrap(),
            1
        );
        assert_eq!(
            shared_frame.frame_to_beat(1).unwrap(),
            BeatTime::from_raw(exact_half + 1)
        );
        assert_eq!(
            shared_frame.beat_lower_bound_for_frame(1).unwrap(),
            u128::from(exact_half)
        );
    }

    #[test]
    fn lower_bound_differs_from_latest_boundary_inverse_and_has_exact_sentinel() {
        let shared = TempoMap::new(
            8_000,
            vec![
                TempoPoint::new(BeatTime::ZERO, tempo(120_000_000)),
                TempoPoint::new(BeatTime::from_raw(1), tempo(60_000_000)),
                TempoPoint::new(BeatTime::from_raw(2), tempo(240_000_000)),
            ],
        )
        .unwrap();
        assert_eq!(shared.frame_to_beat(0).unwrap(), BeatTime::from_raw(2));
        assert_eq!(shared.beat_lower_bound_for_frame(0).unwrap(), 0);
        let first_frame_beat = shared.beat_lower_bound_for_frame(1).unwrap();
        assert!(first_frame_beat > 2);
        assert_lower_bound(&shared, 1, first_frame_beat);

        let map = TempoMap::constant(192_000, tempo(1_000_000)).unwrap();
        let last_frame = map.beat_to_frame(BeatTime::from_raw(u64::MAX)).unwrap();
        let last_frame_lower = map.beat_lower_bound_for_frame(last_frame).unwrap();
        assert_lower_bound(&map, last_frame, last_frame_lower);
        assert_lower_bound(&map, last_frame + 1, TempoMap::BEAT_RANGE_SENTINEL);
        assert_lower_bound(&map, u64::MAX, TempoMap::BEAT_RANGE_SENTINEL);
    }

    #[test]
    fn lower_bound_matches_exhaustive_small_raw_domains_across_rates_and_tempos() {
        const MAX_RAW: u64 = 65_535;
        for sample_rate in [8_000, 44_100, 192_000] {
            for tempos in [
                [1_000_000, 10_000_000, 900_000_000, 1_000_000],
                [750_000_000, 1_000_000_000, 2_000_000, 500_000_000],
            ] {
                let points = [0, 31, 4_093, 16_381]
                    .into_iter()
                    .zip(tempos)
                    .map(|(raw, bpm)| TempoPoint::new(BeatTime::from_raw(raw), tempo(bpm)))
                    .collect();
                let map = TempoMap::new(sample_rate, points).unwrap();
                let frames: Vec<_> = (0..=MAX_RAW)
                    .map(|raw| map.beat_to_frame(BeatTime::from_raw(raw)).unwrap())
                    .collect();
                let max_frame = *frames.last().unwrap();
                for frame in 0..=max_frame {
                    let expected = frames
                        .iter()
                        .position(|candidate| *candidate >= frame)
                        .map_or(TempoMap::BEAT_RANGE_SENTINEL, |raw| raw as u128);
                    assert_lower_bound(&map, frame, expected);
                }
            }
        }
    }

    #[test]
    fn lower_bound_uses_logarithmic_prefix_search_for_1024_points() {
        let points: Vec<_> = (0..MAX_TEMPO_POINTS)
            .map(|index| {
                TempoPoint::new(
                    BeatTime::from_raw(index as u64 * 1_000),
                    tempo(1_000_000 + (index % 11) as u32 * 10_000),
                )
            })
            .collect();
        let map = TempoMap::new(48_000, points).unwrap();
        let last = map
            .beat_to_frame(BeatTime::from_raw((MAX_TEMPO_POINTS as u64 - 1) * 1_000))
            .unwrap();
        for frame in (0..=last).step_by(7) {
            let lower = map.beat_lower_bound_for_frame(frame).unwrap();
            assert_lower_bound(&map, frame, lower);
        }
    }

    #[test]
    fn deterministic_integer_probes_remain_minimal_across_sample_rates() {
        for sample_rate in [8_000, 22_050, 44_100, 96_000, 192_000] {
            let map = TempoMap::new(
                sample_rate,
                vec![
                    TempoPoint::new(BeatTime::ZERO, tempo(123_456_789)),
                    TempoPoint::new(BeatTime::from_raw(1 << 30), tempo(987_654_321)),
                    TempoPoint::new(BeatTime::from_raw(7 << 32), tempo(77_777_777)),
                    TempoPoint::new(BeatTime::from_raw(31 << 32), tempo(999_999_999)),
                ],
            )
            .unwrap();
            let mut random = u64::from(sample_rate) ^ 0x9e37_79b9_7f4a_7c15;
            for _ in 0..256 {
                random = random
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                for raw in [random % (64_u64 << 32), random, u64::MAX - 1, u64::MAX] {
                    let frame = map.beat_to_frame(BeatTime::from_raw(raw)).unwrap();
                    let lower = map.beat_lower_bound_for_frame(frame).unwrap();
                    assert_lower_bound(&map, frame, lower);
                    if frame > 0 {
                        assert_lower_bound(
                            &map,
                            frame - 1,
                            map.beat_lower_bound_for_frame(frame - 1).unwrap(),
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn ceil_ratio_checks_quotient_increment() {
        assert_eq!(ceil_ratio(0, 2).unwrap(), 0);
        assert_eq!(ceil_ratio(4, 2).unwrap(), 2);
        assert_eq!(ceil_ratio(5, 2).unwrap(), 3);
        assert!(ceil_ratio(1, 0).is_err());
    }
}
