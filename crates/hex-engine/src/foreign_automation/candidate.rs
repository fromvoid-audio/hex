//! Canonical VST3 lanes for Arrangement preparation.
use super::{Knot, Lane, Program, QuantumProfile};
use crate::AudioStorageError as Error;
use hex_lattice::{DevicePreparationContext, NativeDeviceRegistry, ProcessorInstanceIdentity};
use hex_lattice_contract::DeviceAutomationTarget;
use hex_project::{Device, DeviceAutomation, Project};

fn effects(project: &Project) -> impl Iterator<Item = &Device> {
    project
        .tracks()
        .iter()
        .flat_map(|track| track.devices())
        .chain(project.return_tracks().iter().flat_map(|bus| bus.devices()))
        .chain(project.main_devices())
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
        .native_sources()
        .iter()
        .flat_map(|source| source.devices())
        .any(has_authored)
    {
        return Err(Error::UnsupportedGraph);
    }
    quote_devices(project, effects(project))
}

pub(crate) fn quote_ordinary(project: &Project) -> Result<(usize, usize), Error> {
    quote_devices(project, effects(project).chain(instruments(project)))
}

pub(crate) fn quote_instruments(project: &Project) -> Result<usize, Error> {
    quote_devices(project, instruments(project)).map(|(bytes, _)| bytes)
}

fn instruments(project: &Project) -> impl Iterator<Item = &Device> {
    project
        .native_sources()
        .iter()
        .filter(|source| source.receiver().is_some())
        .map(|source| source.instrument())
}

fn quote_devices<'a>(
    project: &Project,
    devices: impl Iterator<Item = &'a Device>,
) -> Result<(usize, usize), Error> {
    let mut rows = 0usize;
    let mut bytes = 0usize;
    for device in devices.filter(|device| has_authored(device)) {
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
