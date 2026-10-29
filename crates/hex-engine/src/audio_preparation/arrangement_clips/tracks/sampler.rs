//! Constructor-paired finite Arrangement MIDI storage for one current sampler.
use super::*;
use crate::native_device_resources::current_sampler::CurrentSamplerConfiguration;
use crate::{
    InstrumentTrack, owned_shared::SharedDeclaration, shared_analytic::SharedAnalyticContextTable,
};

/// Explicit upper bounds for the existing compiler's admitted finite occurrence class.
/// Rust 1.94 stable driftsort uses max(N,48) scratch element slots beyond
/// its <=20-element insertion path (48 = SMALL_SORT_GENERAL_SCRATCH_LEN).
/// Pinned sources:
/// https://github.com/rust-lang/rust/blob/1.94.0/library/core/src/slice/sort/stable/mod.rs
/// https://github.com/rust-lang/rust/blob/1.94.0/library/core/src/slice/sort/shared/smallsort.rs
/// This includes the small-slice heap minimum, even
/// where the implementation can use its stack buffer. Sort does not overlap
/// density validation; summing those phase bounds is conservative.
pub(crate) fn compiler_quote(
    track: &hex_project::Track,
) -> Result<(usize, usize), AudioStorageError> {
    let notes = track
        .midi_clips()
        .iter()
        .filter(|clip| clip.active())
        .try_fold(0usize, |total, clip| {
            let playback = clip
                .source_playback()
                .map_err(|_| AudioStorageError::UnsupportedGraph)?;
            clip.notes()
                .iter()
                .filter(|note| note.active())
                .try_fold(total, |total, note| {
                    let count = usize::try_from(
                        playback
                            .note_occurrences(note.start(), note.duration())
                            .map_err(|_| AudioStorageError::UnsupportedGraph)?
                            .count(),
                    )
                    .map_err(|_| AudioStorageError::Overflow)?;
                    total
                        .checked_add(count)
                        .filter(|n| *n <= crate::midi_compile::MAX_COMPILED_MIDI_OCCURRENCES)
                        .ok_or(AudioStorageError::UnsupportedGraph)
                })
        })?;
    let workspace = compiler_workspace_quote(notes, track.midi_clips().len())?;
    let (payload, overhead) = crate::owned_shared::OwnedShared::<
        crate::midi_note_bank::PreparedMidiNoteBank,
    >::declared_parts()
    .ok_or(AudioStorageError::UnknownStorage)?;
    let retained = notes
        .checked_mul(
            mem::size_of::<crate::midi_compile::CompiledNoteSpan>()
                .checked_add(mem::size_of::<crate::midi_compile::SpanNode>())
                .ok_or(AudioStorageError::Overflow)?,
        )
        .and_then(|n| {
            n.checked_add(if track.midi_clips().is_empty() {
                0
            } else {
                payload.checked_add(overhead)?
            })
        })
        .ok_or(AudioStorageError::Overflow)?;
    let retained = retained
        .checked_add(
            crate::midi_clip_volume::arrangement::PreparedArrangementVolumes::closed_quote(track)?,
        )
        .ok_or(AudioStorageError::Overflow)?;
    Ok((retained, workspace))
}
pub(crate) fn compiler_workspace_quote(
    notes: usize,
    clips: usize,
) -> Result<usize, AudioStorageError> {
    let sort_slots = if notes > 20 { notes.max(48) } else { 0 };
    let sort_bytes = sort_slots
        .checked_mul(mem::size_of::<crate::midi_compile::CompiledNoteSpan>())
        .ok_or(AudioStorageError::Overflow)?;
    let density_stride = mem::size_of::<u64>()
        .checked_add(
            mem::size_of::<(u64, bool)>()
                .checked_mul(2)
                .ok_or(AudioStorageError::Overflow)?,
        )
        .ok_or(AudioStorageError::Overflow)?;
    // NoteKeys is Vec<u16>, appended once per emitted authored note. Pinned
    // RawVec grows from min4 slots, then doubles; max(4,3N) also covers old+new
    // allocation overlap during growth, before the old key buffer is freed.
    // https://github.com/rust-lang/rust/blob/1.94.0/library/alloc/src/raw_vec/mod.rs
    let key_slots = if notes == 0 {
        0
    } else {
        notes
            .checked_mul(3)
            .ok_or(AudioStorageError::Overflow)?
            .max(4)
    };
    let keys = key_slots
        .checked_mul(mem::size_of::<u16>())
        .ok_or(AudioStorageError::Overflow)?;
    notes
        .checked_mul(density_stride)
        .and_then(|n| n.checked_add(keys))
        .and_then(|n| n.checked_add(sort_bytes))
        .and_then(|n| n.checked_add(clips.checked_mul(mem::size_of::<usize>())?))
        .ok_or(AudioStorageError::Overflow)
}

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
    volume_identity: Option<crate::midi_clip_volume::arrangement::ArrangementVolumeIdentity>,
    installed: bool,
    looping_source: bool,
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
            || self.volume_identity.is_some()
            || self.handles != [0; 2]
            || config.track != track.id()
            || track.instrument().is_none_or(|d| d.id() != config.device)
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.looping_source = track.midi_clips().iter().any(|c| c.active() && c.looping());
        self.track = Some(track.id());
        self.device = Some(config.device);
        self.scope = Some(scope);
        self.input = config.input_gain;
        self.output = config.output_gain;
        self.tuning = contexts.tuning_identity();
        self.handles = [handle()?, handle()?];
        let (contexts, paired) = contexts.clone_on_control().into_paired_parts_on_control();
        self.contexts = Some(paired);
        let instrument = crate::midi_compile::compile_current_sampler_track(
            project,
            track,
            &config.prepared,
            contexts,
            &mut self.bank,
        )
        .map_err(invalid)?;
        let volume_identity = instrument
            .as_ref()
            .and_then(|i| i.volumes.as_ref())
            .map(|volumes| volumes.preparation_identity())
            .transpose()
            .map_err(invalid)?;
        // All fallible preparation precedes these persistent owner moves.
        self.instrument = instrument;
        self.volume_identity = volume_identity;
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
        track.mutable_looping_source = self.looping_source;
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
                || track.mutable_looping_source != self.looping_source
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
            match (instrument.volumes.as_ref(), self.volume_identity.as_ref()) {
                (None, None) => {}
                (Some(value), Some(identity)) if identity.matches(value) => {}
                _ => return Err(AudioStorageError::SourceMismatch),
            }
        } else if self.volume_identity.is_some() {
            return Err(AudioStorageError::SourceMismatch);
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
        let bytes = instrument
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
            .ok_or(AudioStorageError::Overflow)?;
        let mut bytes = bytes;
        if let Some(volumes) = &instrument.volumes {
            bytes = bytes
                .checked_add(volumes.allocated_row_bytes().map_err(invalid)?)
                .and_then(|n| n.checked_add(volumes.retained_control_metadata_bytes()))
                .ok_or(AudioStorageError::Overflow)?;
        }
        if let Some(identity) = &self.volume_identity {
            bytes = bytes
                .checked_add(identity.retained_bytes()?)
                .ok_or(AudioStorageError::Overflow)?;
        }
        Ok(bytes)
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
        if let Some(volumes) = &instrument.volumes {
            volumes.visit_storage(ledger)?;
        }
        if let Some(identity) = &self.volume_identity {
            identity.visit_storage(ledger)?;
        }
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
        track.mutable_looping_source = false;
        self.installed = false;
        Ok(())
    }
    pub(super) fn dispose(&mut self) {
        drop(self.instrument.take());
        drop(self.volume_identity.take());
        self.bank = None;
        self.contexts = None;
        self.device = None;
        self.track = None;
        self.scope = None;
        self.handles = [0; 2];
        self.installed = false;
    }
}
