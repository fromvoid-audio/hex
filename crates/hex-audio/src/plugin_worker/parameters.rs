//! Coalesced processor observations in the original worker/completion storage.
//! This read-only control query is neither authored automation nor host-delivery
//! acceptance. The callback never touches its existing capture-slot mutex.
use super::*;
use std::sync::TryLockError;

/// Matches the native helper's admitted metadata bound. Observations retain
/// the last value of every observed ID, rather than only the most recent block.
pub const MAX_OBSERVED_PARAMETERS: usize = 128;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NativeParameterValue {
    pub id: u32,
    pub normalized: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct NativeParameterValues {
    values: [NativeParameterValue; MAX_OBSERVED_PARAMETERS],
    len: usize,
    completed_blocks: u64,
}
impl Default for NativeParameterValues {
    fn default() -> Self {
        Self {
            values: [NativeParameterValue::default(); MAX_OBSERVED_PARAMETERS],
            len: 0,
            completed_blocks: 0,
        }
    }
}
impl NativeParameterValues {
    pub fn as_slice(&self) -> &[NativeParameterValue] {
        &self.values[..self.len]
    }
    /// Exact original worker reply prefix, not final callback delivery.
    pub fn completed_blocks(&self) -> u64 {
        self.completed_blocks
    }
    fn observe(&mut self, points: &Parameters, completed_blocks: u64) -> Result<(), Fault> {
        let mut next = *self;
        for point in points.as_slice() {
            let index = next
                .as_slice()
                .iter()
                .position(|value| value.id == point.id);
            let index = match index {
                Some(index) => index,
                None if next.len < next.values.len() => {
                    let index = next.len;
                    next.len += 1;
                    index
                }
                None => return Err(Fault::InvalidParameters),
            };
            next.values[index] = NativeParameterValue {
                id: point.id,
                normalized: point.normalized,
            };
        }
        next.completed_blocks = completed_blocks;
        *self = next;
        Ok(())
    }
}
#[derive(Default)]
pub(super) struct Work {
    values: NativeParameterValues,
    dirty: bool,
}
impl State {
    pub(super) fn observe_parameter_reply(&mut self, reply: &Block) -> Result<(), Fault> {
        if !reply.parameters.as_slice().is_empty() {
            self.parameters.values.observe(
                &reply.parameters,
                reply.sequence.checked_add(1).ok_or(Fault::Protocol)?,
            )?;
            self.parameters.dirty = true;
        }
        Ok(())
    }
    pub(super) fn publish_parameter_observations(&mut self, signal: &Signal) -> Result<(), Fault> {
        if self.parameters.dirty {
            match signal.capture.try_lock() {
                Ok(mut slot) => {
                    slot.parameters = self.parameters.values;
                    self.parameters.dirty = false;
                }
                Err(TryLockError::WouldBlock) => {} // Original worker retains all IDs for retry.
                Err(TryLockError::Poisoned(_)) => return Err(Fault::Protocol),
            }
        }
        Ok(())
    }
}
impl NativeWorker {
    /// Inspect the retained original source's latest processor observations.
    /// Busy refuses this read only; it does not consume values or disturb capture.
    /// Source return, retirement and transport revocation invalidate the query.
    pub fn parameter_values_on_control(&self) -> Result<NativeParameterValues, &'static str> {
        if !self.transport_is_live_on_control() || !self.retains_original_source_on_control() {
            return Err("native parameter source is not live");
        }
        let slot = self
            .reaper
            .completed
            .capture
            .try_lock()
            .map_err(|_| "native parameter observation is busy")?;
        Ok(slot.parameters)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observations_retain_distinct_ids_coalesce_last_values_and_refuse_overflow_atomically() {
        let mut values = NativeParameterValues::default();
        for block in 0..2 {
            let mut points = Parameters::default();
            for id in block * 64..(block + 1) * 64 {
                points
                    .push(ParameterPoint {
                        frame: 0,
                        id,
                        normalized: 0.25,
                    })
                    .unwrap();
            }
            values.observe(&points, u64::from(block + 1)).unwrap();
        }
        let mut points = Parameters::default();
        points
            .push(ParameterPoint {
                frame: 0,
                id: 0,
                normalized: 0.75,
            })
            .unwrap();
        values.observe(&points, 3).unwrap();
        assert_eq!(values.as_slice().len(), 128);
        assert_eq!(
            values.as_slice()[0],
            NativeParameterValue {
                id: 0,
                normalized: 0.75
            }
        );
        assert_eq!(
            values.as_slice()[127],
            NativeParameterValue {
                id: 127,
                normalized: 0.25
            }
        );
        let before = values;
        let mut points = Parameters::default();
        points
            .push(ParameterPoint {
                frame: 0,
                id: 0,
                normalized: 0.125,
            })
            .unwrap();
        points
            .push(ParameterPoint {
                frame: 0,
                id: 128,
                normalized: 0.5,
            })
            .unwrap();
        assert_eq!(values.observe(&points, 4), Err(Fault::InvalidParameters));
        assert_eq!(values.as_slice(), before.as_slice());
        assert_eq!(values.completed_blocks(), 3);
    }
}
