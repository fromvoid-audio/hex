//! Borrow an already admitted graph program in the original PCM transaction.
use hex_lattice::ForeignAutomation;
use hex_plugin_host::transport::{
    AuthoredParameterProgram, Fault, MAX_PARAMETER_POINTS, MAX_TIMING_CHANGES, ParameterPoint,
    Parameters, TimingChanges,
};

pub(super) struct TransportProgram<'a>(pub(super) ForeignAutomation<'a>);

impl AuthoredParameterProgram for TransportProgram<'_> {
    fn quantum_program_key(&self) -> std::num::NonZeroU64 {
        self.0.revision
    }

    fn render_quantum(
        &self,
        origins: &[Option<u64>],
        timing: &TimingChanges,
        output: &mut Parameters,
    ) -> Result<(), Fault> {
        let mut cuts = [0; MAX_TIMING_CHANGES];
        let changes = timing.as_slice();
        for (cut, change) in cuts.iter_mut().zip(changes) {
            *cut = change.frame;
        }
        let mut points = [ParameterPoint::default(); MAX_PARAMETER_POINTS];
        let len = self
            .0
            .program
            .render_quantum(origins, &cuts[..changes.len()], &mut points)
            .map_err(|_| Fault::InvalidParameters)?;
        for &point in points.get(..len).ok_or(Fault::InvalidParameters)? {
            output.push(point)?;
        }
        Ok(())
    }
}

/// The saved target selection retained by control across source renewal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthoredSelection {
    class: hex_plugin_state::ClassId,
    ids: [u32; MAX_PARAMETER_POINTS],
    len: usize,
}
impl AuthoredSelection {
    pub fn from_targets(
        targets: impl IntoIterator<Item = (u32, Option<[u8; 16]>)>,
    ) -> Result<Option<Self>, &'static str> {
        let mut targets = targets.into_iter();
        let Some((first, class)) = targets.next() else {
            return Ok(None);
        };
        let class = hex_plugin_state::ClassId::new(class.ok_or("authored vst3 class missing")?)
            .map_err(|_| "authored vst3 class is invalid")?;
        let mut selected = Self {
            class,
            ids: [0; MAX_PARAMETER_POINTS],
            len: 0,
        };
        for (id, candidate) in std::iter::once((first, Some(class.bytes()))).chain(targets) {
            if candidate != Some(class.bytes()) || id > i32::MAX as u32 {
                return Err("authored vst3 parameter identity differs");
            }
            let slot = selected
                .ids
                .get_mut(selected.len)
                .ok_or("authored vst3 target count exceeds its point budget")?;
            *slot = id;
            selected.len += 1;
        }
        let ids = &mut selected.ids[..selected.len];
        ids.sort_unstable();
        if ids.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err("duplicate authored vst3 parameter target");
        }
        Ok(Some(selected))
    }

    pub(crate) fn parameters(&self) -> (hex_plugin_state::ClassId, &[u32]) {
        (self.class, &self.ids[..self.len])
    }
}

/// The configuration is copied between existing paired owners. Fixed inline
/// storage accounts for both copies through their enclosing owner sizes.
#[derive(Clone)]
pub(super) struct SourceCapabilities {
    class: Option<hex_plugin_state::ClassId>,
    parameters: [hex_lattice::ForeignParameterCapability; MAX_PARAMETER_POINTS],
    len: usize,
}

impl Default for SourceCapabilities {
    fn default() -> Self {
        Self {
            class: None,
            parameters: [hex_lattice::ForeignParameterCapability { id: 0, steps: 0 };
                MAX_PARAMETER_POINTS],
            len: 0,
        }
    }
}

pub(super) type CanonicalParameterBases = [Option<(u32, u64)>; MAX_PARAMETER_POINTS];

impl SourceCapabilities {
    pub(super) fn arm(
        instance: &mut hex_plugin_host::native::Prepared,
        class: hex_plugin_state::ClassId,
        ids: &[u32],
    ) -> Result<(Self, CanonicalParameterBases), hex_plugin_host::Error> {
        use hex_plugin_host::Error;

        if ids.len() > MAX_PARAMETER_POINTS {
            return Err(Error::Protocol(
                "authored parameter selection exceeds the PCM point budget",
            ));
        }
        let metadata = instance.read_authored_parameter_metadata_on_control(class, ids)?;
        let mut source = Self {
            class: Some(class),
            len: ids.len(),
            ..Self::default()
        };
        let mut bases = [None; MAX_PARAMETER_POINTS];
        for ((entry, base), &id) in source.parameters.iter_mut().zip(&mut bases).zip(ids) {
            *base = Some((
                id,
                metadata
                    .initial_normalized(id)
                    .ok_or(Error::Protocol("authored canonical value missing"))?
                    .to_bits(),
            ));
            *entry = hex_lattice::ForeignParameterCapability {
                id,
                steps: metadata
                    .step_count(id)
                    .ok_or(Error::Protocol("authored parameter capability missing"))?,
            };
        }
        instance.arm_authored_parameter_metadata_on_control(metadata)?;
        Ok((source, bases))
    }

    pub(super) fn is_armed(&self) -> bool {
        self.class.is_some()
    }

    pub(super) fn view(
        &self,
        binding: hex_lattice_contract::ForeignProcessorBinding,
        sample_rate: u32,
        quantum_frames: u16,
    ) -> Option<hex_lattice::ForeignAutomationSource<'_>> {
        Some(hex_lattice::ForeignAutomationSource {
            binding,
            class: self.class?.bytes(),
            sample_rate,
            quantum_frames,
            parameters: &self.parameters[..self.len],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_targets_form_one_bounded_class_selection_with_raw_zero() {
        let class = Some([3; 16]);
        let selection = AuthoredSelection::from_targets([(17, class), (0, class), (2, class)])
            .unwrap()
            .unwrap();
        assert_eq!(selection.parameters().0.bytes(), [3; 16]);
        assert_eq!(selection.parameters().1, &[0, 2, 17]);
        assert!(AuthoredSelection::from_targets([]).unwrap().is_none());
        assert!(AuthoredSelection::from_targets([(0, class), (0, class)]).is_err());
        assert!(AuthoredSelection::from_targets([(0, class), (1, Some([4; 16]))]).is_err());
        assert!(AuthoredSelection::from_targets([(0, None)]).is_err());
        assert!(AuthoredSelection::from_targets([(u32::MAX, class)]).is_err());
        assert!(
            AuthoredSelection::from_targets(
                (0..=MAX_PARAMETER_POINTS as u32).map(|id| (id, class))
            )
            .is_err()
        );
    }
}
