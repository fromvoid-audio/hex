//! Complete reserved-data traversal of the engine-owned Return feed DAG.
//! Plan/Vec headers already live in the containing graph/outer Vec allocation.
use super::{Feed, ReturnFeedPlan};
use crate::storage_report::{PlaybackStorageVisitor, Report, StorageOwner, StorageReportError};

impl ReturnFeedPlan {
    pub(crate) fn visit_storage<'a, V: PlaybackStorageVisitor<'a>>(
        &'a self,
        report: &mut Report<'a, '_, V>,
        owner: StorageOwner<'a>,
    ) -> Result<(), StorageReportError> {
        let Self { order, outgoing } = self;
        report.vec(owner, "return_feed.order", [0; 2], order)?;
        // Backing slots include inner Vec headers, including unused capacity.
        report.vec(owner, "return_feed.outgoing", [0; 2], outgoing)?;
        for (index, feeds) in outgoing.iter().enumerate() {
            report.vec(owner, "return_feed.feeds", [index, 0], feeds)?;
            for feed in feeds {
                // Exhaustive scalar-only leaf. A new field requires this
                // owning module to review its storage contract too.
                let Feed {
                    destination: _,
                    gain: _,
                    id: _,
                } = feed;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
