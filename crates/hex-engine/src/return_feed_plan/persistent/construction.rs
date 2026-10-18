//! Pure additional backing for the ACTUAL persistent paired feed constructor.
//! The separate fresh compiler plan and its validation scratch are not this plan.
use super::*;
fn replacement<T>(values: Option<&Vec<T>>, wanted: usize) -> Result<usize, Error> {
    if wanted <= values.map_or(0, Vec::capacity) {
        return Ok(0);
    }
    std::alloc::Layout::array::<T>(wanted).map_err(|_| Error::Overflow)?;
    wanted.checked_mul(size_of::<T>()).ok_or(Error::Overflow)
}
impl FeedPlanCustody {
    /// Add to an input union already charging ALL current capacities. Old
    /// replacement backing remains in that union during reserve/reallocation.
    /// The paired constructor uses in-place scalar validation/sort: unlike
    /// ReturnFeedPlan::prepare, it allocates no six-vector routing validator.
    pub(crate) fn construction_cost(&self, project: &hex_project::Project) -> Result<usize, Error> {
        self.require_vacant()?;
        let count = if project.authored_routing().is_empty() {
            0
        } else {
            project.return_tracks().len()
        };
        if count > hex_project::MAX_RETURN_TRACKS
            || project.authored_routing().len() > hex_project::MAX_RETURN_FEEDS
        {
            return Err(Error::UnsupportedGraph);
        }
        let mut result = [
            replacement(Some(&self.rows), count)?,
            replacement(Some(&self.order), count)?,
            replacement(Some(&self.outgoing), count)?,
            replacement(Some(&self.incoming), count)?,
            replacement(Some(&self.visited), count)?,
        ]
        .into_iter()
        .try_fold(0usize, |n, x| n.checked_add(x).ok_or(Error::Overflow))?;
        for (index, bus) in project.return_tracks().iter().enumerate().take(count) {
            let wanted = project
                .authored_routing()
                .iter()
                .filter(|feed| feed.source() == bus.id())
                .count();
            result = result
                .checked_add(replacement(
                    self.rows.get(index).map(|row| &row.feeds),
                    wanted,
                )?)
                .ok_or(Error::Overflow)?;
        }
        Ok(result)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor};
    fn project(routed: bool) -> hex_project::Project {
        let mut editor = Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
        let mut ids = [None; 3];
        for id in &mut ids {
            let Created::ReturnTrack(bus) = editor
                .execute(Command::AddReturnTrack { name: "bus".into() })
                .unwrap()
            else {
                panic!("return")
            };
            *id = Some(bus);
        }
        if routed {
            let snapshot = editor
                .project()
                .snapshot()
                .with_return_feed(ids[0].unwrap(), ids[1].unwrap(), 0.5)
                .unwrap()
                .with_return_feed(ids[1].unwrap(), ids[2].unwrap(), 0.3)
                .unwrap();
            hex_project::Project::from_snapshot(snapshot).unwrap()
        } else {
            editor.project().clone()
        }
    }
    #[test]
    fn paired_feed_cost_preserves_spare_backing_and_bounds_reserve_prepare_install() {
        for routed in [false, true] {
            for capacity in [0, 1, 7] {
                let project = project(routed);
                let mut feed = FeedPlanCustody::default();
                feed.rows.reserve_exact(capacity);
                while feed.rows.len() < capacity {
                    feed.rows.push(Row::default());
                }
                for row in &mut feed.rows {
                    row.feeds.reserve_exact(5);
                }
                feed.order.reserve_exact(capacity);
                feed.outgoing.reserve_exact(capacity);
                feed.incoming.reserve_exact(capacity);
                feed.visited.reserve_exact(capacity);
                let before = feed.capacity_at(false, None).unwrap().bytes().unwrap();
                let pointer = feed.rows.as_ptr();
                let additional = feed.construction_cost(&project).unwrap();
                assert_eq!(feed.rows.as_ptr(), pointer);
                assert!(feed.phase == Phase::Vacant);
                if !routed {
                    assert_eq!(additional, 0);
                }
                feed.reserve(&project).unwrap();
                feed.prepare(&project).unwrap();
                assert!(feed.install());
                let actual = feed.capacity_at(true, None).unwrap();
                assert!(
                    actual.bytes().unwrap() + actual.previous_growth_backing_bytes
                        <= before + additional
                );
                assert_eq!(
                    feed.plan.as_ref().unwrap().edges().count(),
                    if routed { 2 } else { 0 }
                );
                assert!(matches!(
                    feed.construction_cost(&project),
                    Err(Error::Capacity)
                ));
                eprintln!(
                    "SUPPORTED paired feed routed={routed} capacity={capacity} before={before} additional={additional}"
                );
            }
        }
    }
}
