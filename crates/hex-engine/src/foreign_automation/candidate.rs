//! Canonical VST3 effect lanes for the closed Arrangement revision builder.
use super::{Knot, Lane, Program, QuantumProfile};
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

pub(crate) fn has_authored(device: &Device) -> bool {
    lanes(device).next().is_some()
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
            .checked_add(std::mem::size_of::<Program>() + 3 * std::mem::size_of::<usize>())
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
        }
    }
    Ok((bytes, rows))
}

pub(crate) fn prepare_device(
    device: &Device,
    identity: ProcessorInstanceIdentity,
    effects: &NativeDeviceRegistry<'_>,
    processor: hex_lattice::PreparedForeignProcessor,
    tempo: &hex_project::TempoMap,
    region: Option<hex_project::LoopRegion>,
) -> Result<Option<hex_lattice::PreparedForeignAutomation>, Error> {
    if lanes(device).next().is_none() {
        return Ok(None);
    }
    let context = DevicePreparationContext {
        sample_rate: tempo.sample_rate(),
        tuning: &hex_lattice::TuningTable::default(),
        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
    };
    let source = effects
        .definition(device.state().type_id())
        .ok_or(Error::UnsupportedGraph)?
        .foreign_automation_source(identity, device.state(), &context)
        .map_err(|_| Error::UnsupportedGraph)?
        .ok_or(Error::UnsupportedGraph)?;
    if source.sample_rate != tempo.sample_rate()
        || source.quantum_frames != processor.quantum_frames()
        || Some(source.binding) != processor.binding()
    {
        return Err(Error::SourceMismatch);
    }
    let revision = std::num::NonZeroU64::new(source.binding.epoch).ok_or(Error::SourceMismatch)?;
    let program = Program::prepare(
        device.automation(),
        source.class,
        source.parameters,
        tempo,
        QuantumProfile::arrangement(source.quantum_frames, region),
    )
    .map_err(|_| Error::UnsupportedGraph)?
    .ok_or(Error::SourceMismatch)?;
    let bytes = program.backing_bytes().ok_or(Error::Overflow)?;
    hex_lattice::PreparedForeignAutomation::new(source.binding, revision, program, bytes)
        .map(Some)
        .map_err(|_| Error::Overflow)
}
