//! Fixed slot signal. Never reset, no job lock or raw atomic/route escape.
use super::RetainedControlJobRoute;
use crate::{AudioRootStorageLedger, AudioStorageError, SharedMetadata};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug)]
struct Watermark {
    cancelled_through: AtomicU64,
}
/// Nonallocating borrowed query for long control-side scans. Implementing this
/// interface conveys no storage provenance, owning lifetime or admission proof.
pub trait CancellationQuery {
    fn is_cancelled(&self) -> bool;
}
// Legacy/offline callers remain query-compatible; this implementation makes
// NO owner/layout qualification for their raw AtomicBool/Arc routes.
impl CancellationQuery for std::sync::atomic::AtomicBool {
    fn is_cancelled(&self) -> bool {
        self.load(Ordering::Acquire)
    }
}
/// One constructor-paired signal belonging to an actual persistent slot, beside
/// its lock. No Clone, raw Arc ingress, atomic accessor or reset operation.
/// ```compile_fail
/// let raw = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
/// let _: hex_engine::RetainedControlCancellationSignal = raw.into();
/// ```
/// ```compile_fail
/// let signal = hex_engine::RetainedControlCancellationSignal::try_new_on_control(4096).unwrap();
/// let _raw = signal.state;
/// ```
#[derive(Debug)]
pub struct RetainedControlCancellationSignal {
    state: SharedMetadata<Watermark>,
}
/// Paired worker handle bound on control to an ACTUAL validated job route.
/// Only the audited slot adapter can establish the route/slot relationship;
/// constructing a handle is not permission to publish a production endpoint.
#[derive(Clone, Debug)]
pub struct RetainedControlCancellation {
    state: SharedMetadata<Watermark>,
    route: RetainedControlJobRoute,
}
impl RetainedControlCancellationSignal {
    pub fn allocation_quote_on_control() -> Result<usize, AudioStorageError> {
        let (body, header) = crate::owned_shared::OwnedShared::<Watermark>::declared_parts()
            .ok_or(AudioStorageError::UnsupportedConstructor)?;
        body.checked_add(header).ok_or(AudioStorageError::Overflow)
    }
    /// Component quote must be reserved together with ALL configured signals,
    /// slot backing and startup overlap under the existing joint ceiling.
    pub fn try_new_on_control(max_bytes: usize) -> Result<Self, AudioStorageError> {
        if Self::allocation_quote_on_control()? > max_bytes {
            return Err(AudioStorageError::ReservationExceeded);
        }
        Ok(Self {
            state: SharedMetadata::new_on_control(Watermark {
                cancelled_through: AtomicU64::new(0),
            }),
        })
    }
    pub fn bind_worker_on_control(
        &self,
        actual_route: RetainedControlJobRoute,
    ) -> RetainedControlCancellation {
        RetainedControlCancellation {
            state: self.state.clone(),
            route: actual_route,
        }
    }
    /// Direct atomic progress, independent of job writer/readiness/cleanup.
    /// Delayed old requests cannot clear newer cancellation or cancel a later
    /// route. A cancelled route remains cancelled even after holder consumption.
    /// Actual pairing check for the audited slot completion protocol only.
    /// Equality is not allocation provenance or an outstanding-child proof.
    pub fn matches_worker_on_control(
        &self,
        worker: &RetainedControlCancellation,
        route: RetainedControlJobRoute,
    ) -> bool {
        worker.route == route && self.state.shares_owner_with(&worker.state)
    }
    pub fn cancel(&self, actual_route: RetainedControlJobRoute) {
        self.state
            .cancelled_through
            .fetch_max(actual_route.cancellation_ordinal(), Ordering::Release);
    }
    pub fn visit_storage_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        visit(&self.state, ledger)
    }
    // ONE Engine-driven family traversal already owns mutate(). The public
    // standalone wrapper above keeps its original fail-closed behavior.
    pub(crate) fn visit_family_field_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        let Self { state } = self;
        let Watermark { cancelled_through } = state.as_ref();
        let _: &AtomicU64 = cancelled_through;
        crate::sampler_host_fields::SharedMetadataRef::of(state).observe(ledger)
    }
}
impl CancellationQuery for RetainedControlCancellation {
    fn is_cancelled(&self) -> bool {
        self.state.cancelled_through.load(Ordering::Acquire) >= self.route.cancellation_ordinal()
    }
}
impl RetainedControlCancellation {
    /// Opaque correlation for the concrete neutral adapter, not numeric state,
    /// ownership/provenance or an independently reusable publication permit.
    pub fn route_on_control(&self) -> RetainedControlJobRoute {
        self.route
    }

