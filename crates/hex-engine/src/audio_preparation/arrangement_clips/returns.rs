//! Private actual Return buses + feed plan in the existing preparation root.
use super::*;
pub use crate::return_feed_plan::persistent::ReturnFeedCapacity;
use crate::{ReturnBus, owned_shared::OwnedShared, return_feed_plan::persistent::FeedPlanCustody};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArrangementReturnCapacity {
    pub row_bytes: usize,
    pub chain_payload_bytes: usize,
    pub chain_declared_overhead_bytes: usize,
    pub meter_payload_bytes: usize,
    pub meter_declared_overhead_bytes: usize,
    pub reserved_handle_destinations: usize,
    pub independent_handle_obligations: usize,
    pub feed: ReturnFeedCapacity,
    pub previous_growth_backing_bytes: usize,
}
impl ArrangementReturnCapacity {
    pub(crate) fn bytes(self) -> Result<usize, AudioStorageError> {
        self.row_bytes
            .checked_add(self.chain_payload_bytes)
            .and_then(|n| n.checked_add(self.chain_declared_overhead_bytes))
            .and_then(|n| n.checked_add(self.meter_payload_bytes))
            .and_then(|n| n.checked_add(self.meter_declared_overhead_bytes))
            .and_then(|n| n.checked_add(self.feed.bytes().ok()?))
            .ok_or(AudioStorageError::Overflow)
    }
}
#[derive(Default)]
struct Row {
    id: Option<hex_project::ReturnId>,
    ordinal: usize,
    gain: f32,
    muted: bool,
    chain: Option<OwnedShared<[hex_lattice::PreparedRuntimeInstance]>>,
    bare: Option<Arc<[hex_lattice::PreparedRuntimeInstance]>>,
    declaration: Option<SharedDeclaration>,
    handle: u64,
    meter: Option<OwnedShared<hex_lattice::DeviceMeter>>,
    bare_meter: Option<Arc<hex_lattice::DeviceMeter>>,
    meter_declaration: Option<SharedDeclaration>,
    meter_handle: u64,
    bus: Option<ReturnBus>,
    root_index: Option<usize>,
    disposal: Disposal,
    effects: super::chains::ChainCustody,
}
#[derive(Default)]
pub(super) struct ReturnAssembly {
    rows: Vec<Row>,
    feed: FeedPlanCustody,
    active: usize,
    cursor: usize,
    phase: Phase,
    growth: usize,
    #[cfg(test)]
    pub(super) fail_after: Option<usize>,
    #[cfg(test)]
    pub(super) panic_after: Option<usize>,
    #[cfg(test)]
    pub(super) probe: Option<DropProbe>,
}
impl Row {
    fn actual<'a>(
        &'a self,
        root: &'a [ReturnBus],
    ) -> Result<Option<&'a ReturnBus>, AudioStorageError> {
        if let Some(index) = self.root_index {
            if self.bus.is_some() || self.ordinal != index {
                return Err(AudioStorageError::SourceMismatch);
            }
            let bus = root.get(index).ok_or(AudioStorageError::SourceMismatch)?;
            if Some(bus.id) != self.id {
                return Err(AudioStorageError::SourceMismatch);
            }
            Ok(Some(bus))
        } else {
            Ok(self.bus.as_ref())
        }
    }
}
impl ReturnAssembly {
    #[cfg(test)]
    pub(super) fn capacity(
        &self,
        growth: bool,
    ) -> Result<ArrangementReturnCapacity, AudioStorageError> {
        self.capacity_at(growth, &[], None)
    }
    pub(super) fn root_count(&self) -> usize {
        self.active
    }
    pub(super) fn preflight_root_move(&self) -> Result<(), AudioStorageError> {
        if self.phase != Phase::Installed
            || !self.feed.can_move_root()
            || self.rows[..self.active].iter().enumerate().any(|(i, r)| {
                r.root_index.is_some()
                    || r.ordinal != i
                    || r.bus.as_ref().is_none_or(|b| Some(b.id) != r.id)
            })
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok(())
    }
    pub(super) fn move_root_row(&mut self, index: usize, target: &mut Vec<ReturnBus>) {
        if let Some(bus) = self.rows[index].bus.take() {
            target.push(bus);
            self.rows[index].root_index = Some(index);
        }
    }
    pub(super) fn move_root_feed(&mut self, target: &mut crate::return_feed_plan::ReturnFeedPlan) {
        self.feed.move_root(target);
    }
    pub(super) fn preflight_root_restore(
        &self,
        root: &[ReturnBus],
        has_feed: bool,
    ) -> Result<(), AudioStorageError> {
        for (index, bus) in root.iter().enumerate() {
            let row = self
                .rows
                .get(index)
                .ok_or(AudioStorageError::SourceMismatch)?;
            if row.root_index != Some(index) || row.bus.is_some() || Some(bus.id) != row.id {
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        if self.rows.iter().filter(|r| r.root_index.is_some()).count() != root.len()
            || (has_feed && !self.feed.can_restore_root())
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        Ok(())
    }
    pub(super) fn restore_root(
        &mut self,
        root: &mut Vec<ReturnBus>,
        feed: &mut crate::return_feed_plan::ReturnFeedPlan,
        has_feed: bool,
    ) {
        if has_feed {
            self.feed.restore_root(feed);
        }
        while let Some(bus) = root.pop() {
            let row = &mut self.rows[root.len()];
            row.bus = Some(bus);
            row.root_index = None;
        }
    }
    pub(super) fn capacity_at(
        &self,
        growth: bool,
        root: &[ReturnBus],
        feed: Option<&crate::return_feed_plan::ReturnFeedPlan>,
    ) -> Result<ArrangementReturnCapacity, AudioStorageError> {
        let feed = self.feed.capacity_at(growth, feed)?;
        let mut c = ArrangementReturnCapacity {
            row_bytes: self
                .rows
                .capacity()
                .checked_mul(mem::size_of::<Row>())
                .ok_or(AudioStorageError::Overflow)?,
            reserved_handle_destinations: self
                .active
                .checked_mul(2)
                .ok_or(AudioStorageError::Overflow)?,
            previous_growth_backing_bytes: if growth && self.phase != Phase::Vacant {
                self.growth
                    .checked_add(feed.previous_growth_backing_bytes)
                    .ok_or(AudioStorageError::Overflow)?
            } else {
                0
            },
            feed,
            ..Default::default()
        };
        for r in &self.rows {
            let meter_parts = if let Some(bus) = r.actual(root)? {
                r.meter_declaration
                    .as_ref()
                    .and_then(|d| d.parts(&bus.output_meter))
            } else if let Some(meter) = &r.meter {
                meter.parts()
            } else {
                r.bare_meter
                    .as_ref()
                    .and_then(|meter| r.meter_declaration.as_ref().and_then(|d| d.parts(meter)))
            };
            if r.meter.is_some()
                || r.bare_meter.is_some()
                || r.root_index.is_some()
                || r.bus.is_some()
            {
                let (payload, overhead) =
                    meter_parts.ok_or(AudioStorageError::UnsupportedConstructor)?;
                c.meter_payload_bytes = c
                    .meter_payload_bytes
                    .checked_add(payload)
                    .ok_or(AudioStorageError::Overflow)?;
                c.meter_declared_overhead_bytes = c
                    .meter_declared_overhead_bytes
                    .checked_add(overhead)
                    .ok_or(AudioStorageError::Overflow)?;
                c.independent_handle_obligations = c
                    .independent_handle_obligations
                    .checked_add(1)
                    .ok_or(AudioStorageError::Overflow)?;
            }
            let nested = if let Some(bus) = r.actual(root)? {
                r.effects.bytes(&bus.device_chain)?
            } else if let Some(chain) = &r.chain {
                r.effects.bytes(chain)?
            } else if let Some(chain) = &r.bare {
                r.effects.bytes(chain)?
            } else {
                r.effects.metadata_bytes()?
            };
            c.chain_payload_bytes = c
                .chain_payload_bytes
                .checked_add(nested)
                .ok_or(AudioStorageError::Overflow)?;
            c.independent_handle_obligations = c
                .independent_handle_obligations
                .checked_add(r.effects.handles())
                .ok_or(AudioStorageError::Overflow)?;
            c.reserved_handle_destinations = c
                .reserved_handle_destinations
                .checked_add(r.effects.handles())
                .ok_or(AudioStorageError::Overflow)?;
            let parts = if let Some(bus) = r.actual(root)? {
                if r.bare.is_some() || r.chain.is_some() {
                    return Err(AudioStorageError::UnknownStorage);
                }
                r.declaration
                    .as_ref()
                    .and_then(|d| d.parts(&bus.device_chain))
            } else if let Some(chain) = &r.chain {
                chain.parts()
            } else {
                r.bare
                    .as_ref()
                    .and_then(|a| r.declaration.as_ref().and_then(|d| d.parts(a)))
            };
            if r.chain.is_some() || r.bare.is_some() || r.root_index.is_some() || r.bus.is_some() {
                let (p, o) = parts.ok_or(AudioStorageError::UnsupportedConstructor)?;
                c.chain_payload_bytes = c
                    .chain_payload_bytes
                    .checked_add(p)
                    .ok_or(AudioStorageError::Overflow)?;
                c.chain_declared_overhead_bytes = c
                    .chain_declared_overhead_bytes
                    .checked_add(o)
                    .ok_or(AudioStorageError::Overflow)?;
                c.independent_handle_obligations = c
                    .independent_handle_obligations
                    .checked_add(1)
                    .ok_or(AudioStorageError::Overflow)?;
            }
        }
        c.bytes()?;
        Ok(c)
    }
    pub(super) fn reserve_with_effects(
        &mut self,
        project: &hex_project::Project,
        effects: Option<&hex_lattice::NativeDeviceRegistry<'_>>,
        native_input: Option<crate::device_automation::NativeParameterInputTarget>,
    ) -> Result<(), AudioStorageError> {
        if self.phase != Phase::Vacant
            || self.rows.iter().any(|r| {
                r.id.is_some()
                    || r.chain.is_some()
                    || r.bare.is_some()
                    || r.bus.is_some()
                    || r.declaration.is_some()
                    || r.handle != 0
                    || r.meter.is_some()
                    || r.bare_meter.is_some()
                    || r.meter_declaration.is_some()
                    || r.meter_handle != 0
            })
        {
            return Err(AudioStorageError::Capacity);
        }
        if project.return_tracks().len() > hex_project::MAX_RETURN_TRACKS
            || project
                .return_tracks()
                .iter()
                .any(|b| effects.is_none() && !b.devices().is_empty())
        {
            return Err(AudioStorageError::UnsupportedGraph);
        }
        for b in project.return_tracks() {
            if !b.gain().is_finite() || !(0.0..=2.0).contains(&b.gain()) {
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        self.phase = Phase::Reserved;
        self.active = 0;
        self.growth = 0;
        self.feed.reserve(project)?;
        let count = project.return_tracks().len();
        if count > self.rows.capacity() {
            self.growth = self
                .rows
                .capacity()
                .checked_mul(mem::size_of::<Row>())
                .ok_or(AudioStorageError::Overflow)?;
        }
        self.rows
            .try_reserve_exact(count.saturating_sub(self.rows.len()))
            .map_err(|_| AudioStorageError::Allocation)?;
        while self.rows.len() < count {
            self.rows.push(Row::default());
        }
        self.active = count;
        for (index, (r, b)) in self
            .rows
            .iter_mut()
            .zip(project.return_tracks())
            .enumerate()
        {
            r.id = Some(b.id());
            r.ordinal = index;
            r.gain = b.gain();
            r.muted = b.muted();
            r.disposal = Disposal::Unstarted;
        }
        // Every active destination has a cleanup state before even scalar
        // handle exhaustion can interrupt acquisition. No owner is created yet.
        for r in &mut self.rows[..self.active] {
            r.handle = handle()?;
            r.meter_handle = handle()?;
        }
        for (r, b) in self.rows[..self.active]
            .iter_mut()
            .zip(project.return_tracks())
        {
            r.meter = Some(OwnedShared::new_owned(hex_lattice::DeviceMeter::default()));
            let meter = r.meter.as_ref().ok_or(AudioStorageError::UnknownStorage)?;
            let (payload, _) = meter
                .parts()
                .ok_or(AudioStorageError::UnsupportedConstructor)?;
            if meter.control_storage_declaration()
                != (hex_lattice::PreparedStorageDeclaration::ExclusiveOnly { bytes: payload })
            {
                return Err(AudioStorageError::UnknownStorage);
            }
            r.chain = Some(r.effects.prepare_with_effects_and_input(
                b.devices(),
                hex_lattice::ProcessorChainOwner::Return(b.id().value()),
                project.sample_rate(),
                effects,
                native_input,
                Some(project),
            )?);
            self.growth = self
                .growth
                .checked_add(r.effects.construction_overlap())
                .ok_or(AudioStorageError::Overflow)?;
            r.chain
                .as_ref()
                .and_then(OwnedShared::parts)
                .ok_or(AudioStorageError::UnsupportedConstructor)?;
        }
        self.feed.prepare(project)?;
        Ok(())
    }
    pub(super) fn construct(&mut self) -> Result<(), AudioStorageError> {
        if self.phase != Phase::Reserved || !self.feed.can_install() {
            return Err(AudioStorageError::Capacity);
        }
        for (index, r) in self.rows[..self.active].iter().enumerate() {
            if r.ordinal != index
                || r.id.is_none()
                || r.handle == 0
                || r.meter_handle == 0
                || r.meter.is_none()
                || r.bare_meter.is_some()
                || r.meter_declaration.is_some()
                || r.chain.is_none()
                || r.bare.is_some()
                || r.declaration.is_some()
                || r.bus.is_some()
                || r.disposal != Disposal::Unstarted
            {
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        for r in &mut self.rows[..self.active] {
            if let Some(meter) = r.meter.take() {
                let (arc, declaration) = meter.into_paired_parts_on_control();
                r.meter_declaration = Some(declaration);
                r.bare_meter = Some(arc);
            }
            if let Some(chain) = r.chain.take() {
                let (arc, declaration) = chain.into_paired_parts_on_control();
                r.declaration = Some(declaration);
                r.bare = Some(arc);
            }
            if let (Some(id), Some(device_chain), Some(output_meter)) =
                (r.id, r.bare.take(), r.bare_meter.take())
            {
                r.bus = Some(ReturnBus {
                    id,
                    gain: r.gain,
                    muted: r.muted,
                    matrix: None,
                    device_chain,
                    output_meter,
                });
            }
            #[cfg(test)]
            {
                assert_ne!(
                    self.panic_after,
                    Some(r.ordinal),
                    "persistent Return construction panic"
                );
                if self.fail_after == Some(r.ordinal) {
                    return Err(AudioStorageError::Allocation);
                }
            }
        }
        self.feed.install();
        self.phase = Phase::Installed;
        #[cfg(test)]
        {
            assert_ne!(
                self.panic_after,
                Some(self.active),
                "persistent installed feed panic"
            );
            if self.fail_after == Some(self.active) {
                return Err(AudioStorageError::Allocation);
            }
        }
        Ok(())
    }
    pub(super) fn has_next(&self) -> bool {
        !matches!(self.phase, Phase::Vacant | Phase::Complete)
    }
    pub(super) fn dispose_next(&mut self) -> Result<(), AudioStorageError> {
        self.phase = Phase::Cleaning;
        if self.cursor == self.active {
            self.feed.cleanup_on_control()?;
            self.phase = Phase::Complete;
            return Ok(());
        }
        let r = &mut self.rows[self.cursor];
        if let Some(b) = &r.bus {
            if r.bare.is_some()
                || r.chain.is_some()
                || r.meter.is_some()
                || r.bare_meter.is_some()
                || r.meter_declaration.is_none()
                || b.id != r.id.ok_or(AudioStorageError::SourceMismatch)?
                || b.gain != r.gain
                || b.muted != r.muted
                || b.matrix.is_some()
                || r.declaration.is_none()
                || r.disposal != Disposal::Unstarted
            {
                return Err(AudioStorageError::UnknownStorage);
            }
            if let Some(ReturnBus {
                id: _,
                gain: _,
                muted: _,
                matrix: _,
                device_chain,
                output_meter,
            }) = r.bus.take()
            {
                r.bare = Some(device_chain);
                r.bare_meter = Some(output_meter);
            }
        }
        if r.disposal != Disposal::Unstarted {
            return Err(AudioStorageError::UnknownStorage);
        }
        r.disposal = Disposal::Pending;
        drop(r.chain.take());
        drop(r.bare.take());
        drop(r.meter.take());
        drop(r.bare_meter.take());
        #[cfg(test)]
        drop(self.probe.take());
        r.disposal = Disposal::Complete;
        r.effects.clear();
        r.handle = 0;
        r.meter_handle = 0;
        r.id = None;
        r.declaration = None;
        r.meter_declaration = None;
        self.cursor += 1;
        Ok(())
    }
    pub(super) fn clear_disposed(&mut self) -> Result<(), AudioStorageError> {
        if self.phase == Phase::Vacant {
            return Ok(());
        }
        if self.phase != Phase::Complete
            || self.rows.iter().any(|r| {
                r.id.is_some()
                    || r.bus.is_some()
                    || r.chain.is_some()
                    || r.bare.is_some()
                    || r.handle != 0
                    || r.meter.is_some()
                    || r.bare_meter.is_some()
                    || r.meter_handle != 0
            })
        {
            return Err(AudioStorageError::Capacity);
        }
        self.feed.clear_disposed()?;
        self.active = 0;
        self.cursor = 0;
        self.phase = Phase::Vacant;
        Ok(())
    }
    pub(super) fn metadata_at<'a>(
        &'a self,
        root: &'a [ReturnBus],
        feed: Option<&'a crate::return_feed_plan::ReturnFeedPlan>,
    ) -> Option<ArrangementReturnMetadata<'a>> {
        if self.phase != Phase::Installed || (!self.feed.installed() && feed.is_none()) {
            return None;
        }
        Some(ArrangementReturnMetadata {
            owner: self,
            root,
            feed,
        })
    }
}
/// Borrowed scalar metadata only, never a ReturnBus, plan, Arc or mutable graph.
/// ```compile_fail
/// fn escape(v: hex_engine::ArrangementReturnMetadata<'_>) { let _ = v.device_chain(); }
/// ```
pub struct ArrangementReturnMetadata<'a> {
    owner: &'a ReturnAssembly,
    root: &'a [ReturnBus],
    feed: Option<&'a crate::return_feed_plan::ReturnFeedPlan>,
}
impl ArrangementReturnMetadata<'_> {
    pub fn buses(&self) -> impl Iterator<Item = (hex_project::ReturnId, f32, bool)> + '_ {
        self.owner.rows[..self.owner.active].iter().filter_map(|r| {
            r.actual(self.root)
                .ok()
                .flatten()
                .map(|b| (b.id, b.gain, b.muted))
        })
    }
    pub fn order(&self) -> impl Iterator<Item = usize> + '_ {
        self.owner.feed.order().chain(
            self.feed
                .into_iter()
                .flat_map(|p| p.order().iter().copied()),
        )
    }
    pub fn feeds(&self) -> impl Iterator<Item = (usize, usize, u64, f32)> + '_ {
        self.owner
            .feed
            .feeds()
            .chain(self.feed.into_iter().flat_map(|p| p.edges()))
    }
}
#[cfg(test)]
#[path = "return_tests.rs"]
mod tests;

