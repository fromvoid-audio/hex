//! Shared transient launch grid for Arrangement and Session playback.
//! Queries borrow prepared timing and canonical meter on bounded event boundaries.
//! Triplets use rational Q32 positions rather than accumulating rounded divisions;
//! bar choices follow numbered canonical bars, including fragmentary meter changes.
//! Global choices follow the transport contract documented at
//! <https://docs.cycling74.com/apiref/lom/song/#clip_trigger_quantization>.
use crate::{MidiTiming, PreparedTempoError};
use hex_project::{BeatTime, MeterMap};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchQuantization {
    Immediate,
    EightBars,
    FourBars,
    TwoBars,
    OneBar,
    Half,
    HalfTriplet,
    Quarter,
    QuarterTriplet,
    Eighth,
    EighthTriplet,
    Sixteenth,
    SixteenthTriplet,
    ThirtySecond,
    /// Existing fixed four-quarter API; global UI uses the meter-aware OneBar.
    FourQuarter,
}

impl LaunchQuantization {
    pub const GLOBAL_CHOICES: [Self; 14] = [
        Self::Immediate,
        Self::EightBars,
        Self::FourBars,
        Self::TwoBars,
        Self::OneBar,
        Self::Half,
        Self::HalfTriplet,
        Self::Quarter,
        Self::QuarterTriplet,
        Self::Eighth,
        Self::EighthTriplet,
        Self::Sixteenth,
        Self::SixteenthTriplet,
        Self::ThirtySecond,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Immediate => "none",
            Self::EightBars => "8 bars",
            Self::FourBars => "4 bars",
            Self::TwoBars => "2 bars",
            Self::OneBar => "1 bar",
            Self::Half => "1/2",
            Self::HalfTriplet => "1/2t",
            Self::Quarter => "1/4",
            Self::QuarterTriplet => "1/4t",
            Self::Eighth => "1/8",
            Self::EighthTriplet => "1/8t",
            Self::Sixteenth => "1/16",
            Self::SixteenthTriplet => "1/16t",
            Self::ThirtySecond => "1/32",
            Self::FourQuarter => "4 qn",
        }
    }

    /// First grid boundary at/after this live performance frame. A stopped
    /// clock and `none` are immediate. `minimum_beat` excludes an older side
    /// of an existing clock translation, matching Session handoff semantics.
    pub fn scheduled_at(
        self,
        timing: &impl MidiTiming,
        meter: &MeterMap,
        frame: u64,
        playing: bool,
        minimum_beat: Option<BeatTime>,
    ) -> Result<(u64, BeatTime), PreparedTempoError> {
        let current = timing.frame_to_beat(frame)?;
        if !playing || self == Self::Immediate {
            return Ok((frame, current));
        }
        let grid_start = minimum_beat.map_or(current, |minimum| current.max(minimum));
        let (previous, next) = self.neighbours(meter, grid_start)?;
        if minimum_beat.is_none_or(|minimum| previous >= minimum)
            && timing.beat_to_frame(previous)? == frame
        {
            return Ok((frame, previous));
        }
        Ok((timing.beat_to_frame(next)?.max(frame), next))
    }

    fn neighbours(
        self,
        meter: &MeterMap,
        current: BeatTime,
    ) -> Result<(BeatTime, BeatTime), PreparedTempoError> {
        let invalid = || PreparedTempoError::BeatToFrame("launch grid exceeds the beat range");
        let bars = match self {
            Self::EightBars => Some(8),
            Self::FourBars => Some(4),
            Self::TwoBars => Some(2),
            Self::OneBar => Some(1),
            _ => None,
        };
        if let Some(bars) = bars {
            let position = meter.position(current).map_err(|_| invalid())?;
            let mut previous = position.bar_start();
            for _ in 0..((position.bar() - 1) % bars) {
                previous = meter
                    .position(BeatTime::from_raw(
                        previous.raw().checked_sub(1).ok_or_else(invalid)?,
                    ))
                    .map_err(|_| invalid())?
                    .bar_start();
            }
            let mut next = previous;
            for _ in 0..bars {
                next = meter
                    .position(next)
                    .map_err(|_| invalid())?
                    .bar_end()
                    .ok_or_else(invalid)?;
            }
            return Ok((previous, next));
        }
        let (numerator, denominator) = match self {
            Self::FourQuarter => (4_u128, 1_u128),
            Self::Half => (2, 1),
            Self::HalfTriplet => (4, 3),
            Self::Quarter => (1, 1),
            Self::QuarterTriplet => (2, 3),
            Self::Eighth => (1, 2),
            Self::EighthTriplet => (1, 3),
            Self::Sixteenth => (1, 4),
            Self::SixteenthTriplet => (1, 6),
            Self::ThirtySecond => (1, 8),
            _ => return Ok((current, current)),
        };
        let step = u128::from(BeatTime::ONE.raw()) * numerator;
        let index = u128::from(current.raw()) * denominator / step;
        let boundary = |index: u128| {
            u64::try_from((index * step + denominator / 2) / denominator)
                .map(BeatTime::from_raw)
                .map_err(|_| invalid())
        };
        Ok((boundary(index)?, boundary(index + 1)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PreparedTempoMap;
    use hex_project::{Meter, MeterPoint, Tempo, TempoMap};

    #[test]
    fn repeated_triplets_keep_projected_boundary_and_minimum_beat() {
        let timing = PreparedTempoMap::prepare(
            TempoMap::constant(44_100, Tempo::new(137_123_457).unwrap()).unwrap(),
            44_100,
        )
        .unwrap();
        for (choice, numerator, denominator) in [
            (LaunchQuantization::HalfTriplet, 4_u128, 3_u128),
            (LaunchQuantization::QuarterTriplet, 2, 3),
            (LaunchQuantization::EighthTriplet, 1, 3),
            (LaunchQuantization::SixteenthTriplet, 1, 6),
        ] {
            let boundary = |index: u128| {
                BeatTime::from_raw(
                    u64::try_from(
                        (index * u128::from(BeatTime::ONE.raw()) * numerator + denominator / 2)
                            / denominator,
                    )
                    .unwrap(),
                )
            };
            for index in [1, 2, 3, 7, 31, 127] {
                let beat = boundary(index);
                let frame = timing.beat_to_frame(beat).unwrap();
                let next = boundary(index + 1);
                let next_frame = timing.beat_to_frame(next).unwrap();
                assert_eq!(
                    choice
                        .scheduled_at(&timing, &MeterMap::common_time(), frame - 1, true, None)
                        .unwrap(),
                    (frame, beat),
                    "before {choice:?} {index}"
                );
                assert_eq!(
                    choice
                        .scheduled_at(&timing, &MeterMap::common_time(), frame, true, None)
                        .unwrap(),
                    (frame, beat),
                    "exact {choice:?} {index}"
                );
                assert_eq!(
                    choice
                        .scheduled_at(&timing, &MeterMap::common_time(), frame + 1, true, None)
                        .unwrap(),
                    (next_frame, next),
                    "after {choice:?} {index}"
                );
                assert_eq!(
                    choice
                        .scheduled_at(
                            &timing,
                            &MeterMap::common_time(),
                            frame,
                            true,
                            Some(BeatTime::from_raw(beat.raw() + 1))
                        )
                        .unwrap(),
                    (next_frame, next),
                    "anchor excludes older boundary {choice:?} {index}"
                );
            }
        }
    }

    #[test]
    fn global_choices_have_exact_labels_and_sample_grid_boundaries() {
        let timing = PreparedTempoMap::prepare(
            TempoMap::constant(48_000, Tempo::new(120_000_000).unwrap()).unwrap(),
            48_000,
        )
        .unwrap();
        let labels = [
            "none", "8 bars", "4 bars", "2 bars", "1 bar", "1/2", "1/2t", "1/4", "1/4t", "1/8",
            "1/8t", "1/16", "1/16t", "1/32",
        ];
        let frames = [
            1, 768_000, 384_000, 192_000, 96_000, 48_000, 32_000, 24_000, 16_000, 12_000, 8_000,
            6_000, 4_000, 3_000,
        ];
        for ((choice, label), expected) in LaunchQuantization::GLOBAL_CHOICES
            .into_iter()
            .zip(labels)
            .zip(frames)
        {
            assert_eq!(choice.label(), label);
            assert_eq!(
                choice
                    .scheduled_at(&timing, &MeterMap::common_time(), 1, true, None)
                    .unwrap()
                    .0,
                expected,
                "{label}"
            );
            assert_eq!(
                choice
                    .scheduled_at(&timing, &MeterMap::common_time(), expected, true, None)
                    .unwrap()
                    .0,
                expected,
                "exact {label}"
            );
            assert_eq!(
                choice
                    .scheduled_at(&timing, &MeterMap::common_time(), 17, false, None)
                    .unwrap()
                    .0,
                17
            );
        }
        let meter = MeterMap::new(vec![
            MeterPoint::new(BeatTime::ZERO, Meter::new(3, 4).unwrap()),
            MeterPoint::new(
                BeatTime::from_quarters(5).unwrap(),
                Meter::new(7, 8).unwrap(),
            ),
        ])
        .unwrap();
        assert_eq!(
            LaunchQuantization::OneBar
                .scheduled_at(&timing, &meter, 1, true, None)
                .unwrap()
                .0,
            72_000
        );
        assert_eq!(
            LaunchQuantization::TwoBars
                .scheduled_at(&timing, &meter, 72_001, true, None)
                .unwrap()
                .0,
            120_000,
            "fragmentary second bar ends at meter change"
        );
    }
}
