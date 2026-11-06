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

impl SourceCapabilities {
    pub(super) fn arm(
        instance: &mut hex_plugin_host::native::Prepared,
        class: hex_plugin_state::ClassId,
        ids: &[u32],
    ) -> Result<Self, hex_plugin_host::Error> {
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
        for (entry, &id) in source.parameters.iter_mut().zip(ids) {
            *entry = hex_lattice::ForeignParameterCapability {
                id,
                steps: metadata
                    .step_count(id)
                    .ok_or(Error::Protocol("authored parameter capability missing"))?,
            };
        }
        instance.arm_authored_parameter_metadata_on_control(metadata)?;
        Ok(source)
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
