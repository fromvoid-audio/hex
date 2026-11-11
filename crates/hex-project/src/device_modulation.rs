//! Authored device-local modules and routes, independent of playback state.
//!
//! Module payloads use the existing versioned native-state envelope. The source
//! registry validates their algorithms, outputs and parameter capabilities during
//! prepare-before-commit; this owner validates identity, finite routing values,
//! aggregate bounds and an acyclic enabled source-dependency graph. Effective
//! values and oscillator phase never enter a project snapshot or history edge.
//! New source and route IDs reserve one contiguous range from the project cursor;
//! history never rewinds it. An existing slot keeps its source type. Copies
//! retain local IDs under a fresh DeviceId.
use crate::{Created, DeviceId, ElementColor, Invalid, Project};
use hex_lattice_contract::{DeviceAutomationTarget, DeviceTypeId, NativeDeviceState, ParameterId};
pub use hex_lattice_contract::{ModulationOutputId, ModulationSourceId};

pub const MAX_MODULATORS_PER_DEVICE: usize = 32;
pub const MAX_MODULATION_ROUTES_PER_DEVICE: usize = 256;
pub const MAX_PROJECT_MODULATORS: usize = 1_024;
pub const MAX_PROJECT_MODULATION_ROUTES: usize = 8_192;
pub const MAX_PROJECT_MODULATOR_STATE_BYTES: usize = 8 * 1024 * 1024;

/// Stable route identity scoped by the owning device, independent of order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModulationRouteId(u64);
impl ModulationRouteId {
    pub fn new(value: u64) -> Result<Self, Invalid> {
        if value == 0 {
            return Err(Invalid("modulation route identity must be nonzero"));
        }
        Ok(Self(value))
    }
    pub fn value(self) -> u64 {
        self.0
    }
}

/// A finite signed normalized offset. Negative amounts reverse modulation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModulationAmount(u32);
impl ModulationAmount {
    pub fn new(value: f32) -> Result<Self, Invalid> {
        if !value.is_finite() || !(-1.0..=1.0).contains(&value) {
            return Err(Invalid("modulation amount must be finite and within -1..1"));
        }
        Ok(Self(if value == 0.0 { 0 } else { value.to_bits() }))
    }
    pub fn get(self) -> f32 {
        f32::from_bits(self.0)
    }
}

/// Input range maps a source's -1..1 output linearly onto these endpoints.
/// Signed amount is applied afterwards, followed by the host's final clamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModulationRange {
    pub start: ModulationAmount,
    pub end: ModulationAmount,
}
impl Default for ModulationRange {
    fn default() -> Self {
        Self {
            start: ModulationAmount((-1.0_f32).to_bits()),
            end: ModulationAmount(1.0_f32.to_bits()),
        }
    }
}

