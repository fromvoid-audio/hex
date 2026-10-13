use hex_lattice::{AnalyticContinuation, AnalyticContinuationId};
use std::mem::size_of;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Key {
    pub namespace: u64,
    pub generation: u64,
    pub context: u64,
}
impl Key {
    pub fn valid(self) -> bool {
        self.namespace != 0 && self.generation != 0 && self.context != 0
    }
    pub fn id(self) -> AnalyticContinuationId {
        AnalyticContinuationId::new(self.context).unwrap()
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Receipt {
    pub source: Key,
    pub target: Key,
    pub clock_generation: u64,
    pub boundary: u64,
    pub source_configuration: u64,
    pub target_configuration: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Clock {
    pub start: u64,
    pub frames: u32,
    pub reserved: u32,
}
impl Clock {
    pub fn amount(self, at: u64) -> Option<f64> {
        if self.frames == 0 || self.frames > 4096 || self.reserved != 0 {
            return None;
        }
        self.start.checked_add(u64::from(self.frames))?;
        Some(
            f64::from(u32::try_from(at.checked_sub(self.start)?.min(u64::from(self.frames))).ok()?)
                / f64::from(self.frames),
        )
    }
}

/// Independent existing enable clock/law; never restarted by a harmonic edit.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct EnableBinding {
    pub law: Key,
    pub transition: Option<AnalyticContinuation>,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub(super) struct Bank {
    pub coefficients: [[f64; 64]; 4],
    pub builtin_weights: [[f64; 4]; 4],
}
impl PartialEq for Bank {
    fn eq(&self, other: &Self) -> bool {
        self.coefficients
            .iter()
            .flatten()
            .zip(other.coefficients.iter().flatten())
            .all(|(a, b)| a.to_bits() == b.to_bits())
            && self
                .builtin_weights
                .iter()
                .flatten()
                .zip(other.builtin_weights.iter().flatten())
                .all(|(a, b)| a.to_bits() == b.to_bits())
    }
}
impl Default for Bank {
    fn default() -> Self {
        Self {
            coefficients: [[0.0; 64]; 4],
            builtin_weights: [[0.0; 4]; 4],
        }
    }
}
impl Bank {
    pub fn finite(self) -> bool {
        self.coefficients.iter().flatten().all(|v| v.is_finite())
            && self.builtin_weights.iter().flatten().all(|v| v.is_finite())
            && self
                .coefficients
                .iter()
                .flatten()
                .chain(self.builtin_weights.iter().flatten())
                .try_fold(0.0f64, |n, v| {
                    let next = n + v.abs();
                    next.is_finite().then_some(next)
                })
                .is_some()
    }
    pub fn interpolate(self, target: Self, amount: f64) -> Option<Self> {
        if !self.finite()
            || !target.finite()
            || !amount.is_finite()
            || !(0.0..=1.0).contains(&amount)
        {
            return None;
        }
        if amount == 0.0 {
            return Some(self);
        }
        if amount == 1.0 {
            return Some(target);
        }
        let mut output = Self::default();
        for i in 0..4 {
            for j in 0..64 {
                let delta = target.coefficients[i][j] - self.coefficients[i][j];
                if !delta.is_finite() {
                    return None;
                }
                output.coefficients[i][j] = self.coefficients[i][j] + delta * amount;
            }
            for j in 0..4 {
                let delta = target.builtin_weights[i][j] - self.builtin_weights[i][j];
                if !delta.is_finite() {
                    return None;
                }
                output.builtin_weights[i][j] = self.builtin_weights[i][j] + delta * amount;
            }
        }
        output.finite().then_some(output)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Record {
    pub bank: Bank,
    pub levels: [f64; 4],
    pub receipt: Receipt,
    pub harmonic_clock: Clock,
    pub enable: EnableBinding,
    pub recipe: u64,
    pub format: u32,
    pub reserved: u32,
}
impl PartialEq for Record {
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

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Handle {
    pub slot: u32,
    pub kind: u32,
    pub generation: u64,
    pub namespace: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Metadata {
    pub generation: u64,
    pub committed: u32,
    pub staged: u32,
    pub flags: u32,
    pub free_next: u32,
    pub target_context_slot: u32,
    pub retirement_next: u32,
    pub retirement_epoch: u64,
    pub retained_charge: u64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Slot {
    pub record: Record,
    pub metadata: Metadata,
}

#[repr(C)]
pub(super) struct PoolLayout {
    pub slots: Vec<Slot>,
    pub current: Vec<Handle>,
    pub staging: Vec<Handle>,
    pub free_head: u32,
    pub live: u32,
    pub next_generation: u64,
    pub retained_bytes: usize,
    pub namespace: u64,
}
pub(super) fn pool_bytes() -> usize {
    let mut slots = Vec::<Slot>::new();
    slots.try_reserve_exact(1142).unwrap();
    let mut current = Vec::<Handle>::new();
    current.try_reserve_exact(256).unwrap();
    let mut staging = Vec::<Handle>::new();
    staging.try_reserve_exact(256).unwrap();
    assert_eq!(
        (slots.capacity(), current.capacity(), staging.capacity()),
        (1142, 256, 256)
    );
    size_of::<PoolLayout>()
        + slots.capacity() * size_of::<Slot>()
        + (current.capacity() + staging.capacity()) * size_of::<Handle>()
}

// Same concrete private proposal as frozen engine proof, not a production pair.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Payload {
    prefix: [u8; 380],
    authored: [[f32; 64]; 4],
    ranges: [u32; 4],
    modes: [u32; 4],
}
#[allow(dead_code)]
pub(super) struct Context {
    configuration: crate::fm4::PreparedOperatorConfiguration,
    authored: hex_lattice_contract::NativeDeviceState,
    effective_levels: [f64; 4],
    format: hex_lattice::AnalyticContinuationFormat,
    sine: &'static crate::SineTable,
    normalization: [f64; 4],
    source_format: u32,
    reserved: u32,
}
pub(super) fn context_bytes() -> usize {
    size_of::<Context>() + size_of::<Payload>() + 2 * size_of::<usize>()
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Phase {
    fraction: f64,
    cycle: u64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Voice {
    phases: [u64; 4],
    previous: [f64; 4],
    age: u64,
    gate: Option<u64>,
    occurrence: u64,
    lfo: Phase,
    active: bool,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct EnableRamp {
    start: [f64; 4],
    elapsed: u32,
    frames: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct History {
    voices: [Voice; 16],
    pitches: [u8; 16],
    velocities: [u8; 16],
    device_lfo: Phase,
    elapsed: u32,
    enable: Option<EnableRamp>,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Envelope {
    frames: [u64; 3],
    coefficients: [f64; 3],
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Law {
    payload: Payload,
    normalization: [f64; 4],
    envelopes: [Envelope; 4],
    origins: [u64; 4],
    order: [u8; 4],
    sine: Option<&'static crate::SineTable>,
    builtin: Option<&'static [f32]>,
    rate: u32,
    frames: u32,
    release: u64,
    target: u64,
    coefficients: [[f64; 64]; 4],
    weights: [[f64; 4]; 4],
    scalars: [[f64; 4]; 3],
    source: u64,
    boundary: u64,
    namespace: u64,
    format: u32,
    reserved: u32,
}
#[repr(C)]
pub(super) struct OriginalPair {
    law: Law,
    histories: [History; 2],
    transaction: u64,
}
#[repr(C)]
pub(super) struct AdditivePair {
    original: OriginalPair,
    receipt: Receipt,
    enable: EnableBinding,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub(super) struct NoteBinding {
    pub owner: Key,
    pub note: hex_lattice::PreparedSharedAnalyticNote,
    pub occurrence: u64,
    pub onset: u64,
    pub gate: Option<u64>,
    pub envelope: Key,
    pub enable: EnableBinding,
    pub recipe: Handle,
    pub custom_caps: [u8; 4],
}
