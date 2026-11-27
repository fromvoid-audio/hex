//! Only the original joining owner can approve an isolated canonical base.
//! Callback configuration retains a nonissuing verifier, never the authority.
use super::*;
use crate::plugin_effects::manual_base_capture::CompletedManualBase;

/// Deterministic inline fingerprint of a VST3 processing payload (`state_version`,
/// byte length, FNV-1a 64-bit hash). Avoids retaining extra `NativeDeviceState`
/// clones on `Configuration`.
pub(super) type StateFingerprint = (u32, usize, u64);

pub(super) fn state_fingerprint(state: &NativeDeviceState) -> StateFingerprint {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in state.payload() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (state.state_version(), state.payload().len(), hash)
}

/// Single-point manual flush observation collected during `ManualBaseCapture`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ManualFlushObservation {
    pub(crate) id: u32,
    pub(crate) flushed_normalized: f64,
    pub(crate) before_fingerprint: StateFingerprint,
    pub(crate) after_fingerprint: StateFingerprint,
}

/// Fixed-size sorted set of armed parameter IDs observed not to persist through
/// `IComponent::getState`/`setState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct UnpersistedParameters {
    ids: [u32; MAX_FOREIGN_AUTOMATION_TARGETS],
    len: usize,
}

impl Default for UnpersistedParameters {
    fn default() -> Self {
        Self {
            ids: [0; MAX_FOREIGN_AUTOMATION_TARGETS],
            len: 0,
        }
    }
}

impl UnpersistedParameters {
    pub(super) fn as_slice(&self) -> &[u32] {
        &self.ids[..self.len]
    }

    pub(super) fn insert(&mut self, id: u32) {
        let Err(index) = self.ids[..self.len].binary_search(&id) else {
            return;
        };
        if self.len >= self.ids.len() {
            return;
        }
        self.ids.copy_within(index..self.len, index + 1);
        self.ids[index] = id;
        self.len += 1;
    }

    pub(super) fn remove(&mut self, id: u32) {
        let Ok(index) = self.ids[..self.len].binary_search(&id) else {
            return;
        };
        self.ids.copy_within(index + 1..self.len, index);
        self.len -= 1;
        self.ids[self.len] = 0;
    }

    pub(super) fn retain_in(&mut self, capabilities: &automation::SourceCapabilities) {
        let mut kept = 0;
        for index in 0..self.len {
            let id = self.ids[index];
            if capabilities
                .parameters()
                .binary_search_by_key(&id, |parameter| parameter.id)
                .is_ok()
            {
                self.ids[kept] = id;
                kept += 1;
            }
        }
        for slot in &mut self.ids[kept..self.len] {
            *slot = 0;
        }
        self.len = kept;
    }
}

#[derive(Clone, Copy, Debug)]
struct PendingManualFlush {
    id: u32,
    flushed_bits: u64,
    before_base_bits: u64,
    after_fingerprint: StateFingerprint,
}

/// Inline control-side tracker distinguishing persisted VST3 parameters from
/// transient/performance parameters whose `getState`/`setState` chunks reset
/// back to the pre-edit value.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ManualPersistenceTracker {
    unpersisted: UnpersistedParameters,
    observed_canonical: Option<(StateFingerprint, automation::CanonicalParameterBases)>,
    pending_flush: Option<PendingManualFlush>,
}

const FLUSH_DELTA_THRESHOLD: f64 = 1e-3;
const RESTORE_UNCHANGED_TOLERANCE: f64 = 1e-6;

impl ManualPersistenceTracker {
    pub(super) fn from_renewal(
        mut unpersisted: UnpersistedParameters,
        capabilities: &automation::SourceCapabilities,
    ) -> Self {
        unpersisted.retain_in(capabilities);
        Self {
            unpersisted,
            observed_canonical: None,
            pending_flush: None,
        }
    }

    pub(super) fn unpersisted(&self) -> UnpersistedParameters {
        self.unpersisted
    }

    pub(super) fn unpersisted_slice(&self) -> &[u32] {
        self.unpersisted.as_slice()
    }

