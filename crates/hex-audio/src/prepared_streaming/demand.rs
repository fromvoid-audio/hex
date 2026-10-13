use super::*;

/// Control-preallocated demand storage. No graph/page ownership or Clone.
pub(crate) struct DemandWorkspace {
    extents: Vec<PageExtent>,
    limit: usize,
    pending: Option<ReadinessStamp>,
}
impl DemandWorkspace {
    pub(crate) fn new(limit: usize) -> Result<Self, Invalid> {
        if limit == 0 || limit >= hex_stream::StreamConfig::initial().page_buffers {
            return Err(Invalid("invalid streaming demand reservation"));
        }
        let mut extents = Vec::new();
        extents
            .try_reserve_exact(limit)
            .map_err(|_| Invalid("stream demand allocation refused"))?;
        Ok(Self {
            extents,
            limit,
            pending: None,
        })
    }
    pub(crate) fn clear(&mut self) {
        self.extents.clear();
    }
    pub(crate) fn visit(&mut self, extent: StreamPageExtent) -> Result<(), Invalid> {
        // Coalesce actual accesses to the same page without a temporary buffer.
        let key = PageKey {
            source: extent.source,
            page_index: extent.page_index,
        };
        if let Some(existing) = self.extents.iter_mut().find(|existing| existing.key == key) {
            existing.start = existing.start.min(extent.start);
            existing.end = existing.end.max(extent.end);
        } else {
            if self.extents.len() == self.limit {
                return Err(Invalid("stream demand capacity refused"));
            }
            self.extents.push(PageExtent {
                key,
                start: extent.start,
                end: extent.end,
            });
        }
        Ok(())
    }
    pub(super) fn allocation_capacity(&self) -> hex_stream::AllocationCapacity {
        hex_stream::AllocationCapacity {
            capacity: self.extents.capacity(),
            element_bytes: std::mem::size_of::<PageExtent>(),
        }
    }
    pub(crate) fn allocation_bytes(&self) -> Option<usize> {
        self.extents
            .capacity()
            .checked_mul(std::mem::size_of::<PageExtent>())
    }
    /// Stamps are caller context plus actual engine window/command prefix. A
    /// retry uses a new nonwrapping request epoch; old tokens never authorize it.
    pub(crate) fn install(
        &mut self,
        readiness: &mut PageReadiness,
        catalog: &CatalogLease,
        role: ProtectionRole,
        epochs: &mut ReadinessEpochs,
        mut context: ReadinessStamp,
        window: StreamChunkWindow,
    ) -> Result<bool, Invalid> {
        self.extents
            .sort_unstable_by_key(|extent| (extent.key.source, extent.key.page_index));
        context.performance_frame = window.performance_frame;
        context.arrangement_frame = window.arrangement_frame;
        context.route_generation = window.route_generation;
        context.window_frames = window.frames as u64;
        // Preserve a pending intent only while all context and actual extents
        // remain unchanged. Replacing it explicitly cancels its own protection.
        if let Some(old) = self.pending {
            context.request_epoch = old.request_epoch;
            if context != old {
                readiness.cancel(role, old.request_epoch);
                self.pending = None;
            }
        }
        if self.pending.is_none() {
            context.request_epoch = epochs
                .advance()
                .map_err(|_| Invalid("stream readiness epoch exhausted"))?;
            match readiness.submit_borrowed(role, catalog, context, &self.extents) {
                IntentStatus::Accepted | IntentStatus::Pending => self.pending = Some(context),
                IntentStatus::Refused(_) => {
                    return Err(Invalid("stream readiness submission refused"));
                }
            }
        }
        let current = self.pending.expect("accepted intent");
        if readiness.installed_borrowed(role, catalog, current, &self.extents) {
            return Ok(true);
        }
        match readiness.service(role, current, self.limit, self.limit) {
            ReadinessProgress::InstalledProtected => {
                if readiness
                    .acknowledge_borrowed(role, catalog, current, &self.extents)
                    .is_none()
                {
                    return Err(Invalid("stream installed receipt mismatch"));
                }
                Ok(true)
            }
            ReadinessProgress::Pending(_) => Ok(false),
            ReadinessProgress::Refused(_) => Err(Invalid("stream readiness window refused")),
        }
    }
}
