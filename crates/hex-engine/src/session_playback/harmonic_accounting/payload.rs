use crate::mutable_instrument::MutableOccurrence;
use hex_devices::{SineTable, fm4::PreparedOperatorConfiguration};
use hex_lattice::{
    AnalyticContinuationFormat, NativeInstrumentRuntimePair, NativeOwnedInstrumentRuntimePair,
};
use hex_lattice_contract::NativeDeviceState;
use std::mem::size_of;

pub(super) const TRACKS: usize = 256;
pub(super) const RETAINED: usize = 630;
pub(super) const SLOTS: usize = 2 * TRACKS + RETAINED;

/// Storage proposal, not a versioned codec. Domain/control laws remain unresolved.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Payload {
    pub prefix: [u8; 380],
    pub authored: [[f32; 64]; 4],
    pub ranges: [u32; 4],
    pub mode_history: [u32; 4],
}

pub(super) struct Context {
    configuration: PreparedOperatorConfiguration,
    // Real generic state's fat Arc header. Only its proposed allocation size
    // is modeled; no version9 NativeDeviceState is constructed or published.
    authored: NativeDeviceState,
    effective_levels: [f64; 4],
    format: AnalyticContinuationFormat,
    sine: &'static SineTable,
    normalization: [f64; 4],
    source_format: u32,
    reserved: u32,
}

pub(super) fn context_bytes() -> usize {
    size_of::<Context>() + size_of::<Payload>() + 2 * size_of::<usize>()
}

