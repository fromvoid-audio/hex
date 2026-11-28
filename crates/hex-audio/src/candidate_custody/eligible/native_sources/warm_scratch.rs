//! Bounded warm `Quiescent` scratch custody for control-side VST3 base captures.
//! Retains at most one private, non-audio-callback scratch helper per Root so
//! repeated `ManualBaseCapture` and `CanonicalBasesCapture` calls reuse an
//! already-loaded helper via quiescent `restore_state` instead of cold-spawning
//! a new OS child process on every knob release or toggle click.
//!
//! Invariants:
//! - Strictly private to control: zero audio callback endpoints, zero graph
//!   references, zero `NativeWorker` threads (`Owner::Quiescent` only).
//! - Bounded to at most one warm scratch cell within the existing reserved pool.
//! - Auto-evicts (with `auto_reap`) whenever its linked original source retires,
//!   another plugin claims the warm cell, pool pressure needs a vacant cell, or
//!   the output closes.
//! - Any SDK refusal during warm reuse disables warm reuse for that original
//!   source binding so callers transparently fall back to fresh cold boots.
use super::*;
use hex_plugin_host::protocol::PresentationPlan;
use hex_plugin_state::{ClassId, Identity};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WarmScratchKey {
    pub(crate) original_binding: Binding,
    pub(crate) identity: Identity,
    pub(crate) sample_rate_bits: u64,
    pub(crate) input_channels: u8,
    pub(crate) output_channels: u8,
    pub(crate) presentation: PresentationPlan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct WarmScratchSlot {
    pub(super) key: WarmScratchKey,
    pub(super) in_use: bool,
}

impl Bank {
    pub(super) fn evict_idle_warm_scratch(
        &mut self,
        predicate: impl Fn(WarmScratchKey) -> bool,
    ) -> Option<Cancellation> {
        for slot in &mut self.slots {
            if let Some(entry) = slot.warm_scratch
                && !entry.in_use
                && predicate(entry.key)
            {
                slot.warm_scratch = None;
                slot.retiring = true;
                slot.auto_reap = true;
                return slot.request_cancellation();
            }
        }
        None
    }

    pub(super) fn evict_warm_scratch_for_original(
        &mut self,
        original: Binding,
    ) -> Option<Cancellation> {
        let mut cancellation = None;
        for slot in &mut self.slots {
            let Some(entry) = slot.warm_scratch else {
                continue;
            };
            if entry.key.original_binding != original {
                continue;
            }
            if entry.in_use {
                // Active caller still owns this slot through its terminal;
                // clearing warm_scratch prevents re-parking when it finishes.
                slot.warm_scratch = None;
            } else {
                slot.warm_scratch = None;
                slot.retiring = true;
                slot.auto_reap = true;
                if cancellation.is_none() {
                    cancellation = slot.request_cancellation();
                }
            }
        }
        cancellation
    }

    pub(super) fn disable_warm_scratch_for_original(
        &mut self,
        original: Binding,
    ) -> Option<Cancellation> {
        if let Some(index) = self.index(original) {
            self.slots[index].warm_scratch_disabled = true;
        }
        self.evict_warm_scratch_for_original(original)
    }
}

impl Custody {
    /// Checkout an idle parked warm `Quiescent` scratch helper matching `key`.
    /// Evicts any mismatched idle warm scratch so at most one warm helper
    /// occupies the pool. Returns `None` if unavailable, busy, or disabled.
    pub(in crate::candidate_custody) fn acquire_warm_scratch(
        &self,
        key: WarmScratchKey,
    ) -> Option<Binding> {
        let work = self.work.try_lock().ok()?;
        let (matched, evicted_cancel) = {
            let mut bank = lock(&self.bank);
            if !self.enabled() || bank.closing {
                return None;
            }
            let original_ok = bank.sources.index(key.original_binding).is_some_and(|idx| {
                let slot = &bank.sources.slots[idx];
                slot.phase == Phase::Accepted
                    && !slot.retiring
                    && !slot.starting
                    && !slot.warm_scratch_disabled
            });
            if !original_ok {
                return None;
            }
            let mut matched = None;
            let bank_ref = &mut *bank;
            for (index, slot) in bank_ref.sources.slots.iter_mut().enumerate() {
                let Some(entry) = slot.warm_scratch else {
                    continue;
                };
                if entry.in_use || entry.key != key {
                    continue;
                }
                let cell = &work.sources.cells[index];
                let healthy = slot.phase == Phase::Accepted
                    && !slot.retiring
                    && !slot.starting
                    && !slot.endpoint_loaned
                    && slot.endpoint_return.is_none()
                    && slot.processor.is_none()
                    && slot.presentation.is_none()
                    && !slot.cancellation_requested
                    && slot.cancellation.is_some()
                    && !bank_ref.publication.references_native_source(index)
                    && !cell.pending
                    && cell.first_panic.is_none()
                    && cell.cause().is_none()
                    && cell.cleanup_error.is_none()
                    && cell.disposal.is_none()
                    && matches!(cell.owner, Some(Owner::Quiescent(_)));
                if healthy && let Some(binding) = slot.binding {
                    slot.warm_scratch = Some(WarmScratchSlot { key, in_use: true });
                    matched = Some(binding);
                    break;
                }
            }
            let evicted = if matched.is_none() {
                bank.sources
                    .evict_idle_warm_scratch(|existing| existing != key)
            } else {
                None
            };
            (matched, evicted)
        };
        drop(work);
        if let Some(cancel) = evicted_cancel {
            cancel.cancel_on_control();
        }
        matched
    }

    /// Park a healthy, non-retiring `Quiescent` scratch slot as this Root's
    /// single warm scratch helper for `key`. Returns `false` if parking is
    /// refused (caller must retire and recover the slot normally).
    pub(in crate::candidate_custody) fn park_warm_scratch(
        &self,
        scratch: Binding,
        key: WarmScratchKey,
    ) -> bool {
        let Ok(mut work) = self.work.try_lock() else {
            return false;
        };
        let (parked, evicted_cancel) = {
            let mut bank = lock(&self.bank);
            if !self.enabled() || bank.closing || bank.sources.slots.len() < 2 {
                return false;
            }
            let original_ok = bank.sources.index(key.original_binding).is_some_and(|idx| {
                let slot = &bank.sources.slots[idx];
                slot.phase == Phase::Accepted
                    && !slot.retiring
                    && !slot.starting
                    && !slot.warm_scratch_disabled
            });
            if !original_ok {
                return false;
            }
            let Some(index) = bank.sources.index(scratch) else {
                return false;
            };
            let slot = &bank.sources.slots[index];
            let cell = &work.sources.cells[index];
            let owner_matches = match cell.owner.as_ref() {
                Some(Owner::Quiescent(owner)) => {
                    let config = owner.configuration();
                    config.identity == key.identity
                        && config.sample_rate.to_bits() == key.sample_rate_bits
                        && config.input_channels == key.input_channels
                        && config.output_channels == key.output_channels
                        && slot.facts == Some(owner.facts())
                        && owner.retained_storage_on_control().is_some()
                }
                _ => false,
            };
            let healthy = slot.phase == Phase::Accepted
                && !slot.retiring
                && !slot.starting
                && !slot.endpoint_loaned
                && slot.endpoint_return.is_none()
                && slot.processor.is_none()
                && slot.presentation.is_none()
                && !slot.cancellation_requested
                && slot.cancellation.is_some()
                && !bank.publication.references_native_source(index)
                && !cell.pending
                && cell.first_panic.is_none()
                && cell.cause().is_none()
                && cell.cleanup_error.is_none()
                && cell.disposal.is_none()
                && owner_matches;
            if !healthy {
                return false;
            }
            // Enforce at most one warm scratch helper per Root.
            let mut evicted = None;
            for (other_idx, other_slot) in bank.sources.slots.iter_mut().enumerate() {
                if other_idx == index {
                    continue;
                }
                if let Some(entry) = other_slot.warm_scratch {
                    if entry.in_use {
                        // Another warm scratch is actively checked out; keep bounded at 1.
                        return false;
                    }
                    other_slot.warm_scratch = None;
                    other_slot.retiring = true;
                    other_slot.auto_reap = true;
                    if evicted.is_none() {
                        evicted = other_slot.request_cancellation();
                    }
                }
            }
            // Parked warm scratch carries no renewal authority for the original source.
            work.sources.cells[index].renewal = None;
            bank.sources.slots[index].warm_scratch = Some(WarmScratchSlot { key, in_use: false });
            (true, evicted)
        };
        drop(work);
        if let Some(cancel) = evicted_cancel {
            cancel.cancel_on_control();
        }
        parked
    }

    pub(in crate::candidate_custody) fn disable_warm_scratch_for_source(&self, original: Binding) {
        let cancel = {
            let mut bank = lock(&self.bank);
            bank.sources.disable_warm_scratch_for_original(original)
        };
        if let Some(cancel) = cancel {
            cancel.cancel_on_control();
        }
    }

    pub(in crate::candidate_custody) fn restore_native_source_state(
        &self,
        binding: Binding,
        state: NativeDeviceState,
        is_cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<Option<(hex_plugin_host::protocol::Facts, u16, u8)>, &'static str> {
        self.with_quiescent_native_source(binding, |owner| {
            let owner = owner.restore_state_retained_while_on_control(state, is_cancelled)?;
            let facts = owner.facts();
            let quantum = owner.configuration().quantum;
            let input_channels = owner.configuration().input_channels;
            Ok((owner, (facts, quantum, input_channels)))
        })
    }

    pub(in crate::candidate_custody) fn read_quiescent_authored_metadata(
        &self,
        binding: Binding,
        class: ClassId,
        ids: &[u32],
        is_cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<Option<hex_plugin_host::native::AuthoredParameterMetadata>, &'static str> {
        self.with_quiescent_native_source(binding, |owner| {
            owner.read_authored_parameter_metadata_retained_while_on_control(
                class,
                ids,
                is_cancelled,
            )
        })
    }
}

impl CandidateRoot {
    pub(crate) fn acquire_warm_scratch_native_source(
        &self,
        key: WarmScratchKey,
    ) -> Option<Binding> {
        self.require_native_source_control_open().ok()?;
        let result = self.0.eligible.acquire_warm_scratch(key);
        crate::lifecycle::request_candidate_cleanup(self);
        result
    }

    pub(crate) fn park_warm_scratch_native_source(
        &self,
        scratch: Binding,
        key: WarmScratchKey,
    ) -> bool {
        if self.require_native_source_control_open().is_err() {
            return false;
        }
        let parked = self.0.eligible.park_warm_scratch(scratch, key);
        crate::lifecycle::request_candidate_cleanup(self);
        parked
    }

    pub(crate) fn disable_warm_scratch_native_source(&self, original: Binding) {
        self.0.eligible.disable_warm_scratch_for_source(original);
        crate::lifecycle::request_candidate_cleanup(self);
    }

    pub(crate) fn restore_native_source_state(
        &self,
        binding: Binding,
        state: NativeDeviceState,
        is_cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<Option<(hex_plugin_host::protocol::Facts, u16, u8)>, &'static str> {
        self.require_native_source_control_open()?;
        let result = self
            .0
            .eligible
            .restore_native_source_state(binding, state, is_cancelled);
        crate::lifecycle::request_candidate_cleanup(self);
        self.require_native_source_control_open()?;
        result
    }

    pub(crate) fn read_quiescent_authored_metadata(
        &self,
        binding: Binding,
        class: ClassId,
        ids: &[u32],
        is_cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<Option<hex_plugin_host::native::AuthoredParameterMetadata>, &'static str> {
        self.require_native_source_control_open()?;
        let result = self
            .0
            .eligible
            .read_quiescent_authored_metadata(binding, class, ids, is_cancelled);
        crate::lifecycle::request_candidate_cleanup(self);
        self.require_native_source_control_open()?;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_plugin_host::protocol::PresentationLatency;
    use hex_plugin_state::BinaryIdentity;

    fn sample_key(original_epoch: u64, sample_rate: f64) -> WarmScratchKey {
        WarmScratchKey {
            original_binding: Binding {
                root: 1,
                instance: 20,
                graph: 1,
                epoch: original_epoch,
            },
            identity: Identity {
                class: ClassId::new([7; 16]).unwrap(),
                binary: BinaryIdentity {
                    executable_sha256: [1; 32],
                    info_sha256: [2; 32],
                },
            },
            sample_rate_bits: sample_rate.to_bits(),
            input_channels: 0,
            output_channels: 2,
            presentation: PresentationPlan::Known(PresentationLatency {
                input_frames: 0,
                output_frames: 64,
            }),
        }
    }

    #[test]
    fn evict_for_original_retires_idle_warm_slot_and_unparks_in_use_slot() {
        let mut bank = Bank {
            slots: vec![Slot::vacant(), Slot::vacant(), Slot::vacant()],
            ..Bank::default()
        };
        let key_a = sample_key(1, 48_000.0);
        let key_b = sample_key(2, 48_000.0);
        bank.slots[0].phase = Phase::Accepted;
        bank.slots[0].warm_scratch = Some(WarmScratchSlot {
            key: key_a,
            in_use: false,
        });
        bank.slots[1].phase = Phase::Accepted;
        bank.slots[1].warm_scratch = Some(WarmScratchSlot {
            key: key_b,
            in_use: true,
        });

        let _ = bank.evict_warm_scratch_for_original(key_a.original_binding);
        assert!(bank.slots[0].warm_scratch.is_none());
        assert!(bank.slots[0].retiring);
        assert!(bank.slots[0].auto_reap);
        assert_eq!(bank.slots[1].warm_scratch.map(|entry| entry.key), Some(key_b));

        // In-use warm scratch for key_b is detached (cannot re-park) without forcing auto_reap.
        let _ = bank.evict_warm_scratch_for_original(key_b.original_binding);
        assert!(bank.slots[1].warm_scratch.is_none());
        assert!(!bank.slots[1].retiring);
        assert!(!bank.slots[1].auto_reap);
    }

    #[test]
    fn disable_marks_original_slot_and_evicts_idle_warm_scratch() {
        let key = sample_key(3, 48_000.0);
        let identity = ProcessorInstanceIdentity::new(
            hex_lattice::ProcessorChainOwner::Main,
            key.original_binding.instance,
        )
        .unwrap();
        let mut bank = Bank {
            slots: vec![Slot::vacant(), Slot::vacant()],
            ..Bank::default()
        };
        bank.slots[0].phase = Phase::Accepted;
        bank.slots[0].binding = Some(key.original_binding);
        bank.slots[0].identity = Some(identity);
        bank.slots[1].phase = Phase::Accepted;
        bank.slots[1].warm_scratch = Some(WarmScratchSlot { key, in_use: false });

        let _ = bank.disable_warm_scratch_for_original(key.original_binding);
        assert!(bank.slots[0].warm_scratch_disabled);
        assert!(bank.slots[1].warm_scratch.is_none());
        assert!(bank.slots[1].retiring);
        assert!(bank.slots[1].auto_reap);
    }

    #[test]
    fn evict_idle_warm_scratch_skips_in_use_and_targets_mismatched_key() {
        let key_a = sample_key(1, 48_000.0);
        let key_b = sample_key(1, 44_100.0);
        let mut bank = Bank {
            slots: vec![Slot::vacant(), Slot::vacant()],
            ..Bank::default()
        };
        bank.slots[0].phase = Phase::Accepted;
        bank.slots[0].warm_scratch = Some(WarmScratchSlot {
            key: key_a,
            in_use: false,
        });
        assert!(bank.evict_idle_warm_scratch(|k| k == key_b).is_none());
        assert!(bank.slots[0].warm_scratch.is_some());

        let _ = bank.evict_idle_warm_scratch(|k| k != key_b);
        assert!(bank.slots[0].warm_scratch.is_none());
        assert!(bank.slots[0].retiring);
        assert!(bank.slots[0].auto_reap);
    }

    #[test]
    fn auto_reap_complete_slot_is_reported_as_bank_work() {
        let mut bank = Bank {
            slots: vec![Slot::vacant()],
            ..Bank::default()
        };
        let pub_bank = publication::Bank::default();
        bank.slots[0].phase = Phase::Complete;
        assert!(!bank.has_work(false, &pub_bank));
        bank.slots[0].auto_reap = true;
        assert!(bank.has_work(false, &pub_bank));
    }
}
