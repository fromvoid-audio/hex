use super::{
    inventory::{self, Instrument, Shape},
    payload,
    planner::{self, Input, OwnerCharge, Recipe, Refusal, Root},
};
use std::mem::{align_of, offset_of, size_of};

const TRACKS: usize = super::super::ADMITTED_TRACKS;

fn empty(pool: payload::PoolCapacity) -> Input<'static> {
    Input {
        base: 0,
        roots: &[
            Root {
                owner: 1,
                next_generation: u64::MAX,
            },
            Root {
                owner: 2,
                next_generation: u64::MAX,
            },
        ],
        staged_bindings: 0,
        active: &[],
        retained: &[],
        current_contexts: &[],
        retained_contexts: &[],
        recipes: &[],
        pool,
        context_capacity: payload::ContextCapacity::allocated(),
        active_effects: 0,
        retained_effects: 0,
        active_effect_bytes: 0,
        retained_effect_bytes: 0,
        suffix_entries: 0,
    }
}
fn recipe(id: u64) -> Recipe {
    Recipe {
        owner: 1,
        generation: 1,
        id,
        context: 1,
        boundary: 96,
        clock_generation: 1,
        context_bytes: payload::context_bytes(),
    }
}

#[test]
#[cfg(target_pointer_width = "64")]
fn actual_layout_charges_every_field_and_both_histories() {
    assert_eq!(size_of::<payload::Payload>(), 1436);
    assert_eq!(align_of::<payload::Payload>(), 4);
    assert_eq!(offset_of!(payload::Payload, prefix), 0);
    assert_eq!(offset_of!(payload::Payload, authored), 380);
    assert_eq!(offset_of!(payload::Payload, ranges), 1404);
    assert_eq!(offset_of!(payload::Payload, mode_history), 1420);
    assert_eq!(size_of::<payload::Record>(), 2256);
    assert_eq!(size_of::<payload::Metadata>(), 48);
    assert_eq!(size_of::<payload::Slot>(), 2304);
    assert_eq!(size_of::<payload::Handle>(), 24);
    assert_eq!(size_of::<payload::Store>(), 104);
    let pool = payload::PoolCapacity::allocated();
    assert_eq!((pool.slots, pool.current, pool.staging), (1142, 256, 256));
    assert_eq!(pool.bytes(), Some(2_643_560));
    let contexts = payload::ContextCapacity::allocated();
    assert_eq!((contexts.owners, contexts.index), (256, 256));
    assert!(contexts.overhead(256, 256).unwrap() > 6_200);
    assert_eq!(
        size_of::<payload::Pair>(),
        size_of::<payload::Law>() + 2 * size_of::<payload::History>() + 8
    );
    assert!(size_of::<payload::Pair>() <= 8192);
    assert_eq!(size_of::<payload::Pair>(), 8112);
    assert!(payload::history_sentinels_roundtrip());
    assert!(payload::context_bytes() <= 4096);
    eprintln!(
        "proposed actual layout: payload={} context_inline={} context_charge={} history={} law={} pair={} host={} tail={} pool={} retained_recipes={}",
        size_of::<payload::Payload>(),
        size_of::<payload::Context>(),
        payload::context_bytes(),
        size_of::<payload::History>(),
        size_of::<payload::Law>(),
        size_of::<payload::Pair>(),
        size_of::<payload::HostOwner>(),
        size_of::<payload::TailOwner>(),
        pool.bytes().unwrap(),
        2 * 1024 * 1024 / (size_of::<payload::Slot>() + payload::context_bytes())
    );
    // Actual negative layouts: duplicating a normalized bank is never free.
    #[allow(dead_code)]
    struct DuplicateContext {
        context: payload::Context,
        bank: [[f64; 64]; 4],
    }
    #[allow(dead_code)]
    struct DuplicatePair {
        pair: payload::Pair,
        bank: [[f64; 64]; 4],
    }
    let duplicated_context = size_of::<DuplicateContext>() + size_of::<payload::Payload>() + 16;
    assert!(duplicated_context > 4096);
    assert!(size_of::<DuplicatePair>() > 8192);
}

