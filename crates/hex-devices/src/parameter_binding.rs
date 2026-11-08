//! Crate-private typed parameter bindings and fixed-inline effective controls.
//! One table owns metadata/read/write for a provider. Values are final canonical
//! controls; a future host modulation composer uses the same seam without
//! per-device source fields or premature modulation capability flags.
//! The callback resolves IDs
//! once per block, samples all lanes together, and applies only changed typed
//! values. Updates are staged: callers install the returned Copy candidate only
//! after their concrete DSP coefficient preparation succeeds. No DSP, envelope,
//! resource custody or smoothing owner is introduced here.
use hex_lattice::NativeParameterAutomation;
use hex_lattice_contract::{
    DeviceContractError as Error, ParameterAccess, ParameterDescriptor, ParameterId,
    ParameterSemantics, ParameterValue,
};
pub(crate) struct Binding<C> {
    pub descriptor: ParameterDescriptor,
    pub semantics: ParameterSemantics,
    access: Access<C>,
}
enum Access<C> {
    Float {
        read: fn(&C) -> f32,
        write: fn(&mut C, f32),
    },
    Bool {
        read: fn(&C) -> bool,
        write: fn(&mut C, bool),
    },
    Choice {
        read: fn(&C) -> u32,
        write: fn(&mut C, u32) -> Result<(), Error>,
    },
}
impl<C> Binding<C> {
    pub const fn float(
        descriptor: ParameterDescriptor,
        semantics: ParameterSemantics,
        read: fn(&C) -> f32,
        write: fn(&mut C, f32),
    ) -> Self {
        Self {
            descriptor,
            semantics,
            access: Access::Float { read, write },
        }
    }
    pub const fn boolean(
        descriptor: ParameterDescriptor,
        semantics: ParameterSemantics,
        read: fn(&C) -> bool,
        write: fn(&mut C, bool),
    ) -> Self {
        Self {
            descriptor,
            semantics,
            access: Access::Bool { read, write },
        }
    }
    pub const fn choice(
        descriptor: ParameterDescriptor,
        semantics: ParameterSemantics,
        read: fn(&C) -> u32,
        write: fn(&mut C, u32) -> Result<(), Error>,
    ) -> Self {
        Self {
            descriptor,
            semantics,
            access: Access::Choice { read, write },
        }
    }
    pub fn read(&self, controls: &C) -> ParameterValue {
        match self.access {
            Access::Float { read, .. } => ParameterValue::Float(read(controls)),
            Access::Bool { read, .. } => ParameterValue::Bool(read(controls)),
            Access::Choice { read, .. } => ParameterValue::Choice(read(controls)),
        }
    }
    /// Declaration only: family runtime admission is a separate decision.
    pub fn declares_arrangement_automation(&self) -> bool {
        self.semantics.access == ParameterAccess::ReadWrite
            && self.semantics.automation
                == hex_lattice_contract::ParameterAutomation::DeviceArrangementEnvelope
    }
    pub fn write(&self, controls: &mut C, value: ParameterValue) -> Result<(), Error> {
        if self.semantics.access == ParameterAccess::ReadOnly {
            return Err(Error::InvalidParameterValue);
        }
        self.descriptor.validate(value)?;
        match (&self.access, value) {
            (Access::Float { write, .. }, ParameterValue::Float(v)) => write(controls, v),
            (Access::Bool { write, .. }, ParameterValue::Bool(v)) => write(controls, v),
            (Access::Choice { write, .. }, ParameterValue::Choice(v)) => write(controls, v)?,
            _ => return Err(Error::InvalidParameterValue),
        }
        Ok(())
    }
}
/// Explicit declarations for the two currently implemented direct envelope
/// families. Callers select each binding's declaration; presentation and table
/// membership never grant a capability. Modulation remains unsupported.
pub(crate) const SCALAR_ENVELOPE: ParameterSemantics = ParameterSemantics {
    access: ParameterAccess::ReadWrite,
    normalization: hex_lattice_contract::ParameterNormalization::Linear,
    stepping: hex_lattice_contract::ParameterStepping::Continuous,
    automation: hex_lattice_contract::ParameterAutomation::DeviceArrangementEnvelope,
    modulation: hex_lattice_contract::ParameterModulation::Unsupported,
    smoothing: hex_lattice_contract::ParameterSmoothing::None,
    change_impact: hex_lattice_contract::ParameterChangeImpact::ValueOnly,
};
pub(crate) const BOOLEAN_ENVELOPE: ParameterSemantics = ParameterSemantics {
    normalization: hex_lattice_contract::ParameterNormalization::Discrete,
    stepping: hex_lattice_contract::ParameterStepping::Boolean,
    ..SCALAR_ENVELOPE
};
pub(crate) const CHOICE_ENVELOPE: ParameterSemantics = ParameterSemantics {
    normalization: hex_lattice_contract::ParameterNormalization::Discrete,
    stepping: hex_lattice_contract::ParameterStepping::Choice,
    ..SCALAR_ENVELOPE
};
pub(crate) const fn descriptors<C, const N: usize>(
    bindings: &[Binding<C>; N],
    seed: ParameterDescriptor,
) -> [ParameterDescriptor; N] {
    let mut result = [seed; N];
    let mut index = 0;
    while index < N {
        result[index] = bindings[index].descriptor;
        index += 1;
    }
    result
}
pub(crate) fn binding<C>(bindings: &[Binding<C>], id: ParameterId) -> Result<&Binding<C>, Error> {
    let index = bindings
        .binary_search_by_key(&id, |b| b.descriptor.id)
        .map_err(|_| Error::UnknownParameter)?;
    bindings.get(index).ok_or(Error::UnknownParameter)
}
const PRESENCE_WORDS: usize =
    hex_lattice_contract::MAX_PARAMETERS_PER_DEVICE.div_ceil(u64::BITS as usize);
