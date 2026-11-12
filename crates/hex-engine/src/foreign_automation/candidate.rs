//! Canonical VST3 lanes for Arrangement preparation.
use super::{Knot, Lane, Program, QuantumProfile};
use crate::{AudioStorageError as Error, Invalid};
use hex_lattice::{DevicePreparationContext, NativeDeviceRegistry, ProcessorInstanceIdentity};
use hex_lattice_contract::DeviceAutomationTarget;
use hex_project::{Device, DeviceAutomation, Project};

pub(crate) enum PreparationError {
    Source(Error),
    Program(Invalid),
}

impl PreparationError {
    pub(crate) fn into_storage(self) -> Error {
        match self {
            Self::Source(error) => error,
            Self::Program(_) => Error::UnsupportedGraph,
        }
    }
}

impl From<PreparationError> for Invalid {
    fn from(error: PreparationError) -> Self {
        match error {
            PreparationError::Program(reason) => reason,
            PreparationError::Source(Error::SourceMismatch) => {
                Self("vst3 automation source differs from the prepared device")
            }
            PreparationError::Source(Error::Overflow) => {
                Self("vst3 automation storage size overflow")
            }
            PreparationError::Source(_) => Self("vst3 automation source is unavailable"),
        }
    }
}

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
    for device in devices
        .filter(|device| device.state().type_id() == hex_lattice_contract::VST3_DEVICE_TYPE_ID)
    {
        bytes = bytes
            .checked_add(std::mem::size_of::<Program>() + 3 * std::mem::size_of::<usize>())
            .and_then(|bytes| {
                bytes.checked_add(
                    super::MAX_TARGETS
                        * (std::mem::size_of::<Lane>() + std::mem::size_of::<Knot>()),
                )
            })
            .ok_or(Error::Overflow)?;
        rows = rows.checked_add(1).ok_or(Error::Overflow)?;
        for lane in lanes(device) {
            let knots = super::knot_reservation(&lane.points, project.tempo_map())
                .ok_or(Error::Overflow)?
                .max(1);
            bytes = bytes
                .checked_add(
                    knots
                        .checked_mul(std::mem::size_of::<Knot>())
                        .ok_or(Error::Overflow)?,
                )
                .and_then(|bytes| {
                    bytes.checked_add(super::touch::Overlay::reservation_bytes(
                        &lane.touch,
                        knots,
                    )?)
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
) -> Result<Option<hex_lattice::PreparedForeignAutomation>, PreparationError> {
    let Some(source) = exact_source(device, identity, effects, processor, tempo.sample_rate())
        .map_err(PreparationError::Source)?
    else {
        return if has_authored(device) {
            Err(PreparationError::Source(Error::UnsupportedGraph))
        } else {
            Ok(None)
        };
    };
    let revision = source.revision;
    let profile = if processor.is_instrument_profile() {
        QuantumProfile::for_arrangement_instrument(source.quantum_frames, region)
    } else {
        QuantumProfile::arrangement(source.quantum_frames, region)
    };
    let program = Program::prepare_source(device.automation(), source, tempo, profile)
        .map_err(PreparationError::Program)?;
    let Some(program) = program else {
        return Ok(None);
    };
    let overflow = || PreparationError::Source(Error::Overflow);
    let bytes = program.backing_bytes().ok_or_else(overflow)?;
    hex_lattice::PreparedForeignAutomation::new(source.binding, revision, program, bytes)
        .map(Some)
        .map_err(|_| overflow())
}

pub(crate) fn exact_source<'a>(
    device: &Device,
    identity: ProcessorInstanceIdentity,
    effects: &'a NativeDeviceRegistry<'_>,
    processor: hex_lattice::PreparedForeignProcessor,
    sample_rate: u32,
) -> Result<Option<hex_lattice::ForeignAutomationSource<'a>>, Error> {
    let context = DevicePreparationContext {
        sample_rate,
        tuning: &hex_lattice::TuningTable::default(),
        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
    };
    let source = effects
        .definition(device.state().type_id())
        .ok_or(Error::UnsupportedGraph)?
        .foreign_automation_source(identity, device.state(), &context)
        .map_err(|_| Error::UnsupportedGraph)?;
    if source.is_some_and(|source| {
        source.sample_rate != sample_rate
            || source.quantum_frames != processor.quantum_frames()
            || Some(source.binding) != processor.binding()
            || source.binding.instance != identity.device_id()
    }) {
        return Err(Error::SourceMismatch);
    }
    Ok(source)
}