    fn base_for(
        &self,
        original_state: &NativeDeviceState,
        authored: &automation::SourceCapabilities,
        fingerprint: StateFingerprint,
        id: u32,
    ) -> Option<f64> {
        let constructor = authored
            .parameters()
            .binary_search_by_key(&id, |parameter| parameter.id)
            .ok()
            .map(|index| f64::from_bits(authored.parameters()[index].normalized_base_bits))?;
        if fingerprint == state_fingerprint(original_state) {
            return Some(constructor);
        }
        if let Some((observed_fp, bases)) = &self.observed_canonical
            && *observed_fp == fingerprint
            && let Some((_, bits)) = bases
                .iter()
                .flatten()
                .find(|(candidate, _)| *candidate == id)
        {
            return Some(f64::from_bits(*bits));
        }
        Some(constructor)
    }

    pub(super) fn observe_manual_base(
        &mut self,
        original_state: &NativeDeviceState,
        authored: &automation::SourceCapabilities,
        observation: ManualFlushObservation,
    ) {
        if !observation.flushed_normalized.is_finite() {
            return;
        }
        let Some(before_base) = self.base_for(
            original_state,
            authored,
            observation.before_fingerprint,
            observation.id,
        ) else {
            return;
        };
        if observation.after_fingerprint == observation.before_fingerprint {
            self.pending_flush = None;
            if (observation.flushed_normalized - before_base).abs() > FLUSH_DELTA_THRESHOLD {
                self.unpersisted.insert(observation.id);
            }
        } else {
            self.pending_flush = Some(PendingManualFlush {
                id: observation.id,
                flushed_bits: observation.flushed_normalized.to_bits(),
                before_base_bits: before_base.to_bits(),
                after_fingerprint: observation.after_fingerprint,
            });
        }
    }

    pub(super) fn observe_canonical_bases(
        &mut self,
        authored: &automation::SourceCapabilities,
        canonical_state: &NativeDeviceState,
        capabilities: &automation::SourceCapabilities,
    ) {
        let fingerprint = state_fingerprint(canonical_state);
        self.observed_canonical = Some((fingerprint, parameter_extension::bases(capabilities)));
        let Some(pending) = self.pending_flush.take() else {
            return;
        };
        if pending.after_fingerprint != fingerprint
            || authored
                .parameters()
                .binary_search_by_key(&pending.id, |parameter| parameter.id)
                .is_err()
        {
            return;
        }
        let Some(restored) = capabilities
            .parameters()
            .binary_search_by_key(&pending.id, |parameter| parameter.id)
            .ok()
            .map(|index| f64::from_bits(capabilities.parameters()[index].normalized_base_bits))
        else {
            return;
        };
        let flushed = f64::from_bits(pending.flushed_bits);
        let before = f64::from_bits(pending.before_base_bits);
        if (flushed - before).abs() > FLUSH_DELTA_THRESHOLD
            && (restored - before).abs() <= RESTORE_UNCHANGED_TOLERANCE
        {
            self.unpersisted.insert(pending.id);
        } else if (restored - before).abs() > RESTORE_UNCHANGED_TOLERANCE {
            self.unpersisted.remove(pending.id);
        }
    }
}

fn approve(
    authority: &hex_lattice_contract::Vst3StateAuthority,
    configuration: &Configuration,
    state: &NativeDeviceState,
) -> Result<NativeDeviceState, &'static str> {
    validate_identity(configuration, state)?;
    authority
        .approve_on_control(state.clone())
        .map_err(|_| "native captured state approval refused")
}

fn validate_identity(
    configuration: &Configuration,
    state: &NativeDeviceState,
) -> Result<(), &'static str> {
    if Hosting::saved_identity_on_control(state).map_err(|_| "native captured state is invalid")?
        != Hosting::saved_identity_on_control(&configuration.state)
            .map_err(|_| "native source state is invalid")?
    {
        return Err("native captured state identity differs");
    }
    Ok(())
}

fn approve_processing(
    authority: &hex_lattice_contract::Vst3StateAuthority,
    configuration: &Configuration,
    facts: &CapturedCanonicalBases,
) -> Result<NativeDeviceState, &'static str> {
    let state = facts.canonical_state();
    validate_identity(configuration, state)?;
    if facts.binding() != configuration.binding
        || facts.prepared() != configuration.prepared
        || facts.sample_rate() != configuration.sample_rate
        || !(state.processing_eq(&configuration.state)
            || configuration
                .state_verifier
                .is_some_and(|verifier| verifier.approves(state)))
    {
        return Err("native processing approval source or profile differs");
    }
    authority
        .approve_processing_on_control(state.clone())
        .map_err(|_| "native processing approval refused")
}