/// The existing payload budgets exclude table/wrapper ownership. Charge those
/// bytes separately, using real types and actual reserved proposed capacities.
#[derive(Clone, Copy)]
pub(super) struct ContextCapacity {
    pub owners: usize,
    pub index: usize,
}
impl ContextCapacity {
    pub fn allocated() -> Self {
        let mut owners = Vec::<std::sync::Arc<crate::shared_analytic::SharedContextOwner>>::new();
        owners.try_reserve_exact(TRACKS).unwrap();
        let mut index = Vec::<(crate::shared_analytic::SharedContextId, u16)>::new();
        index.try_reserve_exact(TRACKS).unwrap();
        Self {
            owners: owners.capacity(),
            index: index.capacity(),
        }
    }
    pub fn overhead(self, current: usize, retained: usize) -> Option<usize> {
        let count = current.checked_add(retained)?;
        // Owner Arc counters and prepared-context Arc counters. The canonical
        // state's Arc counters are already included in context_bytes().
        let per_owner = size_of::<crate::shared_analytic::SharedContextOwner>()
            .checked_add(4 * size_of::<usize>())?;
        self.owners
            .checked_mul(size_of::<
                std::sync::Arc<crate::shared_analytic::SharedContextOwner>,
            >())?
            .checked_add(
                self.index
                    .checked_mul(size_of::<(crate::shared_analytic::SharedContextId, u16)>())?,
            )?
            .checked_add(size_of::<crate::shared_analytic::SharedAnalyticContextTable>())?
            .checked_add(2 * size_of::<usize>())? // table Arc counters
            .checked_add(count.checked_mul(per_owner)?)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Phase {
    fraction: f64,
    cycle: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Voice {
    phases: [u64; 4],
    previous: [f64; 4],
    age: u64,
    // Feedback exposes an independent Option gate. Do not collapse None and
    // Some(u64::MAX) into the LFO runtime's held-gate sentinel.
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
#[derive(Clone, Copy, Default)]
pub(super) struct History {
    voices: [Voice; 16],
    pitches: [u8; 16],
    velocities: [u8; 16],
    device_lfo: Phase,
    transition_elapsed: u32,
    enable_ramp: Option<EnableRamp>,
}

pub(super) fn history_sentinels_roundtrip() -> bool {
    let mut history = History::default();
    history.voices[0].gate = None;
    history.voices[0].active = true;
    history.voices[1].gate = Some(u64::MAX);
    history.voices[1].active = true;
    history.voices[2].occurrence = 42;
    history.voices[2].previous[3] = -0.0;
    let copied = history;
    copied.voices[0].gate.is_none()
        && copied.voices[0].active
        && copied.voices[1].gate == Some(u64::MAX)
        && copied.voices[1].active
        && !copied.voices[2].active
        && copied.voices[2].occurrence == 42
        && copied.voices[2].previous[3].to_bits() == (-0.0f64).to_bits()
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EnvelopeTiming {
    frames: [u64; 3],
    coefficients: [f64; 3],
}

/// Authored coefficients exist ONCE in Payload; normalization scalars are charged.
/// No duplicate target f64 bank and no borrowed per-edit sampling owner.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Law {
    payload: Payload,
    normalization: [f64; 4],
    envelopes: [EnvelopeTiming; 4],
    phase_origins: [u64; 4],
    routing_order: [u8; 4],
    sine: Option<&'static SineTable>,
    builtin_bank: Option<&'static [f32]>,
    sample_rate: u32,
    transition_frames: u32,
    release_frames: u64,
    target_configuration: u64,
    source_partials: [[f64; 64]; 4],
    source_builtin_weights: [[f64; 4]; 4],
    source_scalars: [[f64; 4]; 3],
    source_configuration: u64,
    frozen: u64,
    owner_namespace: u64,
    provider_format: u32,
    reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Pair {
    law: Law,
    histories: [History; 2],
    transaction: u64,
}

pub(super) enum Guard {
    Independent(NativeInstrumentRuntimePair),
    Owned(NativeOwnedInstrumentRuntimePair),
}
pub(super) struct HostOwner {
    guard: Guard,
    frozen_parameters: crate::mutable_instrument::frozen_parameters::FrozenParameters,
    modulation: Option<Box<hex_lattice::ModulationRuntime>>,
    output_trim: Option<crate::mutable_instrument::output_trim::OutputTrim>,
    // Mirror the actual inline optional lease header. Harmonic fixtures have no
    // resource lease; source/configuration owners use the separate resource ledger.
    resources: Option<crate::native_device_resources::PreparedDeviceResourceLease>,
    // Mirrors the source-owned live-ledger box header. The fixed ledger payload
    // is charged separately with the occurrence ledgers in OwnerCharge.
    live: Option<Box<()>>,
    // Mirror the actual host's new inline controller target and frozen value;
    // these bytes are additional to the provider payload and both ledgers.
    expression_identity: Option<hex_project::DeviceId>,
    expression: Option<crate::live_expression::Snapshot>,
    primary: Vec<MutableOccurrence>,
    scratch: Vec<MutableOccurrence>,
    next_id: u64,
    next_id_scratch: u64,
}
pub(super) struct TailOwner {
    track_id: hex_project::TrackId,
    owner: HostOwner,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Handle {
    pub slot: u32,
    pub kind: u32,
    pub generation: u64,
    pub owner: u64,
}

#[repr(C)]
pub(super) struct Record {
    // Full exact f64 source, not the existing eight-word enable frame.
    coefficients: [[f64; 64]; 4],
    builtin_weights: [[f64; 4]; 4],
    levels: [f64; 4],
    source_context: u64,
    target_context: u64,
    recipe: u64,
    start: u64,
    end: u64,
    duration: u32,
    provider_format: u32,
}
#[repr(C)]
pub(super) struct Metadata {
    generation: u64,
    committed_refs: u32,
    staged_refs: u32,
    flags: u32,
    free_next: u32,
    target_context_slot: u32,
    retirement_next: u32,
    retirement_epoch: u64,
    retained_charge: u64,
}
#[repr(C)]
pub(super) struct Slot {
    record: Record,
    metadata: Metadata,
}
#[repr(C)]
pub(super) struct Store {
    slots: Vec<Slot>,
    current: Vec<Handle>,
    staging: Vec<Handle>,
    free_head: u32,
    live: u32,
    next_generation: u64,
    retained_bytes: usize,
    owner_namespace: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PoolCapacity {
    pub slots: usize,
    pub current: usize,
    pub staging: usize,
    pub owner_namespace: u64,
    pub next_generation: u64,
}
impl PoolCapacity {
    pub fn allocated() -> Self {
        let mut slots = Vec::<Slot>::new();
        slots.try_reserve_exact(SLOTS).unwrap();
        let mut current = Vec::<Handle>::new();
        current.try_reserve_exact(TRACKS).unwrap();
        let mut staging = Vec::<Handle>::new();
        staging.try_reserve_exact(TRACKS).unwrap();
        Self {
            slots: slots.capacity(),
            current: current.capacity(),
            staging: staging.capacity(),
            owner_namespace: 1,
            next_generation: 1,
        }
    }
    pub fn bytes(self) -> Option<usize> {
        self.slots
            .checked_mul(size_of::<Slot>())?
            .checked_add(self.current.checked_mul(size_of::<Handle>())?)?
            .checked_add(self.staging.checked_mul(size_of::<Handle>())?)?
            .checked_add(size_of::<Store>())
    }
}
