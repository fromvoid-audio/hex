//! Prepared, bounded parameter composition over borrowed source sample blocks.
//!
//! This is a value-delivery primitive, not source, project, clock, graph or
//! eligibility ownership. The host must admit only targets whose actual runtime
//! supports the requested rate. Base values and routes are prepared on control;
//! rendering borrows all storage and never changes authored values. Upstream
//! manual/automation selection wins over base; bipolar sources are summed in
//! normalized units, then clamped and converted to canonical units once.
//! An already admitted structural automation lane retains its exact typed value
//! and can coexist with routes to other parameters.
use crate::parameter_automation::Mapping;
use crate::{NativeParameterAutomation, NormalizedAutomationValue};
use hex_lattice_contract::{
    DeviceContractError as Error, HostGain, ParameterAccess, ParameterAutomation,
    ParameterChangeImpact, ParameterDescriptor, ParameterId, ParameterModulation,
    ParameterSemantics, ParameterStepping, ParameterValue,
};

/// Bounds apply to one device-local prepared program and its borrowed blocks.
pub const MAX_MODULATION_SOURCES: usize = 32;
pub const MAX_MODULATION_ROUTES: usize = 512;
pub const MAX_MODULATION_BLOCK_FRAMES: usize = 8192;

/// One real provider target and its immutable authored base value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModulationTarget {
    pub descriptor: ParameterDescriptor,
    pub semantics: ParameterSemantics,
    pub base: ParameterValue,
}

/// Map the bipolar sample to the given range, then apply the signed amount.
/// The source index is local to this prepared program; it is not a persistent
/// identity and must be resolved from the canonical route by the host.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModulationRoute {
    pub source: usize,
    pub target: ParameterId,
    pub amount: f32,
    pub range: [f32; 2],
}

/// A signed normalized offset to the one foreign target of a prepared bank.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ForeignModulationRoute {
    pub source: usize,
    pub amount: f32,
    pub range: [f32; 2],
}

/// A device-local source route to one of the two host gain stages.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HostGainModulationRoute {
    pub source: usize,
    pub amount: f32,
    pub range: [f32; 2],
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct HostGainTarget {
    base: HostGain,
    route_start: usize,
    route_end: usize,
}
type HostGainRoutes<'a> = Option<(HostGain, &'a [HostGainModulationRoute])>;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Target {
    authored: ModulationTarget,
    normalized_base: f32,
    mapping: Mapping,
    route_start: usize,
    route_end: usize,
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct Route {
    source: usize,
    target_index: usize,
    order: usize,
    contribution: RangeContribution,
}

/// A validated affine mapping from bipolar samples to normalized offsets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RangeContribution {
    scale: f64,
    offset: f64,
}

impl RangeContribution {
    pub(crate) fn prepare(amount: f32, range: [f32; 2]) -> Result<Self, Error> {
        if !(-1.0..=1.0).contains(&amount) || range.iter().any(|v| !(-1.0..=1.0).contains(v)) {
            return Err(Error::InvalidParameterValue);
        }
        let [start, end] = range.map(f64::from);
        let half_amount = f64::from(amount) * 0.5;
        Ok(Self {
            scale: half_amount * (end - start),
            offset: half_amount * (end + start),
        })
    }

    pub(crate) fn sample(self, bipolar: f32) -> f64 {
        f64::from(bipolar) * self.scale + self.offset
    }
}

/// One inline route for an occurrence-owned pressure input. The provider
/// admits the real destination and owns the pressure and its lifetime.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreparedPressureRoute {
    descriptor: ParameterDescriptor,
    semantics: ParameterSemantics,
    mapping: Mapping,
    contribution: RangeContribution,
}

impl PreparedPressureRoute {
    pub fn new(
        descriptor: ParameterDescriptor,
        semantics: ParameterSemantics,
        amount: f32,
        range: [f32; 2],
    ) -> Result<Self, Error> {
        semantics.validate(descriptor)?;
        if semantics.access != ParameterAccess::ReadWrite
            || semantics.change_impact != ParameterChangeImpact::ValueOnly
            || semantics.modulation != ParameterModulation::SampleRate
            || semantics.stepping == ParameterStepping::Unspecified
        {
            return Err(Error::InvalidDescriptor);
        }
        Ok(Self {
            descriptor,
            semantics,
            mapping: Mapping::prepare(descriptor, semantics)?,
            contribution: RangeContribution::prepare(amount, range)?,
        })
    }

