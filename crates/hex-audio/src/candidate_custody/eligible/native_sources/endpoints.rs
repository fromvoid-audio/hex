//! Unique native endpoints use the existing output publication/return carrier.
//! Root keeps the original worker; a request/signal is never endpoint return.
use super::*;
use crate::plugin_effects::{NativeEffectCycle, NativeEffectEntry};
mod rendering;

pub(crate) struct NativeEndpointEnvelope(Box<Endpoint>);
struct Endpoint {
    slot: usize,
    binding: Binding,
    callback: Option<NativeEffectEntry>,
    signal: Arc<EndpointSignal>,
}
impl NativeEndpointEnvelope {
    pub(super) fn loan_data_bytes() -> Option<usize> {
        std::mem::size_of::<Endpoint>().checked_add(std::mem::size_of::<EndpointSignal>())
    }
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
    // Sorted scalar indexes into the same fixed owner cells, not another bank.
    routing: [usize; MAX_SOURCES],
    routing_len: usize,
    cycle: NativeEffectCycle,
    // An exact stopped-runtime return or the last actual requested endpoint
    // return establishes the empty frontier. Consumed only by the first fresh
    // live constructor-paired endpoint after every old endpoint has returned.
    epoch_returned: bool,
    ordinary_requirements: bool,
}
impl NativeEndpointBank {
    pub(crate) fn take_accepted_manual_parameter(
        &mut self,
        binding: Binding,
    ) -> Option<hex_plugin_host::transport::AcceptedManualParameter> {
        self.entries
            .iter_mut()
            .flatten()
            .find(|entry| entry.0.binding == binding)?
            .0
            .callback
            .as_mut()?
            .take_accepted_manual_parameter()
    }
    /// Headless control-side progress only. Compare each original installed
    /// endpoint's committed request prefix with its matching actual worker;
    /// source swaps, idle sources and new epochs have independent prefixes.
    /// None includes control contention or a source no longer queryable.
    #[cfg(feature = "allocation-probe")]
    pub(crate) fn workers_idle_on_control(&self, root: &CandidateRoot) -> Option<bool> {
        for endpoint in self.entries.iter().flatten() {
            let callback = endpoint.0.callback.as_ref()?;
            let quantum = u64::try_from(callback.quantum_frames()).ok()?;
            let requested = callback.committed_frames_for_test().checked_div(quantum)?;
            if root.native_source_completed_blocks(endpoint.0.binding)? < requested {
                return Some(false);
            }
        }
        Some(true)
    }
    #[cfg(feature = "allocation-probe")]
    pub(crate) fn single_worker_phase_for_test(&self) -> Option<usize> {
        let mut endpoints = self.entries.iter().flatten();
        let callback = endpoints.next()?.0.callback.as_ref()?;
        if endpoints.next().is_some() {
            return None;
        }
        Some(
            (callback.committed_frames_for_test()
                % u64::try_from(callback.quantum_frames()).ok()?) as usize,
        )
    }
    pub(crate) fn runtime_transports_are_live(
        &self,
        playback: &hex_engine::playback::Playback,
    ) -> bool {
        if self.cycle.revoked() {
            return false;
        }
        let mut live = true;
        playback.visit_foreign_runtime_bindings(&mut |binding| {
            let Some(binding) = binding else {
                live = false;
                return;
            };
            match self.routing[..self.routing_len].binary_search_by_key(&Some(binding), |slot| {
                self.entries[*slot].as_ref().map(|entry| entry.0.binding)
            }) {
                Ok(position) => {
                    live &= self.entries[self.routing[position]]
                        .as_ref()
                        .and_then(|entry| entry.0.callback.as_ref())
                        .is_some_and(NativeEffectEntry::transport_is_live);
                }
                Err(_) => live = false,
            }
        });
        live
    }
    pub(crate) fn ordinary_runtime_requires_sources(&self) -> bool {
        self.ordinary_requirements
            && self
                .entries
                .iter()
                .flatten()
                .any(|entry| entry.0.signal.runtime_required.load(Ordering::Acquire))
    }
    pub(crate) fn refresh_runtime_requirements(
        &mut self,
        playback: Option<&hex_engine::playback::Playback>,
    ) {
        if self.routing_len == 0 || (playback.is_none() && !self.ordinary_requirements) {
            return;
        }
        self.ordinary_requirements = playback.is_some();
        let mut required = [false; MAX_SOURCES];
        let mut matched = true;
        if let Some(playback) = playback {
            playback.visit_foreign_runtime_bindings(&mut |binding| {
                let Some(binding) = binding else {
                    matched = false;
                    return;
                };
                match self.routing[..self.routing_len]
                    .binary_search_by_key(&Some(binding), |slot| {
                        self.entries[*slot].as_ref().map(|entry| entry.0.binding)
                    }) {
                    Ok(position) => required[self.routing[position]] = true,
                    Err(_) => matched = false,
                }
            });
        }
        // Unknown runtime facts fail closed. Never release an original source
        // because an unmatched node could not be mapped to its actual endpoint.
        for (index, endpoint) in self.entries.iter().enumerate() {
            if let Some(endpoint) = endpoint {
                endpoint
                    .0
                    .signal
                    .runtime_required
                    .store(required[index] || !matched, Ordering::Release);
            }
        }
    }
    pub(crate) fn new() -> Result<Self, &'static str> {
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(MAX_SOURCES)
            .map_err(|_| "native endpoint bank allocation refused")?;
        if entries.capacity() != MAX_SOURCES {
            return Err("native endpoint bank capacity differs");
        }
        entries.resize_with(MAX_SOURCES, || None);
        Ok(Self {
            entries,
            routing: [0; MAX_SOURCES],
            routing_len: 0,
            cycle: NativeEffectCycle::default(),
            epoch_returned: false,
            ordinary_requirements: false,
        })
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
        if self.cycle.active()
            || !endpoint
                .0
                .callback
                .as_ref()
                .is_some_and(NativeEffectEntry::has_live_epoch)
            || (self.cycle.revoked() && (!self.epoch_returned || self.routing_len != 0))
            || self.entries.get(index).is_none_or(Option::is_some)
            || self.routing_len == MAX_SOURCES
        {
            return Err(endpoint);
        }
        let position = match self.routing[..self.routing_len]
            .binary_search_by_key(&Some(endpoint.0.binding), |slot| {
                self.entries[*slot].as_ref().map(|e| e.0.binding)
            }) {
            Ok(_) => return Err(endpoint),
            Err(position) => position,
        };
        self.routing
            .copy_within(position..self.routing_len, position + 1);
        self.routing[position] = index;
        self.routing_len += 1;
        if self.cycle.revoked() {
            // This is a new cohort built from actual returned predecessors and
            // a live constructor pair, never reuse of a revoked old transport.
            self.cycle = NativeEffectCycle::default();
        }
        self.epoch_returned = false;
        self.entries[index] = Some(endpoint);
        if let Some(endpoint) = self.entries[index].as_ref() {
            endpoint.0.signal.installed.store(true, Ordering::Release);
        }
        Ok(())
    }
    pub(crate) fn take_requested(&mut self, index: usize) -> Option<NativeEndpointEnvelope> {
        if self.cycle.active() {
            return None;
        }
        let entry = self.entries.get_mut(index)?;
        if entry
            .as_ref()
            .is_some_and(NativeEndpointEnvelope::return_requested)
        {
            let position = self.routing[..self.routing_len]
                .iter()
                .position(|slot| *slot == index)?;
            self.routing
                .copy_within(position + 1..self.routing_len, position);
            self.routing_len -= 1;
            if let Some(endpoint) = entry.as_ref() {
                endpoint.0.signal.installed.store(false, Ordering::Release);
            }
            let returned = entry.take();
            if self.routing_len == 0 && self.cycle.revoked() {
                // Stop can win after endpoint installation but before any
                // graph uses the cohort. Requests come from the exact stopped
                // runtime return or Root's unreferenced-source retirement.
                // Their last unique callback return is the empty frontier;
                // installation still requires a fresh live original pair.
                self.epoch_returned = true;
            }
            returned
        } else {
            None
        }
    }
    pub(crate) fn has_endpoints(&self) -> bool {
        self.routing_len != 0
    }
    pub(crate) fn request_epoch_return(&mut self) {
        self.reject_rendering();
        for endpoint in self.entries.iter().flatten() {
            endpoint.0.signal.store(true, Ordering::Release);
        }
        self.epoch_returned = true;
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
        // Terminal extraction occurs only after callback/backend quiescence.
        // A lost host completion must revoke staged work before endpoint return.
        if self.cycle.active() {
            self.reject_rendering();
        }
        for (source, target) in self.entries.iter_mut().zip(destination) {
            if let Some(endpoint) = source.as_ref() {
                endpoint.0.signal.installed.store(false, Ordering::Release);
            }
            *target = source.take();
        }
        self.routing_len = 0;
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
                        .checked_add(std::mem::size_of::<EndpointSignal>())
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
        let signal = Arc::new(EndpointSignal::new());
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
                    envelope.0.callback = Some(NativeEffectEntry::new(callback));
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
            .and_then(|n| n.checked_add(std::mem::size_of::<EndpointSignal>()))
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
                .is_some_and(|callback| control.matches_callback(callback.callback()))
        {
            return Err(endpoint);
        }
        // Every refusal above leaves both actual owners untouched. Reuniting is
        // control-only; secure the original pair before dropping the empty Box.
        let Some(callback) = endpoint.0.callback.take() else {
            return Err(endpoint);
        };
        match cell.owner.take() {
            Some(Owner::Loaned(control)) => match control.reunite(callback.into_callback()) {
                Ok(effect) => cell.owner = Some(Owner::Effect(effect)),
                Err((control, callback)) => {
                    cell.owner = Some(Owner::Loaned(control));
                    endpoint.0.callback = Some(NativeEffectEntry::new(callback));
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
    fn actual_root_edit_receipts_follow_original_loan_and_terminal() {
        use hex_lattice::{ForeignProcessorNode, ForeignProcessors};
        use hex_plugin_host::protocol::ParameterEditKind;
        use std::time::{Duration, Instant};
        if std::env::var_os("HEX_ROOT_EDIT_CHILD").is_none() {
            let dir = std::env::temp_dir().join(format!("hex-root-edits-{}", std::process::id()));
            std::fs::create_dir(&dir).unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    std::thread::current().name().unwrap(),
                    "--ignored",
                    "--nocapture",
                ])
                .env("HEX_ROOT_EDIT_CHILD", "1")
                .env("HEX_ORIGINAL_INSTANCE_VARIANT", "85")
                .env("HEX_ORIGINAL_INSTANCE_TRACE", dir.join("trace"))
                .env("HEX_ORIGINAL_INSTANCE_RELEASE", dir.join("release"))
                .output()
                .unwrap();
            std::fs::remove_dir_all(dir).unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
            return;
        }
        let root = root();
        let binding = ready(&root);
        let confirmed = root.native_source_prepared_state(binding).unwrap();
        let (identity, prepared, configuration) = {
            let work = lock(&root.work);
            let Some(Owner::Effect(owner)) = work.sources.cells[0].owner.as_ref() else {
                panic!("actual effect")
            };
            (
                owner.identity(),
                owner.prepared(),
                owner.configuration().clone(),
            )
        };
        let loan = root.loan_native_endpoint(binding).unwrap();
        let mut bank = NativeEndpointBank::new().unwrap();
        assert!(bank.install(binding.root, loan).is_ok());
        assert!(
            root.native_parameter_edits(Binding {
                epoch: binding.epoch + 1,
                ..binding
            })
            .is_err()
        );
        {
            let _work = lock(&root.work);
            assert!(matches!(
                root.native_parameter_edits(binding),
                Err("native preparation is busy")
            ));
        }
        std::fs::write(
            std::env::var_os("HEX_ORIGINAL_INSTANCE_RELEASE").unwrap(),
            b"release",
        )
        .unwrap();
        let mut edits = Vec::new();
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            match root.native_parameter_edits(binding) {
                Ok(Some(batch)) => {
                    edits.extend_from_slice(batch.as_slice());
                    if batch.ready > 0 && edits.len() == 1 {
                        break;
                    }
                }
                Ok(None) | Err("native edit observation is busy") => {}
                Err(error) => panic!("original edit observation: {error}"),
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
        assert_eq!(edits.len(), 1);
        let node = ForeignProcessorNode {
            owner: identity.owner(),
            device_id: identity.device_id(),
            device_type: configuration.type_id(),
            sample_rate: 48_000,
            configuration: &configuration,
            prepared,
            automation: None,
            bypassed: false,
            input_gain: 1.0,
            output_gain: 1.0,
        };
        let mut frames = [[0.5, -0.25]; 8];
        bank.begin_rendering(8).unwrap();
        bank.process_segment(node, 0, &mut frames, None).unwrap();
        bank.finish_rendering(true).unwrap();
        assert_eq!(frames, [[0.0; 2]; 8]); // declared pipeline warmup
        let until = Instant::now() + Duration::from_secs(3);
        while edits.len() < 3 {
            match root.native_parameter_edits(binding) {
                Ok(Some(batch)) => edits.extend_from_slice(batch.as_slice()),
                Ok(None) | Err("native edit observation is busy") => {}
                Err(error) => panic!("original applied observation: {error}"),
            }
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
        assert_eq!(
            edits
                .iter()
                .map(|edit| (edit.sequence, edit.gesture, edit.id, edit.kind))
                .collect::<Vec<_>>(),
            [
                (1, 1, 7, ParameterEditKind::Begin),
                (2, 1, 7, ParameterEditKind::Perform),
                (3, 1, 7, ParameterEditKind::End)
            ]
        );
        assert_eq!((edits[1].normalized, edits[1].first_frame), (0.75, 0));
        assert_eq!(root.native_source_prepared_state(binding), Some(confirmed));
        bank.request_epoch_return();
        assert!(
            root.ingest_native_endpoint(bank.take_requested(0).unwrap())
                .is_ok()
        );
        root.close();
        assert!(root.native_parameter_edits(binding).is_err());
        dispose(&root, binding);
        let returned = root.recover_native_source(binding).unwrap();
        assert!(returned.error.is_none() && returned.cleanup_error.is_none());
        assert!(
            matches!(returned.disposal, PreparationDisposal::Reaped(retired) if retired.status.success() && retired.terminal.is_some_and(|terminal| terminal.blocks == 1 && terminal.frames == 8))
        );
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
        let copied = first.native_source_metadata(a).unwrap();
        let endpoint_a = first.loan_native_endpoint(a).unwrap();
        let endpoint_b = second.loan_native_endpoint(b).unwrap();
        assert_eq!(
            copied,
            first.native_source_metadata(a).unwrap(),
            "metadata stays with original control custody while its actual endpoint is loaned"
        );
        let endpoint_a = second
            .ingest_native_endpoint(endpoint_a)
            .expect_err("different original source");
        assert!(second.ingest_native_endpoint(endpoint_b).is_ok());
        assert!(first.ingest_native_endpoint(endpoint_a).is_ok());
        assert_eq!(copied, first.native_source_metadata(a).unwrap());
        for owner in [&first, &second] {
            assert!(owner.retire_native_source(a));
            dispose(owner, a);
            let returned = owner.recover_native_source(a).unwrap();
            assert!(matches!(returned.disposal, PreparationDisposal::Reaped(_)));
            assert!(returned.error.is_none() && returned.cleanup_error.is_none());
        }
    }
    #[test]
    #[ignore = "explicit source-bound native instance and original SDK fixtures required"]
    fn actual_root_parameter_segments_and_feedback_keep_original_loan_and_final_acceptance() {
        use hex_lattice::{ForeignProcessorNode, ForeignProcessors};
        use hex_lattice_contract::ForeignParameterPoint;
        use std::time::{Duration, Instant};
        if std::env::var_os("HEX_ROOT_GRAPH_PARAMETERS").is_none() {
            let name = std::thread::current().name().unwrap().to_owned();
            for variant in ["37", "80"] {
                let result = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        &name,
                        "--ignored",
                        "--test-threads=1",
                        "--nocapture",
                    ])
                    .env("HEX_ROOT_GRAPH_PARAMETERS", variant)
                    .env("HEX_ORIGINAL_INSTANCE_VARIANT", variant)
                    .output()
                    .unwrap();
                assert!(
                    result.status.success(),
                    "{}\n{}",
                    String::from_utf8_lossy(&result.stdout),
                    String::from_utf8_lossy(&result.stderr)
                );
                assert!(String::from_utf8_lossy(&result.stdout).contains("1 passed"));
            }
            return;
        }
        let parameter_id = if std::env::var("HEX_ROOT_GRAPH_PARAMETERS").unwrap() == "80" {
            0
        } else {
            7
        };
        let root = root();
        let binding = ready(&root);
        let (identity, prepared, configuration) = {
            let work = lock(&root.work);
            let Some(Owner::Effect(owner)) = work.sources.cells[0].owner.as_ref() else {
                panic!("actual effect")
            };
            (
                owner.identity(),
                owner.prepared(),
                owner.configuration().clone(),
            )
        };
        assert!(
            root.native_parameter_values(binding)
                .unwrap()
                .as_slice()
                .is_empty()
        );
        let loan = root.loan_native_endpoint(binding).unwrap();
        assert!(
            root.native_parameter_values(binding)
                .unwrap()
                .as_slice()
                .is_empty()
        );
        let mut bank = NativeEndpointBank::new().unwrap();
        assert!(bank.install(binding.root, loan).is_ok());
        let node = ForeignProcessorNode {
            owner: identity.owner(),
            device_id: identity.device_id(),
            device_type: configuration.type_id(),
            sample_rate: 48_000,
            configuration: &configuration,
            prepared,
            automation: None,
            bypassed: false,
            input_gain: 1.0,
            output_gain: 1.0,
        };
        for block in 0..4_u64 {
            let mut frames = [[0.5, -0.25]; 8];
            let points = [ForeignParameterPoint {
                frame: 2,
                id: parameter_id,
                normalized: (block + 1) as f64 / 8.0,
            }];
            let render = || {
                bank.begin_rendering(8).unwrap();
                bank.process_segment_with_parameters(node, 0, &mut frames[..3], &points, None)
                    .unwrap();
                bank.process_segment_with_parameters(node, 3, &mut frames[3..], &[], None)
                    .unwrap();
                bank.finish_rendering(true).unwrap();
            };
            #[cfg(feature = "allocation-probe")]
            {
                let measured = allocation_counter::measure(render);
                assert_eq!(
                    (
                        measured.count_total,
                        measured.bytes_total,
                        measured.count_current,
                        measured.bytes_current
                    ),
                    (0, 0, 0, 0)
                );
            }
            #[cfg(not(feature = "allocation-probe"))]
            {
                let mut render = render;
                render();
            }
            for (frame, actual) in frames.iter().enumerate() {
                let normalized = if block < 2 {
                    0.0
                } else {
                    let source = block - 2;
                    let end = (source + 1) as f64 / 8.0;
                    let before = if source == 0 {
                        0.25
                    } else {
                        source as f64 / 8.0
                    };
                    before + (end - before) * (frame + 1).min(3) as f64 / 3.0
                };
                let gain = normalized * 4.0 * (0.5 + block.saturating_sub(2) as f64 * 0.125);
                for (channel, sample) in actual.iter().enumerate() {
                    let input = if channel == 0 { 0.5 } else { -0.25 };
                    assert!(
                        (f64::from(*sample) - input * gain).abs() < 1e-6,
                        "block{block}/frame{frame}/channel{channel}"
                    );
                }
            }
            let until = Instant::now() + Duration::from_secs(2);
            loop {
                if let Ok(values) = root.native_parameter_values(binding)
                    && values.completed_blocks() == block + 1
                {
                    assert_eq!(
                        values.as_slice(),
                        &[crate::plugin_worker::NativeParameterValue {
                            id: parameter_id,
                            normalized: (block + 1) as f64 / 8.0
                        }]
                    );
                    break;
                }
                assert!(Instant::now() < until, "actual original worker feedback");
                std::thread::yield_now();
            }
        }
        assert!(
            root.native_parameter_values(Binding {
                epoch: binding.epoch + 1,
                ..binding
            })
            .is_err()
        );
        {
            let _work = lock(&root.work);
            assert!(matches!(
                root.native_parameter_values(binding),
                Err("native preparation is busy")
            ));
        }
        bank.begin_rendering(8).unwrap();
        let mut refused = [[0.5, -0.25]; 8];
        bank.process_segment_with_parameters(
            node,
            0,
            &mut refused,
            &[ForeignParameterPoint {
                frame: 0,
                id: parameter_id,
                normalized: 0.875,
            }],
            None,
        )
        .unwrap();
        assert!(bank.finish_rendering(false).is_err());
        assert!(
            root.native_parameter_values(binding).is_err(),
            "rejected epoch cannot provide live observations"
        );
        bank.request_epoch_return();
        let returned = bank.take_requested(0).unwrap();
        assert!(root.ingest_native_endpoint(returned).is_ok());
        assert!(root.native_parameter_values(binding).is_err());
        assert!(root.retire_native_source(binding));
        dispose(&root, binding);
        let returned = root.recover_native_source(binding).unwrap();
        assert!(returned.error.is_none() && returned.cleanup_error.is_none());
        assert!(
            matches!(returned.disposal,PreparationDisposal::Reaped(retired) if retired.status.success() && retired.terminal.is_some_and(|terminal| terminal.blocks==4 && terminal.frames==32))
        );
    }
}
