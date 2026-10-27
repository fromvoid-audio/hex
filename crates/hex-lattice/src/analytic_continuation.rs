//! Fixed, provider-validated analytic continuation data.
//!
//! This module defines no device law and owns no notes or context leases. A
//! host admits the exact provider target and validates all carried notes before
//! changing ownership. Progress uses its committed absolute performance frame,
//! so rollback needs no independently advancing transition clock.
use crate::{PreparedSharedAnalyticNote, StereoFrame};
use hex_lattice_contract::{DeviceContractError, DeviceTypeId};
use std::any::Any;

pub const ANALYTIC_CONTINUATION_WORDS: usize = 8;
pub const MAX_ANALYTIC_CONTINUATION_FRAMES: u32 = 4_096;

/// Process-local target identity assigned by the host before publication.
/// Hosts must never reuse a value for a different prepared context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnalyticContinuationId(u64);

impl AnalyticContinuationId {
    pub const fn new(value: u64) -> Result<Self, AnalyticContinuationError> {
        if value == 0 {
            Err(AnalyticContinuationError::InvalidIdentity)
        } else {
            Ok(Self(value))
        }
    }
}

/// Opaque fixed coefficients bound to an exact host-admitted target.
/// Only the provider interprets/validates words; construction alone does not
/// establish a valid coefficient law or cross-provider compatibility.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnalyticParameterFrame {
    target: AnalyticContinuationId,
    words: [u64; ANALYTIC_CONTINUATION_WORDS],
}

impl AnalyticParameterFrame {
    pub const fn new(
        target: AnalyticContinuationId,
        words: [u64; ANALYTIC_CONTINUATION_WORDS],
    ) -> Self {
        Self { target, words }
    }

    pub const fn target(&self) -> AnalyticContinuationId {
        self.target
    }

    pub const fn words(&self) -> &[u64; ANALYTIC_CONTINUATION_WORDS] {
        &self.words
    }
}

/// Provider identity and format are separate from an individual host target.
/// A provider must compare its complete immutable law when admitting a source;
/// equal metadata alone never proves sound/note compatibility.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnalyticContinuationFormat {
    provider: DeviceTypeId,
    version: u32,
}

impl AnalyticContinuationFormat {
    pub fn new(provider: DeviceTypeId, version: u32) -> Result<Self, DeviceContractError> {
        provider.validate()?;
        if version == 0 {
            return Err(DeviceContractError::InvalidStateVersion);
        }
        Ok(Self { provider, version })
    }

    pub const fn provider(&self) -> DeviceTypeId {
        self.provider
    }

    pub const fn version(&self) -> u32 {
        self.version
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnalyticContinuationError {
    InvalidIdentity,
    InvalidDuration,
    WrongTarget,
    BeforeStart,
    FrameOverflow,
    IncompatibleSource,
    InvalidParameters,
    InvalidNote,
}

/// One copyable transition. Detached notes may snapshot this value and lease
/// its target context; subsequent current-owner edits cannot change the copy.
/// The provider owns the endpoint and interpolation law, not this clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnalyticContinuation {
    start: AnalyticParameterFrame,
    start_frame: u64,
    duration: u32,
}

impl AnalyticContinuation {
    pub fn new(
        target: AnalyticContinuationId,
        start: AnalyticParameterFrame,
        start_frame: u64,
        duration: u32,
    ) -> Result<Self, AnalyticContinuationError> {
        if start.target != target {
            return Err(AnalyticContinuationError::WrongTarget);
        }
        if duration == 0 || duration > MAX_ANALYTIC_CONTINUATION_FRAMES {
            return Err(AnalyticContinuationError::InvalidDuration);
        }
        start_frame
            .checked_add(u64::from(duration))
            .ok_or(AnalyticContinuationError::FrameOverflow)?;
        Ok(Self {
            start,
            start_frame,
            duration,
        })
    }

    /// Five milliseconds rounded to the nearest frame, ties upward. This is
    /// a Hex transition policy, not a measured reference-device law.
    pub fn default_duration(sample_rate: u32) -> Result<u32, AnalyticContinuationError> {
        if !(8_000..=192_000).contains(&sample_rate) {
            return Err(AnalyticContinuationError::InvalidDuration);
        }
        Ok(((u64::from(sample_rate) * 5 + 500) / 1_000) as u32)
    }

    pub const fn target(&self) -> AnalyticContinuationId {
        self.start.target
    }

    pub const fn start(&self) -> &AnalyticParameterFrame {
        &self.start
    }