    pub const fn descriptor(&self) -> ParameterDescriptor {
        self.descriptor
    }

    pub const fn semantics(&self) -> ParameterSemantics {
        self.semantics
    }

    /// A pressure of 0/127 selects the first/last authored range endpoint.
    /// Add its signed contribution to the source-global value before the
    /// final clamp. Absent a global route, normalize the provider's base.
    pub fn compose(
        &self,
        base: ParameterValue,
        normalized_before_clamp: Option<f64>,
        pressure: u8,
    ) -> Result<ParameterValue, Error> {
        if pressure > 127 || normalized_before_clamp.is_some_and(|value| !value.is_finite()) {
            return Err(Error::InvalidParameterValue);
        }
        self.descriptor.validate(base)?;
        let bipolar = f32::from(pressure) * (2.0 / 127.0) - 1.0;
        let contribution = self.contribution.sample(bipolar);
        if contribution == 0.0 {
            return Ok(base);
        }
        let normalized = match normalized_before_clamp {
            Some(value) => value,
            None => f64::from(
                self.semantics
                    .normalization
                    .normalize(self.descriptor, base)?,
            ),
        };
        self.mapping
            .value((normalized + contribution).clamp(0.0, 1.0) as f32)?
            .provider()
            .ok_or(Error::InvalidDescriptor)
    }
}

/// Control-prepared target and route inventory. IDs are stable and sorted.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedModulation {
    ids: Vec<ParameterId>,
    targets: Vec<Target>,
    routes: Vec<Route>,
    input: Option<HostGainTarget>,
    output: Option<HostGainTarget>,
    source_count: usize,
}
impl PreparedModulation {
    /// Authored host output trim only when this program supplies that stage.
    pub fn host_output_gain_base(&self) -> Option<HostGain> {
        self.output.map(|target| target.base)
    }

    /// Copy a prepared program on control, reporting allocation failure.
    /// Runtime publication must account the copy as separately owned storage.
    pub fn try_clone(&self) -> Result<Self, Error> {
        fn copy<T: Copy>(input: &[T]) -> Result<Vec<T>, Error> {
            let mut output = Vec::new();
            output
                .try_reserve_exact(input.len())
                .map_err(|_| Error::RuntimeAllocation)?;
            output.extend_from_slice(input);
            Ok(output)
        }
        Ok(Self {
            ids: copy(&self.ids)?,
            targets: copy(&self.targets)?,
            routes: copy(&self.routes)?,
            input: self.input,
            output: self.output,
            source_count: self.source_count,
        })
    }
    /// Rejects invalid targets, routes and capacities before constructing a program.
    /// Callers remain responsible for state-aware runtime admission.
    pub fn new(
        targets: &[ModulationTarget],
        routes: &[ModulationRoute],
        source_count: usize,
    ) -> Result<Self, Error> {
        Self::with_host_gains(targets, routes, source_count, None, None)
    }