    pub fn visit_storage_on_control<'a>(
        &'a self,
        ledger: &mut AudioRootStorageLedger<'a>,
    ) -> Result<(), AudioStorageError> {
        visit(&self.state, ledger)
    }
}
fn visit<'a>(
    state: &'a SharedMetadata<Watermark>,
    ledger: &mut AudioRootStorageLedger<'a>,
) -> Result<(), AudioStorageError> {
    // Exhaustive fixed-leaf audit: a new owning field must update this pattern.
    let Watermark { cancelled_through } = state.as_ref();
    let _: &AtomicU64 = cancelled_through;
    ledger.borrow_shared_metadata_body_on_control(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RetainedControlJob, RetainedControlJobProgress};
    fn route() -> RetainedControlJobRoute {
        RetainedControlJob::try_from_owned_on_control(&mut Some(0_u8), usize::MAX, |_: &mut u8| {
            Ok::<_, ()>(RetainedControlJobProgress::Pending)
        })
        .unwrap()
        .route()
    }
    #[test]
    fn slot_cancel_watermark_never_resets_or_lets_old_requests_cancel_new_routes() {
        if !crate::audio_storage_constructor_layout_supported() {
            return;
        }
        eprintln!("SUPPORTED actual paired slot cancellation watermark");
        let quote = RetainedControlCancellationSignal::allocation_quote_on_control().unwrap();
        assert!(matches!(
            RetainedControlCancellationSignal::try_new_on_control(quote - 1),
            Err(AudioStorageError::ReservationExceeded)
        ));
        let signal = RetainedControlCancellationSignal::try_new_on_control(quote).unwrap();
        let old_route = route();
        let old = signal.bind_worker_on_control(old_route);
        assert!(!old.is_cancelled());
        signal.cancel(old_route);
        assert!(old.is_cancelled());
        let new_route = route();
        let new = signal.bind_worker_on_control(new_route);
        assert!(!new.is_cancelled());
        signal.cancel(old_route);
        assert!(!new.is_cancelled());
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for _ in 0..128 {
                    signal.cancel(new_route);
                }
            });
            scope.spawn(|| {
                for _ in 0..128 {
                    signal.cancel(old_route);
                }
            });
        });
        signal.cancel(old_route);
        assert!(new.is_cancelled());
        assert!(old.is_cancelled());
        let later = signal.bind_worker_on_control(route());
        assert!(!later.is_cancelled());
    }
    #[test]
    fn slot_signal_and_worker_fields_require_actual_aliases_with_one_fixed_body() {
        if !crate::audio_storage_constructor_layout_supported() {
            return;
        }
        eprintln!("SUPPORTED actual fixed signal and paired worker field accounting");
        let signal = RetainedControlCancellationSignal::try_new_on_control(usize::MAX).unwrap();
        let worker = signal.bind_worker_on_control(route());
        let transform = worker.clone();
        let limits = crate::AudioStorageLedgerReservation {
            data_rows: 16,
            handle_rows: 16,
            root_rows: 4,
            max_retained_bytes: 65_536,
            max_construction_peak_bytes: 65_536,
        };
        let collect = |limits| -> Result<_, AudioStorageError> {
            let mut ledger = AudioRootStorageLedger::try_new_on_control(limits)?;
            signal.visit_storage_on_control(&mut ledger)?;
            worker.visit_storage_on_control(&mut ledger)?;
            assert!(
                ledger
                    .validate_selected_shared_handles_on_control()
                    .is_err()
            );
            transform.visit_storage_on_control(&mut ledger)?;
            ledger.validate_selected_shared_handles_on_control()?;
            let summary = ledger.summary()?;
            assert_eq!(summary.data_allocations, 1);
            assert_eq!(
                summary.data_bytes,
                RetainedControlCancellationSignal::allocation_quote_on_control()?
            );
            signal.visit_storage_on_control(&mut ledger)?;
            assert_eq!(ledger.summary()?, summary);
            Ok(summary)
        };
        let measured = collect(limits).unwrap();
        let mut exact = limits;
        exact.max_retained_bytes = measured.retained_bytes;
        exact.max_construction_peak_bytes = measured.construction_peak_bytes;
        assert_eq!(collect(exact).unwrap(), measured);
        exact.max_retained_bytes -= 1;
        assert!(collect(exact).is_err());
    }
}
