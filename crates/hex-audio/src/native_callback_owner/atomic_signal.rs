//! Private legacy or constructor-paired AtomicU64 field. Never adopt a raw Arc.
use crate::candidate_custody::CandidateRoot;
use hex_engine::{
    Invalid,
    sampler_host_fields::{NativeAtomicSignal, NativeAtomicSignalRegistration},
};
use std::{
    ops::Deref,
    sync::{Arc, atomic::AtomicU64},
};
pub(crate) enum Signal {
    Legacy(Arc<AtomicU64>),
    Qualified(NativeAtomicSignal),
}
impl Signal {
    pub(crate) fn matches_constructor_record(
        &self,
        record: &NativeAtomicSignalRegistration,
        endpoint: usize,
    ) -> bool {
        match self {
            Self::Qualified(field) => record.matches_actual_field_on_control(field, endpoint),
            Self::Legacy(_) => false,
        }
    }
}
impl Deref for Signal {
    type Target = AtomicU64;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Legacy(value) => value,
            Self::Qualified(value) => value,
        }
    }
}
impl AsRef<AtomicU64> for Signal {
    fn as_ref(&self) -> &AtomicU64 {
        self
    }
}
pub(crate) fn pair(root: Option<&CandidateRoot>, slot: usize) -> Result<[Signal; 2], Invalid> {
    if let Some(root) = root {
        let (record, fields) = NativeAtomicSignalRegistration::fresh_pair_on_control(0)
            .map_err(|_| Invalid("atomic signal constructor pairing refused"))?;
        if !record.matches_actual_fields_on_control([&fields[0], &fields[1]]) {
            return Err(Invalid("atomic signal constructor fields differ"));
        }
        root.retain_atomic_signal(slot, record).map_err(Invalid)?;
        Ok(fields.map(Signal::Qualified))
    } else {
        let value = Arc::new(AtomicU64::new(0));
        Ok([Signal::Legacy(value.clone()), Signal::Legacy(value)])
    }
}
