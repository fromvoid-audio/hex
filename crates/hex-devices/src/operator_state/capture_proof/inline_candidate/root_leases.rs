//! Explicit three-root host model. Its NEW table is fully charged, never hidden.
use super::super::{
    adapter::Root,
    layout::{Handle, Key, Receipt, Record},
};
use super::{enable::Binding, root_refs::*};
use std::{mem::size_of, sync::Arc};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    Format,
    Root,
    Owner,
    Clock,
    Stale,
    Overflow,
    Budget,
    References,
}
#[repr(C)]
pub(super) struct Entry {
    pub root: Arc<Root>,
    pub counts: [u32; 4],
    pub location: RootRef,
}
#[repr(C)]
pub(super) struct Catalog {
    pub entries: Vec<Entry>,
    pub epoch: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Plan {
    pub epoch: u64,
    pub locations: [RootRef; 3],
    pub keys: [Key; 3],
}
impl Catalog {
    pub fn resolve(&self, location: RootRef, key: Key) -> Result<&Arc<Root>, Error> {
        if !location.usable() {
            return Err(Error::Root);
        }
        let mut matches = self.entries.iter().filter(|e| e.location == location);
        let entry = matches.next().ok_or(Error::Root)?;
        if matches.next().is_some() || entry.root.key != key {
            return Err(Error::Owner);
        }
        Ok(&entry.root)
    }
    pub fn find(&self, key: Key) -> Result<RootRef, Error> {
        let mut matches = self.entries.iter().filter(|e| e.root.key == key);
        let e = matches.next().ok_or(Error::Root)?;
        for alias in matches {
            if !Arc::ptr_eq(&e.root, &alias.root) {
                return Err(Error::Owner);
            }
        }
        if !e.location.usable() {
            return Err(Error::Root);
        }
        Ok(e.location)
    }
    pub fn preflight(
        &self,
        record: Record,
        cache: CachedRoots,
        actual: Receipt,
        format: u32,
    ) -> Result<Plan, Error> {
        if format != 1 || record.format != 1 || record.reserved != 0 {
            return Err(Error::Format);
        }
        if record.receipt != actual || actual.clock_generation == 0 {
            return Err(Error::Clock);
        }
        let keys = [
            record.receipt.source,
            record.receipt.target,
            record.enable.law,
        ];
        if keys.iter().any(|key| !key.valid()) {
            return Err(Error::Owner);
        }
        let locations = [self.find(keys[0])?, cache.target, cache.enable];
        let source = self.resolve(locations[0], keys[0])?;
        let target = self.resolve(locations[1], keys[1])?;
        let enable = self.resolve(locations[2], keys[2])?;
        if source.configuration() != actual.source_configuration
            || target.configuration() != actual.target_configuration
            || !source.compatible(target)
            || !source.compatible(enable)
            || [source, target, enable]
                .iter()
                .any(|r| r.context.declared_context_bytes() > 4096)
        {
            return Err(Error::Owner);
        }
        if Binding::validated(record.enable, enable, actual.boundary).is_none()
            || record.harmonic_clock.start != actual.boundary
            || record.harmonic_clock.amount(actual.boundary) != Some(0.0)
            || enable
                .levels(record.enable, actual.boundary)
                .map(|v| v.map(f64::to_bits))
                != Some(record.levels.map(f64::to_bits))
            || !record.bank.finite()
        {
            return Err(Error::Clock);
        }
        Ok(Plan {
            epoch: self.epoch,
            locations,
            keys,
        })
    }
    fn indices(&self, plan: Plan) -> Result<Vec<usize>, Error> {
        if plan.epoch != self.epoch {
            return Err(Error::Stale);
        }
        let mut unique = Vec::new();
        for (location, key) in plan.locations.into_iter().zip(plan.keys) {
            let owner = self.resolve(location, key)?;
            // Dedup ACTUAL owner, not state bytes or numeric key alone.
            let i = self
                .entries
                .iter()
                .position(|e| Arc::ptr_eq(&e.root, owner))
                .ok_or(Error::Root)?;
            if !unique.contains(&i) {
                unique.push(i);
            }
        }
        Ok(unique)
    }
    pub fn acquire(&mut self, plan: Plan, role: usize) -> Result<(), Error> {
        if role >= 4 {
            return Err(Error::References);
        }
        let indices = self.indices(plan)?;
        let next = self.epoch.checked_add(1).ok_or(Error::Overflow)?;
        for i in &indices {
            self.entries[*i].counts[role]
                .checked_add(1)
                .ok_or(Error::Overflow)?;
        }
        if self.context_bytes_after(&indices)? > 2 * 1024 * 1024 {
            return Err(Error::Budget);
        }
        for i in indices {
            self.entries[i].counts[role] += 1;
        }
        self.epoch = next;
        Ok(())
    }
    pub fn transfer(&mut self, keys: [Key; 3], from: usize, to: usize) -> Result<(), Error> {
        if from >= 4 || to >= 4 || from == to {
            return Err(Error::References);
        }
        let plan = Plan {
            epoch: self.epoch,
            locations: [
                self.find(keys[0])?,
                self.find(keys[1])?,
                self.find(keys[2])?,
            ],
            keys,
        };
        let indices = self.indices(plan)?;
        let next = self.epoch.checked_add(1).ok_or(Error::Overflow)?;
        for i in &indices {
            if self.entries[*i].counts[from] == 0 {
                return Err(Error::References);
            }
            self.entries[*i].counts[to]
                .checked_add(1)
                .ok_or(Error::Overflow)?;
        }
        for i in indices {
            self.entries[i].counts[from] -= 1;
            self.entries[i].counts[to] += 1;
        }
        self.epoch = next;
        Ok(())
    }
    /// Role3 is retired-but-still-owned. Only control can clear it and drop roots.
    pub fn reclaim_control(&mut self) -> Result<(), Error> {
        let next = self.epoch.checked_add(1).ok_or(Error::Overflow)?;
        for entry in &mut self.entries {
            entry.counts[3] = 0;
        }
        let live: Vec<_> = self
            .entries
            .iter()
            .filter(|e| e.counts.iter().any(|n| *n != 0))
            .map(|e| Arc::clone(&e.root))
            .collect();
        self.entries
            .retain(|entry| live.iter().any(|root| Arc::ptr_eq(root, &entry.root)));
        self.epoch = next;
        Ok(())
    }
    fn context_bytes_after(&self, adding: &[usize]) -> Result<usize, Error> {
        let mut seen = Vec::<&Arc<Root>>::new();
        let mut bytes = 0usize;
        for (i, e) in self.entries.iter().enumerate() {
            if (e.counts.iter().any(|n| *n != 0) || adding.contains(&i))
                && !seen.iter().any(|r| Arc::ptr_eq(r, &e.root))
            {
                bytes = bytes
                    .checked_add(e.root.context.declared_context_bytes())
                    .ok_or(Error::Overflow)?;
                seen.push(&e.root);
            }
        }
        Ok(bytes)
    }
    pub fn context_bytes(&self) -> usize {
        self.context_bytes_after(&[]).unwrap()
    }
    pub fn bookkeeping_bytes(&self) -> usize {
        size_of::<Self>() + self.entries.capacity() * size_of::<Entry>()
    }
    pub fn owner_header_bytes(&self) -> usize {
        let mut seen = Vec::<&Arc<Root>>::new();
        for e in &self.entries {
            if !seen.iter().any(|r| Arc::ptr_eq(r, &e.root)) {
                seen.push(&e.root);
            }
        }
        seen.len() * (2 * size_of::<usize>() + size_of::<Root>())
    }
    pub fn declared_closure_bytes(&self) -> usize {
        let mut seen = Vec::<&Arc<Root>>::new();
        let mut bytes = self.bookkeeping_bytes();
        for e in &self.entries {
            if !seen.iter().any(|r| Arc::ptr_eq(r, &e.root)) {
                // Root Arc allocation header + actual Root + full Box-provider declaration.
                bytes += 2 * size_of::<usize>()
                    + size_of::<Root>()
                    + e.root.context.declared_context_bytes();
                seen.push(&e.root);
            }
        }
        bytes
    }
    pub fn fingerprint(&self) -> Vec<(RootRef, Key, [u32; 4])> {
        self.entries
            .iter()
            .map(|e| (e.location, e.root.key, e.counts))
            .collect()
    }
}
pub(super) fn remap(source: &Catalog, target: &Catalog, plan: Plan) -> Result<Plan, Error> {
    source.indices(plan)?;
    let mut locations = plan.locations;
    for (i, key) in plan.keys.into_iter().enumerate() {
        let old = source.resolve(plan.locations[i], key)?;
        let location = target.find(key)?;
        if !Arc::ptr_eq(old, target.resolve(location, key)?) {
            return Err(Error::Owner);
        }
        locations[i] = location;
    }
    Ok(Plan {
        epoch: target.epoch,
        locations,
        keys: plan.keys,
    })
}
pub(super) fn resolve_recipe(
    slot: &CachedSlot,
    expected: Handle,
    store_namespace: u64,
    slot_index: u32,
) -> Result<Record, Error> {
    if expected.namespace != store_namespace
        || expected.kind != 1
        || expected.slot != slot_index
        || expected.generation == 0
        || expected.generation != slot.metadata.generation
        || slot.metadata.flags != 1
    {
        return Err(Error::Stale);
    }
    slot.record.decode().ok_or(Error::Format)
}
