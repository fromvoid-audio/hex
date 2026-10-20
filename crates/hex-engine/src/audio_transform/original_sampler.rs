//! Existing original-only cache writers, retained across inspect then construct.
//! This is not a permit, arbitrary transaction callback or a general Audio path.
use super::*;
use crate::AudioStorageError;
use std::cell::RefMut;

pub struct OriginalSamplerTransaction<'a> {
    owner: &'a AudioTransformPreparation,
    cache: RefMut<'a, Cache>,
    pending: RefMut<'a, Option<Vec<Entry>>>,
    tempo: RefMut<'a, Option<Arc<hex_project::TempoMap>>>,
}
impl AudioTransformPreparation {
    /// Internal integration boundary: Session acquires its waveform writer first.
    /// Acquisition and class validation do not reserve, mutate or create owners.
    pub fn original_sampler_transaction_on_control(
        &self,
    ) -> Result<OriginalSamplerTransaction<'_>, AudioStorageError> {
        let result = OriginalSamplerTransaction {
            owner: self,
            cache: self
                .cache
                .try_borrow_mut()
                .map_err(|_| AudioStorageError::ExternalReadersPending)?,
            pending: self
                .pending
                .try_borrow_mut()
                .map_err(|_| AudioStorageError::ExternalReadersPending)?,
            tempo: self
                .tempo
                .try_borrow_mut()
                .map_err(|_| AudioStorageError::ExternalReadersPending)?,
        };
        // The SAME borrowed shape used by normal inventory validates eligibility.
        result.inventory().original_only_storage_cost()?;
        if result.pending.is_some() {
            return Err(AudioStorageError::UnknownStorage);
        }
        Ok(result)
    }
}
impl<'owner> OriginalSamplerTransaction<'owner> {
    pub(crate) fn preparation(&self) -> &'owner AudioTransformPreparation {
        self.owner
    }
    /// Borrowed physical fields under the SAME exclusive guards. A ledger using
    /// this view must end before the constructor can mutably borrow this loan.
    pub fn inventory(&self) -> AudioTransformPcmInventory<'_> {
        AudioTransformPcmInventory {
            owner: self.owner,
            cache: &self.cache,
            pending: &self.pending,
            tempo: &self.tempo,
        }
    }
    /// Only the private original-Sampler constructor calls this implementation.
    /// Its preflight has priced cache growth BEFORE this first reserve. The
    /// supported class never creates transform entries or tempo owners.
    pub(crate) fn construct<T>(
        &mut self,
        prepare: impl FnOnce(&AudioTransformPreparation) -> Result<T, Invalid>,
    ) -> Result<T, Invalid> {
        self.owner.require_usable()?;
        if self.owner.cancellation_requested_on_control() {
            return Err(Invalid("audio preparation cancelled"));
        }
        if self.pending.is_some() || !self.cache.entries.is_empty() || self.tempo.is_some() {
            return Err(Invalid("original sampler transaction class changed"));
        }
        let extra = self
            .owner
            .entry_limit
            .saturating_sub(self.cache.entries.len());
        self.cache
            .entries
            .try_reserve(extra)
            .map_err(|_| Invalid("audio cache metadata allocation failed"))?;
        *self.pending = Some(Vec::new());
        let mut failure = TransactionFailure {
            failed: &self.owner.failed,
            completed: false,
        };
        let result = prepare(self.owner);
        self.owner.require_usable()?;
        // No general transform code may insert through these held writers.
        // Keep an unexpected owner rooted and the failure sticky, never drop it
        // as though this restricted class had completed normally.
        if self
            .pending
            .as_ref()
            .is_none_or(|entries| !entries.is_empty())
        {
            return Err(Invalid(
                "original sampler transaction produced derived owners",
            ));
        }
        self.pending.take();
        failure.completed = true;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn context() -> (
        AudioTransformPreparation,
        crate::RetainedControlCancellationSignal,
        crate::RetainedControlJob,
    ) {
        let signal = crate::RetainedControlCancellationSignal::try_new_on_control(4096).unwrap();
        let job = crate::RetainedControlJob::try_from_owned_on_control(
            &mut Some(0_u8),
            usize::MAX,
            |_: &mut u8| Ok::<_, ()>(crate::RetainedControlJobProgress::Pending),
        )
        .unwrap();
        let worker = signal.bind_worker_on_control(job.route());
        (
            AudioTransformPreparation::with_retained_cancellation_on_control(worker),
            signal,
            job,
        )
    }
    #[test]
    fn original_sampler_writers_acquire_without_mutation_then_complete_same_fields() {
        let (context, _signal, _job) = context();
        let old_capacity = context.cache.borrow().entries.capacity();
        let mut acquired = None;
        let acquisition = allocation_counter::measure(|| {
            acquired = Some(context.original_sampler_transaction_on_control().unwrap());
        });
        assert_eq!((acquisition.count_total, acquisition.count_current), (0, 0));
        let mut transaction = acquired.unwrap();
        assert_eq!(transaction.cache.entries.capacity(), old_capacity);
        assert!(transaction.pending.is_none());
        assert!(transaction.tempo.is_none());
        transaction
            .inventory()
            .original_only_storage_cost()
            .unwrap();
        assert!(context.with_pcm_inventory_on_control(|_| Ok(())).is_err());
        assert!(matches!(
            context.original_sampler_transaction_on_control(),
            Err(AudioStorageError::ExternalReadersPending)
        ));
        transaction.construct(|_| Ok(())).unwrap();
        assert!(transaction.pending.is_none());
        assert!(transaction.cache.entries.capacity() >= context.entry_limit);
        transaction
            .construct(|_| Err::<(), _>(Invalid("ordinary rollback")))
            .unwrap_err();
        assert!(transaction.pending.is_none());
        assert!(!context.failed_incomplete_on_control());
        drop(transaction);
        context.transaction(|| Ok(())).unwrap();
    }
    #[test]
    fn original_sampler_busy_each_writer_and_pending_class_refuse_without_birth() {
        let (context, _signal, _job) = context();
        let refuse = || {
            let probe = allocation_counter::measure(|| {
                assert!(matches!(
                    context.original_sampler_transaction_on_control(),
                    Err(AudioStorageError::ExternalReadersPending)
                ));
            });
            assert_eq!((probe.count_total, probe.count_current), (0, 0));
            assert!(!context.failed_incomplete_on_control());
        };
        let cache = context.cache.borrow_mut();
        refuse();
        drop(cache);
        let pending = context.pending.borrow_mut();
        refuse();
        drop(pending);
        let tempo = context.tempo.borrow_mut();
        refuse();
        drop(tempo);
        *context.pending.borrow_mut() = Some(Vec::new());
        let probe = allocation_counter::measure(|| {
            assert!(matches!(
                context.original_sampler_transaction_on_control(),
                Err(AudioStorageError::UnknownStorage)
            ));
        });
        assert_eq!((probe.count_total, probe.count_current), (0, 0));
        assert!(context.pending.borrow().is_some());
        assert!(!context.failed_incomplete_on_control());
    }
    #[test]
    fn original_sampler_writer_unwind_retains_pending_and_failure() {
        let (context, _signal, _job) = context();
        let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut transaction = context.original_sampler_transaction_on_control().unwrap();
            transaction
                .construct::<()>(|_| std::panic::panic_any(91_u32))
                .unwrap();
        }))
        .unwrap_err();
        assert_eq!(payload.downcast_ref::<u32>(), Some(&91));
        assert!(context.failed_incomplete_on_control());
        assert!(context.pending.borrow().is_some());
        assert!(context.original_sampler_transaction_on_control().is_err());
    }
}