    pub const fn start_frame(&self) -> u64 {
        self.start_frame
    }

    pub const fn duration(&self) -> u32 {
        self.duration
    }

    /// Elapsed frames clamp only at the checked exact endpoint. A frame
    /// before admission is an error; query ordering never advances state.
    pub fn elapsed_at(&self, performance_frame: u64) -> Result<u32, AnalyticContinuationError> {
        let elapsed = performance_frame
            .checked_sub(self.start_frame)
            .ok_or(AnalyticContinuationError::BeforeStart)?;
        Ok(elapsed.min(u64::from(self.duration)) as u32)
    }
}

/// Optional capability of an immutable shared context. All methods used by
/// publication/rendering must be bounded, allocation-free, and read-only.
/// Host ownership and identity checks accompany, rather than replace, these
/// provider checks. Default-unsupported context opt-in is integrated separately.
pub trait PreparedAnalyticContinuation: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn format(&self) -> AnalyticContinuationFormat;

    /// Ordinary target endpoint. Reject foreign identities/invalid words
    /// whenever a parameter frame enters any subsequent method.
    fn endpoint(&self, target: AnalyticContinuationId) -> AnalyticParameterFrame;

    /// Validate the actual source's immutable law and actual boundary frame,
    /// then return the same coefficients rebound to this target. This must
    /// refuse unsupported authored edits, even when their effective endpoints
    /// happen to be equal. Failure changes neither context nor host state.
    fn prepare_start(
        &self,
        target: AnalyticContinuationId,
        source: &dyn PreparedAnalyticContinuation,
        source_target: AnalyticContinuationId,
        source_boundary: &AnalyticParameterFrame,
    ) -> Result<AnalyticParameterFrame, AnalyticContinuationError>;

    /// Provider validates retained payload/release compatibility before any
    /// voice is rebound. Note preparation may not occur in this method.
    fn validate_note(
        &self,
        note: &PreparedSharedAnalyticNote,
    ) -> Result<(), AnalyticContinuationError>;

    /// Evaluate at an absolute committed boundary. Endpoints must be exact;
    /// no previous contexts or independently advancing clocks are retained.
    fn parameters_at(
        &self,
        target: AnalyticContinuationId,
        transition: &AnalyticContinuation,
        performance_frame: u64,
    ) -> Result<AnalyticParameterFrame, AnalyticContinuationError>;