impl ReturnAssembly {
    pub(super) fn visit_audio_storage<'a>(
        &'a self,
        l: &mut crate::AudioRootStorageLedger<'a>,
        root: &'a [ReturnBus],
        external_feed: Option<&'a crate::return_feed_plan::ReturnFeedPlan>,
    ) -> Result<(), AudioStorageError> {
        use crate::audio_storage::Owner;
        self.capacity_at(false, root, external_feed)?;
        let Self {
            rows,
            feed,
            active: _,
            cursor: _,
            phase: _,
            growth,
            #[cfg(test)]
                fail_after: _,
            #[cfg(test)]
                panic_after: _,
            #[cfg(test)]
                probe: _,
        } = self;
        l.vec(rows)?;
        l.growth(*growth)?;
        feed.visit_audio_storage(l, external_feed)?;
        for r in rows {
            let Row {
                id: _,
                ordinal: _,
                gain: _,
                muted: _,
                chain,
                bare,
                declaration,
                handle,
                meter,
                bare_meter,
                meter_declaration,
                meter_handle,
                bus: _,
                root_index: _,
                disposal: _,
                effects,
            } = r;
            effects.visit_metadata(l)?;
            if let Some(bus) = r.actual(root)? {
                let ReturnBus {
                    id: _,
                    gain: _,
                    muted: _,
                    matrix: _,
                    device_chain,
                    output_meter,
                } = bus;
                l.shared(
                    output_meter,
                    meter_declaration
                        .as_ref()
                        .and_then(|d| d.parts(output_meter)),
                    Owner::exclusive(output_meter),
                    *meter_handle,
                    None,
                )?;
                effects.visit(device_chain, l)?;
                l.shared(
                    device_chain,
                    declaration.as_ref().and_then(|d| d.parts(device_chain)),
                    Owner::exclusive(device_chain),
                    *handle,
                    None,
                )?;
            } else {
                if let Some(owner) = meter {
                    l.shared(
                        owner.arc(),
                        owner.parts(),
                        Owner::exclusive(owner),
                        *meter_handle,
                        None,
                    )?;
                }
                if let Some(owner) = bare_meter {
                    l.shared(
                        owner,
                        meter_declaration.as_ref().and_then(|d| d.parts(owner)),
                        Owner::exclusive(owner),
                        *meter_handle,
                        None,
                    )?;
                }
                if let Some(owner) = chain {
                    effects.visit(owner.arc(), l)?;
                    l.shared(
                        owner.arc(),
                        owner.parts(),
                        Owner::exclusive(owner),
                        *handle,
                        None,
                    )?;
                }
                if let Some(owner) = bare {
                    effects.visit(owner, l)?;
                    l.shared(
                        owner,
                        declaration.as_ref().and_then(|d| d.parts(owner)),
                        Owner::exclusive(owner),
                        *handle,
                        None,
                    )?;
                }
            }
        }
        Ok(())
    }
}
