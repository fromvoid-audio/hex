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

    fn contains_quantum_program(&self, key: std::num::NonZeroU64) -> bool {
        self.0.contains_revision(key)
    }

    fn render_quantum_program(
        &self,
        key: std::num::NonZeroU64,
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
            .render_revision(key, origins, &cuts[..changes.len()], &mut points)
            .map_err(|_| Fault::InvalidParameters)?;
        for &point in points.get(..len).ok_or(Fault::InvalidParameters)? {
            output.push(point)?;
        }
        Ok(())
    }

    fn render_quantum(
        &self,
        origins: &[Option<u64>],
        timing: &TimingChanges,
        output: &mut Parameters,
    ) -> Result<(), Fault> {
        self.render_quantum_program(self.0.revision, origins, timing, output)
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

    /// Selection membership only; actual SDK/source admission stays with Root.
    pub fn contains(&self, class: [u8; 16], id: u32) -> bool {
        self.class.bytes() == class && self.ids[..self.len].binary_search(&id).is_ok()
    }
    /// Keep already armed targets when a canonical lane stops reading or is cleared.
    pub fn covers(&self, requested: Option<&Self>) -> bool {
        requested.is_none_or(|requested| {
            self.class == requested.class
                && requested.ids[..requested.len]
                    .iter()
                    .all(|id| self.ids[..self.len].binary_search(id).is_ok())
        })
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
            parameters: [hex_lattice::ForeignParameterCapability {
                id: 0,
                steps: 0,
                normalized_base_bits: 0,
            }; MAX_PARAMETER_POINTS],
            len: 0,
        }
    }
}

pub(super) type CanonicalParameterBases = [Option<(u32, u64)>; MAX_PARAMETER_POINTS];

impl SourceCapabilities {
    /// The original source reserved its native metadata container. An empty
    /// set grants no target; each later ID still needs its actual SDK ACK.
    pub(super) fn reserved(class: hex_plugin_state::ClassId) -> Self {
        Self {
            class: Some(class),
            ..Self::default()
        }
    }

