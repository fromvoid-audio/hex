//! Control-side plan for the admitted stereo, post-Return additive feed DAG.
//! Existing track sends and Main summation remain owned by their old kernels.
mod borrowed;
pub(crate) mod persistent;
use crate::{Frame, Invalid};

#[derive(Clone, Copy, Debug)]
struct Feed {
    destination: usize,
    gain: f32,
    id: u64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ReturnFeedPlan {
    order: Vec<usize>,
    outgoing: Vec<Vec<Feed>>,
}
fn plan_vec<T>(capacity: usize) -> Result<Vec<T>, Invalid> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(capacity)
        .map_err(|_| Invalid("return plan allocation failed"))?;
    Ok(result)
}

impl ReturnFeedPlan {
    pub(crate) fn has_no_owned_storage(&self) -> bool {
        let Self { order, outgoing } = self;
        order.capacity() == 0 && outgoing.capacity() == 0
    }

    pub(crate) fn prepare(project: &hex_project::Project) -> Result<Self, Invalid> {
        Self::validate_admission(project)?;
        if project.authored_routing().is_empty() {
            return Ok(Self::default());
        }
        let returns = project.return_tracks();
        // Every backing is reserved from validated scalar counts before fill.
        // No push below can trigger implicit growth or a stable-sort workspace.
        let mut outgoing = plan_vec(returns.len())?;
        let mut incoming = plan_vec(returns.len())?;
        incoming.resize(returns.len(), 0_usize);
        let mut visited = plan_vec(returns.len())?;
        visited.resize(returns.len(), false);
        let mut order = plan_vec(returns.len())?;
        for bus in returns {
            let count = project
                .authored_routing()
                .iter()
                .filter(|feed| feed.source() == bus.id())
                .count();
            outgoing.push(plan_vec(count)?);
        }
        for feed in project.authored_routing() {
            let source = returns
                .iter()
                .position(|bus| bus.id() == feed.source())
                .ok_or(Invalid("return feed source is missing"))?;
            let destination = returns
                .iter()
                .position(|bus| bus.id() == feed.destination())
                .ok_or(Invalid("return feed destination is missing"))?;
            outgoing[source].push(Feed {
                destination,
                gain: feed.gain(),
                id: feed.id().value(),
            });
            incoming[destination] += 1;
        }
        for feeds in &mut outgoing {
            // The complete key includes globally unique route identity. Equal
            // keys cannot exist, so stability cannot affect processing order.
            feeds.sort_unstable_by_key(|feed| (feed.destination, feed.id));
        }
        while order.len() < returns.len() {
            // Pinned ready tie-order, independent of route serialization order.
            let index = (0..returns.len())
                .find(|&i| !visited[i] && incoming[i] == 0)
                .ok_or(Invalid("return feed cycle is unsupported"))?;
            visited[index] = true;
            order.push(index);
            for feed in &outgoing[index] {
                incoming[feed.destination] -= 1;
            }
        }
        Ok(Self { order, outgoing })
    }
    pub(crate) fn validate_admission(project: &hex_project::Project) -> Result<(), Invalid> {
        borrowed::validate(project)
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
    pub(crate) fn edges(&self) -> impl Iterator<Item = (usize, usize, u64, f32)> + '_ {
        self.outgoing
            .iter()
            .enumerate()
            .flat_map(|(source, feeds)| {
                feeds
                    .iter()
                    .map(move |feed| (source, feed.destination, feed.id, feed.gain))
            })
    }
    #[cfg(test)]
    pub(crate) fn test_edges(&self) -> impl Iterator<Item = (usize, usize, u64, f32)> + '_ {
        self.edges()
    }
    pub(crate) fn order(&self) -> &[usize] {
        &self.order
    }

    /// Source scratch already contains post-chain/matrix/gain/mute output.
    /// All targets are still inputs, because preparation proved topological order.
    pub(crate) fn propagate(&self, source: usize, scratch: &mut [[Frame; 256]], count: usize) {
        for feed in &self.outgoing[source] {
            let (src, dst) = if source < feed.destination {
                let (left, right) = scratch.split_at_mut(feed.destination);
                (&left[source], &mut right[0])
            } else {
                let (left, right) = scratch.split_at_mut(source);
                (&right[0], &mut left[feed.destination])
            };
            for (target, sample) in dst[..count].iter_mut().zip(&src[..count]) {
                target[0] += sample[0] * feed.gain;
                target[1] += sample[1] * feed.gain;
            }
        }
    }
}

