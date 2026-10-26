//! Control-side plan for the admitted stereo, post-Return additive feed DAG.
//! Existing track sends and Main summation remain owned by their old kernels.
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
impl ReturnFeedPlan {
    pub(crate) fn has_no_owned_storage(&self) -> bool {
        let Self { order, outgoing } = self;
        order.capacity() == 0 && outgoing.capacity() == 0
    }

    pub(crate) fn prepare(project: &hex_project::Project) -> Result<Self, Invalid> {
        if project.authored_routing().is_empty() {
            return Ok(Self::default());
        }
        Self::validate_admission(project)?;
        let returns = project.return_tracks();
        let mut outgoing = vec![Vec::new(); returns.len()];
        let mut incoming = vec![0_usize; returns.len()];
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
            feeds.sort_by_key(|feed| (feed.destination, feed.id));
        }
        let mut order = Vec::with_capacity(returns.len());
        let mut visited = vec![false; returns.len()];
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
        // Canonical empty-feed projects keep the legacy preparation path.
        if !project.authored_routing().is_empty() {
            // Recheck both combined graphs through their canonical owner. The
            // input is already typed; do not clone/reconstruct unrelated content.
            project
                .validate_playback_routing()
                .map_err(|error| Invalid(error.0))?;
        }
        Ok(())
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
