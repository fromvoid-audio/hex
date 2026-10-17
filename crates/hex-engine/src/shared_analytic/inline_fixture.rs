//! Headless fixture over the ACTUAL bank/voice/retained-owner sampling path.
//! No registered device, native publication permit or extra PCM ownership.
use super::*;
use crate::{
    midi_note_bank::{PreparedMidiNoteBank, ScopedMidiNote, SharedContextRef},
    native_device_resources::PreparedDeviceResourceLease,
};

pub struct InlineResourceFixtureConfiguration {
    pub state: NativeDeviceState,
    pub resources: PreparedDeviceResourceLease,
    pub note: hex_lattice::PreparedAnalyticNote,
}
struct Instrument(hex_lattice::PreparedAnalyticNote);
impl PreparedNativeInstrument for Instrument {
    fn prepare_note(
        &self,
        _: u8,
        _: u8,
    ) -> Result<hex_lattice::PreparedAnalyticNote, hex_lattice_contract::DeviceContractError> {
        Ok(self.0)
    }
}
pub struct InlineResourceHostFixture {
    table: Arc<SharedAnalyticContextTable>,
    retained: Vec<Option<crate::shared_analytic::SharedContextLease>>,
    current: ScopedMidiNote,
    carried: crate::midi_template::MidiReleaseVoice,
}
impl InlineResourceHostFixture {
    pub fn prepare_on_control(
        sample_rate: u32,
        gate_frames: u64,
        current: InlineResourceFixtureConfiguration,
        carried: InlineResourceFixtureConfiguration,
    ) -> Result<Self, Invalid> {
        let mut editor =
            hex_project::Editor::new(Project::new(sample_rate).map_err(|e| Invalid(e.0))?, 8)
                .map_err(|e| Invalid(e.0))?;
        let hex_project::Created::Track(track) = editor
            .execute(hex_project::Command::AddMidiTrack {
                name: "inline scope fixture".into(),
            })
            .map_err(|e| Invalid(e.0))?
        else {
            return Err(Invalid("fixture track missing"));
        };
        let device = editor
            .project()
            .track(track)
            .and_then(hex_project::Track::instrument)
            .ok_or(Invalid("fixture instrument missing"))?
            .id();
        let prepare = |configuration: InlineResourceFixtureConfiguration| -> Result<
            (crate::shared_analytic::SharedContextLease, ScopedMidiNote),
            Invalid,
        > {
            hex_lattice::DevicePreparationContext {
                sample_rate,
                tuning: &hex_lattice::TuningTable::default(),
                resources: &configuration.resources,
            }
            .validate_for(&configuration.state)
            .map_err(|_| Invalid("fixture scope mismatch"))?;
            if configuration.state.asset_references().is_empty() {
                return Err(Invalid("fixture requires resource references"));
            }
            let owner = crate::owned_shared::OwnedShared::new_owned(
                SharedContextOwner::from_inline_resources_on_control(
                    SharedContextId::fresh()?,
                    SharedContextIdentity {
                        tuning: TuningIdentity::default(),
                        resource_scope: 0,
                        track,
                        device,
                        device_type: configuration.state.type_id(),
                        state: configuration.state,
                        sample_rate,
                    },
                    configuration
                        .resources
                        .try_into_configuration_on_control(4096)
                        .map_err(|_| Invalid("fixture configuration preparation refused"))?,
                    4096,
                )?,
            );
            let mut keys = PreparedMidiNoteBank::empty_keys();
            PreparedMidiNoteBank::add_key(&mut keys, 60, 127)?;
            let bank = PreparedMidiNoteBank::prepare(
                &mut keys,
                &Instrument(configuration.note),
                None,
                Some((0, &owner)),
            )?;
            let note = bank
                .sampler(bank.index(60, 127).ok_or(Invalid("fixture note missing"))?)
                .ok_or(Invalid("fixture sampler missing"))?;
            Ok((owner, note))
        };
        let (current_owner, current) = prepare(current)?;
        let (carried_owner, carried) = prepare(carried)?;
        let end = gate_frames
            .checked_add(carried.release_frames())
            .ok_or(Invalid("fixture release overflow"))?;
        let mut carried = crate::midi_template::MidiReleaseVoice::from_finite_span_preserving_gate(
            0,
            gate_frames,
            end,
            carried,
            0,
            0,
        )
        .map_err(|_| Invalid("fixture release refused"))?
        .ok_or(Invalid("fixture release is empty"))?;
        carried.detach(&[]);
        carried.set_context_ref(SharedContextRef::Retained(0));
        Ok(Self {
            table: Arc::new(SharedAnalyticContextTable {
                owners: vec![current_owner],
                ..Default::default()
            }),
            retained: vec![Some(carried_owner)],
            current,
            carried,
        })
    }
    pub fn sample_current(&self, age: u64, gate: u64) -> Result<crate::Frame, Invalid> {
        self.current
            .sample_at(&self.table, &self.retained, &[], 0, age, gate)
    }
    pub fn sample_carried(&self, age: u64, gate: u64) -> Result<crate::Frame, Invalid> {
        if gate != self.carried.gate_end_frame() - self.carried.start_frame() {
            return Err(Invalid("fixture release gate mismatch"));
        }
        self.carried
            .sample_at_with_continuation(age, &self.table, &self.retained, &[])
            .map(|frame| frame.unwrap_or([0.0; 2]))
    }
}
