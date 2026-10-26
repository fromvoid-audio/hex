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
    DeviceContractError as Error, ParameterDescriptor, ParameterId, ParameterKind,
    ParameterPresentation, ParameterSemantics, ParameterValue,
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
        read: fn(&C) -> f32,
        write: fn(&mut C, f32),
    ) -> Self {
        Self {
            descriptor,
            semantics: arrangement(descriptor),
            access: Access::Float { read, write },
        }
    }
    pub const fn boolean(
        descriptor: ParameterDescriptor,
        read: fn(&C) -> bool,
        write: fn(&mut C, bool),
    ) -> Self {
        Self {
            descriptor,
            semantics: arrangement(descriptor),
            access: Access::Bool { read, write },
        }
    }
    pub const fn choice(
        descriptor: ParameterDescriptor,
        read: fn(&C) -> u32,
        write: fn(&mut C, u32) -> Result<(), Error>,
    ) -> Self {
        Self {
            descriptor,
            semantics: arrangement(descriptor),
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
    pub fn write(&self, controls: &mut C, value: ParameterValue) -> Result<(), Error> {
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
const fn arrangement(descriptor: ParameterDescriptor) -> ParameterSemantics {
    use hex_lattice_contract::{
        ParameterAccess as A, ParameterAutomation as E, ParameterChangeImpact as I,
        ParameterModulation as M, ParameterNormalization as N, ParameterSmoothing as S,
        ParameterStepping as T,
    };
    let (normalization, stepping) = match descriptor.kind {
        ParameterKind::Bool => (N::Discrete, T::Boolean),
        ParameterKind::Choice { .. } => (N::Discrete, T::Choice),
        ParameterKind::Float { .. } => (
            match descriptor.presentation {
                ParameterPresentation::Logarithmic => N::PositiveLogarithmic,
                _ => N::Linear,
            },
            T::Continuous,
        ),
    };
    ParameterSemantics {
        access: A::ReadWrite,
        normalization,
        stepping,
        automation: E::DeviceArrangementEnvelope,
        modulation: M::Unsupported,
        smoothing: S::None,
        change_impact: I::ValueOnly,
    }
}
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
#[derive(Clone, Copy, Debug)]
pub(crate) struct Resolved<const N: usize> {
    indices: [usize; N],
    len: usize,
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Controls<C: Copy, const N: usize> {
    base: C,
    effective: C,
    admitted: [bool; N],
    last: [Option<ParameterValue>; N],
    initialized: bool,
}
impl<C: Copy + PartialEq, const N: usize> Controls<C, N> {
    pub fn new(base: C, admitted: [bool; N]) -> Self {
        Self {
            base,
            effective: base,
            admitted,
            last: [None; N],
            initialized: false,
        }
    }
    pub fn base(&self) -> C {
        self.base
    }
    pub fn effective(&self) -> C {
        self.effective
    }
    pub fn admitted(&self, index: usize) -> bool {
        self.admitted.get(index).copied().unwrap_or(false)
    }
    pub fn supports(&self, bindings: &[Binding<C>; N], id: ParameterId) -> bool {
        bindings
            .binary_search_by_key(&id, |b| b.descriptor.id)
            .is_ok_and(|i| self.admitted(i))
    }
    pub fn resolve(
        &self,
        bindings: &[Binding<C>; N],
        values: &dyn NativeParameterAutomation,
    ) -> Result<Resolved<N>, Error> {
        let ids = values.parameter_ids();
        if ids.len() > N || ids.windows(2).any(|p| p[0] >= p[1]) {
            return Err(Error::UnknownParameter);
        }
        let mut resolved = Resolved {
            indices: [0; N],
            len: ids.len(),
        };
        for (slot, &id) in resolved.indices.iter_mut().zip(ids) {
            let index = bindings
                .binary_search_by_key(&id, |b| b.descriptor.id)
                .map_err(|_| Error::UnknownParameter)?;
            if !self.admitted(index) {
                return Err(Error::UnknownParameter);
            }
            *slot = index;
        }
        Ok(resolved)
    }
    pub fn sample(
        &self,
        bindings: &[Binding<C>; N],
        resolved: &Resolved<N>,
        values: &dyn NativeParameterAutomation,
        offset: usize,
    ) -> Result<Option<Self>, Error> {
        let mut sampled = [None; N];
        values.values_at(offset, &mut sampled[..resolved.len])?;
        let mut current = [None; N];
        for (value, &index) in sampled.iter().zip(&resolved.indices).take(resolved.len) {
            *current.get_mut(index).ok_or(Error::UnknownParameter)? = *value;
        }
        if self.initialized && current == self.last {
            return Ok(None);
        }
        let mut next = *self;
        if !self.initialized {
            next.effective = self.base;
        }
        for (index, binding) in bindings.iter().enumerate() {
            if !self.initialized || current[index] != self.last[index] {
                let value = current[index].unwrap_or_else(|| binding.read(&self.base));
                binding.write(&mut next.effective, value)?;
            }
        }
        next.last = current;
        next.initialized = true;
        Ok(Some(next))
    }
    pub fn reset(&mut self) {
        *self = Self::new(self.base, self.admitted);
    }
    /// Keep candidate base/admission; same-candidate transaction copies retain
    /// the full cache. A different base forces a full reconstruction on the
    /// first sample, even if its surviving automated values are unchanged.
    pub fn inherit(&mut self, previous: &Self) {
        let compatible = self.base == previous.base && self.admitted == previous.admitted;
        self.effective = previous.effective;
        self.last = previous.last;
        self.initialized = compatible && previous.initialized;
    }
    /// Exhaustive helper-field review plus a concrete provider's complete
    /// controls review. A Copy bound alone does not prove resource ownership.
    pub fn check_inline(&self, check: fn(&C)) {
        let Self {
            base,
            effective,
            admitted,
            last,
            initialized,
        } = self;
        check(base);
        check(effective);
        let _: &[bool; N] = admitted;
        let _: &[Option<ParameterValue>; N] = last;
        let _: &bool = initialized;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let map = c.resolve(&TABLE, v)?;
        if let Some(next) = c.sample(&TABLE, &map, v, 0)? {
            *c = next;
        }
        Ok(())
    }
    #[test]
    fn staging_validation_absent_origin_subset_and_removed_targets_are_atomic() {
        let mut c = Controls::new(Pair { a: 1.0, b: 2.0 }, [true; 2]);
        apply(&mut c, &values(Some(3.0), Some(4.0))).unwrap();
        assert_eq!(c.effective(), Pair { a: 3.0, b: 4.0 });
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
                    &transaction.resolve(&TABLE, &subset).unwrap(),
                    &subset,
                    0
                )
                .unwrap()
                .is_none()
        );
        apply(&mut transaction, &values(None, None)).unwrap_err();
        assert_eq!(transaction.effective(), candidate.effective());
    }
}
