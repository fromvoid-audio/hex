//! Canonical VST3 effect lanes for the closed Arrangement revision builder.
use super::{BoundProgram, Knot, Lane, QuantumProfile};
use crate::AudioStorageError as Error;
use hex_lattice::{
    DevicePreparationContext, NativeDeviceRegistry, ProcessorChainOwner, ProcessorInstanceIdentity,
};
use hex_lattice_contract::DeviceAutomationTarget;
use hex_project::{Device, DeviceAutomation, Project};

fn devices(project: &Project) -> impl Iterator<Item = (ProcessorChainOwner, &Device)> {
    project
        .tracks()
        .iter()
        .flat_map(|track| {
            track
                .devices()
                .iter()
                .map(move |device| (ProcessorChainOwner::Track(track.id().value()), device))
        })
        .chain(project.return_tracks().iter().flat_map(|bus| {
            bus.devices()
                .iter()
                .map(move |device| (ProcessorChainOwner::Return(bus.id().value()), device))
        }))
        .chain(
            project
                .main_devices()
                .iter()
                .map(|device| (ProcessorChainOwner::Main, device)),
        )
}

fn lanes(device: &Device) -> impl Iterator<Item = &DeviceAutomation> {
    device
        .reading_automation()
        .filter(|lane| matches!(lane.target, DeviceAutomationTarget::Vst3 { .. }))
}

pub(crate) fn quote(project: &Project) -> Result<(usize, usize), Error> {
    if project
        .tracks()
        .iter()
        .filter_map(|track| track.instrument())
        .any(|device| lanes(device).next().is_some())
        || project
            .native_sources()
            .iter()
            .flat_map(|source| source.devices())
            .any(|device| lanes(device).next().is_some())
    {
        return Err(Error::UnsupportedGraph);
    }
    let mut rows = 0usize;
    let mut bytes = 0usize;
    for (_, device) in devices(project).filter(|(_, device)| lanes(device).next().is_some()) {
        bytes = bytes
            .checked_add(std::mem::size_of::<BoundProgram>())
            .ok_or(Error::Overflow)?;
        rows = rows.checked_add(1).ok_or(Error::Overflow)?;
        for lane in lanes(device) {
            let knots = super::knot_reservation(&lane.points, project.tempo_map())
                .ok_or(Error::Overflow)?;
            bytes = bytes
                .checked_add(std::mem::size_of::<Lane>())
                .and_then(|bytes| {
                    bytes.checked_add(knots.checked_mul(std::mem::size_of::<Knot>())?)
                })
                .ok_or(Error::Overflow)?;
            rows = rows.checked_add(1).ok_or(Error::Overflow)?;
        }
    }
    Ok((
        bytes,
        rows.checked_add(usize::from(rows != 0))
            .ok_or(Error::Overflow)?,
    ))
}

pub(crate) fn prepare(
    project: &Project,
    effects: Option<&NativeDeviceRegistry<'_>>,
) -> Result<Vec<BoundProgram>, Error> {
    let count = devices(project)
        .filter(|(_, device)| lanes(device).next().is_some())
        .count();
    if count == 0 {
        return Ok(Vec::new());
    }
    if count > hex_lattice::MAX_STATEFUL_PROCESSORS {
        return Err(Error::Capacity);
    }
    let effects = effects.ok_or(Error::UnsupportedGraph)?;
    let context = DevicePreparationContext {
        sample_rate: project.sample_rate(),
        tuning: &hex_lattice::TuningTable::default(),
        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
    };
    let mut programs = Vec::new();
    programs
        .try_reserve_exact(count)
        .map_err(|_| Error::Allocation)?;
    for (owner, device) in devices(project).filter(|(_, device)| lanes(device).next().is_some()) {
        let identity = ProcessorInstanceIdentity::new(owner, device.id().value())
            .map_err(|_| Error::SourceMismatch)?;
        let source = effects
            .definition(device.state().type_id())
            .ok_or(Error::UnsupportedGraph)?
            .foreign_automation_source(identity, device.state(), &context)
            .map_err(|_| Error::UnsupportedGraph)?
            .ok_or(Error::UnsupportedGraph)?;
        // Reused source epochs have the same authored curves and tempo. Loop
        // changes only change the admitted bound, so a partial quantum keeps
        // its program identity across publication.
        let revision =
            std::num::NonZeroU64::new(source.binding.epoch).ok_or(Error::SourceMismatch)?;
        let quantum = QuantumProfile::arrangement(source.quantum_frames, project.loop_region());
        let program = BoundProgram::prepare(
            device.automation(),
            source,
            project.tempo_map(),
            quantum.max_runs,
            revision,
        )
        .map_err(|_| Error::UnsupportedGraph)?
        .ok_or(Error::SourceMismatch)?;
        programs.push(program);
    }
    Ok(programs)
}
