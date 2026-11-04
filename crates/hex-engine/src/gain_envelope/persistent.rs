//! Private borrowed preparation: every partial knot remains control-custodied.
use super::*;
#[derive(Debug)]
pub(crate) enum GainCopyError {
    Invalid,
    Capacity,
    Allocation,
    Overflow,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Vacant,
    Reserved,
    Preparing,
    Validated,
    Transferred,
}
#[derive(Default)]
pub(crate) struct GainCopyCustody {
    knots: Vec<GainEnvelopeKnot>,
    phase: Phase,
    authored: usize,
    maximum: usize,
    mix: Option<PreparedTrackMix>,
    fallback: f32,
    pub(crate) growth: usize,
    #[cfg(test)]
    pub(crate) fail_after: Option<usize>,
    #[cfg(test)]
    pub(crate) panic_after: Option<usize>,
}
impl GainCopyCustody {
    pub(crate) fn bytes(&self) -> Result<usize, GainCopyError> {
        self.knots
            .capacity()
            .checked_mul(std::mem::size_of::<GainEnvelopeKnot>())
            .ok_or(GainCopyError::Overflow)
    }
    pub(crate) fn reserve(
        &mut self,
        points: &[GainPoint],
        tempo: &TempoMap,
        mix: PreparedTrackMix,
        fallback: f32,
    ) -> Result<(), GainCopyError> {
        if self.phase != Phase::Vacant || !self.knots.is_empty() {
            return Err(GainCopyError::Capacity);
        }
        if !fallback.is_finite() || !(0.0..=2.0).contains(&fallback) {
            return Err(GainCopyError::Invalid);
        }
        self.fallback = fallback;
        let count = knot_count(points, tempo).map_err(|_| GainCopyError::Invalid)?;
        if count > MAX_PREPARED_GAIN_KNOTS {
            return Err(GainCopyError::Invalid);
        }
        count
            .checked_mul(std::mem::size_of::<GainEnvelopeKnot>())
            .ok_or(GainCopyError::Overflow)?;
        self.growth = if count > self.knots.capacity() {
            self.bytes()?
        } else {
            0
        };
        self.phase = Phase::Reserved;
        self.authored = points.len();
        self.maximum = count;
        self.mix = Some(mix);
        self.knots
            .try_reserve_exact(count)
            .map_err(|_| GainCopyError::Allocation)?;
        Ok(())
    }
    pub(crate) fn fill(
        &mut self,
        points: &[GainPoint],
        tempo: &TempoMap,
    ) -> Result<(), GainCopyError> {
        if self.phase != Phase::Reserved
            || points.len() != self.authored
            || knot_count(points, tempo).map_err(|_| GainCopyError::Invalid)? != self.maximum
            || self.knots.capacity() < self.maximum
        {
            return Err(GainCopyError::Capacity);
        }
        self.phase = Phase::Preparing;
        #[cfg(test)]
        let (fail, panic) = (self.fail_after, self.panic_after);
        fill_knots(&mut self.knots, points, tempo, |index| {
            #[cfg(test)]
            {
                assert_ne!(panic, Some(index), "persistent gain fill panic");
                if fail == Some(index) {
                    return Err(Invalid("injected gain fill failure"));
                }
            }
            #[cfg(not(test))]
            let _ = index;
            Ok(())
        })
        .map_err(|_| GainCopyError::Invalid)?;
        self.phase = Phase::Validated;
        Ok(())
    }
    pub(crate) fn can_install(&self, destination: &Option<PreparedGainEnvelope>) -> bool {
        self.phase == Phase::Validated && destination.is_none() && self.mix.is_some()
    }
    pub(crate) fn install(&mut self, destination: &mut Option<PreparedGainEnvelope>) -> bool {
        if !self.can_install(destination) {
            return false;
        }
        if self.authored != 0
            && let Some(mix) = self.mix
        {
            *destination = Some(PreparedGainEnvelope {
                knots: std::mem::take(&mut self.knots),
                mix,
                authored_points: self.authored,
                authored_units: self.authored,
                touch: PreparedTouch::default(),
                fallback: self.fallback,
            });
        }
        self.phase = Phase::Transferred;
        true
    }
    pub(crate) fn restore(
        &mut self,
        source: &mut Option<PreparedGainEnvelope>,
    ) -> Result<(), GainCopyError> {
        if source.is_some() {
            if self.phase != Phase::Transferred
                || self.knots.capacity() != 0
                || source.as_ref().is_some_and(|envelope| {
                    envelope.touch.allocated_bytes() != 0
                        || envelope.authored_units != envelope.authored_points
                })
            {
                return Err(GainCopyError::Capacity);
            }
            if let Some(PreparedGainEnvelope {
                knots,
                mix: _,
                authored_points: _,
                authored_units: _,
                touch: _,
                fallback: _,
            }) = source.take()
            {
                self.knots = knots;
            }
        }
        Ok(())
    }
    pub(crate) fn clear_on_control(&mut self) {
        // Knots are scalar-only. Exhaustive owning fields stay in this module.
        let Self {
            knots,
            phase,
            authored,
            maximum,
            mix,
            fallback: _,
            growth: _,
            #[cfg(test)]
                fail_after: _,
            #[cfg(test)]
                panic_after: _,
        } = self;
        knots.clear();
        *phase = Phase::Vacant;
        *authored = 0;
        *maximum = 0;
        *mix = None;
    }
}

impl GainCopyCustody {
    pub(crate) fn visit_audio_storage<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        let Self {
            knots,
            phase: _,
            authored: _,
            maximum: _,
            mix: _,
            fallback: _,
            growth,
            #[cfg(test)]
                fail_after: _,
            #[cfg(test)]
                panic_after: _,
        } = self;
        let _ = growth;
        ledger.vec(knots)
    }
}
