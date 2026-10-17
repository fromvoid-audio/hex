//! Checked inline-only encoding; not a provider or runtime capability.
use super::super::{adapter::Root, layout::*};
use hex_lattice::{
    AnalyticContinuation, AnalyticContinuationFormat, AnalyticContinuationId,
    AnalyticParameterFrame,
};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::operator_state::capture_proof) struct Transition {
    pub target: AnalyticContinuationId,
    pub levels: [u64; 4],
    pub start: u64,
    pub duration: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::operator_state::capture_proof) struct Binding {
    pub law: Key,
    pub transition: Option<Transition>,
}
impl Binding {
    pub fn encode(binding: EnableBinding, format: AnalyticContinuationFormat) -> Option<Self> {
        let expected =
            AnalyticContinuationFormat::new(crate::operator_state::OPERATOR_TYPE_ID, 2).ok()?;
        if format != expected {
            return None;
        }
        let transition = match binding.transition {
            None => None,
            Some(t) => {
                if t.start().words()[4..].iter().any(|word| *word != 0) {
                    return None;
                }
                Some(Transition {
                    target: t.target(),
                    levels: std::array::from_fn(|i| t.start().words()[i]),
                    start: t.start_frame(),
                    duration: t.duration(),
                })
            }
        };
        Some(Self {
            law: binding.law,
            transition,
        })
    }
    pub fn decode(self) -> Option<EnableBinding> {
        let transition = match self.transition {
            None => None,
            Some(t) => {
                let mut words = [0; 8];
                words[..4].copy_from_slice(&t.levels);
                let frame = AnalyticParameterFrame::new(t.target, words);
                Some(AnalyticContinuation::new(t.target, frame, t.start, t.duration).ok()?)
            }
        };
        Some(EnableBinding {
            law: self.law,
            transition,
        })
    }
    /// The unchanged provider validates values and independent target/owner.
    pub fn validated(binding: EnableBinding, root: &Root, at: u64) -> Option<Self> {
        root.levels(binding, at)?;
        Self::encode(binding, root.context.analytic_continuation()?.format())
    }
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(in crate::operator_state::capture_proof) struct CompactRecord {
    pub bank: Bank,
    pub levels: [f64; 4],
    pub receipt: Receipt,
    pub harmonic_clock: Clock,
    pub enable: Binding,
    pub recipe: u64,
    pub format: u32,
    pub reserved: u32,
}
impl PartialEq for CompactRecord {
    fn eq(&self, other: &Self) -> bool {
        self.bank == other.bank
            && self.levels.map(f64::to_bits) == other.levels.map(f64::to_bits)
            && self.receipt == other.receipt
            && self.harmonic_clock == other.harmonic_clock
            && self.enable == other.enable
            && self.recipe == other.recipe
            && self.format == other.format
            && self.reserved == other.reserved
    }
}
impl CompactRecord {
    pub fn encode(r: Record, root: &Root) -> Option<Self> {
        Some(Self {
            bank: r.bank,
            levels: r.levels,
            receipt: r.receipt,
            harmonic_clock: r.harmonic_clock,
            enable: Binding::validated(r.enable, root, r.receipt.boundary)?,
            recipe: r.recipe,
            format: r.format,
            reserved: r.reserved,
        })
    }
    pub fn decode(self) -> Option<Record> {
        Some(Record {
            bank: self.bank,
            levels: self.levels,
            receipt: self.receipt,
            harmonic_clock: self.harmonic_clock,
            enable: self.enable.decode()?,
            recipe: self.recipe,
            format: self.format,
            reserved: self.reserved,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(in crate::operator_state::capture_proof) struct CompactSlot {
    pub record: CompactRecord,
    pub metadata: Metadata,
}
#[repr(C)]
pub(in crate::operator_state::capture_proof) struct CompactPool {
    pub slots: Vec<CompactSlot>,
    pub current: Vec<Handle>,
    pub staging: Vec<Handle>,
    pub free_head: u32,
    pub live: u32,
    pub next_generation: u64,
    pub retained_bytes: usize,
    pub namespace: u64,
}
pub(in crate::operator_state::capture_proof) fn pool_bytes() -> usize {
    use std::mem::size_of;
    let mut slots = Vec::<CompactSlot>::new();
    slots.try_reserve_exact(1142).unwrap();
    let mut current = Vec::<Handle>::new();
    current.try_reserve_exact(256).unwrap();
    let mut staging = Vec::<Handle>::new();
    staging.try_reserve_exact(256).unwrap();
    assert_eq!(
        (slots.capacity(), current.capacity(), staging.capacity()),
        (1142, 256, 256)
    );
    size_of::<CompactPool>()
        + slots.capacity() * size_of::<CompactSlot>()
        + (current.capacity() + staging.capacity()) * size_of::<Handle>()
}