impl NativeEffect {
    pub(crate) fn approve_guarded_capture_on_control(
        &self,
        fact: &crate::plugin_worker::NativeWorkerStateAfterGuardedWrite,
    ) -> Result<NativeDeviceState, &'static str> {
        if self.observe_guarded_capture_input(fact).is_none() {
            return Err("native guarded capture is not current");
        }
        approve(
            &self.state_authority,
            &self.callback.configuration,
            fact.state(),
        )
    }

    pub(crate) fn approve_sequenced_capture_on_control(
        &self,
        fact: &crate::plugin_worker::NativeWorkerStateAfterParameter,
    ) -> Result<NativeDeviceState, &'static str> {
        if !self.sequenced_capture_is_current(fact)? {
            return Err("native sequenced capture is not current");
        }
        approve(
            &self.state_authority,
            &self.callback.configuration,
            fact.state(),
        )
    }
    pub(crate) fn approve_canonical_processing_on_control(
        &mut self,
        facts: &CapturedCanonicalBases,
    ) -> Result<NativeDeviceState, &'static str> {
        let state = approve_processing(&self.state_authority, &self.callback.configuration, facts)?;
        self.callback
            .configuration
            .manual_persistence
            .observe_canonical_bases(
                &self.callback.configuration.authored,
                facts.canonical_state(),
                &facts.capabilities,
            );
        Ok(state)
    }
    pub(crate) fn approve_manual_base_on_control(
        &mut self,
        captured: &CompletedManualBase,
    ) -> Result<NativeDeviceState, &'static str> {
        let state = approve(
            &self.state_authority,
            &self.callback.configuration,
            captured.state(),
        )?;
        if let Some(observation) = captured.manual_flush() {
            self.callback
                .configuration
                .manual_persistence
                .observe_manual_base(
                    &self.callback.configuration.state,
                    &self.callback.configuration.authored,
                    observation,
                );
        }
        Ok(state)
    }
}

impl NativeEffectControl {
    pub(crate) fn approve_guarded_capture_on_control(
        &self,
        fact: &crate::plugin_worker::NativeWorkerStateAfterGuardedWrite,
    ) -> Result<NativeDeviceState, &'static str> {
        if self.observe_guarded_capture_input(fact).is_none() {
            return Err("native guarded capture is not current");
        }
        approve(&self.state_authority, &self.configuration, fact.state())
    }

    pub(crate) fn approve_sequenced_capture_on_control(
        &self,
        fact: &crate::plugin_worker::NativeWorkerStateAfterParameter,
    ) -> Result<NativeDeviceState, &'static str> {
        if !self.sequenced_capture_is_current(fact)? {
            return Err("native sequenced capture is not current");
        }
        approve(&self.state_authority, &self.configuration, fact.state())
    }
    pub(crate) fn approve_canonical_processing_on_control(
        &mut self,
        facts: &CapturedCanonicalBases,
    ) -> Result<NativeDeviceState, &'static str> {
        let state = approve_processing(&self.state_authority, &self.configuration, facts)?;
        self.configuration
            .manual_persistence
            .observe_canonical_bases(
                &self.configuration.authored,
                facts.canonical_state(),
                &facts.capabilities,
            );
        Ok(state)
    }
    pub(crate) fn approve_manual_base_on_control(
        &mut self,
        captured: &CompletedManualBase,
    ) -> Result<NativeDeviceState, &'static str> {
        let state = approve(&self.state_authority, &self.configuration, captured.state())?;
        if let Some(observation) = captured.manual_flush() {
            self.configuration.manual_persistence.observe_manual_base(
                &self.configuration.state,
                &self.configuration.authored,
                observation,
            );
        }
        Ok(state)
    }
}

