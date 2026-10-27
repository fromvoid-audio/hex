//! Pre-reserved exact-pair disposal on control, separate from native quiescence.
//!
//! The actual control/pages pair enters persistent storage only after preflight. External catalog
//! leases yield Busy before destructive progress. The real worker handle leaves its mutex before
//! join, and every fixed page must be recovered in owned worker/endpoint storage before physical
//! completion. Pending, first panic and remaining endpoints persist on failure; mismatches never
//! manufacture a successful pool report or release a host's independent native output lease.
use super::*;
use std::{
    any::Any,
    panic::{AssertUnwindSafe, catch_unwind},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamDisposalPhase {
    Prepared,
    Owned,
    Joining,
    Verifying,
    Destroying,
    Complete,
    FailedIncomplete,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamDisposalError {
    PairMismatch,
    Capacity,
    Incomplete,
    Consumed,
}
/// Constructor-paired reservation. Actual pair owners enter persistent fields
/// only after preflight; failed cleanup never drops them as an error side effect.
pub struct PreparedStreamDisposal {
    pair: Arc<()>,
    control: Option<StreamControl>,
    pages: Option<StreamCallbackPages>,
    worker: Option<JoinHandle<Vec<Box<PageBuffer>>>>,
    worker_pages: Option<Vec<Box<PageBuffer>>>,
    endpoint_pages: Vec<Box<PageBuffer>>,
    first_panic: Option<Box<dyn Any + Send>>,
    phase: StreamDisposalPhase,
    expected_pages: usize,
    cached_returned: usize,
    ready_count: usize,
    held_count: usize,
    report: Option<PagePoolReport>,
}
impl PreparedStreamDisposal {
    pub fn try_prepare_on_control(
        control: &StreamControl,
        pages: &StreamCallbackPages,
    ) -> Result<Self, StreamDisposalError> {
        if !Arc::ptr_eq(&control.pair_id, &pages.pair_id)
            || !Arc::ptr_eq(&control.owner, &pages._owner)
        {
            return Err(StreamDisposalError::PairMismatch);
        }
        Self::reserve_for_pair(
            Arc::clone(&control.pair_id),
            control.owner.config.page_buffers,
        )
    }
    /// Factory-only reservation before any source worker starts. The actual
    /// pair ID is created by that same factory and never escapes independently.
    pub(super) fn reserve_for_pair(
        pair: Arc<()>,
        expected_pages: usize,
    ) -> Result<Self, StreamDisposalError> {
        let mut endpoint_pages = Vec::new();
        endpoint_pages
            .try_reserve_exact(expected_pages)
            .map_err(|_| StreamDisposalError::Capacity)?;
        Ok(Self {
            pair,
            control: None,
            pages: None,
            worker: None,
            worker_pages: None,
            endpoint_pages,
            first_panic: None,
            phase: StreamDisposalPhase::Prepared,
            expected_pages,
            cached_returned: 0,
            ready_count: 0,
            held_count: 0,
            report: None,
        })
    }
    #[cfg(test)]
    pub(super) fn test_identity(&self) -> (usize, usize, usize) {
        (
            Arc::as_ptr(&self.pair) as usize,
            self.endpoint_pages.as_ptr() as usize,
            self.endpoint_pages.capacity(),
        )
    }
    pub(super) fn matches_prepared_pair(
        &self,
        control: &StreamControl,
        pages: &StreamCallbackPages,
    ) -> bool {
        self.phase == StreamDisposalPhase::Prepared
            && self.control.is_none()
            && self.pages.is_none()
            && self.expected_pages == control.owner.config.page_buffers
            && Arc::ptr_eq(&self.pair, &control.pair_id)
            && Arc::ptr_eq(&self.pair, &pages.pair_id)
            && self.endpoint_pages.capacity() >= self.expected_pages
    }
    /// On refusal the original pair remains in the caller's persistent slot.
    pub fn begin_on_control(
        &mut self,
        source: &mut Option<(StreamControl, StreamCallbackPages)>,
    ) -> Result<(), StreamDisposalError> {
        if self.phase != StreamDisposalPhase::Prepared
            || self.control.is_some()
            || self.pages.is_some()
        {
            return Err(StreamDisposalError::Consumed);
        }
        let (control, pages) = source.as_ref().ok_or(StreamDisposalError::PairMismatch)?;
        if !Arc::ptr_eq(&self.pair, &control.pair_id)
            || !Arc::ptr_eq(&self.pair, &pages.pair_id)
            || !Arc::ptr_eq(&control.owner, &pages._owner)
        {
            return Err(StreamDisposalError::PairMismatch);
        }
        if let Some((control, pages)) = source.take() {
            self.control = Some(control);
            self.pages = Some(pages);
            self.phase = StreamDisposalPhase::Owned;
        }
        Ok(())
    }
    pub fn phase(&self) -> StreamDisposalPhase {
        self.phase
    }
    /// Only the newly reserved endpoint-page container DATA. The worker result
    /// Vec transfers its existing factory backing; it is not charged again here.
    /// Arc/worker/channel/OS costs and pending destruction
    /// remain unresolved. No caller should use this as a complete domain total.
    pub fn metadata_bytes(&self) -> Option<usize> {
        let Self {
            pair: _,
            control: _,
            pages: _,
            worker: _,
            worker_pages: _,
            endpoint_pages,
            first_panic: _,
            phase: _,
            expected_pages: _,
            cached_returned: _,
            ready_count: _,
            held_count: _,
            report: _,
        } = self;
        endpoint_pages
            .capacity()
            .checked_mul(std::mem::size_of::<Box<PageBuffer>>())
    }
    /// A Busy lease check makes no stop/join mutation. Joining, conservation and
    /// destruction progress remain in this borrowed capsule through unwind.
    pub fn advance_on_control(&mut self) -> Result<Option<PagePoolReport>, StreamDisposalError> {
        if matches!(
            self.phase,
            StreamDisposalPhase::FailedIncomplete
                | StreamDisposalPhase::Joining
                | StreamDisposalPhase::Destroying
        ) {
            return Err(StreamDisposalError::Incomplete);
        }
        if self.phase == StreamDisposalPhase::Complete {
            return Err(StreamDisposalError::Consumed);
        }
        if self.phase == StreamDisposalPhase::Owned {
            let control = self
                .control
                .as_ref()
                .ok_or(StreamDisposalError::Incomplete)?;
            let pages = self.pages.as_mut().ok_or(StreamDisposalError::Incomplete)?;
            if Arc::strong_count(&control.owner) > 2 {
                return Ok(None);
            }
            self.phase = StreamDisposalPhase::Joining;
            self.cached_returned = pages.endpoint.return_cached_pages();
            control.owner.stop.store(true, Ordering::Release);
            control.wake_worker();
            // Secure the actual JoinHandle, then release the ordinary worker
            // mutex before joining or touching any panic payload/destructor.
            {
                let mut worker = lock_unpoisoned(&control.owner.worker);
                self.worker = worker.take();
            }
            let Some(worker) = self.worker.take() else {
                self.phase = StreamDisposalPhase::FailedIncomplete;
                return Err(StreamDisposalError::Incomplete);
            };
            match catch_unwind(AssertUnwindSafe(|| worker.join())) {
                Ok(Ok(pages)) => self.worker_pages = Some(pages),
                Ok(Err(payload)) | Err(payload) => {
                    self.first_panic = Some(payload);
                    self.phase = StreamDisposalPhase::FailedIncomplete;
                    return Err(StreamDisposalError::Incomplete);
                }
            }
            self.phase = StreamDisposalPhase::Verifying;
            return Ok(None); // retained join result; no physical completion yet
        }
        if self.phase != StreamDisposalPhase::Verifying {
            return Err(StreamDisposalError::Incomplete);
        }
        let pages = self.pages.as_mut().ok_or(StreamDisposalError::Incomplete)?;
        let Some((ready, held)) = pages
            .endpoint
            .collect_disposal_pages(&mut self.endpoint_pages)
        else {
            self.phase = StreamDisposalPhase::FailedIncomplete;
            return Err(StreamDisposalError::Incomplete);
        };
        self.ready_count = ready;
        self.held_count = held;
        let worker_count = self.worker_pages.as_ref().map_or(0, Vec::len);
        if ready
            .checked_add(held)
            .and_then(|n| n.checked_add(worker_count))
            != Some(self.expected_pages)
        {
            self.phase = StreamDisposalPhase::FailedIncomplete;
            return Err(StreamDisposalError::Incomplete);
        }
        let bytes = self
            .control
            .as_ref()
            .map(|control| &control.owner.transport_data)
            .and_then(|data| {
                crate::footprint::sum([
                    data.pcm_boxes,
                    data.scalar_boxes,
                    data.scalar_coverage_boxes,
                ])
            })
            .ok_or(StreamDisposalError::Capacity)?;
        self.phase = StreamDisposalPhase::Destroying;
        let result = catch_unwind(AssertUnwindSafe(|| {
            self.endpoint_pages.clear();
            drop(self.worker_pages.take());
            drop(self.pages.take());
            drop(self.control.take());
        }));
        if let Err(payload) = result {
            self.first_panic = Some(payload);
            self.phase = StreamDisposalPhase::FailedIncomplete;
            return Err(StreamDisposalError::Incomplete);
        }
        self.phase = StreamDisposalPhase::Complete;
        self.report = Some(PagePoolReport {
            page_buffers: self.expected_pages,
            page_bytes: bytes,
            cached_pages_returned: self.cached_returned,
            queued_pages_reclaimed: ready,
            held_return_pages_reclaimed: held,
            worker_pages_reclaimed: worker_count,
            worker_joined: true,
            accounting_complete: true,
        });
        Ok(self.report.take())
    }
}

#[cfg(test)]
thread_local! { pub(super) static WORKER_PANIC: std::cell::RefCell<Option<Box<dyn Any + Send>>> = std::cell::RefCell::new(None); }
#[cfg(test)]
pub(super) fn take_worker_test_panic() -> Option<Box<dyn Any + Send>> {
    WORKER_PANIC.with(|p| p.borrow_mut().take())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Weak, atomic::AtomicUsize};
    fn pair() -> (StreamControl, StreamCallbackPages) {
        let mut runtime = StreamRuntime::new(StreamConfig::with_pool_pages(4), vec![]).unwrap();
        runtime.wait_until_ready(Duration::from_secs(2)).unwrap();
        runtime.split().ok().unwrap()
    }
    #[test]
    fn real_pair_busy_is_nonmutating_then_normal_conservation_and_completion_are_once_only() {
        let (control, pages) = pair();
        let lease = control.catalog_lease().unwrap();
        let mut disposal =
            PreparedStreamDisposal::try_prepare_on_control(&control, &pages).unwrap();
        let mut input = Some((control, pages));
        disposal.begin_on_control(&mut input).unwrap();
        assert!(input.is_none());
        let metadata = disposal.metadata_bytes();
        assert!(disposal.advance_on_control().unwrap().is_none());
        assert_eq!(disposal.phase(), StreamDisposalPhase::Owned);
        assert!(
            !disposal
                .control
                .as_ref()
                .unwrap()
                .owner
                .stop
                .load(Ordering::Acquire)
        );
        assert!(lock_unpoisoned(&disposal.control.as_ref().unwrap().owner.worker).is_some());
        drop(lease);
        assert!(disposal.advance_on_control().unwrap().is_none());
        assert_eq!(disposal.phase(), StreamDisposalPhase::Verifying);
        assert!(disposal.control.is_some() && disposal.pages.is_some());
        let report = disposal.advance_on_control().unwrap().unwrap();
        assert!(report.worker_joined && report.accounting_complete);
        assert_eq!(
            report.worker_pages_reclaimed
                + report.queued_pages_reclaimed
                + report.held_return_pages_reclaimed,
            4
        );
        assert_eq!(disposal.metadata_bytes(), metadata);
        assert_eq!(
            disposal.advance_on_control(),
            Err(StreamDisposalError::Consumed)
        );
    }
    #[test]
    fn wrong_actual_pair_refuses_before_owner_moves() {
        let (a, pages_a) = pair();
        let (b, pages_b) = pair();
        let mut disposal = PreparedStreamDisposal::try_prepare_on_control(&a, &pages_a).unwrap();
        let identity = Arc::as_ptr(&b.pair_id);
        let mut wrong = Some((b, pages_b));
        assert_eq!(
            disposal.begin_on_control(&mut wrong),
            Err(StreamDisposalError::PairMismatch)
        );
        assert_eq!(Arc::as_ptr(&wrong.as_ref().unwrap().0.pair_id), identity);
        assert!(disposal.control.is_none() && disposal.pages.is_none());
        let (b, pages_b) = wrong.take().unwrap();
        b.shutdown(pages_b).unwrap();
        a.shutdown(pages_a).unwrap();
    }
    #[test]
    fn missing_actual_page_retains_all_remaining_owners_and_never_rejoins_or_reports_success() {
        let (control, pages) = pair();
        let mut disposal =
            PreparedStreamDisposal::try_prepare_on_control(&control, &pages).unwrap();
        disposal
            .begin_on_control(&mut Some((control, pages)))
            .unwrap();
        assert!(disposal.advance_on_control().unwrap().is_none());
        let outside_page = disposal.worker_pages.as_mut().unwrap().pop().unwrap();
        let remaining = disposal.worker_pages.as_ref().unwrap().len();
        assert_eq!(
            disposal.advance_on_control(),
            Err(StreamDisposalError::Incomplete)
        );
        assert_eq!(disposal.phase(), StreamDisposalPhase::FailedIncomplete);
        assert!(disposal.control.is_some() && disposal.pages.is_some());
        assert_eq!(disposal.worker_pages.as_ref().unwrap().len(), remaining);
        assert_eq!(
            disposal.advance_on_control(),
            Err(StreamDisposalError::Incomplete)
        );
        assert_eq!(disposal.worker_pages.as_ref().unwrap().len(), remaining);
        drop(outside_page); // explicit test-only disposal after refusal evidence
    }
    #[test]
    fn actual_factory_worker_panic_payload_is_retained_without_drop_under_worker_mutex() {
        struct Payload {
            drops: Arc<AtomicUsize>,
            owner: Arc<Mutex<Option<Weak<CatalogOwner>>>>,
        }
        impl Drop for Payload {
            fn drop(&mut self) {
                if let Some(owner) = lock_unpoisoned(&self.owner)
                    .as_ref()
                    .and_then(Weak::upgrade)
                {
                    assert!(
                        owner.worker.try_lock().is_ok(),
                        "payload destructor ran under worker mutex"
                    );
                }
                self.drops.fetch_add(1, Ordering::SeqCst);
            }
        }
        let drops = Arc::new(AtomicUsize::new(0));
        let owner = Arc::new(Mutex::new(None));
        WORKER_PANIC.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(Payload {
                drops: drops.clone(),
                owner: owner.clone(),
            }))
        });
        let runtime = StreamRuntime::new(StreamConfig::with_pool_pages(4), vec![]).unwrap();
        let (control, pages) = runtime.split().ok().unwrap();
        *lock_unpoisoned(&owner) = Some(Arc::downgrade(&control.owner));
        let mut disposal =
            PreparedStreamDisposal::try_prepare_on_control(&control, &pages).unwrap();
        disposal
            .begin_on_control(&mut Some((control, pages)))
            .unwrap();
        assert_eq!(
            disposal.advance_on_control(),
            Err(StreamDisposalError::Incomplete)
        );
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        let identity =
            disposal.first_panic.as_ref().unwrap().as_ref() as *const dyn Any as *const ();
        assert_eq!(
            disposal.advance_on_control(),
            Err(StreamDisposalError::Incomplete)
        );
        assert_eq!(
            disposal.first_panic.as_ref().unwrap().as_ref() as *const dyn Any as *const (),
            identity
        );
        assert!(disposal.control.is_some() && disposal.pages.is_some());
        drop(disposal.first_panic.take()); // explicit test control, mutex is absent
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
}
