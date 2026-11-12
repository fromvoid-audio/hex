use super::payload::{ContextCapacity, PoolCapacity, RETAINED, SLOTS, Slot, TRACKS};
use std::mem::size_of;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    Overflow,
    Namespace,
    OwnerContract,
    SourceRoot,
    PoolCapacity,
    Pair,
    Context,
    CurrentContexts,
    RetainedContexts,
    RecipeIdentity,
    RecipeConflict,
    Recipes,
    RetainedBytes,
    Playback,
    Processors,
    Suffixes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OwnerCharge {
    pub header: usize,
    pub payload: usize,
    pub ledgers: usize,
    pub frozen: usize,
    pub modulation: usize,
}
impl OwnerCharge {
    pub fn bytes(self) -> Result<usize, Refusal> {
        self.header
            .checked_add(self.payload)
            .and_then(|n| n.checked_add(self.ledgers))
            .and_then(|n| n.checked_add(self.frozen))
            .and_then(|n| n.checked_add(self.modulation))
            .ok_or(Refusal::Overflow)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Recipe {
    pub owner: u64,
    pub generation: u64,
    pub id: u64,
    pub context: u64,
    pub boundary: u64,
    pub clock_generation: u64,
    pub context_bytes: usize,
}
impl Recipe {
    fn key(self) -> (u64, u64, u64, u64, u64, u64) {
        (
            self.owner,
            self.generation,
            self.id,
            self.context,
            self.boundary,
            self.clock_generation,
        )
    }
}
#[derive(Clone, Copy)]
pub(super) struct Root {
    pub owner: u64,
    pub next_generation: u64,
}

pub(super) struct Input<'a> {
    /// All remaining actual ownership, including unchanged duplicate policy charges.
    /// Actual pair headers/payload/ledgers are split out and recomposed separately.
    pub base: usize,
    pub roots: &'a [Root],
    pub staged_bindings: usize,
    pub active: &'a [OwnerCharge],
    pub retained: &'a [OwnerCharge],
    pub current_contexts: &'a [usize],
    pub retained_contexts: &'a [usize],
    pub recipes: &'a [Recipe],
    pub pool: PoolCapacity,
    pub context_capacity: ContextCapacity,
    pub active_effects: usize,
    pub retained_effects: usize,
    pub active_effect_bytes: usize,
    pub retained_effect_bytes: usize,
    pub suffix_entries: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Plan {
    pub runtime: usize,
    pub next_generation: u64,
    pub pool: usize,
    pub recipes: usize,
    pub retained_bytes: usize,
    pub current_context_bytes: usize,
    pub retained_context_bytes: usize,
    pub context_overhead: usize,
    /// Separate existing context budgets; never mislabeled as playback bytes.
    pub complete_owned_bytes: usize,
}

pub(super) fn plan(input: &Input<'_>) -> Result<Plan, Refusal> {
    if input.pool.slots < SLOTS || input.pool.current < TRACKS || input.pool.staging < TRACKS {
        return Err(Refusal::PoolCapacity);
    }
    if input.pool.owner_namespace == 0 || input.pool.next_generation == 0 {
        return Err(Refusal::Namespace);
    }
    if input.staged_bindings > TRACKS {
        return Err(Refusal::PoolCapacity);
    }
    let next_generation = input
        .pool
        .next_generation
        .checked_add(u64::try_from(input.staged_bindings).map_err(|_| Refusal::Overflow)?)
        .ok_or(Refusal::Overflow)?;
    let pool = input.pool.bytes().ok_or(Refusal::Overflow)?;
    let mut runtime = input.base;
    for (owners, effects, effect_bytes) in [
        (
            input.active,
            input.active_effects,
            input.active_effect_bytes,
        ),
        (
            input.retained,
            input.retained_effects,
            input.retained_effect_bytes,
        ),
    ] {
        if owners.len().checked_add(effects).ok_or(Refusal::Overflow)? > 256 {
            return Err(Refusal::Processors);
        }
        let mut declared = effect_bytes;
        for &owner in owners {
            let owner_bytes = owner.bytes()?;
            if owner.header
                < std::mem::size_of::<crate::mutable_instrument::MutableInstrumentTrackRuntime>()
                || owner.ledgers
                    < 128 * std::mem::size_of::<crate::mutable_instrument::MutableOccurrence>()
            {
                return Err(Refusal::OwnerContract);
            }
            let declared_owner = owner
                .payload
                .checked_add(owner.modulation)
                .ok_or(Refusal::Overflow)?;
            if owner.payload == 0 || owner.payload > 8192 {
                return Err(Refusal::Pair);
            }
            declared = declared
                .checked_add(declared_owner)
                .ok_or(Refusal::Overflow)?;
            runtime = runtime.checked_add(owner_bytes).ok_or(Refusal::Overflow)?;
        }
        if declared > 2 * 1024 * 1024 {
            return Err(Refusal::Pair);
        }
    }
    if input.suffix_entries > 4096 {
        return Err(Refusal::Suffixes);
    }
    if input.current_contexts.len() > 256 {
        return Err(Refusal::CurrentContexts);
    }
    if input.retained_contexts.len() > 16_384 {
        return Err(Refusal::RetainedContexts);
    }
    if input.context_capacity.owners < input.current_contexts.len()
        || input.context_capacity.index < input.current_contexts.len()
    {
        return Err(Refusal::PoolCapacity);
    }
    let mut context_totals = [0usize; 2];
    for ((contexts, cause), destination) in [
        (input.current_contexts, Refusal::CurrentContexts),
        (input.retained_contexts, Refusal::RetainedContexts),
    ]
    .into_iter()
    .zip(&mut context_totals)
    {
        let mut total = 0usize;
        for &bytes in contexts {
            if bytes == 0 || bytes > 4096 {
                return Err(Refusal::Context);
            }
            total = total.checked_add(bytes).ok_or(Refusal::Overflow)?;
        }
        if total > 2 * 1024 * 1024 {
            return Err(cause);
        }
        *destination = total;
    }
    let mut count = 0usize;
    let mut retained_bytes = 0usize;
    for (index, recipe) in input.recipes.iter().enumerate() {
        if recipe.owner == 0
            || recipe.generation == 0
            || recipe.id == 0
            || recipe.context == 0
            || recipe.clock_generation == 0
        {
            return Err(Refusal::RecipeIdentity);
        }
        if !input
            .roots
            .iter()
            .any(|root| root.owner == recipe.owner && recipe.generation < root.next_generation)
        {
            return Err(Refusal::SourceRoot);
        }
        if recipe.context_bytes == 0 || recipe.context_bytes > 4096 {
            return Err(Refusal::Context);
        }
        if let Some(old) = input.recipes[..index]
            .iter()
            .find(|old| old.key() == recipe.key())
        {
            if old.context_bytes != recipe.context_bytes {
                return Err(Refusal::RecipeConflict);
            }
            continue;
        }
        count = count.checked_add(1).ok_or(Refusal::Overflow)?;
        if count > RETAINED {
            return Err(Refusal::Recipes);
        }
        retained_bytes = retained_bytes
            .checked_add(size_of::<Slot>())
            .and_then(|n| n.checked_add(recipe.context_bytes))
            .ok_or(Refusal::Overflow)?;
        if retained_bytes > 2 * 1024 * 1024 {
            return Err(Refusal::RetainedBytes);
        }
    }
    runtime = runtime.checked_add(pool).ok_or(Refusal::Overflow)?;
    if runtime > 32 * 1024 * 1024 {
        return Err(Refusal::Playback);
    }
    let context_overhead = input
        .context_capacity
        .overhead(input.current_contexts.len(), input.retained_contexts.len())
        .ok_or(Refusal::Overflow)?;
    let complete_owned_bytes = runtime
        .checked_add(context_totals[0])
        .and_then(|n| n.checked_add(context_totals[1]))
        .and_then(|n| n.checked_add(context_overhead))
        .ok_or(Refusal::Overflow)?;
    Ok(Plan {
        runtime,
        next_generation,
        pool,
        recipes: count,
        retained_bytes,
        current_context_bytes: context_totals[0],
        retained_context_bytes: context_totals[1],
        context_overhead,
        complete_owned_bytes,
    })
}