#[test]
fn proposal_preserves_actual_legacy_prefix_signed_zero_and_inactive_bank_bits() {
    let original = crate::default_native_device_state_for_class(
        crate::NativeDeviceClass::Instrument,
        hex_devices::operator_state::OPERATOR_TYPE_ID,
    )
    .unwrap();
    let original = crate::patch_native_device_parameter_for_class(
        crate::NativeDeviceClass::Instrument,
        &original,
        hex_devices::operator_state::parameter::amplitude_velocity(1),
        hex_lattice_contract::ParameterValue::Float(-0.0),
    )
    .unwrap();
    assert_eq!(
        original.state_version(),
        hex_devices::operator_state::OPERATOR_STATE_VERSION
    );
    // Fine-Hz and pitch-keytracking each add four f32 values after the
    // historical 1,728-byte payload; the proposal below is not a codec.
    assert_eq!(original.payload().len(), 1760);
    let original_bytes = original.payload().to_vec();
    assert_eq!(&original.payload()[288..292], &(-0.0f32).to_le_bytes());
    let mut proposal = payload::Payload {
        prefix: original.payload()[..380].try_into().unwrap(),
        authored: [[0.0; 64]; 4],
        ranges: [0; 4],
        mode_history: [0; 4],
    };
    // Storage only: no amplitude/Normalize/Repeat/range behavior is selected.
    proposal.authored[3][63] = -0.0;
    proposal.authored[2][62] = f32::from_bits(0x3f12_3456);
    proposal.mode_history[3] = 0x1020_3040;
    let copied = proposal;
    assert_eq!(copied.prefix.as_slice(), &original.payload()[..380]);
    for oscillator in 0..4 {
        for partial in 0..64 {
            assert_eq!(
                copied.authored[oscillator][partial].to_bits(),
                proposal.authored[oscillator][partial].to_bits()
            );
        }
    }
    assert_eq!(copied.mode_history, proposal.mode_history);
    assert_eq!(original.payload(), original_bytes);
    assert_eq!(
        original.state_version(),
        hex_devices::operator_state::OPERATOR_STATE_VERSION
    );
}

#[test]
fn independent_byte_count_and_overflow_boundaries_are_not_hidden() {
    let pool = payload::PoolCapacity::allocated();
    let mut input = empty(pool);
    input.base = 32 * 1024 * 1024 - pool.bytes().unwrap();
    assert_eq!(planner::plan(&input).unwrap().runtime, 32 * 1024 * 1024);
    input.base += 1;
    assert_eq!(planner::plan(&input), Err(Refusal::Playback));
    input.base = usize::MAX;
    assert_eq!(planner::plan(&input), Err(Refusal::Overflow));
    input = empty(pool);
    input.pool.slots = usize::MAX;
    assert_eq!(planner::plan(&input), Err(Refusal::Overflow));
    input = empty(pool);
    input.context_capacity.index = usize::MAX;
    assert_eq!(planner::plan(&input), Err(Refusal::Overflow));
    input = empty(pool);
    input.pool.slots -= 1;
    assert_eq!(planner::plan(&input), Err(Refusal::PoolCapacity));
    let owner = OwnerCharge {
        header: size_of::<payload::HostOwner>(),
        payload: 8192,
        ledgers: 128 * size_of::<crate::mutable_instrument::MutableOccurrence>(),
    };
    let owners = [owner];
    input = empty(pool);
    input.active = &owners;
    assert!(planner::plan(&input).is_ok());
    let too_large = [OwnerCharge {
        payload: 8193,
        ..owner
    }];
    input.active = &too_large;
    assert_eq!(planner::plan(&input), Err(Refusal::Pair));
    let maximal = [owner; 256];
    input.active = &maximal;
    assert!(planner::plan(&input).is_ok()); // exact active2MiB declaration
    input.active_effect_bytes = 1;
    assert_eq!(planner::plan(&input), Err(Refusal::Pair));
    input.active_effect_bytes = 0;
    input.active_effects = 1;
    assert_eq!(planner::plan(&input), Err(Refusal::Processors));
    // The retained bank has its own declaration/count limit, independent of active.
    input = empty(pool);
    input.retained = &maximal;
    assert!(planner::plan(&input).is_ok());
    input.retained_effect_bytes = 1;
    assert_eq!(planner::plan(&input), Err(Refusal::Pair));
    input.retained_effect_bytes = 0;
    input.retained_effects = 1;
    assert_eq!(planner::plan(&input), Err(Refusal::Processors));
    input = empty(pool);
    input.suffix_entries = 4096;
    assert!(planner::plan(&input).is_ok());
    input.suffix_entries += 1;
    assert_eq!(planner::plan(&input), Err(Refusal::Suffixes));
    let contexts = [4096; 512];
    input = empty(pool);
    input.retained_contexts = &contexts;
    assert!(planner::plan(&input).is_ok());
    let contexts_over = [4096; 513];
    input.retained_contexts = &contexts_over;
    assert_eq!(planner::plan(&input), Err(Refusal::RetainedContexts));
    let context_over = [4097];
    input = empty(pool);
    input.current_contexts = &context_over;
    assert_eq!(planner::plan(&input), Err(Refusal::Context));
    let too_many_current = [1; 257];
    input.current_contexts = &too_many_current;
    assert_eq!(planner::plan(&input), Err(Refusal::CurrentContexts));
    let overflow_owner = [OwnerCharge {
        header: usize::MAX,
        ..owner
    }];
    input = empty(pool);
    input.active = &overflow_owner;
    assert_eq!(planner::plan(&input), Err(Refusal::Overflow));
}

