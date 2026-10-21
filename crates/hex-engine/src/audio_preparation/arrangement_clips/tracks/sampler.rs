//! Constructor-paired finite Arrangement MIDI storage for one current sampler.
use super::*;
use crate::native_device_resources::current_sampler::CurrentSamplerConfiguration;
use crate::{
    InstrumentTrack, owned_shared::SharedDeclaration, shared_analytic::SharedAnalyticContextTable,
};

#[derive(Default)]
pub(super) struct SamplerTrackConstruction {
    device: Option<hex_project::DeviceId>,
    track: Option<hex_project::TrackId>,
    scope: Option<u64>,
    input: f32,
    output: f32,
    tuning: crate::shared_analytic::TuningIdentity,
    bank: Option<SharedDeclaration>,
    contexts: Option<SharedDeclaration>,
    handles: [u64; 2],
    instrument: Option<InstrumentTrack>,
    installed: bool,
}
impl SamplerTrackConstruction {
    pub(super) fn handles(&self) -> usize {
        if self.present() { 2 } else { 0 }
    }
    pub(super) fn present(&self) -> bool {
        self.device.is_some()
    }
    pub(super) fn reserve(
        &mut self,
        project: &hex_project::Project,
        track: &hex_project::Track,
        config: &CurrentSamplerConfiguration,
        scope: u64,
        contexts: &OwnedShared<SharedAnalyticContextTable>,
    ) -> Result<(), AudioStorageError> {
        if self.present()
            || self.instrument.is_some()
            || self.handles != [0; 2]
            || config.track != track.id()
            || track.instrument().is_none_or(|d| d.id() != config.device)
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.track = Some(track.id());
        self.device = Some(config.device);
        self.scope = Some(scope);
        self.input = config.input_gain;
        self.output = config.output_gain;
        self.tuning = contexts.tuning_identity();
        self.handles = [handle()?, handle()?];
        let (contexts, paired) = contexts.clone_on_control().into_paired_parts_on_control();
        self.contexts = Some(paired);
        self.instrument = crate::midi_compile::compile_current_sampler_track(
            project,
            track,
            &config.prepared,
            contexts,
            &mut self.bank,
        )
        .map_err(invalid)?;
        Ok(())
    }
    pub(super) fn install(&mut self, track: &mut Track) -> Result<(), AudioStorageError> {
        if !self.present() {
            return Ok(());
        }
        if self.installed || track.track_id != self.track || track.instrument.is_some() {
            return Err(AudioStorageError::SourceMismatch);
        }
        track.instrument = self.instrument.take();
        track.instrument_id = self.device;
        track.instrument_input_gain = self.input;
        track.instrument_output_gain = self.output;
        track.instrument_latency = Some(0);
        track.instrument_runtime_latency = Some(0);
        track.tuning_identity = self.tuning;
        track.mutable_instrument = true;
        self.installed = true;
        Ok(())
    }
    fn actual<'a>(
        &'a self,
        track: Option<&'a Track>,
    ) -> Result<Option<&'a InstrumentTrack>, AudioStorageError> {
        if self.installed {
            let track = track.ok_or(AudioStorageError::SourceMismatch)?;
            if self.instrument.is_some()
                || track.track_id != self.track
                || track.instrument_id != self.device
                || track.instrument_state.is_some()
                || track.instrument_input_gain != self.input
                || track.instrument_output_gain != self.output
                || !track.mutable_instrument
                || track.mutable_looping_source
            {
                return Err(AudioStorageError::SourceMismatch);
            }
            Ok(track.instrument.as_ref())
        } else {
            Ok(self.instrument.as_ref())
        }
    }
    pub(super) fn validate(&self, track: Option<&Track>) -> Result<(), AudioStorageError> {
        if let Some(instrument) = self.actual(track)? {
            instrument.bank.require_empty_current_sampler()?;
            if !instrument.mutable
                || self.bank.is_none()
                || self.contexts.is_none()
                || self.handles.contains(&0)
            {
                return Err(AudioStorageError::UnknownStorage);
            }
        }
        Ok(())
    }
    pub(super) fn backing_bytes(&self, track: Option<&Track>) -> Result<usize, AudioStorageError> {
        let Some(instrument) = self.actual(track)? else {
            return Ok(0);
        };
        self.validate(track)?;
        let (payload, overhead) = self
            .bank
            .as_ref()
            .and_then(|d| d.parts(&instrument.bank))
            .ok_or(AudioStorageError::UnknownStorage)?;
        instrument
            .spans
            .capacity()
            .checked_mul(std::mem::size_of::<crate::midi_compile::CompiledNoteSpan>())
            .and_then(|n| {
                n.checked_add(
                    instrument
                        .nodes
                        .capacity()
                        .checked_mul(std::mem::size_of::<crate::midi_compile::SpanNode>())?,
                )
            })
            .and_then(|n| n.checked_add(payload))
            .and_then(|n| n.checked_add(overhead))
            .ok_or(AudioStorageError::Overflow)
    }
    pub(super) fn visit<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
        track: Option<&'a Track>,
    ) -> Result<(), AudioStorageError> {
        use crate::audio_storage::Owner;
        let Some(instrument) = self.actual(track)? else {
            return Ok(());
        };
        self.validate(track)?;
        ledger.vec(&instrument.spans)?;
        ledger.vec(&instrument.nodes)?;
        ledger.shared(
            &instrument.bank,
            self.bank.as_ref().and_then(|d| d.parts(&instrument.bank)),
            Owner::exclusive(&instrument.bank),
            self.handles[0],
            None,
        )?;
        ledger.shared(
            &instrument.shared_contexts,
            self.contexts
                .as_ref()
                .and_then(|d| d.parts(&instrument.shared_contexts)),
            Owner::exclusive(&instrument.shared_contexts),
            self.handles[1],
            None,
        )
    }
    pub(super) fn restore(&mut self, track: &mut Track) -> Result<(), AudioStorageError> {
        if !self.present() {
            return Ok(());
        }
        self.validate(Some(track))?;
        self.instrument = track.instrument.take();
        track.instrument_id = None;
        track.instrument_input_gain = 1.0;
        track.instrument_output_gain = 1.0;
        track.instrument_latency = None;
        track.instrument_runtime_latency = None;
        track.mutable_instrument = false;
        self.installed = false;
        Ok(())
    }
    pub(super) fn dispose(&mut self) {
        drop(self.instrument.take());
        self.bank = None;
        self.contexts = None;
        self.device = None;
        self.track = None;
        self.scope = None;
        self.handles = [0; 2];
        self.installed = false;
    }
}
