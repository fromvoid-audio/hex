//! The existing worker stop fields; no raw Arc can become qualified by reading
//! its address/count/value. The fixed rooted constructor receives the two
//! affine fields from its actual existing NativeBank.
use super::*;
use hex_engine::{
    AudioStorageError,
    sampler_host_fields::{NativeAtomicSignal, NativeAtomicSignalRegistration},
};
pub(crate) enum SpectrumStop {
    Legacy(Arc<AtomicBool>),
    Qualified(NativeAtomicSignal),
}
impl SpectrumStop {
    pub(super) fn pair(
        parent: Option<&crate::native_callback_owner::NativeRetirement>,
    ) -> Result<[Self; 2], SpectrumWorkerError> {
        if let Some(parent) = parent {
            return parent
                .spectrum_stop_pair_on_control()
                .map(|fields| fields.map(Self::Qualified))
                .map_err(|_| SpectrumWorkerError::StorageRefused);
        }
        let value = Arc::new(AtomicBool::new(false));
        Ok([Self::Legacy(value.clone()), Self::Legacy(value)])
    }
    pub(super) fn stopped(&self) -> Result<bool, AudioStorageError> {
        match self {
            Self::Legacy(value) => Ok(value.load(Ordering::Acquire)),
            Self::Qualified(value) => value.try_load_stop_on_control(),
        }
    }
    pub(super) fn request_stop(&self) -> Result<(), AudioStorageError> {
        match self {
            Self::Legacy(value) => {
                value.store(true, Ordering::Release);
                Ok(())
            }
            Self::Qualified(value) => value.try_request_stop_on_control(),
        }
    }
    pub(super) fn identity(&self) -> Option<usize> {
        match self {
            Self::Legacy(value) => Some(Arc::as_ptr(value) as usize),
            Self::Qualified(value) => value.try_value_identity_on_control().ok(),
        }
    }
    pub(super) fn storage(&self) -> Option<crate::native_callback_owner::ChannelStorage> {
        use crate::native_callback_owner::ChannelStorage;
        match self {
            Self::Legacy(value) => Some(ChannelStorage::paired_payload(value.as_ref())),
            Self::Qualified(_) => Some(ChannelStorage {
                data_bytes: NativeAtomicSignalRegistration::requested_bytes_on_control().ok()?,
                shared_groups: 1,
                shared_handles: 5,
            }),
        }
    }
    pub(crate) fn is_legacy(&self) -> bool {
        matches!(self, Self::Legacy(_))
    }
    pub(crate) fn matches_record(
        &self,
        record: &NativeAtomicSignalRegistration,
        role: usize,
    ) -> bool {
        match self {
            Self::Qualified(value) => record.matches_actual_field_on_control(value, role),
            Self::Legacy(_) => false,
        }
    }
}

#[cfg(all(test, feature = "allocation-probe"))]
mod tests {
    use super::*;
    #[test]
    fn request_first_malformed_stop_keeps_actual_reaper_join_and_failed_custody() {
        let root = crate::candidate_custody::CandidateRoot::new(3).unwrap();
        let parent =
            crate::native_callback_owner::NativeRetirement::begin_construction(root.clone());
        let [stop, worker] = SpectrumStop::pair(Some(&parent)).unwrap();
        if let SpectrumStop::Qualified(value) = &stop {
            value.store(2, Ordering::Release); // Actual private test fault, no production ingress.
        } else {
            panic!("rooted constructor must produce qualified stop");
        }
        let join = std::thread::spawn(move || drop(worker));
        let thread = join.thread().id();
        let mut reaper = SpectrumReaper {
            stop,
            join: Some(join),
            native: None, // This fixture constructs no analyzer/Core/receipt.
            phase: ReaperPhase::Ready,
            first_panic: None,
            captured_join: None,
        };
        let measured = allocation_counter::measure(|| {
            assert!(matches!(
                reaper.advance_shutdown_on_control(),
                Err(SpectrumWorkerError::StorageRefused)
            ));
            assert!(matches!(reaper.phase, ReaperPhase::Failed));
            assert_eq!(reaper.join.as_ref().unwrap().thread().id(), thread);
            assert!(matches!(
                reaper.advance_shutdown_on_control(),
                Err(SpectrumWorkerError::StorageRefused)
            ));
            assert_eq!(reaper.join.as_ref().unwrap().thread().id(), thread);
        });
        assert_eq!(
            (
                measured.count_total,
                measured.count_current,
                measured.bytes_total,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(
            root.test_spectrum_stop_data(),
            Err(AudioStorageError::UnknownStorage)
        );
        // Test-only cleanup of the real already-bounded worker; production
        // never consumes this failed slot or treats its disappearance as join.
        reaper.join.take().unwrap().join().unwrap();
        assert!(matches!(reaper.phase, ReaperPhase::Failed));
        assert_eq!(
            root.test_spectrum_stop_data(),
            Err(AudioStorageError::UnknownStorage)
        );
    }
    #[test]
    fn actual_stop_fields_keep_constructor_roles_storage_and_nonallocating_signal() {
        let root = crate::candidate_custody::CandidateRoot::new(3).unwrap();
        let other = crate::candidate_custody::CandidateRoot::new(3).unwrap();
        let parent =
            crate::native_callback_owner::NativeRetirement::begin_construction(root.clone());
        let foreign =
            crate::native_callback_owner::NativeRetirement::begin_construction(other.clone());
        let [reaper, worker] = SpectrumStop::pair(Some(&parent)).unwrap();
        let [foreign_reaper, foreign_worker] = SpectrumStop::pair(Some(&foreign)).unwrap();
        let expected = NativeAtomicSignalRegistration::requested_bytes_on_control().unwrap();
        assert_eq!(root.test_spectrum_stop_data().unwrap(), expected);
        let measured = allocation_counter::measure(|| {
            assert_eq!(worker.stopped(), Ok(false));
            assert!(parent.matches_spectrum_stop(&reaper));
            assert!(!parent.matches_spectrum_stop(&worker));
            assert!(!parent.matches_spectrum_stop(&foreign_reaper));
            assert_eq!(reaper.identity(), worker.identity());
            assert_ne!(reaper.identity(), foreign_reaper.identity());
            reaper.request_stop().unwrap();
            reaper.request_stop().unwrap();
            assert_eq!(worker.stopped(), Ok(true));
        });
        assert_eq!(
            (
                measured.count_total,
                measured.count_current,
                measured.bytes_total,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        drop(worker);
        assert_eq!(root.test_spectrum_stop_data().unwrap(), expected);
        drop(reaper);
        assert_eq!(
            root.test_spectrum_stop_data().unwrap(),
            expected,
            "actual allocation remains rooted by registration after both fields drop"
        );
        drop((foreign_reaper, foreign_worker));
    }
}
