//! Control-only immutable program continuation before revision publication.
//! The predecessor is borrowed; only a unique candidate is changed. A worker
//! observation permits pruning but grants no graph or final Main authority.
use super::*;
use hex_lattice::{PreparedProcessorFactory, PreparedRuntimeInstance};
use hex_lattice_contract::ForeignProcessorBinding as Binding;
use std::num::NonZeroU64;

/// Original-worker pickup state or a separately witnessed stopped source fence.
/// Empty is an observed unprogrammed pickup or zero pickup prefix; it alone
/// does not prove that the callback has no unfinished keyed quantum.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ForeignProgramFrontier {
    #[default]
    Unknown,
    Empty,
    Program(NonZeroU64),
    /// The exact compatible predecessor remains installed under a held stopped
    /// source fence. The caller must establish that `open_quantum` is the only
    /// other program any callback can still need. `worker_empty` records an
    /// actual empty worker receipt, not an inference from the stopped state.
    Stopped {
        open_quantum: Option<NonZeroU64>,
        worker_empty: bool,
    },
}

impl PreparedRevision {
    /// Whether the Arrangement can advance a retained worker's program key.
    pub fn has_arrangement_foreign_programs(&self) -> bool {
        nodes(&self.arrangement).any(|node| node.automation.foreign().is_some())
    }

    /// For each opted-in effect use its own input arrival to bound the
    /// retained clocks still in the copied compensation ring.
    pub(crate) fn proves_delayed_effect_clocks(
        &self,
        previous: &Self,
        history: Option<&crate::compensation::CompensationRuntime>,
        old_clock: NonZeroU64,
        new_clock: NonZeroU64,
        carries_clock: bool,
    ) -> bool {
        nodes(&self.arrangement).all(|node| {
            let Some(current) = node
                .automation
                .foreign()
                .filter(|program| program.effect_clock_splices())
            else {
                return true;
            };
            if self.authored_clock_epoch != new_clock
                || previous.authored_clock_epoch != old_clock
                || (old_clock != new_clock && !carries_clock)
            {
                return false;
            }
            let view = current.borrow();
            let Some(actual) = nodes(&previous.arrangement)
                .filter_map(|node| node.automation.foreign().map(|program| program.borrow()))
                .find(|program| program.binding == view.binding)
            else {
                return false;
            };
            if old_clock != new_clock && !view.retains_program_from(actual, actual.revision) {
                return false;
            }
            let Some(stage) = self
                .arrangement
                .signal_graph
                .as_ref()
                .and_then(|graph| graph.processor_latency(node.device_id, None))
            else {
                return false;
            };
            history
                .and_then(|history| history.oldest_pending_stage_clock(stage.upstream_frames))
                .is_none_or(|oldest| current.covers_effect_clock_range(oldest, new_clock))
        })
    }

    /// Whether an unchanged original binding has exhausted its bounded
    /// predecessor history. This is a control-side hint only: matching
    /// installation, a stopped fence and the open quantum must be established
    /// independently before pruning any program.
    pub fn needs_foreign_history_pruning_from(&self, previous: &Self) -> bool {
        nodes(&self.arrangement)
            .filter_map(|node| node.automation.foreign())
            .any(|program| {
                let binding = program.borrow().binding;
                nodes(&previous.arrangement)
                    .filter_map(|old| old.automation.foreign())
                    .any(|old| {
                        old.borrow().binding == binding
                            && old.storage_owners().count()
                                > hex_lattice::MAX_RETAINED_FOREIGN_PROGRAMS
                    })
            })
    }

    /// Preserve programs needed to finish the original worker's open quantum.
    /// Call before sharing/publication, using the actual preceding revision.
    /// Worker pickup values must come from that exact worker. Unknown/Empty
    /// conservatively retain bounded history; only an actual empty receipt can
    /// admit a matching first programme. Stopped requires the caller to hold
    /// the exact source fence through publication and independently validate
    /// the open quantum. This grants no source, worker, fence or publication
    /// authority. Failure consumes
    /// only this unpublished candidate.
    pub fn with_foreign_program_history_from(
        mut self,
        previous: &Self,
        mut received: impl FnMut(Binding) -> ForeignProgramFrontier,
    ) -> Result<Self, Invalid> {
        if !self.foreign_automation.is_empty() || !previous.foreign_automation.is_empty() {
            return Err(Invalid("legacy foreign program history is unsupported"));
        }
        self.authored_clock_epoch = if self
            .session()
            .timing()
            .same_map(previous.session().timing())
        {
            previous.authored_clock_epoch
        } else {
            previous
                .authored_clock_epoch
                .checked_add(1)
                .ok_or(Invalid("authored arrangement clock exhausted"))?
        };
        if self.has_arrangement_foreign_programs() {
            self.arrangement.retain_foreign_programs(
                &previous.arrangement,
                [previous.authored_clock_epoch, self.authored_clock_epoch],
                &mut received,
            )?;
        }
        Ok(self)
    }
}