    pub(super) fn arm(
        instance: &mut hex_plugin_host::native::Prepared,
        class: hex_plugin_state::ClassId,
        ids: &[u32],
        retained: Option<&Self>,
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
            let previous = retained.and_then(|old| {
                (old.class == Some(class))
                    .then(|| old.parameters().iter().find(|p| p.id == id))
                    .flatten()
            });
            if retained.is_some()
                && previous.is_none_or(|p| metadata.step_count(id) != Some(p.steps))
            {
                return Err(Error::Protocol(
                    "renewed authored parameter capability differs",
                ));
            }
            let normalized = previous
                .map(|p| f64::from_bits(p.normalized_base_bits))
                .or_else(|| metadata.initial_normalized(id))
                .ok_or(Error::Protocol("authored canonical value missing"))?;
            *base = Some((id, normalized.to_bits()));
            *entry = hex_lattice::ForeignParameterCapability {
                id,
                steps: metadata
                    .step_count(id)
                    .ok_or(Error::Protocol("authored parameter capability missing"))?,
                normalized_base_bits: normalized.to_bits(),
            };
        }
        instance.arm_authored_parameter_metadata_on_control(metadata)?;
        Ok((source, bases))
    }

    pub(super) fn from_canonical(
        class: hex_plugin_state::ClassId,
        values: &CanonicalParameterBases,
        steps: &[u32; MAX_PARAMETER_POINTS],
    ) -> Result<Self, &'static str> {
        let mut result = Self {
            class: Some(class),
            ..Self::default()
        };
        let mut ended = false;
        for (value, &steps) in values.iter().zip(steps) {
            let Some((id, bits)) = *value else {
                ended = true;
                continue;
            };
            let normalized = f64::from_bits(bits);
            if ended
                || !normalized.is_finite()
                || !(0.0..=1.0).contains(&normalized)
                || result.parameters[..result.len]
                    .last()
                    .is_some_and(|old| old.id >= id)
            {
                return Err("native canonical parameter metadata differs");
            }
            *result
                .parameters
                .get_mut(result.len)
                .ok_or("native canonical parameter capacity exceeded")? =
                hex_lattice::ForeignParameterCapability {
                    id,
                    steps,
                    normalized_base_bits: bits,
                };
            result.len += 1;
        }
        Ok(result)
    }

    pub(super) fn parameters(&self) -> &[hex_lattice::ForeignParameterCapability] {
        &self.parameters[..self.len]
    }

    pub(super) fn class(&self) -> Option<hex_plugin_state::ClassId> {
        self.class
    }

    pub(super) fn same_parameters(&self, other: &Self) -> bool {
        self.class == other.class
            && self.len == other.len
            && self
                .parameters()
                .iter()
                .zip(other.parameters())
                .all(|(a, b)| a.id == b.id && a.steps == b.steps)
    }

    pub(super) fn has_metadata_container(&self) -> bool {
        // The paired callback needs this reserved path, not a mutable target
        // list. Control definitions publish targets; the original SDK worker
        // independently validates every delivered ID against its armed set.
        self.class.is_some()
    }

    pub(super) fn view(
        &self,
        binding: hex_lattice_contract::ForeignProcessorBinding,
        sample_rate: u32,
        quantum_frames: u16,
        revision: std::num::NonZeroU64,
    ) -> Option<hex_lattice::ForeignAutomationSource<'_>> {
        Some(hex_lattice::ForeignAutomationSource {
            binding,
            revision,
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
    fn canonical_metadata_keeps_exact_bases_and_requires_complete_ordered_values() {
        let class = hex_plugin_state::ClassId::new([3; 16]).unwrap();
        let mut values = [None; MAX_PARAMETER_POINTS];
        let steps = [0; MAX_PARAMETER_POINTS];
        let precise = f64::from_bits(0x3fe0000000000001);
        values[0] = Some((0, precise.to_bits()));
        values[1] = Some((7, 0.25f64.to_bits()));
        let first = SourceCapabilities::from_canonical(class, &values, &steps).unwrap();
        assert_eq!(
            first.parameters()[0].normalized_base_bits,
            precise.to_bits()
        );
        values[1] = Some((7, 0.75f64.to_bits()));
        let changed = SourceCapabilities::from_canonical(class, &values, &steps).unwrap();
        assert!(first.same_parameters(&changed));
        let mut changed_steps = steps;
        changed_steps[1] = 3;
        assert!(!first.same_parameters(
            &SourceCapabilities::from_canonical(class, &values, &changed_steps).unwrap()
        ));
        values[0] = None;
        assert!(SourceCapabilities::from_canonical(class, &values, &steps).is_err());
        values[0] = Some((7, precise.to_bits()));
        assert!(SourceCapabilities::from_canonical(class, &values, &steps).is_err());
        values[0] = Some((0, f64::NAN.to_bits()));
        assert!(SourceCapabilities::from_canonical(class, &values, &steps).is_err());
    }

    #[test]
    fn saved_targets_form_one_bounded_class_selection_with_raw_zero() {
        let class = Some([3; 16]);
        let selection = AuthoredSelection::from_targets([(17, class), (0, class), (2, class)])
            .unwrap()
            .unwrap();
        assert_eq!(selection.parameters().0.bytes(), [3; 16]);
        assert_eq!(selection.parameters().1, &[0, 2, 17]);
        assert!(selection.contains([3; 16], 0));
        assert!(!selection.contains([4; 16], 0));
        assert!(selection.covers(None));
        let subset = AuthoredSelection::from_targets([(0, class), (2, class)]).unwrap();
        assert!(selection.covers(subset.as_ref()));
        assert!(
            !selection.covers(
                AuthoredSelection::from_targets([(99, class)])
                    .unwrap()
                    .as_ref()
            )
        );
        assert!(
            !selection.covers(
                AuthoredSelection::from_targets([(0, Some([4; 16]))])
                    .unwrap()
                    .as_ref()
            )
        );
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

#[cfg(all(test, feature = "allocation-probe"))]
mod retained_tests {
    use super::*;
    use hex_lattice::{ForeignAutomationProgram, PreparedForeignAutomation};
    use hex_lattice_contract::ForeignProcessorBinding;
    use hex_plugin_host::transport::{AuthoredSegment, Callback, SegmentTiming, channel};
    use std::num::NonZeroU64;
    struct Value(f64);
    impl ForeignAutomationProgram for Value {
        fn render_quantum(
            &self,
            origins: &[Option<u64>],
            _: &[u16],
            output: &mut [ParameterPoint],
        ) -> Result<usize, hex_lattice_contract::DeviceContractError> {
            output[0] = ParameterPoint {
                id: 0,
                frame: 0,
                normalized: self.0,
            };
            output[1] = ParameterPoint {
                id: 0,
                frame: (origins.len() - 1) as u16,
                normalized: self.0,
            };
            Ok(2)
        }
    }
    fn stage(
        callback: &mut Callback,
        frames: usize,
        start: u64,
        program: &PreparedForeignAutomation,
    ) {
        let transport = TransportProgram(program.borrow());
        let mut output = [[0.0; 2]; 8];
        let measured = allocation_counter::measure(|| {
            callback.begin_physical_callback(frames).unwrap();
            callback
                .stage_segment_with_authored_parameters(
                    0,
                    &[[0.0; 2]; 8][..frames],
                    &[],
                    &mut output[..frames],
                    SegmentTiming {
                        playing: None,
                        at: |_| None,
                    },
                    AuthoredSegment {
                        program: &transport,
                        origin: &|offset| Some(start + offset as u64),
                    },
                )
                .unwrap();
            callback.finish(true).unwrap();
        });
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    #[test]
    fn full_constant_base_program_stages_without_callback_allocation_or_drop() {
        let project = hex_project::Project::new(8000).unwrap();
        let capabilities = (0..MAX_PARAMETER_POINTS as u32)
            .map(|id| hex_engine::foreign_automation::ParameterCapability {
                id,
                steps: 0,
                normalized_base_bits: (f64::from(id) / MAX_PARAMETER_POINTS as f64).to_bits(),
            })
            .collect::<Vec<_>>();
        let program = hex_engine::foreign_automation::Program::prepare(
            &[],
            [1; 16],
            &capabilities,
            project.tempo_map(),
            hex_engine::foreign_automation::QuantumProfile::for_arrangement_instrument(8, None),
        )
        .unwrap()
        .unwrap();
        let binding = ForeignProcessorBinding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 4,
        };
        let bytes = program.backing_bytes().unwrap();
        let program =
            PreparedForeignAutomation::new(binding, NonZeroU64::new(1).unwrap(), program, bytes)
                .unwrap();
        let (_control, mut callback, mut worker) = channel(binding, 8).unwrap();
        stage(&mut callback, 8, 0, &program);
        let request = worker.receive_request().unwrap().unwrap();
        assert_eq!(request.parameter_capacity, 0);
        assert_eq!(
            request.block.parameters.as_slice().len(),
            MAX_PARAMETER_POINTS
        );
        for (point, expected) in request
            .block
            .parameters
            .as_slice()
            .iter()
            .zip(&capabilities)
        {
            assert_eq!((point.frame, point.id), (0, expected.id));
            assert_eq!(point.normalized.to_bits(), expected.normalized_base_bits);
        }
        worker.complete(request.block).unwrap();
    }

    #[test]
    fn borrowed_versions_finish_the_old_quantum_after_original_graph_owners_drop() {
        let binding = ForeignProcessorBinding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 4,
        };
        let make = |key, value| {
            PreparedForeignAutomation::new(binding, NonZeroU64::new(key).unwrap(), Value(value), 0)
                .unwrap()
        };
        let (_control, mut callback, mut worker) = channel(binding, 8).unwrap();
        let first = make(1, 0.25);
        stage(&mut callback, 2, 0, &first);
        let mut second = make(2, 0.5);
        second.retain_previous_on_control(&first).unwrap();
        drop(first);
        stage(&mut callback, 3, 2, &second);
        let mut third = make(3, 0.75);
        third.retain_previous_on_control(&second).unwrap();
        drop(second);
        stage(&mut callback, 3, 5, &third);
        for expected in [0.25, 0.75] {
            let request = worker.receive_request().unwrap().unwrap();
            assert!(request.authored_program.is_some());
            assert_eq!(request.parameter_capacity, 62);
            assert_eq!(
                request.block.parameters.as_slice(),
                &[
                    ParameterPoint {
                        id: 0,
                        frame: 0,
                        normalized: expected
                    },
                    ParameterPoint {
                        id: 0,
                        frame: 7,
                        normalized: expected
                    }
                ]
            );
            worker.complete(request.block).unwrap();
            if expected == 0.25 {
                stage(&mut callback, 8, 8, &third);
            }
        }
    }
}