/// A slot in its owning device’s authored bank. Routes follow the stable ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Modulator {
    pub id: ModulationSourceId,
    pub color: ElementColor,
    pub enabled: bool,
    pub state: NativeDeviceState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModulationDestination {
    Device {
        device_type: DeviceTypeId,
        target: DeviceAutomationTarget,
        vst3_class: Option<[u8; 16]>,
    },
    Modulator {
        source: ModulationSourceId,
        module_type: DeviceTypeId,
        parameter: ParameterId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceModulationRoute {
    pub id: ModulationRouteId,
    pub source: ModulationSourceId,
    pub output: ModulationOutputId,
    pub destination: ModulationDestination,
    pub amount: ModulationAmount,
    pub range: ModulationRange,
    pub enabled: bool,
}

/// Validated authored order; reordering never changes source or route identity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceModulation {
    sources: Vec<Modulator>,
    routes: Vec<DeviceModulationRoute>,
}
impl DeviceModulation {
    pub fn new(
        sources: Vec<Modulator>,
        routes: Vec<DeviceModulationRoute>,
    ) -> Result<Self, Invalid> {
        if sources.len() > MAX_MODULATORS_PER_DEVICE
            || sources.capacity() > MAX_MODULATORS_PER_DEVICE
            || routes.capacity() > MAX_MODULATION_ROUTES_PER_DEVICE
            || routes.len() > MAX_MODULATION_ROUTES_PER_DEVICE
        {
            return Err(Invalid("device modulation capacity reached"));
        }
        for (index, source) in sources.iter().enumerate() {
            if !source.state.asset_references().is_empty() {
                return Err(Invalid(
                    "asset-backed modulators are not supported by this build; keep the original project and use a compatible build",
                ));
            }
            if sources[..index].iter().any(|old| old.id == source.id) {
                return Err(Invalid("duplicate modulator identity"));
            }
        }
        for (index, route) in routes.iter().enumerate() {
            if routes[..index].iter().any(|old| old.id == route.id) {
                return Err(Invalid("duplicate modulation route identity"));
            }
            if !sources.iter().any(|source| source.id == route.source) {
                return Err(Invalid("modulation route source is missing"));
            }
            match route.destination {
                ModulationDestination::Device { target, .. } => target
                    .validate()
                    .map_err(|_| Invalid("invalid modulation destination"))?,
                ModulationDestination::Modulator {
                    source,
                    module_type,
                    parameter,
                } => {
                    parameter
                        .validate()
                        .map_err(|_| Invalid("invalid modulator parameter identity"))?;
                    if !sources.iter().any(|candidate| {
                        candidate.id == source && candidate.state.type_id() == module_type
                    }) {
                        return Err(Invalid("modulation destination source is missing"));
                    }
                }
            }
        }
        // Stable Kahn traversal, bounded by 32 nodes and 256 edges. Disabled
        // routes are retained but must pass this check when enabled by an edit.
        let mut visited = [false; MAX_MODULATORS_PER_DEVICE];
        for _ in 0..sources.len() {
            let next = sources.iter().enumerate().position(|(index, source)| {
                !visited[index]
                    && !routes.iter().any(|route| {
                        route.enabled
                            && matches!(route.destination,
                                ModulationDestination::Modulator { source: target, .. }
                                    if target == source.id)
                            && sources.iter().enumerate().any(|(upstream, candidate)| {
                                candidate.id == route.source && !visited[upstream]
                            })
                    })
            });
            let Some(next) = next else {
                return Err(Invalid("modulation routes form a cycle"));
            };
            visited[next] = true;
        }
        Ok(Self { sources, routes })
    }
    fn next_identity(&self, previous: &Self, mut next: u64) -> Result<u64, Invalid> {
        let mut added = [0u64; MAX_MODULATORS_PER_DEVICE + MAX_MODULATION_ROUTES_PER_DEVICE];
        let mut count = 0usize;
        for id in self
            .sources
            .iter()
            .filter(|source| !previous.sources.iter().any(|old| old.id == source.id))
            .map(|source| source.id.value())
            .chain(
                self.routes
                    .iter()
                    .filter(|route| !previous.routes.iter().any(|old| old.id == route.id))
                    .map(|route| route.id.value()),
            )
        {
            *added
                .get_mut(count)
                .ok_or(Invalid("modulation identity capacity reached"))? = id;
            count += 1;
        }
        let added = added
            .get_mut(..count)
            .ok_or(Invalid("modulation identity capacity reached"))?;
        added.sort_unstable();
        for &id in added.iter() {
            if id != next {
                return Err(Invalid(
                    "new modulation identities are stale; retry from the current project",
                ));
            }
            next = next
                .checked_add(1)
                .ok_or(Invalid("project identity capacity reached"))?;
        }
        Ok(next)
    }

    /// Copy one slot and its incident assignments into a revision-bound draft.
    /// The copy starts disabled. Inserting the draft with the existing bank
    /// command validates the project cursor and commits one history entry.
    pub fn duplicate_source(
        &self,
        source: ModulationSourceId,
        next_identity: u64,
    ) -> Result<Self, Invalid> {
        let index = self
            .sources
            .iter()
            .position(|slot| slot.id == source)
            .ok_or(Invalid("modulator no longer exists"))?;
        let related = |route: &&DeviceModulationRoute| {
            route.source == source
                || matches!(
                    route.destination,
                    ModulationDestination::Modulator { source: target, .. } if target == source
                )
        };
        let added = self.routes.iter().filter(related).count();
        let count = self.routes.len() + added;
        if self.sources.len() >= MAX_MODULATORS_PER_DEVICE
            || count > MAX_MODULATION_ROUTES_PER_DEVICE
        {
            return Err(Invalid("device modulation capacity reached"));
        }
        next_identity
            .checked_add(added as u64 + 1)
            .ok_or(Invalid("project identity capacity reached"))?;
        let copy = ModulationSourceId::new(next_identity)
            .map_err(|_| Invalid("invalid modulator identity"))?;
        let mut slot = self.sources[index].clone();
        slot.id = copy;
        slot.enabled = false;
        let mut sources = Vec::with_capacity(self.sources.len() + 1);
        sources.extend_from_slice(&self.sources[..=index]);
        sources.push(slot);
        sources.extend_from_slice(&self.sources[index + 1..]);

        let mut routes = Vec::with_capacity(count);
        routes.extend_from_slice(&self.routes);
        for (index, route) in self.routes.iter().filter(related).enumerate() {
            let mut route = *route;
            route.id = ModulationRouteId::new(next_identity + index as u64 + 1)?;
            if route.source == source {
                route.source = copy;
            }
            if let ModulationDestination::Modulator { source: target, .. } = &mut route.destination
                && *target == source
            {
                *target = copy;
            }
            routes.push(route);
        }
        Self::new(sources, routes)
    }

    pub fn sources(&self) -> &[Modulator] {
        &self.sources
    }
    pub fn routes(&self) -> &[DeviceModulationRoute] {
        &self.routes
    }
    /// Sources that can already feed this slot through enabled routes. The slot
    /// itself is included so a mapping picker can exclude self and cycle edges.
    pub fn source_route_ancestors(
        &self,
        source: ModulationSourceId,
    ) -> impl Iterator<Item = ModulationSourceId> {
        let mut reached = [None; MAX_MODULATORS_PER_DEVICE];
        if self.sources.iter().any(|slot| slot.id == source) {
            reached[0] = Some(source);
        }
        let mut count = usize::from(reached[0].is_some());
        for index in 0..self.sources.len() {
            let Some(target) = reached[index] else { break };
            for route in self.routes.iter().filter(|route| route.enabled) {
                if matches!(route.destination, ModulationDestination::Modulator { source, .. } if source == target)
                    && !reached[..count].contains(&Some(route.source))
                    && let Some(slot) = reached.get_mut(count)
                {
                    *slot = Some(route.source);
                    count += 1;
                }
            }
        }
        reached.into_iter().flatten()
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty() && self.routes.is_empty()
    }
}

impl Project {
    pub(crate) fn replace_device_modulation(
        &mut self,
        device: DeviceId,
        modulation: DeviceModulation,
    ) -> Result<Created, Invalid> {
        let previous = self
            .device(device)
            .ok_or(Invalid("device no longer exists"))?;
        let previous = previous.modulation();
        for slot in modulation.sources() {
            if previous
                .sources()
                .iter()
                .any(|old| old.id == slot.id && old.state.type_id() != slot.state.type_id())
            {
                return Err(Invalid("modulator type changed for an existing slot"));
            }
        }
        let next = modulation.next_identity(previous, self.next_id)?;
        self.validate_modulation_inventory(Some((device, &modulation)), next)?;
        self.edit_device(device, move |entry| {
            entry.modulation = modulation;
            Ok(())
        })?;
        self.next_id = next;
        Ok(Created::None)
    }

    pub(crate) fn validate_device_modulation(&self) -> Result<(), Invalid> {
        self.validate_modulation_inventory(None, self.next_id)
    }

    fn validate_modulation_inventory(
        &self,
        replacement: Option<(DeviceId, &DeviceModulation)>,
        next_id: u64,
    ) -> Result<(), Invalid> {
        let mut sources = 0usize;
        let mut routes = 0usize;
        let mut bytes = 0usize;
        for device in self.devices() {
            let modulation = replacement
                .filter(|(id, _)| *id == device.id())
                .map_or(&device.modulation, |(_, value)| value);
            if modulation
                .sources()
                .iter()
                .any(|source| source.id.value() >= next_id)
                || modulation
                    .routes()
                    .iter()
                    .any(|route| route.id.value() >= next_id)
            {
                return Err(Invalid(
                    "next project identity must exceed modulation identities",
                ));
            }
            for route in modulation.routes() {
                if let ModulationDestination::Device {
                    device_type,
                    target,
                    vst3_class,
                } = route.destination
                    && ((!target.is_host() && device.state().type_id() != device_type)
                        || device.automation_target_class(target)? != vst3_class)
                {
                    return Err(Invalid(
                        "modulation destination provider changed; remove or reassign its routes",
                    ));
                }
            }
            sources = sources
                .checked_add(modulation.sources.len())
                .ok_or(Invalid("modulator count overflow"))?;
            routes = routes
                .checked_add(modulation.routes.len())
                .ok_or(Invalid("modulation route count overflow"))?;
            for source in &modulation.sources {
                bytes = bytes
                    .checked_add(
                        source
                            .state
                            .retained_control_metadata_bytes()
                            .ok_or(Invalid("modulator state size overflow"))?,
                    )
                    .ok_or(Invalid("modulator state size overflow"))?;
            }
            if sources > MAX_PROJECT_MODULATORS
                || routes > MAX_PROJECT_MODULATION_ROUTES
                || bytes > MAX_PROJECT_MODULATOR_STATE_BYTES
            {
                return Err(Invalid("project modulation capacity reached"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
