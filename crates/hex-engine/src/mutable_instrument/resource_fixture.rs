//! Headless allocation fixture over the actual mutable track host. This cannot
//! register a device or confer native graph publication/retirement admission.
use super::*;
use crate::native_device_resources::DeviceResourceConfiguration;

/// Current authored filter path for the headless PCM/allocation oracle only.
/// No source/Box provenance or native publication authority is granted here.
pub fn sampler_clean_filter_runtime_for_allocation_probe(
    view: hex_lattice::DeviceResourceView<'_>,
    output_rate: u32,
) -> Result<Box<dyn hex_lattice::PreparedMutableInstrumentRuntime>, hex_devices::Invalid> {
    filtered_sampler_fixture(
        view,
        output_rate,
        hex_devices::sampler::ModulationSettings::default(),
        hex_devices::sampler::FilterEnvelopeSettings::default(),
    )
}
/// Actual enabled amplitude/pan LFO and clean filter; headless allocation only.
pub fn sampler_modulated_runtime_for_allocation_probe(
    view: hex_lattice::DeviceResourceView<'_>,
    output_rate: u32,
) -> Result<Box<dyn hex_lattice::PreparedMutableInstrumentRuntime>, hex_devices::Invalid> {
    use hex_devices::sampler::metadata::parameter::*;
    use hex_lattice_contract::ParameterValue;
    let mut modulation = hex_devices::sampler::ModulationSettings::default();
    for (id, value) in [
        (LFO_ENABLED, ParameterValue::Bool(true)),
        (LFO_WAVEFORM, ParameterValue::Choice(1)),
        (LFO_RATE, ParameterValue::Float(100.0)),
        (LFO_AMPLITUDE, ParameterValue::Float(100.0)),
        (LFO_PAN, ParameterValue::Float(50.0)),
    ] {
        modulation
            .patch_parameter(id, value)
            .map_err(|_| hex_devices::Invalid("sampler modulation allocation fixture refused"))?;
    }
    filtered_sampler_fixture(
        view,
        output_rate,
        modulation,
        hex_devices::sampler::FilterEnvelopeSettings::default(),
    )
}
/// Actual moving filter-envelope coefficient lookup; allocation fixture only.
pub fn sampler_filter_envelope_runtime_for_allocation_probe(
    view: hex_lattice::DeviceResourceView<'_>,
    output_rate: u32,
) -> Result<Box<dyn hex_lattice::PreparedMutableInstrumentRuntime>, hex_devices::Invalid> {
    use hex_devices::sampler::metadata::parameter::*;
    use hex_lattice_contract::ParameterValue;
    let mut envelope = hex_devices::sampler::FilterEnvelopeSettings::default();
    for (id, value) in [(FILTER_ENVELOPE, 12.0), (FILTER_ATTACK, 0.001)] {
        envelope
            .patch_parameter(id, ParameterValue::Float(value))
            .map_err(|_| {
                hex_devices::Invalid("sampler filter envelope allocation fixture refused")
            })?;
    }
    filtered_sampler_fixture(
        view,
        output_rate,
        hex_devices::sampler::ModulationSettings::default(),
        envelope,
    )
}
/// Actual bipolar cutoff LFO; source and graph admission remain separate.
pub fn sampler_filter_lfo_runtime_for_allocation_probe(
    view: hex_lattice::DeviceResourceView<'_>,
    output_rate: u32,
) -> Result<Box<dyn hex_lattice::PreparedMutableInstrumentRuntime>, hex_devices::Invalid> {
    use hex_devices::sampler::metadata::parameter::*;
    use hex_lattice_contract::ParameterValue;
    let refused = |_| hex_devices::Invalid("sampler cutoff LFO allocation fixture refused");
    let mut modulation = hex_devices::sampler::ModulationSettings::default();
    for (id, value) in [
        (LFO_ENABLED, ParameterValue::Bool(true)),
        (LFO_WAVEFORM, ParameterValue::Choice(3)),
        (LFO_RATE, ParameterValue::Float(100.0)),
    ] {
        modulation.patch_parameter(id, value).map_err(refused)?;
    }
    let mut envelope = hex_devices::sampler::FilterEnvelopeSettings::default();
    envelope
        .patch_parameter(LFO_FILTER, ParameterValue::Float(12.0))
        .map_err(refused)?;
    filtered_sampler_fixture(view, output_rate, modulation, envelope)
}
fn filtered_sampler_fixture(
    view: hex_lattice::DeviceResourceView<'_>,
    output_rate: u32,
    modulation: hex_devices::sampler::ModulationSettings,
    envelope: hex_devices::sampler::FilterEnvelopeSettings,
) -> Result<Box<dyn hex_lattice::PreparedMutableInstrumentRuntime>, hex_devices::Invalid> {
    use hex_devices::sampler::{
        metadata::parameter,
        state::{AuthoredMode, SamplerState},
    };
    use hex_lattice_contract::{DeviceAssetReference, ParameterValue};
    let refused = |_| hex_devices::Invalid("filtered sampler allocation fixture refused");
    let mut state = SamplerState::for_source_frames(view.pcm.frames()).map_err(refused)?;
    state.mode = AuthoredMode::Classic {
        loop_region: Some(state.region),
    };
    state.voices = 4;
    state.modulation = modulation;
    state.filter_envelope = envelope;
    state
        .filter
        .patch_parameter(parameter::FILTER_ENABLED, ParameterValue::Bool(true))
        .map_err(refused)?;
    state
        .filter
        .patch_parameter(parameter::FILTER_SLOPE, ParameterValue::Choice(0))
        .map_err(refused)?;
    let state = state
        .to_native()
        .map_err(refused)?
        .with_asset_references(vec![DeviceAssetReference {
            slot: view.slot,
            asset: view.identity.asset,
        }])
        .map_err(refused)?;
    let views = [view];
    let resources = hex_lattice::BorrowedDeviceResources(&views);
    let prepared = hex_devices::sampler::state::prepare_runtime_on_control(
        &state,
        &hex_lattice::DevicePreparationContext {
            sample_rate: output_rate,
            tuning: &hex_lattice::TuningTable::default(),
            resources: &resources,
        },
        MAX_MUTABLE_EVENTS_PER_QUERY,
    )
    .map_err(refused)?;
    Ok(Box::new(prepared))
}

