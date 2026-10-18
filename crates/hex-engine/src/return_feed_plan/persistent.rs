//! Borrowed canonical-input constructor; no snapshot clone/local owning graph.
use super::*;
mod construction;
use crate::AudioStorageError as Error;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReturnFeedCapacity {
    pub row_bytes: usize,
    pub order_bytes: usize,
    pub outgoing_header_bytes: usize,
    pub feed_bytes: usize,
    pub incoming_bytes: usize,
    pub visited_bytes: usize,
    pub previous_growth_backing_bytes: usize,
}
impl ReturnFeedCapacity {
    pub(crate) fn bytes(self) -> Result<usize, Error> {
        [
            self.row_bytes,
            self.order_bytes,
            self.outgoing_header_bytes,
            self.feed_bytes,
            self.incoming_bytes,
            self.visited_bytes,
        ]
        .into_iter()
        .try_fold(0usize, |n, b| n.checked_add(b).ok_or(Error::Overflow))
    }
}
#[derive(Default)]
struct Row {
    feeds: Vec<Feed>,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Vacant,
    Reserved,
    Preparing,
    Validated,
    Installed,
    Cleaning,
    Complete,
}
#[derive(Default)]
pub(crate) struct FeedPlanCustody {
    rows: Vec<Row>,
    order: Vec<usize>,
    outgoing: Vec<Vec<Feed>>,
    incoming: Vec<usize>,
    visited: Vec<bool>,
    plan: Option<ReturnFeedPlan>,
    active: usize,
    nonempty: bool,
    phase: Phase,
    growth: usize,
    empty_disposal_pending: bool,
    #[cfg(test)]
    pub(crate) fail_after: Option<usize>,
    #[cfg(test)]
    pub(crate) panic_after: Option<usize>,
}
fn bytes<T>(v: &Vec<T>) -> Result<usize, Error> {
    v.capacity()
        .checked_mul(std::mem::size_of::<T>())
        .ok_or(Error::Overflow)
}
fn reserve<T>(v: &mut Vec<T>, n: usize, growth: &mut usize) -> Result<(), Error> {
    if n > v.capacity() {
        *growth = growth.checked_add(bytes(v)?).ok_or(Error::Overflow)?;
    }
    v.try_reserve_exact(n.saturating_sub(v.len()))
        .map_err(|_| Error::Allocation)
}
impl FeedPlanCustody {
    pub(crate) fn can_move_root(&self) -> bool {
        self.phase == Phase::Installed && self.plan.is_some()
    }
    pub(crate) fn move_root(&mut self, target: &mut ReturnFeedPlan) {
        if let Some(plan) = self.plan.take() {
            *target = plan;
        }
    }
    pub(crate) fn can_restore_root(&self) -> bool {
        self.phase == Phase::Installed && self.plan.is_none()
    }
    pub(crate) fn restore_root(&mut self, source: &mut ReturnFeedPlan) {
        self.plan = Some(std::mem::take(source));
    }
    pub(crate) fn capacity_at(
        &self,
        growth: bool,
        external: Option<&ReturnFeedPlan>,
    ) -> Result<ReturnFeedCapacity, Error> {
        let mut c = ReturnFeedCapacity {
            row_bytes: bytes(&self.rows)?,
            order_bytes: bytes(&self.order)?,
            outgoing_header_bytes: bytes(&self.outgoing)?,
            incoming_bytes: bytes(&self.incoming)?,
            visited_bytes: bytes(&self.visited)?,
            previous_growth_backing_bytes: if growth && self.phase != Phase::Vacant {
                self.growth
            } else {
                0
            },
            ..Default::default()
        };
        for r in &self.rows {
            c.feed_bytes = c
                .feed_bytes
                .checked_add(bytes(&r.feeds)?)
                .ok_or(Error::Overflow)?;
        }
        for r in &self.outgoing {
            c.feed_bytes = c.feed_bytes.checked_add(bytes(r)?).ok_or(Error::Overflow)?;
        }
        if self.plan.is_some() && external.is_some() {
            return Err(Error::SourceMismatch);
        }
        if let Some(ReturnFeedPlan { order, outgoing }) = self.plan.as_ref().or(external) {
            c.order_bytes = c
                .order_bytes
                .checked_add(bytes(order)?)
                .ok_or(Error::Overflow)?;
            c.outgoing_header_bytes = c
                .outgoing_header_bytes
                .checked_add(bytes(outgoing)?)
                .ok_or(Error::Overflow)?;
            for feeds in outgoing {
                c.feed_bytes = c
                    .feed_bytes
                    .checked_add(bytes(feeds)?)
                    .ok_or(Error::Overflow)?;
                for Feed {
                    destination: _,
                    gain: _,
                    id: _,
                } in feeds
                {}
            }
        }
        c.bytes()?;
        Ok(c)
    }
    fn require_vacant(&self) -> Result<(), Error> {
        if self.phase != Phase::Vacant
            || self.plan.is_some()
            || !self.order.is_empty()
            || !self.outgoing.is_empty()
            || !self.incoming.is_empty()
            || !self.visited.is_empty()
            || self.rows.iter().any(|r| !r.feeds.is_empty())
            || self.empty_disposal_pending
        {
            return Err(Error::Capacity);
        }
        Ok(())
    }
    pub(crate) fn reserve(&mut self, project: &hex_project::Project) -> Result<(), Error> {
        let before = self.capacity_at(false, None)?.bytes()?;
        let additional = self.construction_cost(project)?;
        let returns = project.return_tracks();
        let feeds = project.authored_routing();
        if returns.len() > hex_project::MAX_RETURN_TRACKS
            || feeds.len() > hex_project::MAX_RETURN_FEEDS
        {
            return Err(Error::UnsupportedGraph);
        }
        // Canonical Project owns validated global identities/high-water. Repeat
        // local port/gain/pair/DAG checks borrowing it, without snapshot allocation.
        for (index, f) in feeds.iter().enumerate() {
            if f.id().value() == 0
                || f.source() == f.destination()
                || !f.gain().is_finite()
                || f.gain() <= 0.0
                || f.gain() > 2.0
                || !returns.iter().any(|r| r.id() == f.source())
                || !returns.iter().any(|r| r.id() == f.destination())
                || feeds[..index].iter().any(|old| {
                    old.id() == f.id()
                        || (old.source() == f.source() && old.destination() == f.destination())
                })
            {
                return Err(Error::SourceMismatch);
            }
        }
        self.phase = Phase::Reserved;
        self.growth = 0;
        self.nonempty = !feeds.is_empty();
        self.active = 0;
        let count = if self.nonempty { returns.len() } else { 0 };
        reserve(&mut self.rows, count, &mut self.growth)?;
        while self.rows.len() < count {
            self.rows.push(Row::default());
        }
        reserve(&mut self.order, count, &mut self.growth)?;
        reserve(&mut self.outgoing, count, &mut self.growth)?;
        reserve(&mut self.incoming, count, &mut self.growth)?;
        reserve(&mut self.visited, count, &mut self.growth)?;
        for (r, bus) in self.rows.iter_mut().zip(returns).take(count) {
            let n = feeds.iter().filter(|f| f.source() == bus.id()).count();
            reserve(&mut r.feeds, n, &mut self.growth)?;
        }
        self.incoming.resize(count, 0);
        self.visited.resize(count, false);
        self.active = count;
        let actual = self.capacity_at(true, None)?;
        let peak = actual
            .bytes()?
            .checked_add(actual.previous_growth_backing_bytes)
            .ok_or(Error::Overflow)?;
        if peak > before.checked_add(additional).ok_or(Error::Overflow)? {
            return Err(Error::UnknownStorage);
        }
        Ok(())
    }
    pub(crate) fn prepare(&mut self, project: &hex_project::Project) -> Result<(), Error> {
        if self.phase != Phase::Reserved
            || self.nonempty == project.authored_routing().is_empty()
            || self.active
                != if self.nonempty {
                    project.return_tracks().len()
                } else {
                    0
                }
        {
            return Err(Error::SourceMismatch);
        }
        self.phase = Phase::Preparing;
        for f in project.authored_routing() {
            let source = project
                .return_tracks()
                .iter()
                .position(|r| r.id() == f.source())
                .ok_or(Error::SourceMismatch)?;
            let destination = project
                .return_tracks()
                .iter()
                .position(|r| r.id() == f.destination())
                .ok_or(Error::SourceMismatch)?;
            if self.rows[source].feeds.len() == self.rows[source].feeds.capacity() {
                return Err(Error::Capacity);
            }
            self.rows[source].feeds.push(Feed {
                destination,
                gain: f.gain(),
                id: f.id().value(),
            });
            self.incoming[destination] = self.incoming[destination]
                .checked_add(1)
                .ok_or(Error::Overflow)?;
            #[cfg(test)]
            self.fault(
                self.rows[..self.active]
                    .iter()
                    .map(|r| r.feeds.len())
                    .sum::<usize>()
                    - 1,
            )?;
        }
        for r in &mut self.rows[..self.active] {
            r.feeds.sort_unstable_by_key(|f| (f.destination, f.id));
        }
        while self.order.len() < self.active {
            let next = (0..self.active)
                .find(|&i| !self.visited[i] && self.incoming[i] == 0)
                .ok_or(Error::SourceMismatch)?;
            self.visited[next] = true;
            self.order.push(next);
            for f in &self.rows[next].feeds {
                self.incoming[f.destination] = self.incoming[f.destination]
                    .checked_sub(1)
                    .ok_or(Error::SourceMismatch)?;
            }
        }
        self.phase = Phase::Validated;
        Ok(())
    }
    #[cfg(test)]
    fn fault(&self, index: usize) -> Result<(), Error> {
        assert_ne!(
            self.panic_after,
            Some(index),
            "persistent feed preparation panic"
        );
        if self.fail_after == Some(index) {
            return Err(Error::Allocation);
        }
        Ok(())
    }
    pub(crate) fn can_install(&self) -> bool {
        self.phase == Phase::Validated
            && self.plan.is_none()
            && self.outgoing.is_empty()
            && self.outgoing.capacity() >= self.active
    }
    pub(crate) fn install(&mut self) -> bool {
        if !self.can_install() {
            return false;
        }
        if self.nonempty {
            for r in &mut self.rows[..self.active] {
                self.outgoing.push(std::mem::take(&mut r.feeds));
            }
            self.plan = Some(ReturnFeedPlan {
                order: std::mem::take(&mut self.order),
                outgoing: std::mem::take(&mut self.outgoing),
            });
        } else {
            self.plan = Some(ReturnFeedPlan::default());
        }
        self.phase = Phase::Installed;
        true
    }
    pub(crate) fn cleanup_on_control(&mut self) -> Result<(), Error> {
        if self.phase == Phase::Vacant || self.phase == Phase::Complete {
            return Ok(());
        }
        self.phase = Phase::Cleaning;
        if let Some(p) = &self.plan {
            if self.nonempty {
                if self.order.capacity() != 0
                    || self.outgoing.capacity() != 0
                    || p.outgoing.len() != self.active
                    || self.rows[..self.active]
                        .iter()
                        .any(|r| r.feeds.capacity() != 0)
                {
                    return Err(Error::Capacity);
                }
                if let Some(ReturnFeedPlan { order, outgoing }) = self.plan.take() {
                    self.order = order;
                    self.outgoing = outgoing;
                }
                for i in (0..self.active).rev() {
                    if let Some(feeds) = self.outgoing.pop() {
                        self.rows[i].feeds = feeds;
                    }
                }
            } else {
                if p.order.capacity() != 0
                    || p.outgoing.capacity() != 0
                    || self.empty_disposal_pending
                {
                    return Err(Error::UnknownStorage);
                }
                self.empty_disposal_pending = true;
                drop(self.plan.take());
                self.empty_disposal_pending = false;
            }
        }
        // Exhaustive scalar Feed leaves; retain all nested/outer spare backing.
        for r in &mut self.rows {
            for Feed {
                destination: _,
                gain: _,
                id: _,
            } in &r.feeds
            {}
            r.feeds.clear();
        }
        self.order.clear();
        self.incoming.clear();
        self.visited.clear();
        self.phase = Phase::Complete;
        Ok(())
    }
    pub(crate) fn clear_disposed(&mut self) -> Result<(), Error> {
        if self.phase == Phase::Vacant {
            return Ok(());
        }
        if self.phase != Phase::Complete
            || self.plan.is_some()
            || self.empty_disposal_pending
            || !self.outgoing.is_empty()
            || self.rows.iter().any(|r| !r.feeds.is_empty())
        {
            return Err(Error::Capacity);
        }
        self.phase = Phase::Vacant;
        self.active = 0;
        self.nonempty = false;
        Ok(())
    }
    pub(crate) fn installed(&self) -> bool {
        self.phase == Phase::Installed && self.plan.is_some()
    }
    pub(crate) fn order(&self) -> impl Iterator<Item = usize> + '_ {
        self.plan.iter().flat_map(|p| p.order.iter().copied())
    }
    pub(crate) fn feeds(&self) -> impl Iterator<Item = (usize, usize, u64, f32)> + '_ {
        self.plan.iter().flat_map(|p| {
            p.outgoing.iter().enumerate().flat_map(|(source, v)| {
                v.iter().map(move |f| (source, f.destination, f.id, f.gain))
            })
        })
    }
    /// Test observations only: each nonzero backing remains anchored in self.
    /// These addresses are never production identity/retirement authority.
    #[cfg(test)]
    pub(crate) fn test_allocations(&self) -> Vec<(usize, usize, usize)> {
        self.rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| {
                let feeds = self
                    .plan
                    .as_ref()
                    .filter(|_| self.nonempty)
                    .and_then(|p| p.outgoing.get(i))
                    .or_else(|| self.outgoing.get(i))
                    .unwrap_or(&r.feeds);
                (feeds.capacity() != 0).then_some((i, feeds.as_ptr() as usize, feeds.capacity()))
            })
            .collect()
    }
    #[cfg(test)]
    pub(crate) fn plan(&self) -> Option<&ReturnFeedPlan> {
        self.plan.as_ref()
    }
}

impl FeedPlanCustody {
    pub(crate) fn visit_audio_storage<'a>(
        &'a self,
        l: &mut crate::AudioRootStorageLedger<'a>,
        external: Option<&'a ReturnFeedPlan>,
    ) -> Result<(), Error> {
        let Self {
            rows,
            order,
            outgoing,
            incoming,
            visited,
            plan,
            active: _,
            nonempty: _,
            phase: _,
            growth,
            empty_disposal_pending: _,
            #[cfg(test)]
                fail_after: _,
            #[cfg(test)]
                panic_after: _,
        } = self;
        if plan.is_some() && external.is_some() {
            return Err(Error::SourceMismatch);
        }
        l.vec(rows)?;
        for Row { feeds } in rows {
            l.vec(feeds)?;
        }
        l.vec(order)?;
        l.vec(outgoing)?;
        for feeds in outgoing {
            l.vec(feeds)?;
        }
        l.vec(incoming)?;
        l.vec(visited)?;
        l.growth(*growth)?;
        if let Some(plan) = plan.as_ref().or(external) {
            plan.visit_audio_storage(l)?;
        }
        Ok(())
    }
}