    /// Compile provider, host input and host output routes into one bounded
    /// program. Neither host stage has a provider parameter identity.
    pub fn with_host_gains(
        targets: &[ModulationTarget],
        routes: &[ModulationRoute],
        source_count: usize,
        input: HostGainRoutes<'_>,
        output: HostGainRoutes<'_>,
    ) -> Result<Self, Error> {
        let route_count = routes
            .len()
            .saturating_add(input.map_or(0, |(_, routes)| routes.len()))
            .saturating_add(output.map_or(0, |(_, routes)| routes.len()));
        if targets.len() > hex_lattice_contract::MAX_PARAMETERS_PER_DEVICE
            || route_count > MAX_MODULATION_ROUTES
            || source_count > MAX_MODULATION_SOURCES
        {
            return Err(Error::RuntimeBudgetExceeded);
        }
        let mut ids = Vec::new();
        ids.try_reserve_exact(targets.len())
            .map_err(|_| Error::RuntimeAllocation)?;
        let mut prepared_targets = Vec::new();
        prepared_targets
            .try_reserve_exact(targets.len())
            .map_err(|_| Error::RuntimeAllocation)?;
        for &target in targets {
            let descriptor = target.descriptor;
            target.semantics.validate(descriptor)?;
            let structural_automation = target.semantics.change_impact
                == ParameterChangeImpact::Structural
                && target.semantics.automation == ParameterAutomation::DeviceArrangementEnvelope
                && target.semantics.modulation == ParameterModulation::Unsupported;
            if target.semantics.access != ParameterAccess::ReadWrite
                || (target.semantics.change_impact != ParameterChangeImpact::ValueOnly
                    && !structural_automation)
                || target.semantics.stepping == ParameterStepping::Unspecified
                || ids
                    .last()
                    .is_some_and(|previous| *previous >= descriptor.id)
            {
                return Err(Error::InvalidDescriptor);
            }
            let normalized_base = target
                .semantics
                .normalization
                .normalize(descriptor, target.base)?;
            let mapping = Mapping::prepare(descriptor, target.semantics)?;
            ids.push(descriptor.id);
            prepared_targets.push(Target {
                authored: target,
                normalized_base,
                mapping,
                route_start: 0,
                route_end: 0,
            });
        }
        let mut prepared_routes = Vec::new();
        prepared_routes
            .try_reserve_exact(route_count)
            .map_err(|_| Error::RuntimeAllocation)?;
        for (order, route) in routes.iter().enumerate() {
            if route.source >= source_count {
                return Err(Error::InvalidParameterValue);
            }
            let target_index = ids
                .binary_search(&route.target)
                .map_err(|_| Error::UnknownParameter)?;
            if prepared_targets[target_index]
                .authored
                .semantics
                .change_impact
                != ParameterChangeImpact::ValueOnly
            {
                return Err(Error::InvalidDescriptor);
            }
            let contribution = RangeContribution::prepare(route.amount, route.range)?;
            prepared_routes.push(Route {
                source: route.source,
                target_index,
                order,
                contribution,
            });
        }
        // Include original order in the key so sorting uses no scratch
        // allocation and preserves the caller's summation order per target.
        prepared_routes.sort_unstable_by_key(|route| (route.target_index, route.order));
        let mut next = 0;
        for (index, target) in prepared_targets.iter_mut().enumerate() {
            target.route_start = next;
            while prepared_routes
                .get(next)
                .is_some_and(|route| route.target_index == index)
            {
                next += 1;
            }
            target.route_end = next;
        }
        let mut append_host_routes = |stage: HostGainRoutes<'_>| -> Result<_, Error> {
            let Some((base, routes)) = stage.filter(|(_, routes)| !routes.is_empty()) else {
                return Ok(None);
            };
            let route_start = prepared_routes.len();
            for (order, route) in routes.iter().enumerate() {
                if route.source >= source_count {
                    return Err(Error::InvalidParameterValue);
                }
                prepared_routes.push(Route {
                    source: route.source,
                    target_index: targets.len(),
                    order,
                    contribution: RangeContribution::prepare(route.amount, route.range)?,
                });
            }
            Ok(Some(HostGainTarget {
                base,
                route_start,
                route_end: prepared_routes.len(),
            }))
        };
        let input = append_host_routes(input)?;
        let output = append_host_routes(output)?;
        Ok(Self {
            ids,
            targets: prepared_targets,
            routes: prepared_routes,
            input,
            output,
            source_count,
        })
    }

    pub(crate) fn for_foreign_target(
        routes: &[ForeignModulationRoute],
        source_count: usize,
    ) -> Result<Self, Error> {
        if routes.len() > MAX_MODULATION_ROUTES || source_count > MAX_MODULATION_SOURCES {
            return Err(Error::RuntimeBudgetExceeded);
        }
        let mut program = Self::new(&[], &[], source_count)?;
        program
            .routes
            .try_reserve_exact(routes.len())
            .map_err(|_| Error::RuntimeAllocation)?;
        for (order, route) in routes.iter().enumerate() {
            if route.source >= source_count {
                return Err(Error::InvalidParameterValue);
            }
            program.routes.push(Route {
                source: route.source,
                target_index: 0,
                order,
                contribution: RangeContribution::prepare(route.amount, route.range)?,
            });
        }
        Ok(program)
    }

    /// Reserved vector capacities, excluding inline state and allocator bookkeeping.
    /// The owning graph must include this quote in its own admission budget.
    pub fn storage_bytes(&self) -> Option<usize> {
        Self::quoted_storage_bytes(
            self.ids.capacity(),
            self.targets.capacity(),
            self.routes.capacity(),
        )
    }

    pub(crate) fn quoted_storage_bytes(ids: usize, targets: usize, routes: usize) -> Option<usize> {
        ids.checked_mul(std::mem::size_of::<ParameterId>())?
            .checked_add(targets.checked_mul(std::mem::size_of::<Target>())?)?
            .checked_add(routes.checked_mul(std::mem::size_of::<Route>())?)
    }

    pub(crate) fn route_count(&self) -> usize {
        self.routes.len()
    }

    /// Stable provider IDs, sorted and unique for runtime admission.
    pub fn parameter_ids(&self) -> &[ParameterId] {
        &self.ids
    }

    /// Descriptor the host must compare with the live provider before use.
    pub fn descriptor(&self, id: ParameterId) -> Option<ParameterDescriptor> {
        let index = self.ids.binary_search(&id).ok()?;
        self.targets
            .get(index)
            .map(|target| target.authored.descriptor)
    }

    /// Semantics the host must revalidate against the current provider state.
    pub fn semantics(&self, id: ParameterId) -> Option<ParameterSemantics> {
        let index = self.ids.binary_search(&id).ok()?;
        self.targets
            .get(index)
            .map(|target| target.authored.semantics)
    }

    /// Whether the provider value has an authored modulation route.
    pub fn has_routes(&self, id: ParameterId) -> bool {
        self.ids.binary_search(&id).ok().is_some_and(|index| {
            let target = &self.targets[index];
            target.route_start != target.route_end
        })
    }

    pub(crate) fn host_input_base(&self) -> Option<HostGain> {
        self.input.map(|target| target.base)
    }

    pub(crate) fn host_output_base(&self) -> Option<HostGain> {
        self.output.map(|target| target.base)
    }

    /// Number of source buffers required by this prepared program.
    pub fn source_count(&self) -> usize {
        self.source_count
    }

    /// Borrow a block. Every input is checked before it is exposed to a provider.
    /// Upstream IDs must be sorted, unique and contained in this program.
    pub fn block<'a>(
        &'a self,
        upstream: Option<&'a dyn NativeParameterAutomation>,
        sources: &'a [&'a [f32]],
        frames: usize,
    ) -> Result<ModulationBlock<'a>, Error> {
        if frames > MAX_MODULATION_BLOCK_FRAMES {
            return Err(Error::RuntimeBudgetExceeded);
        }
        if sources.len() != self.source_count {
            return Err(Error::InvalidParameterValue);
        }
        if sources.iter().any(|samples| {
            samples.len() != frames
                || samples
                    .iter()
                    .any(|v| !v.is_finite() || !(-1.0..=1.0).contains(v))
        }) {
            return Err(Error::InvalidParameterValue);
        }
        if let Some(upstream) = upstream {
            let ids = upstream.parameter_ids();
            if ids.windows(2).any(|pair| pair[0] >= pair[1])
                || ids.iter().any(|id| self.ids.binary_search(id).is_err())
            {
                return Err(Error::UnknownParameter);
            }
        }
        Ok(ModulationBlock {
            program: self,
            upstream,
            sources,
            frames,
        })
    }
}