    /// Borrow an already validated target-bound frame with unchanged note
    /// age/gate. Invalid identities/words/payloads must refuse before sampling.
    fn sample_with_parameters(
        &self,
        target: AnalyticContinuationId,
        parameters: &AnalyticParameterFrame,
        note: &PreparedSharedAnalyticNote,
        age_frames: u64,
        gate_frames: u64,
        _resources: &dyn crate::DeviceResources,
    ) -> Result<StereoFrame, AnalyticContinuationError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: u64) -> AnalyticContinuationId {
        AnalyticContinuationId::new(value).expect("valid fixture identity")
    }

    // An independent one-coefficient fixture proves admission, reversal and
    // snapshot behavior without depending on Operator's future implementation.
    struct GainProvider {
        immutable_law: u32,
        gain: f64,
    }

    impl GainProvider {
        fn gain_at(
            &self,
            target: AnalyticContinuationId,
            frame: &AnalyticParameterFrame,
        ) -> Result<f64, AnalyticContinuationError> {
            if frame.target() != target {
                return Err(AnalyticContinuationError::WrongTarget);
            }
            let gain = f64::from_bits(frame.words()[0]);
            if !gain.is_finite()
                || !(0.0..=1.0).contains(&gain)
                || frame.words()[1..].iter().any(|word| *word != 0)
            {
                return Err(AnalyticContinuationError::InvalidParameters);
            }
            Ok(gain)
        }

        fn frame(target: AnalyticContinuationId, gain: f64) -> AnalyticParameterFrame {
            let mut words = [0; ANALYTIC_CONTINUATION_WORDS];
            words[0] = gain.to_bits();
            AnalyticParameterFrame::new(target, words)
        }
    }

    impl PreparedAnalyticContinuation for GainProvider {
        fn as_any(&self) -> &dyn Any {
            self
        }

        fn format(&self) -> AnalyticContinuationFormat {
            AnalyticContinuationFormat::new(DeviceTypeId::from_static("hex.instrument.fixture"), 1)
                .unwrap()
        }

        fn endpoint(&self, target: AnalyticContinuationId) -> AnalyticParameterFrame {
            Self::frame(target, self.gain)
        }

        fn prepare_start(
            &self,
            target: AnalyticContinuationId,
            source: &dyn PreparedAnalyticContinuation,
            source_target: AnalyticContinuationId,
            source_boundary: &AnalyticParameterFrame,
        ) -> Result<AnalyticParameterFrame, AnalyticContinuationError> {
            let source = source
                .as_any()
                .downcast_ref::<Self>()
                .filter(|source| source.immutable_law == self.immutable_law)
                .ok_or(AnalyticContinuationError::IncompatibleSource)?;
            Ok(Self::frame(
                target,
                source.gain_at(source_target, source_boundary)?,
            ))
        }

        fn validate_note(
            &self,
            note: &PreparedSharedAnalyticNote,
        ) -> Result<(), AnalyticContinuationError> {
            if note.payload() != &[0; 8] || note.release_frames() != 10 {
                return Err(AnalyticContinuationError::InvalidNote);
            }
            Ok(())
        }

        fn parameters_at(
            &self,
            target: AnalyticContinuationId,
            transition: &AnalyticContinuation,
            performance_frame: u64,
        ) -> Result<AnalyticParameterFrame, AnalyticContinuationError> {
            let start = self.gain_at(target, transition.start())?;
            let elapsed = transition.elapsed_at(performance_frame)?;
            let gain = if elapsed == 0 {
                start
            } else if elapsed == transition.duration() {
                self.gain
            } else {
                let progress = f64::from(elapsed) / f64::from(transition.duration());
                start + (self.gain - start) * progress
            };
            Ok(Self::frame(target, gain))
        }

        fn sample_with_parameters(
            &self,
            target: AnalyticContinuationId,
            parameters: &AnalyticParameterFrame,
            note: &PreparedSharedAnalyticNote,
            age_frames: u64,
            gate_frames: u64,
            _resources: &dyn crate::DeviceResources,
        ) -> Result<StereoFrame, AnalyticContinuationError> {
            self.validate_note(note)?;
            let gain = self.gain_at(target, parameters)?;
            let end = gate_frames
                .checked_add(note.release_frames())
                .ok_or(AnalyticContinuationError::FrameOverflow)?;
            let signal = if age_frames < end { gain as f32 } else { 0.0 };
            Ok([signal; 2])
        }
    }

    #[test]
    fn rapid_reversal_uses_actual_boundary_and_detached_copy_finishes_original_ramp() {
        let enabled = GainProvider {
            immutable_law: 7,
            gain: 1.0,
        };
        let disabled = GainProvider {
            immutable_law: 7,
            gain: 0.0,
        };
        let source = enabled.endpoint(id(1));
        let start = disabled
            .prepare_start(id(2), &enabled, id(1), &source)
            .unwrap();
        let off = AnalyticContinuation::new(id(2), start, 100, 4).unwrap();
        let detached = off;
        let boundary = disabled.parameters_at(id(2), &off, 102).unwrap();
        assert_eq!(f64::from_bits(boundary.words()[0]), 0.5);
        let rebound = enabled
            .prepare_start(id(3), &disabled, id(2), &boundary)
            .unwrap();
        let on = AnalyticContinuation::new(id(3), rebound, 102, 4).unwrap();
        let note = PreparedSharedAnalyticNote::new([0; 8], 10);
        let gain = enabled.parameters_at(id(3), &on, 104).unwrap();
        assert_eq!(
            enabled.sample_with_parameters(
                id(3),
                &gain,
                &note,
                25,
                30,
                &crate::EMPTY_DEVICE_RESOURCES
            ),
            Ok([0.75; 2])
        );
        let tail_gain = disabled.parameters_at(id(2), &detached, 104).unwrap();
        assert_eq!(
            disabled.sample_with_parameters(
                id(2),
                &tail_gain,
                &note,
                25,
                30,
                &crate::EMPTY_DEVICE_RESOURCES
            ),
            Ok([0.0; 2])
        );
        let endpoint = enabled.parameters_at(id(3), &on, 106).unwrap();
        assert_eq!(endpoint, enabled.endpoint(id(3)));
        assert_eq!(
            enabled.sample_with_parameters(
                id(3),
                &endpoint,
                &note,
                40,
                30,
                &crate::EMPTY_DEVICE_RESOURCES
            ),
            Ok([0.0; 2])
        );
    }

    #[test]
    fn provider_refuses_wrong_source_law_identity_words_and_note() {
        let source = GainProvider {
            immutable_law: 7,
            gain: 1.0,
        };
        let incompatible = GainProvider {
            immutable_law: 8,
            gain: 1.0,
        };
        let frame = source.endpoint(id(1));
        assert_eq!(
            incompatible.prepare_start(id(2), &source, id(1), &frame),
            Err(AnalyticContinuationError::IncompatibleSource)
        );
        assert_eq!(
            source.prepare_start(id(2), &source, id(3), &frame),
            Err(AnalyticContinuationError::WrongTarget)
        );
        let malformed = GainProvider::frame(id(1), f64::NAN);
        assert_eq!(
            source.prepare_start(id(2), &source, id(1), &malformed),
            Err(AnalyticContinuationError::InvalidParameters)
        );
        let note = PreparedSharedAnalyticNote::new([0; 8], 9);
        assert_eq!(
            source.sample_with_parameters(
                id(1),
                &frame,
                &note,
                0,
                1,
                &crate::EMPTY_DEVICE_RESOURCES
            ),
            Err(AnalyticContinuationError::InvalidNote)
        );
        assert_eq!(
            frame,
            source.endpoint(id(1)),
            "failed preparation leaves the source boundary unchanged"
        );
    }

    #[test]
    fn absolute_clock_is_partition_and_query_order_independent() {
        let frame = AnalyticParameterFrame::new(id(1), [0; ANALYTIC_CONTINUATION_WORDS]);
        let transition = AnalyticContinuation::new(id(1), frame, 900, 240).unwrap();
        for block in [1_u64, 17, 64, 128, 256] {
            let mut cursor = 900;
            while cursor < 1_300 {
                for offset in 0..block.min(1_300 - cursor) {
                    let absolute = cursor + offset;
                    assert_eq!(
                        transition.elapsed_at(absolute),
                        Ok((absolute - 900).min(240) as u32)
                    );
                }
                cursor += block;
            }
        }
        assert_eq!(transition.elapsed_at(1_140), Ok(240));
        assert_eq!(transition.elapsed_at(900), Ok(0));
        assert_eq!(
            transition.elapsed_at(899),
            Err(AnalyticContinuationError::BeforeStart)
        );
        assert_eq!(transition.elapsed_at(u64::MAX), Ok(240));
    }

    #[test]
    fn wrong_target_and_overflow_refuse_without_changing_snapshot() {
        let start = AnalyticParameterFrame::new(id(1), [42; ANALYTIC_CONTINUATION_WORDS]);
        assert_eq!(
            AnalyticContinuationId::new(0),
            Err(AnalyticContinuationError::InvalidIdentity)
        );
        assert_eq!(
            AnalyticContinuation::new(id(2), start, 0, 240),
            Err(AnalyticContinuationError::WrongTarget)
        );
        for duration in [0, MAX_ANALYTIC_CONTINUATION_FRAMES + 1] {
            assert_eq!(
                AnalyticContinuation::new(id(1), start, 0, duration),
                Err(AnalyticContinuationError::InvalidDuration)
            );
        }
        assert_eq!(
            AnalyticContinuation::new(id(1), start, u64::MAX - 239, 240),
            Err(AnalyticContinuationError::FrameOverflow)
        );
        let original = AnalyticContinuation::new(id(1), start, u64::MAX - 240, 240).unwrap();
        let detached = original;
        let replacement =
            AnalyticContinuation::new(id(2), AnalyticParameterFrame::new(id(2), [7; 8]), 0, 40)
                .unwrap();
        assert_ne!(replacement.target(), detached.target());
        assert_eq!(detached, original);
        assert_eq!(detached.elapsed_at(u64::MAX), Ok(240));
        assert_eq!(detached.start().words(), &[42; 8]);
    }

    #[test]
    fn rounded_default_duration_stays_within_the_finite_clock_domain() {
        for (rate, frames) in [(8_000, 40), (44_100, 221), (48_000, 240), (192_000, 960)] {
            assert_eq!(AnalyticContinuation::default_duration(rate), Ok(frames));
        }
        for rate in [0, 7_999, 192_001, u32::MAX] {
            assert_eq!(
                AnalyticContinuation::default_duration(rate),
                Err(AnalyticContinuationError::InvalidDuration)
            );
        }
        let format =
            AnalyticContinuationFormat::new(DeviceTypeId::from_static("hex.instrument.fixture"), 1)
                .unwrap();
        assert_eq!(format.version(), 1);
        assert!(AnalyticContinuationFormat::new(format.provider(), 0).is_err());
        assert!(AnalyticContinuationFormat::new(DeviceTypeId::from_static("invalid"), 1).is_err());
    }
}
