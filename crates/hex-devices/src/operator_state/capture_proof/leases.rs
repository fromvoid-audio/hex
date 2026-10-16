//! Disconnected bounded lease model. Reclamation is an explicit control action.
use super::{
    adapter::Root,
    layout::{Handle, Key, PoolLayout, Receipt, Record, Slot, context_bytes},
};
use std::mem::size_of;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    Identity,
    Context,
    Clock,
    Open,
    Format,
    Values,
    Capacity,
    Generation,
    Retained,
    Stale,
    References,
}
/// Control-issued namespaces, separate from slot generations; no reuse or wrap.
pub(super) struct NamespaceIssuer {
    pub next: u64,
}
impl NamespaceIssuer {
    pub fn issue(&mut self) -> Result<u64, Refusal> {
        if self.next == 0 {
            return Err(Refusal::Identity);
        }
        let after = self.next.checked_add(1).ok_or(Refusal::Generation)?;
        let issued = self.next;
        self.next = after;
        Ok(issued)
    }
}
pub(super) struct Store {
    pub pool: PoolLayout,
    pub roots: Vec<Root>,
}
#[derive(Debug, PartialEq)]
pub(super) struct Fingerprint {
    slots: Vec<Slot>,
    current: Vec<Handle>,
    staging: Vec<Handle>,
    retained: usize,
    next: u64,
    live: u32,
    roots: Vec<Key>,
    namespace: u64,
    free_head: u32,
}
impl Store {
    pub fn new(namespace: u64, count: usize, roots: Vec<Root>) -> Self {
        assert!(namespace != 0 && count <= 1142);
        let mut slots = Vec::new();
        slots.try_reserve_exact(count).unwrap();
        slots.resize(count, Slot::default());
        let mut current = Vec::new();
        current.try_reserve_exact(256).unwrap();
        current.resize(256, Handle::default());
        let mut staging = Vec::new();
        staging.try_reserve_exact(256).unwrap();
        staging.resize(256, Handle::default());
        Self {
            pool: PoolLayout {
                slots,
                current,
                staging,
                free_head: 0,
                live: 0,
                next_generation: 1,
                retained_bytes: 0,
                namespace,
            },
            roots,
        }
    }
    pub fn root(&self, key: Key) -> Result<&Root, Refusal> {
        self.roots
            .iter()
            .find(|root| root.key == key)
            .ok_or(Refusal::Context)
    }
    pub fn validate(&self, record: Record, actual: Receipt, closed: bool) -> Result<(), Refusal> {
        if !closed {
            return Err(Refusal::Open);
        }
        if !record.receipt.source.valid()
            || !record.receipt.target.valid()
            || !record.enable.law.valid()
            || record.recipe == 0
        {
            return Err(Refusal::Identity);
        }
        if record.receipt.source != actual.source || record.receipt.target != actual.target {
            return Err(Refusal::Context);
        }
        if record.receipt.clock_generation == 0
            || record.receipt.clock_generation != actual.clock_generation
            || record.receipt.boundary != actual.boundary
        {
            return Err(Refusal::Clock);
        }
        let source = self.root(record.receipt.source)?;
        let target = self.root(record.receipt.target)?;
        let enabled = self.root(record.enable.law)?;
        if [source, target, enabled]
            .iter()
            .any(|root| root.context.declared_context_bytes() > 4096)
        {
            return Err(Refusal::Context);
        }
        if record.receipt.source_configuration != source.configuration()
            || record.receipt.target_configuration != target.configuration()
            || actual.source_configuration != source.configuration()
            || actual.target_configuration != target.configuration()
            || !source.compatible(target)
            || !source.compatible(enabled)
        {
            return Err(Refusal::Context);
        }
        if record.format != 1 || record.reserved != 0 {
            return Err(Refusal::Format);
        }
        if record.harmonic_clock.start != record.receipt.boundary
            || record.harmonic_clock.amount(actual.boundary) != Some(0.0)
            || enabled
                .levels(record.enable, actual.boundary)
                .map(|levels| levels.map(f64::to_bits))
                != Some(record.levels.map(f64::to_bits))
            || !record.bank.finite()
        {
            return Err(Refusal::Values);
        }
        Ok(())
    }
    pub fn stage(
        &mut self,
        record: Record,
        actual: Receipt,
        closed: bool,
    ) -> Result<Handle, Refusal> {
        self.validate(record, actual, closed)?;
        if record.receipt.target.namespace != self.pool.namespace {
            return Err(Refusal::Identity);
        }
        let next = self
            .pool
            .next_generation
            .checked_add(1)
            .ok_or(Refusal::Generation)?;
        let charge = size_of::<Slot>() + context_bytes();
        let retained = self
            .pool
            .retained_bytes
            .checked_add(charge)
            .ok_or(Refusal::Retained)?;
        if retained > 2 * 1024 * 1024 {
            return Err(Refusal::Retained);
        }
        let index = self
            .pool
            .slots
            .iter()
            .position(|slot| slot.metadata.flags == 0)
            .ok_or(Refusal::Capacity)?;
        let staging = self
            .pool
            .staging
            .iter()
            .position(|handle| handle.generation == 0)
            .ok_or(Refusal::Capacity)?;
        let handle = Handle {
            slot: index as u32,
            kind: 1,
            generation: self.pool.next_generation,
            namespace: self.pool.namespace,
        };
        let slot = &mut self.pool.slots[index];
        slot.record = record;
        slot.metadata.generation = handle.generation;
        slot.metadata.flags = 1;
        slot.metadata.staged = 1;
        slot.metadata.retained_charge = charge as u64;
        self.pool.staging[staging] = handle;
        self.pool.next_generation = next;
        self.pool.retained_bytes = retained;
        self.pool.live += 1;
        Ok(handle)
    }
    fn index(&self, handle: Handle) -> Result<usize, Refusal> {
        let index = handle.slot as usize;
        if handle.namespace != self.pool.namespace
            || handle.kind != 1
            || handle.generation == 0
            || self.pool.slots.get(index).is_none_or(|slot| {
                slot.metadata.generation != handle.generation || slot.metadata.flags != 1
            })
        {
            return Err(Refusal::Stale);
        }
        Ok(index)
    }
    pub fn record(&self, handle: Handle) -> Result<Record, Refusal> {
        Ok(self.pool.slots[self.index(handle)?].record)
    }
    pub fn publish(
        &mut self,
        handle: Handle,
        owner: usize,
        actual: Receipt,
        closed: bool,
    ) -> Result<(), Refusal> {
        let index = self.index(handle)?;
        if owner >= self.pool.current.len() {
            return Err(Refusal::Capacity);
        }
        self.validate(self.pool.slots[index].record, actual, closed)?;
        let staging = self
            .pool
            .staging
            .iter()
            .position(|h| *h == handle)
            .ok_or(Refusal::Stale)?;
        let refs = self.pool.slots[index]
            .metadata
            .committed
            .checked_add(1)
            .ok_or(Refusal::References)?;
        let old = self.pool.current[owner];
        if old.generation != 0 {
            let old_index = self.index(old)?;
            if self.pool.slots[old_index].metadata.committed == 0 {
                return Err(Refusal::References);
            }
        }
        if self.pool.slots[index].metadata.staged == 0 {
            return Err(Refusal::References);
        }
        self.pool.slots[index].metadata.committed = refs;
        self.pool.slots[index].metadata.staged -= 1;
        self.pool.staging[staging] = Handle::default();
        self.pool.current[owner] = handle;
        if old.generation != 0 {
            self.release(old)?;
        }
        Ok(())
    }
    pub fn retain_detached(&mut self, handle: Handle) -> Result<(), Refusal> {
        let i = self.index(handle)?;
        if self.pool.slots[i].metadata.committed == 0 {
            return Err(Refusal::References);
        }
        self.pool.slots[i].metadata.committed = self.pool.slots[i]
            .metadata
            .committed
            .checked_add(1)
            .ok_or(Refusal::References)?;
        Ok(())
    }
    pub fn release(&mut self, handle: Handle) -> Result<(), Refusal> {
        let i = self.index(handle)?;
        let slot = &mut self.pool.slots[i];
        slot.metadata.committed = slot
            .metadata
            .committed
            .checked_sub(1)
            .ok_or(Refusal::References)?;
        self.retire_if_unreferenced(i);
        Ok(())
    }
    pub fn clear_current(&mut self, owner: usize) -> Result<(), Refusal> {
        let handle = *self.pool.current.get(owner).ok_or(Refusal::Capacity)?;
        let index = self.index(handle)?;
        if self.pool.slots[index].metadata.committed == 0 {
            return Err(Refusal::References);
        }
        self.pool.current[owner] = Handle::default();
        self.release(handle)
    }
    fn retire_if_unreferenced(&mut self, index: usize) {
        let slot = &mut self.pool.slots[index];
        if slot.metadata.committed == 0 && slot.metadata.staged == 0 {
            slot.metadata.flags = 2;
        }
    }
    pub fn abort(&mut self, handle: Handle) -> Result<(), Refusal> {
        let i = self.index(handle)?;
        let staging = self
            .pool
            .staging
            .iter()
            .position(|h| *h == handle)
            .ok_or(Refusal::Stale)?;
        if self.pool.slots[i].metadata.staged == 0 {
            return Err(Refusal::References);
        }
        self.pool.staging[staging] = Handle::default();
        self.pool.slots[i].metadata.staged -= 1;
        self.retire_if_unreferenced(i);
        Ok(())
    }
    /// Test model of control reclamation. Retired bytes remain charged until here.
    pub fn reclaim_on_control(&mut self) {
        for slot in &mut self.pool.slots {
            if slot.metadata.flags == 2 {
                self.pool.retained_bytes -= slot.metadata.retained_charge as usize;
                self.pool.live -= 1;
                let generation = slot.metadata.generation;
                *slot = Slot::default();
                slot.metadata.generation = generation;
            }
        }
        self.roots.retain(|root| {
            self.pool.slots.iter().any(|slot| {
                slot.metadata.flags != 0
                    && [
                        slot.record.receipt.source,
                        slot.record.receipt.target,
                        slot.record.enable.law,
                    ]
                    .contains(&root.key)
            })
        });
    }
    pub fn owned_bytes(&self) -> usize {
        size_of::<Self>()
            + self.pool.slots.capacity() * size_of::<Slot>()
            + (self.pool.current.capacity() + self.pool.staging.capacity()) * size_of::<Handle>()
            + self.roots.capacity() * size_of::<Root>()
            + self
                .roots
                .iter()
                .map(|root| root.context.declared_context_bytes())
                .sum::<usize>()
    }
    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint {
            slots: self.pool.slots.clone(),
            current: self.pool.current.clone(),
            staging: self.pool.staging.clone(),
            retained: self.pool.retained_bytes,
            next: self.pool.next_generation,
            live: self.pool.live,
            roots: self.roots.iter().map(|root| root.key).collect(),
            namespace: self.pool.namespace,
            free_head: self.pool.free_head,
        }
    }
}