/// Immutable, freely reorderable parameter queries for one source block.
pub struct ModulationBlock<'a> {
    program: &'a PreparedModulation,
    upstream: Option<&'a dyn NativeParameterAutomation>,
    sources: &'a [&'a [f32]],
    frames: usize,
}
impl ModulationBlock<'_> {
    /// Read the composed post-instrument host trim from the borrowed block.
    /// The instrument owner supplies no separate authored host automation lane.
    pub fn instrument_output_gain_at(&self, offset: usize) -> Result<HostGain, Error> {
        self.host_output_gain_at(offset, None)
    }

    pub(crate) fn host_input_gain_at(
        &self,
        offset: usize,
        upstream: Option<NormalizedAutomationValue>,
    ) -> Result<HostGain, Error> {
        self.host_gain_at(self.program.input, offset, upstream)
    }

    pub(crate) fn host_output_gain_at(
        &self,
        offset: usize,
        upstream: Option<NormalizedAutomationValue>,
    ) -> Result<HostGain, Error> {
        self.host_gain_at(self.program.output, offset, upstream)
    }

    fn host_gain_at(
        &self,
        target: Option<HostGainTarget>,
        offset: usize,
        upstream: Option<NormalizedAutomationValue>,
    ) -> Result<HostGain, Error> {
        if offset >= self.frames {
            return Err(Error::InvalidParameterValue);
        }
        let target = target.ok_or(Error::InvalidDescriptor)?;
        let base =
            upstream.map_or_else(|| target.base.normalized(), NormalizedAutomationValue::get);
        let sum = self.sum_routes(target.route_start..target.route_end, offset)?;
        let final_value = (f64::from(base) + sum).clamp(0.0, 1.0) as f32;
        HostGain::from_normalized(final_value)
    }

    pub(crate) fn compose_foreign_target(
        &self,
        normalized_base: f64,
        output: &mut [f64],
    ) -> Result<(), Error> {
        if output.len() != self.frames {
            return Err(Error::InvalidParameterValue);
        }
        let routes = 0..self.program.routes.len();
        for (offset, value) in output.iter_mut().enumerate() {
            let sum = self.sum_routes(routes.clone(), offset)?;
            *value = (normalized_base + sum).clamp(0.0, 1.0);
        }
        Ok(())
    }

    fn sum_routes(&self, routes: std::ops::Range<usize>, offset: usize) -> Result<f64, Error> {
        let routes = self
            .program
            .routes
            .get(routes)
            .ok_or(Error::InvalidDescriptor)?;
        let mut sum = 0.0;
        for route in routes {
            let sample = self
                .sources
                .get(route.source)
                .and_then(|source| source.get(offset))
                .ok_or(Error::InvalidParameterValue)?;
            sum += route.contribution.sample(*sample);
        }
        Ok(sum)
    }

    fn selected_value(&self, target: &Target, offset: usize) -> Option<ParameterValue> {
        let id = target.authored.descriptor.id;
        self.upstream.and_then(|input| {
            input
                .parameter_ids()
                .binary_search(&id)
                .ok()
                .and_then(|_| input.value_at(id, offset))
        })
    }

    fn sample(&self, index: usize, offset: usize) -> Result<ParameterValue, Error> {
        if offset >= self.frames {
            return Err(Error::InvalidParameterValue);
        }
        let target = self
            .program
            .targets
            .get(index)
            .ok_or(Error::UnknownParameter)?;
        let authored = target.authored;
        let selected = self.selected_value(target, offset);
        let base = selected.unwrap_or(authored.base);
        let sum = self.sum_routes(target.route_start..target.route_end, offset)?;
        if sum == 0.0 {
            authored.descriptor.validate(base)?;
            return Ok(base);
        }
        let normalized = if selected.is_some() {
            authored
                .semantics
                .normalization
                .normalize(authored.descriptor, base)?
        } else {
            target.normalized_base
        };
        let final_value = (f64::from(normalized) + sum).clamp(0.0, 1.0) as f32;
        target
            .mapping
            .value(final_value)?
            .provider()
            .ok_or(Error::InvalidDescriptor)
    }
}
impl NativeParameterAutomation for ModulationBlock<'_> {
    fn normalized_before_clamp(&self, id: ParameterId, offset: usize) -> Option<f64> {
        if offset >= self.frames {
            return None;
        }
        let index = self.program.ids.binary_search(&id).ok()?;
        let target = self.program.targets.get(index)?;
        let routes = target.route_start..target.route_end;
        if routes.is_empty() {
            return None;
        }
        let base = self.selected_value(target, offset).map_or(
            Some(target.normalized_base),
            |selected| {
                target
                    .authored
                    .semantics
                    .normalization
                    .normalize(target.authored.descriptor, selected)
                    .ok()
            },
        )?;
        Some(f64::from(base) + self.sum_routes(routes, offset).ok()?)
    }

    fn parameter_ids(&self) -> &[ParameterId] {
        &self.program.ids
    }

    fn has_sample_rate_modulation(&self, parameter: ParameterId) -> bool {
        self.program.has_routes(parameter)
    }

    fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
        self.program
            .ids
            .binary_search(&id)
            .ok()
            .and_then(|index| self.sample(index, offset).ok())
    }
    fn values_at(&self, offset: usize, output: &mut [Option<ParameterValue>]) -> Result<(), Error> {
        if output.len() != self.program.ids.len() || offset >= self.frames {
            return Err(Error::InvalidParameterValue);
        }
        for (index, value) in output.iter_mut().enumerate() {
            *value = Some(self.sample(index, offset)?);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::{
        ParameterAutomation, ParameterKind, ParameterModulation, ParameterNormalization,
        ParameterPresentation, ParameterSmoothing,
    };

    fn target(id: u32, base: f32) -> ModulationTarget {
        ModulationTarget {
            descriptor: ParameterDescriptor {
                id: ParameterId::from_static(id),
                label: "test",
                kind: ParameterKind::Float {
                    min: 0.0,
                    max: 10.0,
                    unit: None,
                },
                default: ParameterValue::Float(0.0),
                presentation: ParameterPresentation::Linear,
            },
            semantics: ParameterSemantics {
                access: ParameterAccess::ReadWrite,
                normalization: ParameterNormalization::Linear,
                stepping: ParameterStepping::Continuous,
                automation: ParameterAutomation::Unsupported,
                modulation: ParameterModulation::Unsupported,
                smoothing: ParameterSmoothing::None,
                change_impact: ParameterChangeImpact::ValueOnly,
            },
            base: ParameterValue::Float(base),
        }
    }
    #[test]
    fn maps_source_range_before_applying_negative_depth() {
        let target = target(1, 8.0);
        let id = target.descriptor.id;
        let routes = [ModulationRoute {
            source: 0,
            target: id,
            amount: -0.4,
            range: [0.0, 1.0],
        }];
        let program = PreparedModulation::new(&[target], &routes, 1).unwrap();
        let input = [-1.0, 0.0, 1.0];
        let samples: [&[f32]; 1] = [&input];
        let block = program.block(None, &samples, input.len()).unwrap();
        for (offset, expected) in [8.0, 6.0, 4.0].into_iter().enumerate() {
            let Some(ParameterValue::Float(value)) = block.value_at(id, offset) else {
                panic!("missing composed parameter");
            };
            assert!((value - expected).abs() < 1e-6);
        }
    }

    struct Upstream {
        ids: [ParameterId; 1],
    }
    impl NativeParameterAutomation for Upstream {
        fn parameter_ids(&self) -> &[ParameterId] {
            &self.ids
        }
        fn value_at(&self, _: ParameterId, offset: usize) -> Option<ParameterValue> {
            (offset == 1).then_some(ParameterValue::Float(8.0))
        }
    }
    #[test]
    fn many_routes_compose_after_automation_and_keep_base_unchanged() {
        let targets = [target(1, 5.0), target(2, 4.0)];
        let routes = [
            ModulationRoute {
                source: 0,
                target: targets[0].descriptor.id,
                amount: 0.5,
                range: [-1.0, 1.0],
            },
            ModulationRoute {
                source: 1,
                target: targets[0].descriptor.id,
                amount: -0.25,
                range: [-1.0, 1.0],
            },
            ModulationRoute {
                source: 0,
                target: targets[1].descriptor.id,
                amount: 0.25,
                range: [-1.0, 1.0],
            },
        ];
        let program = PreparedModulation::new(&targets, &routes, 2).unwrap();
        assert!(program.storage_bytes().unwrap() > 0);
        assert_eq!(
            program.parameter_ids(),
            &[targets[0].descriptor.id, targets[1].descriptor.id]
        );
        assert_eq!(
            program.descriptor(targets[0].descriptor.id),
            Some(targets[0].descriptor)
        );
        assert_eq!(program.descriptor(ParameterId::from_static(3)), None);
        assert_eq!(
            program.semantics(targets[0].descriptor.id),
            Some(targets[0].semantics)
        );
        assert_eq!(program.semantics(ParameterId::from_static(3)), None);
        assert_eq!(program.source_count(), 2);
        let a = [0.0, 1.0, -1.0];
        let b = [0.0, -1.0, 1.0];
        let samples = [&a[..], &b[..]];
        let upstream = Upstream {
            ids: [targets[0].descriptor.id],
        };
        let block = program.block(Some(&upstream), &samples, 3).unwrap();
        for (offset, expected) in [[5.0, 4.0], [10.0, 6.5], [0.0, 1.5]]
            .into_iter()
            .enumerate()
        {
            let mut output = [None; 2];
            block.values_at(offset, &mut output).unwrap();
            for (actual, expected) in output.into_iter().zip(expected) {
                let Some(ParameterValue::Float(actual)) = actual else {
                    panic!("expected scalar")
                };
                assert!((actual - expected).abs() < 1e-5);
            }
        }
        assert_eq!(targets[0].base, ParameterValue::Float(5.0));
        assert_eq!(block.value_at(targets[0].descriptor.id, 3), None);
    }
    #[test]
    fn structural_automation_preserves_its_values_beside_a_mapped_scalar() {
        struct StructuralLane([ParameterId; 1]);
        impl NativeParameterAutomation for StructuralLane {
            fn parameter_ids(&self) -> &[ParameterId] {
                &self.0
            }

            fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
                (id == self.0[0] && offset == 1).then_some(ParameterValue::Choice(2))
            }
        }

        let scalar = target(1, 5.0);
        let mut layout = target(2, 0.0);
        layout.descriptor.kind = ParameterKind::Choice {
            labels: &["short", "medium", "long"],
        };
        layout.descriptor.default = ParameterValue::Choice(1);
        layout.base = layout.descriptor.default;
        layout.semantics = ParameterSemantics {
            normalization: ParameterNormalization::Discrete,
            stepping: ParameterStepping::Choice,
            automation: ParameterAutomation::DeviceArrangementEnvelope,
            modulation: ParameterModulation::Unsupported,
            change_impact: ParameterChangeImpact::Structural,
            ..layout.semantics
        };
        let route = ModulationRoute {
            source: 0,
            target: scalar.descriptor.id,
            amount: 0.25,
            range: [-1.0, 1.0],
        };
        let program = PreparedModulation::new(&[scalar, layout], &[route], 1).unwrap();
        let upstream = StructuralLane([layout.descriptor.id]);
        let samples: [&[f32]; 1] = [&[-1.0, 1.0, 0.0]];
        let block = program.block(Some(&upstream), &samples, 3).unwrap();
        for (offset, (gain, choice)) in [(2.5, 1), (7.5, 2), (5.0, 1)].into_iter().enumerate() {
            let mut values = [None; 2];
            block.values_at(offset, &mut values).unwrap();
            assert_eq!(
                values,
                [
                    Some(ParameterValue::Float(gain)),
                    Some(ParameterValue::Choice(choice))
                ]
            );
        }
        assert!(!block.has_sample_rate_modulation(layout.descriptor.id));
        let invalid = ModulationRoute {
            target: layout.descriptor.id,
            ..route
        };
        assert!(PreparedModulation::new(&[scalar, layout], &[invalid], 1).is_err());
        layout.semantics.modulation = ParameterModulation::SampleRate;
        assert!(PreparedModulation::new(&[scalar, layout], &[route], 1).is_err());
    }

    #[test]
    fn invalid_program_and_sample_blocks_are_rejected() {
        let t = target(1, 5.0);
        assert!(PreparedModulation::new(&[t, t], &[], 0).is_err());
        let mut read_only = t;
        read_only.semantics.access = ParameterAccess::ReadOnly;
        assert!(PreparedModulation::new(&[read_only], &[], 0).is_err());
        let mut structural = t;
        structural.semantics.change_impact = ParameterChangeImpact::Structural;
        assert!(PreparedModulation::new(&[structural], &[], 0).is_err());
        let mut unspecified = t;
        unspecified.semantics.stepping = ParameterStepping::Unspecified;
        assert!(PreparedModulation::new(&[unspecified], &[], 0).is_err());
        assert!(
            PreparedModulation::new(
                &[t],
                &[ModulationRoute {
                    source: 0,
                    target: t.descriptor.id,
                    amount: f32::NAN,
                    range: [-1.0, 1.0],
                }],
                1
            )
            .is_err()
        );
        let program = PreparedModulation::new(&[t], &[], 1).unwrap();
        let invalid = [f32::NAN];
        assert!(program.block(None, &[&invalid], 1).is_err());
        assert!(program.block(None, &[&invalid], 2).is_err());
        assert!(program.block(None, &[], 0).is_err());
    }

    #[test]
    fn step_quantization_follows_sum_and_zero_restores_exact_base() {
        let mut t = target(1, 3.1);
        t.semantics.stepping = ParameterStepping::Increment(2.0);
        let route = ModulationRoute {
            source: 0,
            target: t.descriptor.id,
            amount: 0.2,
            range: [-1.0, 1.0],
        };
        let program = PreparedModulation::new(&[t], &[route], 1).unwrap();
        let samples = [0.0, 1.0, -1.0];
        let sources = [&samples[..]];
        let block = program.block(None, &sources, 3).unwrap();
        assert_eq!(block.value_at(t.descriptor.id, 0), Some(t.base));
        assert_eq!(
            block.value_at(t.descriptor.id, 1),
            Some(ParameterValue::Float(6.0))
        );
        assert_eq!(
            block.value_at(t.descriptor.id, 2),
            Some(ParameterValue::Float(2.0))
        );
    }

    #[test]
    fn removed_routes_preserve_selected_automation_and_base() {
        let t = target(1, 3.1);
        let program = PreparedModulation::new(&[t], &[], 0).unwrap();
        let upstream = Upstream {
            ids: [t.descriptor.id],
        };
        let block = program.block(Some(&upstream), &[], 2).unwrap();
        assert_eq!(block.value_at(t.descriptor.id, 0), Some(t.base));
        assert_eq!(
            block.value_at(t.descriptor.id, 1),
            Some(ParameterValue::Float(8.0))
        );
    }

    #[test]
    fn same_target_routes_keep_the_authored_summation_order() {
        let target = target(1, 0.0);
        let routes = [0, 1, 2].map(|source| ModulationRoute {
            source,
            target: target.descriptor.id,
            amount: 1.0,
            range: [-1.0, 1.0],
        });
        let program = PreparedModulation::new(&[target], &routes, 3).unwrap();
        let a = [1.0];
        let b = [1.0e-20];
        let c = [-1.0];
        let sources = [&a[..], &b[..], &c[..]];
        let block = program.block(None, &sources, 1).unwrap();
        assert_eq!(block.value_at(target.descriptor.id, 0), Some(target.base));
    }

    #[test]
    fn logarithmic_and_boolean_destinations_convert_after_summing() {
        let mut frequency = target(1, 10.0);
        frequency.descriptor.kind = ParameterKind::Float {
            min: 1.0,
            max: 100.0,
            unit: None,
        };
        frequency.descriptor.default = ParameterValue::Float(1.0);
        frequency.semantics.normalization = ParameterNormalization::PositiveLogarithmic;
        let mut toggle = target(2, 0.0);
        toggle.descriptor.kind = ParameterKind::Bool;
        toggle.descriptor.default = ParameterValue::Bool(false);
        toggle.base = ParameterValue::Bool(false);
        toggle.semantics.normalization = ParameterNormalization::Discrete;
        toggle.semantics.stepping = ParameterStepping::Boolean;
        let routes = [
            ModulationRoute {
                source: 0,
                target: frequency.descriptor.id,
                amount: 0.5,
                range: [-1.0, 1.0],
            },
            ModulationRoute {
                source: 0,
                target: toggle.descriptor.id,
                amount: 0.5,
                range: [-1.0, 1.0],
            },
        ];
        let program = PreparedModulation::new(&[frequency, toggle], &routes, 1).unwrap();
        let samples = [1.0, -1.0];
        let sources = [&samples[..]];
        let block = program.block(None, &sources, 2).unwrap();
        assert_eq!(
            block.value_at(frequency.descriptor.id, 0),
            Some(ParameterValue::Float(100.0))
        );
        assert_eq!(
            block.value_at(frequency.descriptor.id, 1),
            Some(ParameterValue::Float(1.0))
        );
        assert_eq!(
            block.value_at(toggle.descriptor.id, 0),
            Some(ParameterValue::Bool(true))
        );
        assert_eq!(
            block.value_at(toggle.descriptor.id, 1),
            Some(ParameterValue::Bool(false))
        );
    }
}
