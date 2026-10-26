//! Copied VST3 parameter points in the existing PCM carrier. IDs are the
//! provider's raw stable ParamIDs, including zero, rather than Hex's native
//! device IDs. Metadata/flags and actual SDK admission remain the host's job.
use super::Fault;

/// Total distinct parameter/offset pairs in one native quantum, in either
/// direction. Inline storage travels through the existing eight-slot rings;
/// channel and endpoint storage quotes account for its actual size.
pub const MAX_PARAMETER_POINTS: usize = 64;

pub use hex_lattice_contract::ForeignParameterPoint as ParameterPoint;

fn valid_value(point: ParameterPoint) -> bool {
    // The upper half of the SDK ParamID range is reserved for the host.
    point.id <= i32::MAX as u32
        && point.normalized.is_finite()
        && (0.0..=1.0).contains(&point.normalized)
}
pub(super) fn valid(point: ParameterPoint, frames: usize) -> bool {
    usize::from(point.frame) < frames && valid_value(point)
}

/// Globally ordered offsets preserve each parameter queue's ordering. The
/// last value for an equal ID/offset replaces its earlier value, without
/// consuming another slot or changing equal-offset parameter order. Overflow
/// and malformed/out-of-order input refuse before changing this payload.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Parameters {
    values: [ParameterPoint; MAX_PARAMETER_POINTS],
    len: usize,
}
impl Default for Parameters {
    fn default() -> Self {
        Self {
            values: [ParameterPoint::default(); MAX_PARAMETER_POINTS],
            len: 0,
        }
    }
}
impl Parameters {
    pub fn as_slice(&self) -> &[ParameterPoint] {
        &self.values[..self.len]
    }
    pub fn push(&mut self, point: ParameterPoint) -> Result<(), Fault> {
        if !valid_value(point)
            || self
                .as_slice()
                .last()
                .is_some_and(|last| last.frame > point.frame)
        {
            return Err(Fault::InvalidParameters);
        }
        if let Some(previous) = self.values[..self.len]
            .iter_mut()
            .find(|previous| previous.frame == point.frame && previous.id == point.id)
        {
            *previous = point;
            return Ok(());
        }
        let slot = self
            .values
            .get_mut(self.len)
            .ok_or(Fault::InvalidParameters)?;
        *slot = point;
        self.len += 1;
        Ok(())
    }
    pub(super) fn valid(&self, frames: usize) -> bool {
        self.as_slice().iter().all(|point| valid(*point, frames))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_offset_last_value_wins_at_capacity_without_reordering_other_ids() {
        let mut points = Parameters::default();
        for id in 0..MAX_PARAMETER_POINTS as u32 {
            points
                .push(ParameterPoint {
                    frame: 7,
                    id,
                    normalized: 0.25,
                })
                .unwrap();
        }
        points
            .push(ParameterPoint {
                frame: 7,
                id: 0,
                normalized: 0.75,
            })
            .unwrap();
        assert_eq!(points.as_slice().len(), MAX_PARAMETER_POINTS);
        assert_eq!(points.as_slice()[0].normalized, 0.75);
        assert_eq!(points.as_slice()[1].id, 1);
        let before = points;
        assert_eq!(
            points.push(ParameterPoint {
                frame: 7,
                id: 64,
                normalized: 0.5
            }),
            Err(Fault::InvalidParameters)
        );
        assert_eq!(points, before);
        assert!(points.valid(8));
        assert!(!points.valid(7));
    }

    #[test]
    fn invalid_values_reserved_ids_and_decreasing_offsets_preserve_prior_points() {
        let mut points = Parameters::default();
        points
            .push(ParameterPoint {
                frame: 3,
                id: 0,
                normalized: 1.0,
            })
            .unwrap();
        let before = points;
        for point in [
            ParameterPoint {
                frame: 2,
                id: 1,
                normalized: 0.5,
            },
            ParameterPoint {
                frame: 3,
                id: 0x8000_0000,
                normalized: 0.5,
            },
            ParameterPoint {
                frame: 3,
                id: 1,
                normalized: f64::NAN,
            },
            ParameterPoint {
                frame: 3,
                id: 1,
                normalized: f64::INFINITY,
            },
            ParameterPoint {
                frame: 3,
                id: 1,
                normalized: -0.01,
            },
            ParameterPoint {
                frame: 3,
                id: 1,
                normalized: 1.01,
            },
        ] {
            assert_eq!(points.push(point), Err(Fault::InvalidParameters));
            assert_eq!(points, before);
        }
    }
}