#[test]
fn actual_recipe_charge_and_owner_generation_boundary_identity_govern_admission() {
    let pool = payload::PoolCapacity::allocated();
    let charge = size_of::<payload::Slot>() + payload::context_bytes();
    let limit = 2 * 1024 * 1024 / charge;
    assert_eq!(limit, 423); // Current proposed context; unchanged 2 MiB cap.
    assert!(limit * charge <= 2 * 1024 * 1024);
    assert!((limit + 1) * charge > 2 * 1024 * 1024);
    let recipes: Vec<_> = (1..=limit as u64).map(recipe).collect();
    let mut input = empty(pool);
    input.recipes = &recipes;
    let plan = planner::plan(&input).unwrap();
    assert_eq!(plan.recipes, limit);
    assert_eq!(plan.retained_bytes, limit * charge);
    let mut over = recipes.clone();
    over.push(recipe(limit as u64 + 1));
    input.recipes = &over;
    assert_eq!(planner::plan(&input), Err(Refusal::RetainedBytes));
    let base = recipe(1);
    let distinct = [
        base,
        base,
        Recipe { owner: 2, ..base },
        Recipe {
            generation: 2,
            ..base
        },
        Recipe {
            boundary: 97,
            ..base
        },
        Recipe {
            clock_generation: 2,
            ..base
        },
    ];
    input.recipes = &distinct;
    assert_eq!(planner::plan(&input).unwrap().recipes, 5);
    let conflict = [
        base,
        Recipe {
            context_bytes: base.context_bytes + 1,
            ..base
        },
    ];
    input.recipes = &conflict;
    assert_eq!(planner::plan(&input), Err(Refusal::RecipeConflict));
    let invalid = [Recipe {
        generation: 0,
        ..base
    }];
    input.recipes = &invalid;
    assert_eq!(planner::plan(&input), Err(Refusal::RecipeIdentity));
    let count_over: Vec<_> = (1..=631)
        .map(|id| Recipe {
            context_bytes: 1,
            ..recipe(id)
        })
        .collect();
    input.recipes = &count_over;
    assert_eq!(planner::plan(&input), Err(Refusal::Recipes));
}