fn nodes(project: &PlaybackProject) -> impl Iterator<Item = &PreparedRuntimeInstance> {
    project
        .tracks
        .iter()
        .flat_map(|track| {
            track
                .foreign_instrument
                .iter()
                .chain(track.device_chain.iter())
        })
        .chain(
            project
                .return_buses
                .iter()
                .flat_map(|bus| bus.device_chain.iter()),
        )
        .chain(project.main_device_chain.iter())
}

fn retain_node(
    node: &mut PreparedRuntimeInstance,
    previous: &PlaybackProject,
    clocks: [NonZeroU64; 2],
    received: &mut dyn FnMut(Binding) -> ForeignProgramFrontier,
) -> Result<(), Invalid> {
    let Some(program) = node.automation.foreign() else {
        return Ok(());
    };
    let binding = program.borrow().binding;
    let effect = matches!(node.definition, Some(PreparedProcessorFactory::Foreign(facts))
        if !facts.is_instrument_profile());
    if !matches!(node.definition, Some(PreparedProcessorFactory::Foreign(facts))
        if facts.binding() == Some(binding))
    {
        return Err(Invalid("foreign program candidate source differs"));
    }
    let mut old = nodes(previous).filter(|old| {
        matches!(old.definition,
            Some(PreparedProcessorFactory::Foreign(facts)) if facts.binding() == Some(binding)
        )
    });
    let Some(old_node) = old.next() else {
        if effect {
            node.automation
                .bind_foreign_effect_clock_on_control(clocks[1], false);
        }
        return Ok(());
    };
    if old.next().is_some()
        || node.owner != old_node.owner
        || node.device_id != old_node.device_id
        || node.device_type != old_node.device_type
        || node.sample_rate != old_node.sample_rate
        || !matches!((&node.definition, &old_node.definition),
            (Some(PreparedProcessorFactory::Foreign(a)), Some(PreparedProcessorFactory::Foreign(b)))
                if a == b && a.binding() == Some(binding))
    {
        return Err(Invalid("foreign program predecessor source differs"));
    }
    if effect {
        let clock_splices = old_node
            .automation
            .foreign()
            .is_some_and(|old| clocks[0] != clocks[1] || old.effect_clock_splices());
        node.automation
            .bind_foreign_effect_clock_on_control(clocks[1], clock_splices);
    }
    let frontier = received(binding);
    if old_node.automation.foreign().is_none() {
        return if matches!(
            frontier,
            ForeignProgramFrontier::Empty
                | ForeignProgramFrontier::Stopped {
                    open_quantum: None,
                    worker_empty: true,
                }
        ) {
            Ok(())
        } else {
            Err(Invalid(
                "first foreign program requires an observed empty worker frontier",
            ))
        };
    }
    let result = match frontier {
        ForeignProgramFrontier::Stopped { .. }
            if node
                .automation
                .foreign()
                .is_some_and(|program| program.effect_clock_splices()) =>
        {
            node.automation
                .retain_foreign_history_on_control(&old_node.automation, None)
        }
        ForeignProgramFrontier::Stopped { open_quantum, .. } => {
            let required = open_quantum.as_slice();
            node.automation
                .retain_selected_foreign_history_on_control(&old_node.automation, required)
        }
        ForeignProgramFrontier::Program(key) => node
            .automation
            .retain_foreign_history_on_control(&old_node.automation, Some(key)),
        ForeignProgramFrontier::Unknown | ForeignProgramFrontier::Empty => node
            .automation
            .retain_foreign_history_on_control(&old_node.automation, None),
    };
    result.map_err(|_| Invalid("foreign program history or frontier exceeds its contract"))?;
    if node.automation.foreign().is_some_and(|program| {
        program.effect_clock_splices() && !program.effect_clock_splice_fits()
    }) {
        return Err(Invalid(
            "vst3 effect tempo edit exceeds the open automation block",
        ));
    }
    Ok(())
}

pub(crate) fn retain_project(
    project: &mut PlaybackProject,
    previous: &PlaybackProject,
    clocks: [NonZeroU64; 2],
    received: &mut dyn FnMut(Binding) -> ForeignProgramFrontier,
) -> Result<(), Invalid> {
    if project.sample_rate != previous.sample_rate {
        return Err(Invalid("foreign program predecessor sample rate differs"));
    }
    for track in &mut project.tracks {
        if let Some(instrument) = &mut track.foreign_instrument {
            retain_node(instrument, previous, clocks, received)?;
        }
    }
    for chain in project
        .tracks
        .iter_mut()
        .map(|track| &mut track.device_chain)
        .chain(
            project
                .return_buses
                .iter_mut()
                .map(|bus| &mut bus.device_chain),
        )
        .chain(std::iter::once(&mut project.main_device_chain))
    {
        if !chain.iter().any(|node| node.automation.foreign().is_some()) {
            continue;
        }
        let nodes =
            Arc::get_mut(chain).ok_or(Invalid("foreign program chain is already shared"))?;
        for node in nodes {
            retain_node(node, previous, clocks, received)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
