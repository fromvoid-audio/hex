pub use allocation_layout::MeterMapOwnedAllocationLayout;
pub use copy_custody::{MeterMapCopyCustody, MeterMapCopyError, MeterMapCopyPhase};
mod allocation_layout;
mod copy_custody;
use super::{BeatTime, MAX_METER_POINTS, Meter};
use crate::Invalid;

/// Time-signature change at an absolute quarter-note beat position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeterPoint {
    beat: BeatTime,
    meter: Meter,
}

impl MeterPoint {
    pub const fn new(beat: BeatTime, meter: Meter) -> Self {
        Self { beat, meter }
    }

    pub const fn beat(self) -> BeatTime {
        self.beat
    }

    pub const fn meter(self) -> Meter {
        self.meter
    }
}

/// Position in the bar/beat display derived from a canonical meter map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeterPosition {
    bar: u64,
    beat: u8,
    beat_fraction: BeatTime,
    bar_start: BeatTime,
    bar_end: Option<BeatTime>,
    fragmentary: bool,
}

impl MeterPosition {
    /// One-based bar number, continuing across meter changes.
    pub const fn bar(self) -> u64 {
        self.bar
    }

    /// One-based denominator-beat number within the current bar.
    pub const fn beat(self) -> u8 {
        self.beat
    }

    /// Quarter-note offset from the start of this denominator beat.
    pub const fn beat_fraction(self) -> BeatTime {
        self.beat_fraction
    }

    pub const fn bar_start(self) -> BeatTime {
        self.bar_start
    }

    /// Exclusive end of this bar fragment, or None if it exceeds BeatTime's range.
    pub const fn bar_end(self) -> Option<BeatTime> {
        self.bar_end
    }

    /// True when a meter marker ends this bar before its natural meter length.
    pub const fn fragmentary(self) -> bool {
        self.fragmentary
    }
}

/// Piecewise-constant meter map. A meter change may occur mid-bar; the prior
/// bar becomes fragmentary and the new meter starts the next numbered bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeterMap {
    points: Vec<MeterPoint>,
    first_bar_at_point: Vec<u64>,
}

impl MeterMap {
    pub fn common_time() -> Self {
        Self {
            points: vec![MeterPoint::new(BeatTime::ZERO, Meter::COMMON_TIME)],
            first_bar_at_point: vec![1],
        }
    }

    pub fn new(points: Vec<MeterPoint>) -> Result<Self, Invalid> {
        if points.is_empty() || points.len() > MAX_METER_POINTS {
            return Err(Invalid("meter map must contain 1..1024 points"));
        }
        if points[0].beat != BeatTime::ZERO {
            return Err(Invalid("meter map must begin at beat zero"));
        }
        for pair in points.windows(2) {
            if pair[0].beat >= pair[1].beat {
                return Err(Invalid("meter points must be strictly increasing"));
            }
        }

        let mut first_bar_at_point = Vec::with_capacity(points.len());
        first_bar_at_point.push(1_u64);
        for index in 1..points.len() {
            let previous = points[index - 1];
            let current = points[index];
            let delta = current.beat.raw() - previous.beat.raw();
            let previous_bar_length = previous.meter.bar_length_raw();
            let bars = delta.div_ceil(previous_bar_length);
            let first_bar = first_bar_at_point[index - 1]
                .checked_add(bars)
                .ok_or(Invalid("meter bar numbering overflow"))?;
            first_bar_at_point.push(first_bar);
        }
        Ok(Self {
            points,
            first_bar_at_point,
        })
    }

    pub fn points(&self) -> &[MeterPoint] {
        &self.points
    }

    pub fn meter_at(&self, beat: BeatTime) -> Meter {
        self.points[self.segment_for_beat(beat)].meter
    }

    pub fn position(&self, beat: BeatTime) -> Result<MeterPosition, Invalid> {
        let index = self.segment_for_beat(beat);
        let point = self.points[index];
        let delta = beat.raw() - point.beat.raw();
        let bar_length = point.meter.bar_length_raw();
        let beat_length = point.meter.beat_length_raw();
        let elapsed_bars = delta / bar_length;
        let in_bar = delta % bar_length;
        let bar = self.first_bar_at_point[index]
            .checked_add(elapsed_bars)
            .ok_or(Invalid("meter bar numbering overflow"))?;
        let beat_number = (in_bar / beat_length)
            .checked_add(1)
            .ok_or(Invalid("meter beat numbering overflow"))?;
        let beat_fraction = in_bar % beat_length;
        let bar_start_raw =
            u128::from(point.beat.raw()) + u128::from(elapsed_bars) * u128::from(bar_length);
        let bar_start = BeatTime::from_raw(
            u64::try_from(bar_start_raw).map_err(|_| Invalid("meter bar start overflow"))?,
        );
        let natural_end_raw = bar_start_raw + u128::from(bar_length);
        let natural_end = u64::try_from(natural_end_raw).ok().map(BeatTime::from_raw);
        let next_marker = self.points.get(index + 1).map(|next| next.beat);
        let fragmentary = next_marker.is_some_and(|marker| match natural_end {
            None => true,
            Some(natural) => marker.raw() < natural.raw(),
        });
        let bar_end = if fragmentary {
            next_marker
        } else {
            natural_end
        };
        Ok(MeterPosition {
            bar,
            beat: u8::try_from(beat_number)
                .map_err(|_| Invalid("meter beat number exceeds its numerator"))?,
            beat_fraction: BeatTime::from_raw(beat_fraction),
            bar_start,
            bar_end,
            fragmentary,
        })
    }

    pub(crate) fn replace_point(&self, point: MeterPoint) -> Result<Self, Invalid> {
        let mut points = self.points.clone();
        match points.binary_search_by_key(&point.beat, |existing| existing.beat) {
            Ok(index) => points[index] = point,
            Err(index) => {
                if points.len() >= MAX_METER_POINTS {
                    return Err(Invalid("meter point limit reached"));
                }
                points.insert(index, point);
            }
        }
        Self::new(points)
    }

    fn segment_for_beat(&self, beat: BeatTime) -> usize {
        match self.points.binary_search_by_key(&beat, |point| point.beat) {
            Ok(index) => index,
            Err(0) => 0,
            Err(index) => index - 1,
        }
    }
}