fn check_admitted_inventory_and_handoff_ownership(shape: Shape) {
    let pool = payload::PoolCapacity::allocated();
    let runtime = inventory::handoff(&shape).unwrap();
    if shape.returns != 0 {
        assert_eq!(
            runtime.return_effect_tails.slots().iter().flatten().count(),
            shape.returns
        );
    }
    let actual = runtime.accounted_runtime_storage_bytes().unwrap();
    let legacy = inventory::ownership(&runtime, false);
    let recomposed = legacy.base
        + legacy
            .active
            .iter()
            .chain(&legacy.retained)
            .map(|owner| owner.bytes().unwrap())
            .sum::<usize>();
    assert_eq!(
        recomposed, actual,
        "lost owned bytes for {:?}",
        shape.instrument
    );
    let projected = inventory::ownership(&runtime, true);
    let recipes: Vec<_> = (1..=projected.retained_contexts.len() as u64)
        .map(recipe)
        .collect();
    let input = projected.input(&recipes, pool);
    let before = (
        actual,
        runtime.performance_frame(),
        runtime.arrangement_frame(),
        runtime.release_voice_count(),
    );
    let result = planner::plan(&input);
    assert!(
        result.is_ok() || result == Err(Refusal::Playback),
        "unexpected proof refusal: {result:?}"
    );
    assert_eq!(
        before,
        (
            runtime.accounted_runtime_storage_bytes().unwrap(),
            runtime.performance_frame(),
            runtime.arrangement_frame(),
            runtime.release_voice_count()
        )
    );
    eprintln!(
        "reachable proof: effect={} tracks={} returns={} instrument={:?} mutable={:?} actual={} active_pairs={} retained_pairs={} active_effects={} retained_effects={} suffixes={} contexts={} retained_contexts={} projected={result:?}",
        shape.effect,
        shape.effect_tracks,
        shape.returns,
        shape.instrument,
        shape.mutable,
        actual,
        projected.active.len(),
        projected.retained.len(),
        projected.active_effects,
        projected.retained_effects,
        projected.suffixes,
        projected.contexts.len(),
        projected.retained_contexts.len()
    );
    // Force a planner refusal on a real owner; no predecessor/source mutation.
    let mut refused = input;
    refused.base = usize::MAX;
    assert_eq!(planner::plan(&refused), Err(Refusal::Overflow));
    assert_eq!(runtime.accounted_runtime_storage_bytes().unwrap(), actual);
}

macro_rules! ownership_shape_case {
    ($name:ident, $effect:expr, $effect_tracks:expr, $returns:expr, $mutable:expr, $instrument:expr) => {
        #[test]
        fn $name() {
            check_admitted_inventory_and_handoff_ownership(Shape {
                tracks: TRACKS,
                effect: $effect,
                effect_tracks: $effect_tracks,
                returns: $returns,
                mutable: $mutable,
                instrument: $instrument,
            });
        }
    };
}

ownership_shape_case!(
    ownership_effect_utility,
    "hex.utility",
    TRACKS,
    0,
    None,
    Instrument::Analytic
);
ownership_shape_case!(
    ownership_effect_eq8,
    "hex.eq8",
    TRACKS,
    0,
    None,
    Instrument::Analytic
);
ownership_shape_case!(
    ownership_effect_saturator,
    "hex.saturator",
    TRACKS,
    0,
    None,
    Instrument::Analytic
);
ownership_shape_case!(
    ownership_effect_compressor,
    "hex.compressor",
    TRACKS,
    0,
    None,
    Instrument::Analytic
);
ownership_shape_case!(
    ownership_effect_gate,
    "hex.gate",
    TRACKS,
    0,
    None,
    Instrument::Analytic
);
ownership_shape_case!(
    ownership_effect_auto_pan,
    "hex.auto-pan",
    TRACKS,
    0,
    None,
    Instrument::Analytic
);
ownership_shape_case!(
    ownership_effect_overdrive,
    "hex.overdrive",
    TRACKS,
    0,
    None,
    Instrument::Analytic
);
ownership_shape_case!(
    ownership_instrument_lfo,
    "hex.eq8",
    0,
    0,
    None,
    Instrument::Lfo
);
ownership_shape_case!(
    ownership_instrument_feedback,
    "hex.eq8",
    0,
    0,
    None,
    Instrument::Feedback
);
ownership_shape_case!(
    ownership_instrument_sine,
    "hex.eq8",
    0,
    0,
    None,
    Instrument::Sine
);
ownership_shape_case!(
    ownership_instrument_fm4,
    "hex.eq8",
    0,
    0,
    None,
    Instrument::Fm4
);
ownership_shape_case!(
    ownership_mutable_lfo,
    "hex.eq8",
    TRACKS,
    0,
    Some((TRACKS, Instrument::Lfo)),
    Instrument::Analytic
);
ownership_shape_case!(
    ownership_returns_mutable_24,
    "hex.eq8",
    TRACKS,
    16,
    Some((24, Instrument::Feedback)),
    Instrument::Analytic
);
ownership_shape_case!(
    ownership_returns_mutable_16,
    "hex.eq8",
    32,
    16,
    Some((16, Instrument::Feedback)),
    Instrument::Analytic
);