impl ReturnFeedPlan {
    /// Exact requested backing bytes of the actual retained scalar vectors.
    pub(crate) fn backing_bytes(&self) -> Result<usize, crate::AudioStorageError> {
        let overflow = crate::AudioStorageError::Overflow;
        let mut bytes = self
            .order
            .capacity()
            .checked_mul(std::mem::size_of::<usize>())
            .and_then(|n| {
                self.outgoing
                    .capacity()
                    .checked_mul(std::mem::size_of::<Vec<Feed>>())
                    .and_then(|m| n.checked_add(m))
            })
            .ok_or(overflow)?;
        for feeds in &self.outgoing {
            bytes = feeds
                .capacity()
                .checked_mul(std::mem::size_of::<Feed>())
                .and_then(|n| bytes.checked_add(n))
                .ok_or(overflow)?;
        }
        Ok(bytes)
    }
    pub(crate) fn visit_audio_storage<'a>(
        &'a self,
        l: &mut crate::AudioRootStorageLedger<'a>,
    ) -> Result<(), crate::AudioStorageError> {
        let Self { order, outgoing } = self;
        l.vec(order)?;
        l.vec(outgoing)?;
        for feeds in outgoing {
            l.vec(feeds)?;
            for Feed {
                destination: _,
                gain: _,
                id: _,
            } in feeds
            {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Project};
    #[test]
    fn fresh_feed_backing_counts_reserved_empty_and_nested_capacity() {
        let mut plan = ReturnFeedPlan::default();
        plan.order.try_reserve_exact(7).unwrap();
        plan.outgoing.try_reserve_exact(5).unwrap();
        let mut edges = Vec::new();
        edges.try_reserve_exact(11).unwrap();
        plan.outgoing.push(edges);
        let expected = plan.order.capacity() * std::mem::size_of::<usize>()
            + plan.outgoing.capacity() * std::mem::size_of::<Vec<Feed>>()
            + plan.outgoing[0].capacity() * std::mem::size_of::<Feed>();
        assert_eq!(plan.backing_bytes().unwrap(), expected);
        assert!(expected > 0);
        let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(
            crate::AudioStorageLedgerReservation {
                data_rows: 16,
                handle_rows: 16,
                root_rows: 4,
                max_retained_bytes: 65_536,
                max_construction_peak_bytes: 65_536,
            },
        )
        .unwrap();
        ledger
            .root(&plan, crate::AudioStorageRole::Current)
            .unwrap();
        let before = ledger.summary().unwrap();
        plan.visit_audio_storage(&mut ledger).unwrap();
        let after = ledger.summary().unwrap();
        assert_eq!(after.data_bytes - before.data_bytes, expected);
        assert_eq!(after.data_allocations - before.data_allocations, 3);
        plan.visit_audio_storage(&mut ledger).unwrap();
        assert_eq!(ledger.summary().unwrap(), after);
    }
    #[test]
    fn complete_sixteen_return_dag_has_bounded_deterministic_plan_and_safe_scratch() {
        let mut editor = Editor::new(Project::new(48000).unwrap(), 1).unwrap();
        let mut ids = Vec::new();
        for _ in 0..16 {
            let Created::ReturnTrack(id) = editor
                .execute(Command::AddReturnTrack { name: "bus".into() })
                .unwrap()
            else {
                panic!("return")
            };
            ids.push(id);
        }
        let mut snapshot = editor.project().snapshot();
        for (index, &source) in ids.iter().enumerate() {
            for &destination in &ids[index + 1..] {
                snapshot = snapshot.with_return_feed(source, destination, 0.5).unwrap();
            }
        }
        assert_eq!(snapshot.return_feeds.len(), 120);
        snapshot.return_feeds.reverse();
        let project = Project::from_snapshot(snapshot).unwrap();
        let plan = ReturnFeedPlan::prepare(&project).unwrap();
        assert_eq!(plan.order(), &(0..16).collect::<Vec<_>>());
        assert_eq!(plan.outgoing.iter().map(Vec::len).sum::<usize>(), 120);
        assert_eq!(plan.order.capacity(), 16);
        assert_eq!(plan.outgoing.capacity(), 16);
        for (source, feeds) in plan.outgoing.iter().enumerate() {
            assert_eq!(feeds.capacity(), 15 - source);
        }
        let mut scratch = vec![[[0.0; 2]; 256]; 16];
        scratch[0].fill([1.0, -1.0]);
        for &index in plan.order() {
            plan.propagate(index, &mut scratch, 256);
        }
        assert_eq!(
            scratch[15][255],
            [0.5 * 1.5_f32.powi(14), -0.5 * 1.5_f32.powi(14)]
        );
    }
}