impl NativeEffectReference<'_> {
    pub(crate) fn approves_processing_state_on_control(&self, state: &NativeDeviceState) -> bool {
        self.configuration()
            .state_verifier
            .is_some_and(|verifier| verifier.approves_processing(state))
    }
    pub(crate) fn approves_captured_state_on_control(&self, state: &NativeDeviceState) -> bool {
        self.configuration()
            .state_verifier
            .is_some_and(|verifier| verifier.approves(state))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(bytes: &[u8]) -> NativeDeviceState {
        hex_plugin_state::encode(hex_plugin_state::State {
            identity: hex_plugin_state::Identity {
                class: hex_plugin_state::ClassId::new([1; 16]).unwrap(),
                binary: hex_plugin_state::BinaryIdentity {
                    executable_sha256: [2; 32],
                    info_sha256: [3; 32],
                },
            },
            component: Some(bytes),
            controller: Some(bytes),
        })
        .unwrap()
    }

    fn caps(entries: &[(u32, f64)]) -> automation::SourceCapabilities {
        let class = hex_plugin_state::ClassId::new([1; 16]).unwrap();
        let mut values = [None; MAX_FOREIGN_AUTOMATION_TARGETS];
        for (slot, &(id, value)) in values.iter_mut().zip(entries) {
            *slot = Some((id, value.to_bits()));
        }
        automation::SourceCapabilities::from_canonical(
            class,
            &values,
            &[0; MAX_FOREIGN_AUTOMATION_TARGETS],
        )
        .unwrap()
    }

    #[test]
    fn marks_unpersisted_when_state_is_unchanged_or_restores_pre_edit_base() {
        let original = state(b"init");
        let dirty_only = state(b"dirty-flag");
        let persisted_out = state(b"output=1.0");
        let authored = caps(&[(0, 0.5), (1, 0.0), (10, 0.5)]);
        let mut tracker = ManualPersistenceTracker::default();

        // Case 1: Zebra3 first edit to param 1 (0.0 -> 0.72) flips a dirty byte,
        // then CanonicalBasesCapture restores param 1 back to 0.0 -> mark unpersisted.
        tracker.observe_manual_base(
            &original,
            &authored,
            ManualFlushObservation {
                id: 1,
                flushed_normalized: 0.72,
                before_fingerprint: state_fingerprint(&original),
                after_fingerprint: state_fingerprint(&dirty_only),
            },
        );
        assert!(tracker.unpersisted_slice().is_empty());
        tracker.observe_canonical_bases(
            &authored,
            &dirty_only,
            &caps(&[(0, 0.5), (1, 0.0), (10, 0.5)]),
        );
        assert_eq!(tracker.unpersisted_slice(), &[1]);

        // Case 2: Zero-dirty-byte plugin (or Zebra3 second edit) leaves state bytes
        // identical when flushing param 10 (0.5 -> 0.9) -> immediately mark unpersisted.
        tracker.observe_manual_base(
            &original,
            &authored,
            ManualFlushObservation {
                id: 10,
                flushed_normalized: 0.9,
                before_fingerprint: state_fingerprint(&dirty_only),
                after_fingerprint: state_fingerprint(&dirty_only),
            },
        );
        assert_eq!(tracker.unpersisted_slice(), &[1, 10]);

        // Case 3: Persisted param 0 (0.5 -> 1.0) moves restored base -> remains persisted.
        tracker.observe_manual_base(
            &original,
            &authored,
            ManualFlushObservation {
                id: 0,
                flushed_normalized: 1.0,
                before_fingerprint: state_fingerprint(&dirty_only),
                after_fingerprint: state_fingerprint(&persisted_out),
            },
        );
        tracker.observe_canonical_bases(
            &authored,
            &persisted_out,
            &caps(&[(0, 1.0), (1, 0.0), (10, 0.5)]),
        );
        assert_eq!(tracker.unpersisted_slice(), &[1, 10]);

        // Case 4: Slightly quantized persisted param (1.0 -> 0.7177 rounded to 0.72)
        // still moves away from 1.0 -> remains persisted.
        let quantized = state(b"output=0.72");
        tracker.observe_manual_base(
            &original,
            &authored,
            ManualFlushObservation {
                id: 0,
                flushed_normalized: 0.7177,
                before_fingerprint: state_fingerprint(&persisted_out),
                after_fingerprint: state_fingerprint(&quantized),
            },
        );
        tracker.observe_canonical_bases(
            &authored,
            &quantized,
            &caps(&[(0, 0.72), (1, 0.0), (10, 0.5)]),
        );
        assert_eq!(tracker.unpersisted_slice(), &[1, 10]);

        // Case 5: No-op flush (e.g. stepped param rounded back to before_base) does not
        // falsely mark a persisted parameter as unpersisted.
        tracker.observe_manual_base(
            &original,
            &authored,
            ManualFlushObservation {
                id: 0,
                flushed_normalized: 0.5,
                before_fingerprint: state_fingerprint(&original),
                after_fingerprint: state_fingerprint(&original),
            },
        );
        assert_eq!(tracker.unpersisted_slice(), &[1, 10]);

        // Case 6: Renewal retains only IDs present in the renewed capability set.
        let renewed = ManualPersistenceTracker::from_renewal(
            tracker.unpersisted(),
            &caps(&[(0, 0.5), (1, 0.0)]),
        );
        assert_eq!(renewed.unpersisted_slice(), &[1]);
    }
}
