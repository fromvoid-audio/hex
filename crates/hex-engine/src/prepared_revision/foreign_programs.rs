//! Control-only immutable program continuation before revision publication.
//! The predecessor is borrowed; only a unique candidate is changed. A worker
//! observation permits pruning but grants no graph or final Main authority.
use super::*;
use hex_lattice::{PreparedProcessorFactory, PreparedRuntimeInstance};
use hex_lattice_contract::ForeignProcessorBinding as Binding;
use std::num::NonZeroU64;

/// Actual original-worker pickup state, never a queued graph key or final Main
/// ACK. Empty is an observed unprogrammed pickup or zero pickup prefix. It does
/// not assert that the callback has no unfinished keyed quantum.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ForeignProgramFrontier {
    #[default]
    Unknown,
    Empty,
    Program(NonZeroU64),
}

impl PreparedRevision {
    /// Whether the Arrangement can advance a retained worker's program key.
    pub fn has_arrangement_foreign_programs(&self) -> bool {
        nodes(&self.arrangement).any(|node| node.automation.foreign().is_some())
    }

    /// Preserve programs needed to finish the original worker's open quantum.
    /// Call before sharing/publication, using the actual preceding revision.
    /// `received` must query that exact worker. Unknown/Empty conservatively
    /// retain bounded history; only actual Empty can admit a matching first
    /// programme. Failure consumes only this unpublished candidate.
    pub fn with_foreign_program_history_from(
        mut self,
        previous: &Self,
        mut received: impl FnMut(Binding) -> ForeignProgramFrontier,
    ) -> Result<Self, Invalid> {
        if !self.foreign_automation.is_empty() || !previous.foreign_automation.is_empty() {
            return Err(Invalid("legacy foreign program history is unsupported"));
        }
        if self.has_arrangement_foreign_programs() {
            self.arrangement
                .retain_foreign_programs(&previous.arrangement, &mut received)?;
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
    received: &mut dyn FnMut(Binding) -> ForeignProgramFrontier,
) -> Result<(), Invalid> {
    let Some(program) = node.automation.foreign() else {
        return Ok(());
    };
    let binding = program.borrow().binding;
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
    let frontier = received(binding);
    if old_node.automation.foreign().is_none() {
        return if frontier == ForeignProgramFrontier::Empty {
            Ok(())
        } else {
            Err(Invalid(
                "first foreign program requires an observed empty worker frontier",
            ))
        };
    }
    let key = match frontier {
        ForeignProgramFrontier::Program(key) => Some(key),
        ForeignProgramFrontier::Unknown | ForeignProgramFrontier::Empty => None,
    };
    node.automation
        .retain_foreign_history_on_control(&old_node.automation, key)
        .map_err(|_| Invalid("foreign program history or frontier exceeds its contract"))
}

pub(crate) fn retain_project(
    project: &mut PlaybackProject,
    previous: &PlaybackProject,
    received: &mut dyn FnMut(Binding) -> ForeignProgramFrontier,
) -> Result<(), Invalid> {
    if project.sample_rate != previous.sample_rate {
        return Err(Invalid("foreign program predecessor sample rate differs"));
    }
    for track in &mut project.tracks {
        if let Some(instrument) = &mut track.foreign_instrument {
            retain_node(instrument, previous, received)?;
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
            retain_node(node, previous, received)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