#[derive(Clone, Copy, Debug)]
struct Slot {
    index: usize,
    word: usize,
    bit: u64,
}
/// One block's admitted index map and reusable bulk scratch. This is stack
/// staging, never an envelope cursor or retained producer/program ownership.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Resolved<const N: usize> {
    slots: [Slot; N],
    sampled: [Option<ParameterValue>; N],
    present: [u64; PRESENCE_WORDS],
    len: usize,
}
/// A derived configuration may refuse before this staged value/cache is
/// installed. Borrowing the block scratch keeps its sampled values stable until
/// install, without copying immutable base/admission or the full runtime cache.
#[derive(Debug)]
pub(crate) struct Pending<'a, C: Copy, const N: usize> {
    effective: C,
    resolved: &'a Resolved<N>,
}
impl<C: Copy, const N: usize> Pending<'_, C, N> {
    pub fn effective(&self) -> C {
        *self.effective_ref()
    }
    /// Borrow the effective value without copying the complete control set.
    /// This view neither installs a candidate nor changes cache custody.
    #[inline]
    pub fn effective_ref(&self) -> &C {
        &self.effective
    }
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Controls<C: Copy, const N: usize> {
    base: C,
    effective: C,
    admitted: [bool; N],
    last: [Option<ParameterValue>; N],
    present: [u64; PRESENCE_WORDS],
    initialized: bool,
}
impl<C: Copy + PartialEq, const N: usize> Controls<C, N> {
    pub fn new(base: C, admitted: [bool; N]) -> Self {
        Self {
            base,
            effective: base,
            admitted,
            last: [None; N],
            present: [0; PRESENCE_WORDS],
            initialized: false,
        }
    }
    pub fn base(&self) -> C {
        self.base
    }
    pub fn effective(&self) -> C {
        *self.effective_ref()
    }
    /// Borrow the effective value without copying the complete control set.
    /// This view neither installs a candidate nor changes cache custody.
    #[inline]
    pub fn effective_ref(&self) -> &C {
        &self.effective
    }
    /// Presence from the last installed bulk sample, distinct from the
    /// canonical fallback. This never re-queries or advances a producer.
    pub fn has_value(&self, index: usize) -> bool {
        index < N
            && self
                .present
                .get(index / (u64::BITS as usize))
                .is_some_and(|word| word & (1_u64 << (index % (u64::BITS as usize))) != 0)
    }
    pub fn admitted(&self, index: usize) -> bool {
        self.admitted.get(index).copied().unwrap_or(false)
    }
    pub fn supports(&self, bindings: &[Binding<C>; N], id: ParameterId) -> bool {
        bindings
            .binary_search_by_key(&id, |b| b.descriptor.id)
            .is_ok_and(|i| {
                self.admitted(i) && bindings[i].semantics.access == ParameterAccess::ReadWrite
            })
    }
    pub fn resolve(
        &self,
        bindings: &[Binding<C>; N],
        values: &dyn NativeParameterAutomation,
    ) -> Result<Resolved<N>, Error> {
        // Presence follows the existing native parameter limit, not a new
        // per-family cap. Sparse work scans only this family's actual words.
        if N > hex_lattice_contract::MAX_PARAMETERS_PER_DEVICE {
            return Err(Error::RuntimeBudgetExceeded);
        }
        let ids = values.parameter_ids();
        if ids.len() > N || ids.windows(2).any(|p| p[0] >= p[1]) {
            return Err(Error::UnknownParameter);
        }
        let mut resolved = Resolved {
            slots: [Slot {
                index: 0,
                word: 0,
                bit: 0,
            }; N],
            sampled: [None; N],
            present: [0; PRESENCE_WORDS],
            len: ids.len(),
        };
        for (slot, &id) in resolved.slots.iter_mut().zip(ids) {
            let index = bindings
                .binary_search_by_key(&id, |b| b.descriptor.id)
                .map_err(|_| Error::UnknownParameter)?;
            if !self.admitted(index) {
                return Err(Error::UnknownParameter);
            }
            if bindings[index].semantics.access == ParameterAccess::ReadOnly {
                return Err(Error::InvalidParameterValue);
            }
            let shift = u32::try_from(index % (u64::BITS as usize))
                .map_err(|_| Error::RuntimeBudgetExceeded)?;
            *slot = Slot {
                index,
                word: index / (u64::BITS as usize),
                bit: 1u64
                    .checked_shl(shift)
                    .ok_or(Error::RuntimeBudgetExceeded)?,
            };
        }
        Ok(resolved)
    }
    pub fn sample<'a>(
        &self,
        bindings: &[Binding<C>; N],
        resolved: &'a mut Resolved<N>,
        values: &dyn NativeParameterAutomation,
        offset: usize,
    ) -> Result<Option<Pending<'a, C, N>>, Error> {
        values.values_at(offset, &mut resolved.sampled[..resolved.len])?;
        let words = N.div_ceil(u64::BITS as usize);
        resolved.present[..words].fill(0);
        let mut changed = false;
        for (slot, &value) in resolved.slots[..resolved.len]
            .iter()
            .zip(&resolved.sampled[..resolved.len])
        {
            if value.is_some() {
                resolved.present[slot.word] |= slot.bit;
            }
            changed |= value != self.last[slot.index];
        }
        let removed = self.present[..words]
            .iter()
            .zip(&resolved.present[..words])
            .any(|(&old, &now)| old & !now != 0);
        if self.initialized && !changed && !removed {
            return Ok(None);
        }
        let mut effective = if self.initialized {
            self.effective
        } else {
            self.base
        };
        for (slot, &value) in resolved.slots[..resolved.len]
            .iter()
            .zip(&resolved.sampled[..resolved.len])
        {
            if let Some(value) = value
                && (!self.initialized || Some(value) != self.last[slot.index])
            {
                bindings[slot.index].write(&mut effective, value)?;
            }
        }
        if self.initialized {
            for word in 0..words {
                let mut removed = self.present[word] & !resolved.present[word];
                while removed != 0 {
                    let index = word * (u64::BITS as usize) + removed.trailing_zeros() as usize;
                    removed &= removed - 1;
                    bindings[index].write(&mut effective, bindings[index].read(&self.base))?;
                }
            }
        }
        Ok(Some(Pending {
            effective,
            resolved,
        }))
    }
    pub fn install(&mut self, pending: Pending<'_, C, N>) {
        let words = N.div_ceil(u64::BITS as usize);
        for word in 0..words {
            let mut removed = self.present[word] & !pending.resolved.present[word];
            while removed != 0 {
                let index = word * (u64::BITS as usize) + removed.trailing_zeros() as usize;
                removed &= removed - 1;
                self.last[index] = None;
            }
            self.present[word] = pending.resolved.present[word];
        }
        for (slot, &value) in pending.resolved.slots[..pending.resolved.len]
            .iter()
            .zip(&pending.resolved.sampled[..pending.resolved.len])
        {
            self.last[slot.index] = value;
        }
        self.effective = pending.effective;
        self.initialized = true;
    }
    pub fn reset(&mut self) {
        *self = Self::new(self.base, self.admitted);
    }
    pub fn inherit(&mut self, previous: &Self) {
        let compatible = self.base == previous.base && self.admitted == previous.admitted;
        self.effective = previous.effective;
        self.last = previous.last;
        self.present = previous.present;
        self.initialized = compatible && previous.initialized;
    }
    /// Exhaustive fields plus the concrete provider's controls proof. Copy
    /// alone never certifies that a newly introduced field has no resource.
    pub fn check_inline(&self, check: fn(&C)) {
        let Self {
            base,
            effective,
            admitted,
            last,
            present,
            initialized,
        } = self;
        check(base);
        check(effective);
        let _: &[bool; N] = admitted;
        let _: &[Option<ParameterValue>; N] = last;
        let _: &[u64; PRESENCE_WORDS] = present;
        let _: &bool = initialized;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::{ParameterKind, ParameterPresentation};
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Pair {
        a: f32,
        b: f32,
    }
    const TABLE: [Binding<Pair>; 2] = [
        Binding::float(
            ParameterDescriptor {
                id: ParameterId::from_static(1),
                label: "a",
                kind: ParameterKind::Float {
                    min: 0.0,
                    max: 10.0,
                    unit: None,
                },
                default: ParameterValue::Float(1.0),
                presentation: ParameterPresentation::Linear,
            },
            SCALAR_ENVELOPE,
            |c| c.a,
            |c, v| c.a = v,
        ),
        Binding::float(
            ParameterDescriptor {
                id: ParameterId::from_static(2),
                label: "b",
                kind: ParameterKind::Float {
                    min: 0.0,
                    max: 10.0,
                    unit: None,
                },
                default: ParameterValue::Float(1.0),
                presentation: ParameterPresentation::Linear,
            },
            SCALAR_ENVELOPE,
            |c| c.b,
            |c, v| c.b = v,
        ),
    ];
    struct Values {
        ids: Vec<ParameterId>,
        values: Vec<Option<ParameterValue>>,
    }
    impl NativeParameterAutomation for Values {
        fn parameter_ids(&self) -> &[ParameterId] {
            &self.ids
        }
        fn value_at(&self, id: ParameterId, _: usize) -> Option<ParameterValue> {
            self.ids
                .iter()
                .position(|x| *x == id)
                .and_then(|i| self.values[i])
        }
    }
    fn values(a: Option<f32>, b: Option<f32>) -> Values {
        Values {
            ids: vec![ParameterId::from_static(1), ParameterId::from_static(2)],
            values: vec![a.map(ParameterValue::Float), b.map(ParameterValue::Float)],
        }
    }
    fn apply(c: &mut Controls<Pair, 2>, v: &Values) -> Result<(), Error> {
        let mut map = c.resolve(&TABLE, v)?;
        if let Some(next) = c.sample(&TABLE, &mut map, v, 0)? {
            c.install(next);
        }
        Ok(())
    }
    #[test]
    fn staging_validation_absent_origin_subset_and_removed_targets_are_atomic() {
        let mut c = Controls::new(Pair { a: 1.0, b: 2.0 }, [true; 2]);
        apply(&mut c, &values(Some(3.0), Some(4.0))).unwrap();
        assert_eq!(c.effective(), Pair { a: 3.0, b: 4.0 });
        // A consumer can inspect and refuse a staged candidate while the
        // installed cache remains unchanged, then admit a later candidate.
        let v = values(Some(5.0), Some(6.0));
        let mut map = c.resolve(&TABLE, &v).unwrap();
        let next = c.sample(&TABLE, &mut map, &v, 0).unwrap().unwrap();
        assert_eq!(*next.effective_ref(), Pair { a: 5.0, b: 6.0 });
        assert_eq!(*c.effective_ref(), Pair { a: 3.0, b: 4.0 });
        drop(next);
        assert_eq!(*c.effective_ref(), Pair { a: 3.0, b: 4.0 });
        let next = c.sample(&TABLE, &mut map, &v, 0).unwrap().unwrap();
        c.install(next);
        assert_eq!(*c.effective_ref(), Pair { a: 5.0, b: 6.0 });
        apply(&mut c, &values(Some(3.0), Some(4.0))).unwrap();
        assert!(apply(&mut c, &values(Some(5.0), Some(f32::NAN))).is_err());
        assert_eq!(c.effective(), Pair { a: 3.0, b: 4.0 });
        let subset = Values {
            ids: vec![ParameterId::from_static(1)],
            values: vec![Some(ParameterValue::Float(3.0))],
        };
        apply(&mut c, &subset).unwrap();
        assert_eq!(c.effective(), Pair { a: 3.0, b: 2.0 });
        apply(&mut c, &values(None, None)).unwrap();
        assert_eq!(c.effective(), Pair { a: 1.0, b: 2.0 });
        let bad = Values {
            ids: vec![ParameterId::from_static(3)],
            values: vec![None],
        };
        assert!(c.resolve(&TABLE, &bad).is_err());
    }
    #[test]
    fn inheritance_retains_new_base_and_admission_even_with_unchanged_automated_values() {
        let mut old = Controls::new(Pair { a: 1.0, b: 2.0 }, [true; 2]);
        apply(&mut old, &values(Some(3.0), None)).unwrap();
        let mut candidate = Controls::new(Pair { a: 1.0, b: 7.0 }, [true, false]);
        candidate.inherit(&old);
        let subset = Values {
            ids: vec![ParameterId::from_static(1)],
            values: vec![Some(ParameterValue::Float(3.0))],
        };
        apply(&mut candidate, &subset).unwrap();
        assert_eq!(candidate.effective(), Pair { a: 3.0, b: 7.0 });
        assert!(!candidate.admitted(1));
        let mut transaction = Controls::new(Pair { a: 1.0, b: 7.0 }, [true, false]);
        transaction.inherit(&candidate);
        assert!(
            transaction
                .sample(
                    &TABLE,
                    &mut transaction.resolve(&TABLE, &subset).unwrap(),
                    &subset,
                    0
                )
                .unwrap()
                .is_none()
        );
        apply(&mut transaction, &values(None, None)).unwrap_err();
        assert_eq!(transaction.effective(), candidate.effective());
    }
    #[test]
    fn declarations_are_explicit_and_readonly_is_never_written_or_admitted() {
        use hex_lattice_contract::{ParameterAutomation as A, ParameterChangeImpact as I};
        let readonly = Binding::float(
            TABLE[0].descriptor,
            ParameterSemantics {
                access: ParameterAccess::ReadOnly,
                automation: A::Unsupported,
                ..SCALAR_ENVELOPE
            },
            |c: &Pair| c.a,
            |c, v| c.a = v,
        );
        readonly.semantics.validate(readonly.descriptor).unwrap();
        assert!(!readonly.declares_arrangement_automation());
        let mut base = Pair { a: 1.0, b: 2.0 };
        assert!(
            readonly
                .write(&mut base, ParameterValue::Float(3.0))
                .is_err()
        );
        assert_eq!(base, Pair { a: 1.0, b: 2.0 });
        let table = [readonly];
        let controls = Controls::new(base, [true]);
        let input = Values {
            ids: vec![ParameterId::from_static(1)],
            values: vec![Some(ParameterValue::Float(3.0))],
        };
        assert!(!controls.supports(&table, input.ids[0]));
        assert!(controls.resolve(&table, &input).is_err());
        // A missing writable target does not attempt to rewrite read-only base
        // fields during initial effective-state installation.
        let empty = Values {
            ids: Vec::new(),
            values: Vec::new(),
        };
        let mut map = controls.resolve(&table, &empty).unwrap();
        assert_eq!(
            controls
                .sample(&table, &mut map, &empty, 0)
                .unwrap()
                .unwrap()
                .effective(),
            base
        );
        for impact in [I::ValueOnly, I::Structural] {
            let semantics = ParameterSemantics {
                automation: A::Unsupported,
                change_impact: impact,
                ..SCALAR_ENVELOPE
            };
            let binding = Binding::float(
                TABLE[0].descriptor,
                semantics,
                |c: &Pair| c.a,
                |c, v| c.a = v,
            );
            assert_eq!(binding.semantics, semantics);
            assert!(!binding.declares_arrangement_automation());
            // Final-value delivery is producer-neutral. Only the actual family
            // may admit this target; the generic writer never grants envelopes.
            let table = [binding];
            let controls = Controls::new(base, [true]);
            let mut map = controls.resolve(&table, &input).unwrap();
            assert_eq!(
                controls
                    .sample(&table, &mut map, &input, 0)
                    .unwrap()
                    .unwrap()
                    .effective(),
                Pair { a: 3.0, b: 2.0 }
            );
        }
    }
    #[test]
    fn explicit_logarithmic_mapping_does_not_follow_linear_presentation() {
        use hex_lattice_contract::ParameterNormalization;
        let descriptor = ParameterDescriptor {
            kind: ParameterKind::Float {
                min: 1.0,
                max: 9.0,
                unit: None,
            },
            ..TABLE[0].descriptor
        };
        let semantics = ParameterSemantics {
            normalization: ParameterNormalization::PositiveLogarithmic,
            ..SCALAR_ENVELOPE
        };
        let binding = Binding::float(descriptor, semantics, |c: &Pair| c.a, |c, v| c.a = v);
        assert_eq!(
            binding.descriptor.presentation,
            ParameterPresentation::Linear
        );
        assert_eq!(binding.semantics, semantics);
        binding.semantics.validate(binding.descriptor).unwrap();
        assert_eq!(
            binding
                .semantics
                .normalization
                .denormalize(descriptor, 0.5)
                .unwrap(),
            ParameterValue::Float(3.0)
        );
    }
    #[test]
    fn sparse_restore_crosses_presence_words_without_a_new_parameter_cap() {
        type Read = fn(&Pair) -> f32;
        type Write = fn(&mut Pair, f32);
        let table: [Binding<Pair>; 70] = std::array::from_fn(|index| {
            let (read, write): (Read, Write) = if index == 0 {
                (|c| c.b, |c, v| c.b = v)
            } else {
                (|c| c.a, |c, v| c.a = v)
            };
            Binding::float(
                ParameterDescriptor {
                    id: ParameterId::new(u32::try_from(index + 1).unwrap()).unwrap(),
                    ..TABLE[0].descriptor
                },
                SCALAR_ENVELOPE,
                read,
                write,
            )
        });
        let mut c = Controls::new(Pair { a: 1.0, b: 2.0 }, [true; 70]);
        for (ids, values, expected) in [
            (
                vec![1, 66],
                vec![
                    Some(ParameterValue::Float(3.0)),
                    Some(ParameterValue::Float(4.0)),
                ],
                Pair { a: 4.0, b: 3.0 },
            ),
            (
                vec![66],
                vec![Some(ParameterValue::Float(4.0))],
                Pair { a: 4.0, b: 2.0 },
            ),
            (vec![], vec![], Pair { a: 1.0, b: 2.0 }),
        ] {
            let input = Values {
                ids: ids
                    .into_iter()
                    .map(|id| ParameterId::new(id).unwrap())
                    .collect(),
                values,
            };
            let mut map = c.resolve(&table, &input).unwrap();
            let pending = c.sample(&table, &mut map, &input, 0).unwrap().unwrap();
            c.install(pending);
            assert_eq!(c.effective(), expected);
            assert_eq!(
                c.has_value(0),
                input.ids.contains(&ParameterId::from_static(1))
            );
            assert_eq!(
                c.has_value(65),
                input.ids.contains(&ParameterId::from_static(66))
            );
            assert!(!c.has_value(70));
            assert!(!c.has_value(usize::MAX));
        }
    }
}