/// Unregistered sampler DSP allocation seam. It grants neither source storage
/// provenance nor host graph admission; the audio probe retains its own input.
pub fn sampler_runtime_for_allocation_probe(
    view: hex_lattice::DeviceResourceView<'_>,
    output_rate: u32,
) -> Result<Box<dyn hex_lattice::PreparedMutableInstrumentRuntime>, hex_devices::Invalid> {
    use hex_devices::sampler::{
        Mode, PreparedSamplerPlan, Region, SampleChannels, SampleShape, Settings,
    };
    let channels = match view.pcm {
        hex_lattice::DeviceResourcePcm::Mono(_) => SampleChannels::Mono,
        hex_lattice::DeviceResourcePcm::Stereo(_) => SampleChannels::Stereo,
    };
    let region = Region {
        start: 0,
        end: view.pcm.frames(),
    };
    let mut settings = Settings::new(region);
    settings.mode = Mode::Classic {
        loop_region: Some(region),
    };
    let plan = PreparedSamplerPlan::new(
        SampleShape::new(view.pcm.frames(), view.sample_rate, channels)?,
        output_rate,
        settings,
    )?;
    Ok(Box::new(
        plan.bind_source_on_control(view)?
            .prepare_runtime_on_control(
                4,
                MAX_MUTABLE_EVENTS_PER_QUERY,
                &hex_lattice::TuningTable::default(),
            )?,
    ))
}

pub struct MutableResourceHostFixture {
    runtime: MutableInstrumentTrackRuntime,
    track: TrackId,
    onsets: MutableOnsetBuffer,
    events: Vec<NativeInstrumentEvent>,
}
impl MutableResourceHostFixture {
    pub fn prepare_on_control(
        state: &hex_lattice_contract::NativeDeviceState,
        sample_rate: u32,
        resources: DeviceResourceConfiguration,
    ) -> Result<Self, crate::Invalid> {
        let refused = || crate::Invalid("mutable resource fixture preparation refused");
        let prepared = hex_lattice::conformance::resource_pcm::mutable::prepare(
            state,
            &hex_lattice::DevicePreparationContext {
                sample_rate,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &resources,
            },
            0,
        )
        .map_err(|_| refused())?;
        let runtime = MutableInstrumentTrackRuntime::new_with_configuration(
            &prepared,
            sample_rate,
            Some(resources),
        )
        .map_err(|_| refused())?;
        let mut editor = hex_project::Editor::new(
            hex_project::Project::new(sample_rate).map_err(|_| refused())?,
            8,
        )
        .map_err(|_| refused())?;
        let hex_project::Created::Track(track) = editor
            .execute(hex_project::Command::AddMidiTrack {
                name: "mutable resource fixture".into(),
            })
            .map_err(|_| refused())?
        else {
            return Err(refused());
        };
        let mut events = Vec::new();
        events
            .try_reserve_exact(MAX_MUTABLE_EVENTS_PER_QUERY)
            .map_err(|_| refused())?;
        Ok(Self {
            runtime,
            track,
            onsets: MutableOnsetBuffer::default(),
            events,
        })
    }
    pub fn reset(&mut self) {
        self.runtime.reset();
    }
    /// Actual host preflight/transaction/borrow path. A refused or explicitly
    /// speculative segment restores host occurrence and provider state.
    pub fn process_segment(
        &mut self,
        start: u64,
        output: &mut [[f32; 2]],
        commit: bool,
    ) -> Result<(), crate::Invalid> {
        self.runtime.begin_block();
        let result = self.runtime.process_segment(
            self.track,
            start,
            start,
            output,
            &mut self.onsets,
            &[],
            &mut self.events,
        );
        if result.is_ok() && commit {
            self.runtime.commit_block();
        } else {
            self.runtime.rollback_block();
        }
        result.map_err(|_| crate::Invalid("mutable resource fixture segment refused"))
    }
}
