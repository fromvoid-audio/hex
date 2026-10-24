//! Unique native endpoints use the existing output publication/return carrier.
//! Root keeps the original worker; a request/signal is never endpoint return.
use super::*;
use crate::plugin_effects::NativeEffectCallback;

pub(crate) struct NativeEndpointEnvelope(Box<Endpoint>);
struct Endpoint {
    slot: usize,
    binding: Binding,
    callback: Option<NativeEffectCallback>,
    signal: Arc<AtomicBool>,
}
impl NativeEndpointEnvelope {
    fn index_for(&self, root: u64) -> Option<usize> {
        (self.0.binding.root == root && self.0.slot < MAX_SOURCES && self.0.callback.is_some())
            .then_some(self.0.slot)
    }
    fn return_requested(&self) -> bool {
        self.0.signal.load(Ordering::Acquire)
    }
}
/// Fixed pointer cells, allocated on control before callback exposure. Installing
/// or returning an endpoint moves only its Box; no callback destructor occurs.
pub(crate) struct NativeEndpointBank {
    entries: Vec<Option<NativeEndpointEnvelope>>,
}
impl NativeEndpointBank {
    pub(crate) fn new() -> Result<Self, &'static str> {
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(MAX_SOURCES)
            .map_err(|_| "native endpoint bank allocation refused")?;
        if entries.capacity() != MAX_SOURCES {
            return Err("native endpoint bank capacity differs");
        }
        entries.resize_with(MAX_SOURCES, || None);
        Ok(Self { entries })
    }
    pub(crate) fn data_quote() -> Option<usize> {
        MAX_SOURCES.checked_mul(std::mem::size_of::<Option<NativeEndpointEnvelope>>())
    }
    pub(crate) fn data_bytes(&self) -> Option<usize> {
        self.entries
            .capacity()
            .checked_mul(std::mem::size_of::<Option<NativeEndpointEnvelope>>())
    }
    pub(crate) fn install(
        &mut self,
        root: u64,
        endpoint: NativeEndpointEnvelope,
    ) -> Result<(), NativeEndpointEnvelope> {
        let Some(index) = endpoint.index_for(root) else {
            return Err(endpoint);
        };
        let Some(destination) = self.entries.get_mut(index) else {
            return Err(endpoint);
        };
        if destination.is_some() {
            return Err(endpoint);
        }
        *destination = Some(endpoint);
        Ok(())
    }
    pub(crate) fn take_requested(&mut self) -> Option<NativeEndpointEnvelope> {
        self.entries
            .iter_mut()
            .find(|entry| {
                entry
                    .as_ref()
                    .is_some_and(NativeEndpointEnvelope::return_requested)
            })?
            .take()
    }
    pub(crate) fn has_endpoints(&self) -> bool {
        self.entries.iter().any(Option::is_some)
    }
    pub(crate) fn extract(
        &mut self,
        root: u64,
        destination: &mut [Option<NativeEndpointEnvelope>; MAX_SOURCES],
    ) -> bool {
        if destination.iter().any(Option::is_some)
            || self.entries.iter().enumerate().any(|(index, entry)| {
                entry
                    .as_ref()
                    .is_some_and(|e| e.index_for(root) != Some(index))
            })
        {
            return false;
        }
        for (source, target) in self.entries.iter_mut().zip(destination) {
            *target = source.take();
        }
        true
    }
}
impl Custody {
    pub(in crate::candidate_custody) fn loan_native_endpoint(
        &self,
        binding: Binding,
    ) -> Result<NativeEndpointEnvelope, &'static str> {
        let mut work = self
            .work
            .try_lock()
            .map_err(|_| "native preparation is busy")?;
        let index = {
            let mut bank = lock(&self.bank);
            if bank.closing {
                return Err("native preparation is closing");
            }
            let index = bank.sources.index(binding).ok_or("native source differs")?;
            let limits = bank
                .sources
                .limits
                .ok_or("native source data is not reserved")?;
            let slot = &mut bank.sources.slots[index];
            if slot.phase != Phase::Accepted
                || slot.retiring
                || slot.starting
                || slot.endpoint_loaned
            {
                return Err("native endpoint is not available");
            }
            let Some(Owner::Effect(effect)) = work.sources.cells[index].owner.as_ref() else {
                return Err("native effect source owner differs");
            };
            if effect.binding() != binding || slot.identity != Some(effect.identity()) {
                return Err("native effect source binding differs");
            }
            let bytes = effect
                .constructor_storage_on_control()
                .and_then(|s| s.data_bytes())
                .and_then(|n| {
                    n.checked_add(std::mem::size_of::<Endpoint>())?
                        .checked_add(std::mem::size_of::<AtomicBool>())
                })
                .and_then(|n| n.checked_add(hex_plugin_host::NATIVE_DIAGNOSTIC_DATA_BYTES))
                .ok_or("native endpoint data overflow")?;
            if bytes > limits.max_retained_data_bytes {
                return Err("native endpoint data reservation exceeded");
            }
            if slot.cancellation.is_none() {
                return Err("native cancellation owner missing");
            }
            slot.starting = true;
            index
        };
        let cell = &mut work.sources.cells[index];
        cell.pending = true;
        // Allocate before extracting the unique pair. No ordinary bank barrier
        // spans allocation, construction or later endpoint destruction.
        let signal = Arc::new(AtomicBool::new(false));
        let mut envelope = NativeEndpointEnvelope(Box::new(Endpoint {
            slot: index,
            binding,
            callback: None,
            signal: Arc::clone(&signal),
        }));
        let outcome = catch_unwind(AssertUnwindSafe(|| match cell.owner.take() {
            Some(Owner::Effect(effect)) => match effect.loan_on_control() {
                Ok((control, callback)) => {
                    cell.owner = Some(Owner::Loaned(control));
                    envelope.0.callback = Some(callback);
                    true
                }
                Err(effect) => {
                    cell.owner = Some(Owner::Effect(effect));
                    false
                }
            },
            other => {
                cell.owner = other;
                false
            }
        }));
        let Ok(loaned) = outcome else {
            cell.first_panic = outcome.err();
            self.fail_native_source(index);
            return Err("native endpoint construction failed");
        };
        cell.pending = false;
        let mut bank = lock(&self.bank);
        let slot = &mut bank.sources.slots[index];
        slot.starting = false;
        slot.endpoint_loaned = loaned;
        if loaned {
            if slot.retiring {
                signal.store(true, Ordering::Release);
            }
            slot.endpoint_return = Some(signal);
        }
        if !loaned {
            return Err("native effect pair is unavailable");
        }
        Ok(envelope)
    }
    pub(in crate::candidate_custody) fn ingest_native_endpoint(
        &self,
        mut endpoint: NativeEndpointEnvelope,
    ) -> Result<(), NativeEndpointEnvelope> {
        let Ok(mut work) = self.work.try_lock() else {
            return Err(endpoint);
        };
        let mut bank = lock(&self.bank);
        let Some(index) = endpoint.index_for(bank.root) else {
            return Err(endpoint);
        };
        let Some(limits) = bank.sources.limits else {
            return Err(endpoint);
        };
        let Some(slot) = bank.sources.slots.get_mut(index) else {
            return Err(endpoint);
        };
        let Some(cell) = work.sources.cells.get_mut(index) else {
            return Err(endpoint);
        };
        let Some(Owner::Loaned(control)) = cell.owner.as_ref() else {
            return Err(endpoint);
        };
        let backing = control
            .storage()
            .data_bytes()
            .and_then(|n| n.checked_add(std::mem::size_of::<Endpoint>()))
            .and_then(|n| n.checked_add(std::mem::size_of::<AtomicBool>()))
            .and_then(|n| n.checked_add(hex_plugin_host::NATIVE_DIAGNOSTIC_DATA_BYTES));
        if slot.binding != Some(endpoint.0.binding)
            || slot.processor != Some(control.prepared())
            || backing.is_none_or(|bytes| bytes > limits.max_retained_data_bytes)
            || !slot.endpoint_loaned
            || cell.pending
            || !slot
                .endpoint_return
                .as_ref()
                .is_some_and(|signal| Arc::ptr_eq(signal, &endpoint.0.signal))
            || !endpoint
                .0
                .callback
                .as_ref()
                .is_some_and(|callback| control.matches_callback(callback))
        {
            return Err(endpoint);
        }
        // Every refusal above leaves both actual owners untouched. Reuniting is
        // control-only; secure the original pair before dropping the empty Box.
        let Some(callback) = endpoint.0.callback.take() else {
            return Err(endpoint);
        };
        match cell.owner.take() {
            Some(Owner::Loaned(control)) => match control.reunite(callback) {
                Ok(effect) => cell.owner = Some(Owner::Effect(effect)),
                Err((control, callback)) => {
                    cell.owner = Some(Owner::Loaned(control));
                    endpoint.0.callback = Some(callback);
                    return Err(endpoint);
                }
            },
            other => {
                cell.owner = other;
                endpoint.0.callback = Some(callback);
                return Err(endpoint);
            }
        }
        slot.endpoint_loaned = false;
        let signal = slot.endpoint_return.take();
        drop(bank);
        drop(signal);
        drop(endpoint);
        Ok(())
    }
}
impl CandidateRoot {
    pub(crate) fn loan_native_endpoint(
        &self,
        binding: Binding,
    ) -> Result<NativeEndpointEnvelope, &'static str> {
        self.require_native_source_control_open()?;
        self.0.eligible.loan_native_endpoint(binding)
    }
    pub(crate) fn ingest_native_endpoint(
        &self,
        endpoint: NativeEndpointEnvelope,
    ) -> Result<(), NativeEndpointEnvelope> {
        let result = self.0.eligible.ingest_native_endpoint(endpoint);
        crate::lifecycle::request_candidate_cleanup(self);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{dispose, root, source_reservation, start};
    use super::*;
    fn ready(root: &Custody) -> Binding {
        root.configure_native_sources(1, source_reservation())
            .unwrap();
        let binding = start(root);
        root.complete_native_source(
            binding,
            Some(hex_plugin_host::protocol::PresentationLatency {
                input_frames: 77,
                output_frames: 91,
            }),
        )
        .unwrap();
        root.start_native_effect(binding).unwrap();
        binding
    }
    #[test]
    #[ignore = "explicit source-bound native instance and original SDK fixtures required"]
    fn equal_binding_and_state_cannot_return_another_original_endpoint() {
        let first = root();
        let second = root();
        let a = ready(&first);
        let b = ready(&second);
        // These isolated banks deliberately share their fixture root numbers;
        // real output Roots use unique IDs. Even a complete scalar collision
        // and equal captured state cannot substitute a different actual pair.
        assert_eq!(a, b);
        assert_eq!(
            first.native_source_prepared_state(a),
            second.native_source_prepared_state(b)
        );
        let endpoint_a = first.loan_native_endpoint(a).unwrap();
        let endpoint_b = second.loan_native_endpoint(b).unwrap();
        let endpoint_a = second
            .ingest_native_endpoint(endpoint_a)
            .expect_err("different original source");
        assert!(second.ingest_native_endpoint(endpoint_b).is_ok());
        assert!(first.ingest_native_endpoint(endpoint_a).is_ok());
        for owner in [&first, &second] {
            assert!(owner.retire_native_source(a));
            dispose(owner, a);
            let returned = owner.recover_native_source(a).unwrap();
            assert!(matches!(returned.disposal, PreparationDisposal::Reaped(_)));
            assert!(returned.error.is_none() && returned.cleanup_error.is_none());
        }
    }
}