#[test]
fn namespace_generation_ledger_and_retained_slot_refusals_are_independent() {
    let pool = payload::PoolCapacity::allocated();
    let mut input = empty(pool);
    input.pool.owner_namespace = 0;
    assert_eq!(planner::plan(&input), Err(Refusal::Namespace));
    input = empty(pool);
    input.staged_bindings = 1;
    input.pool.next_generation = u64::MAX;
    assert_eq!(planner::plan(&input), Err(Refusal::Overflow));
    input.pool.next_generation = u64::MAX - 1;
    assert_eq!(planner::plan(&input).unwrap().next_generation, u64::MAX);
    input = empty(pool);
    input.staged_bindings = 257;
    assert_eq!(planner::plan(&input), Err(Refusal::PoolCapacity));
    let owner = [OwnerCharge {
        header: size_of::<payload::HostOwner>(),
        payload: size_of::<payload::Pair>(),
        ledgers: 0,
    }];
    input = empty(pool);
    input.active = &owner;
    assert_eq!(planner::plan(&input), Err(Refusal::OwnerContract));
    let unknown = [Recipe {
        owner: 99,
        ..recipe(1)
    }];
    input = empty(pool);
    input.recipes = &unknown;
    assert_eq!(planner::plan(&input), Err(Refusal::SourceRoot));
    let stale = [Recipe {
        generation: 3,
        ..recipe(1)
    }];
    input.recipes = &stale;
    input.roots = &[Root {
        owner: 1,
        next_generation: 3,
    }];
    assert_eq!(planner::plan(&input), Err(Refusal::SourceRoot));
    let contexts = vec![1; 16_385];
    input = empty(pool);
    input.retained_contexts = &contexts;
    assert_eq!(planner::plan(&input), Err(Refusal::RetainedContexts));
}

#[test]
fn refusal_on_real_admitted_track_return_and_mutable_owners_preserves_next_pcm() {
    let shape = Shape {
        tracks: TRACKS,
        effect: "hex.eq8",
        effect_tracks: 32,
        returns: 16,
        mutable: Some((16, Instrument::Feedback)),
        instrument: Instrument::Analytic,
    };
    // Independent playback owners may share immutable revision inputs.
    let (previous, candidate) = std::thread::scope(|scope| {
        let candidate = scope.spawn(|| inventory::revision(&shape, false, 100));
        let previous = inventory::revision(&shape, true, 100_000);
        (previous, candidate.join().unwrap())
    });
    let (mut actual, mut twin) = std::thread::scope(|scope| {
        let twin = scope.spawn(|| {
            inventory::handoff_prepared(&shape, previous.clone(), candidate.clone()).unwrap()
        });
        let actual =
            inventory::handoff_prepared(&shape, previous.clone(), candidate.clone()).unwrap();
        (actual, twin.join().unwrap())
    });
    let projected = inventory::ownership(&actual, true);
    let mut input = projected.input(&[], payload::PoolCapacity::allocated());
    input.base = usize::MAX;
    assert_eq!(planner::plan(&input), Err(Refusal::Overflow));
    let mut observed = [[0.0; 2]; 128];
    let mut expected = [[0.0; 2]; 128];
    actual.render(&mut observed).unwrap();
    twin.render(&mut expected).unwrap();
    assert_eq!(observed, expected);
    assert_eq!(actual.performance_frame(), twin.performance_frame());
    assert_eq!(actual.arrangement_frame(), twin.arrangement_frame());
}
