//! Local Console MIDI templates born in the same sealed CURRENT constructor.
//!
//! Each clip and its validation sidecar retain a canonical source identity. Charge
//! its actual note capacity conservatively per owning token in preflight, installed
//! or restored capacity, and the named control-metadata ledger allowance. These
//! tokens were not born here: no shared declaration or lifetime authority is minted.
use super::*;
use crate::midi_template::CurrentSamplerEventCustody;
use crate::native_device_resources::current_sampler::CurrentSamplerMetadata;

pub(super) struct MidiTarget {
    clip: ClipId,
    track: TrackId,
    scene: SceneId,
    device: DeviceId,
    scope: u64,
    tuning: crate::shared_analytic::TuningIdentity,
    source_identity: MidiSourceIdentity,
    volume_identity: Option<crate::midi_clip_volume::VolumePreparationIdentity>,
    active: bool,
    playback_offset: hex_project::BeatTime,
    limits: crate::midi_compile::MutableInstrumentLimits,
    work: SessionWorkEnvelope,
    factory_address: usize,
    source_address: usize,
    instrument_address: usize,
    source_declaration: SharedDeclaration,
    instrument_declaration: SharedDeclaration,
    events: CurrentSamplerEventCustody,
    handles: [u64; 2],
}
fn identity_metadata_bytes(identity: &MidiSourceIdentity) -> Result<usize, AudioStorageError> {
    identity
        .retained_control_metadata_bytes()
        .ok_or(AudioStorageError::Overflow)
}
/// Both the prepared clip and its continuity sidecar retain canonical source
/// and volume identities. Curves use conservative control-metadata allowances;
/// this grants no declaration or lifetime authority over their external Arc.
/// Used by the caller reservation as well as the constructor's pre-birth quote.
pub(crate) fn canonical_source_metadata_bytes(
    project: &Project,
) -> Result<usize, AudioStorageError> {
    project
        .session_cells()
        .iter()
        .try_fold(0usize, |total, cell| {
            let SessionCellContent::Midi(clip) = cell.content() else {
                return Ok(total);
            };
            identity_metadata_bytes(&clip.source_identity())?
                .checked_add(
                    clip.volume()
                        .retained_control_metadata_bytes()
                        .ok_or(AudioStorageError::Overflow)?,
                )
                .and_then(|bytes| bytes.checked_mul(2))
                .and_then(|bytes| total.checked_add(bytes))
                .ok_or(AudioStorageError::Overflow)
        })
}
pub(crate) fn compiler_quote(project: &Project) -> Result<(usize, usize), AudioStorageError> {
    let mut retained = canonical_source_metadata_bytes(project)?;
    let mut scratch = 0usize;
    for cell in project.session_cells() {
        let SessionCellContent::Midi(clip) = cell.content() else {
            continue;
        };
        let (sr, ss) = PreparedMidiSourceTemplate::current_sampler_quote(if clip.active() {
            clip.source().notes().len()
        } else {
            0
        })?;
        let (ir, is) = PreparedMidiInstrumentTemplate::current_sampler_quote(if clip.active() {
            clip.source().notes().len()
        } else {
            0
        })?;
        retained = retained
            .checked_add(sr)
            .and_then(|n| n.checked_add(ir))
            .and_then(|n| n.checked_add(clip.name().len()))
            .ok_or(AudioStorageError::Overflow)?;
        scratch = scratch.max(ss.checked_add(is).ok_or(AudioStorageError::Overflow)?);
    }
    Ok((retained, scratch))
}
/// Conservative cardinality envelope for callers, not a storage/custody proof.
pub(crate) fn cardinality_quote(
    notes: usize,
    clips: usize,
) -> Result<(usize, usize), AudioStorageError> {
    if clips == 0 {
        return if notes == 0 {
            Ok((0, 0))
        } else {
            Err(AudioStorageError::SourceMismatch)
        };
    }
    let (sr, ss) = PreparedMidiSourceTemplate::current_sampler_quote(notes)?;
    let (ir, is) = PreparedMidiInstrumentTemplate::current_sampler_quote(notes)?;
    let (se, _) = PreparedMidiSourceTemplate::current_sampler_quote(0)?;
    let (ie, _) = PreparedMidiInstrumentTemplate::current_sampler_quote(0)?;
    let retained = sr
        .checked_add(ir)
        .and_then(|n| n.checked_add(se.checked_add(ie)?.checked_mul(clips - 1)?))
        .ok_or(AudioStorageError::Overflow)?;
    Ok((
        retained,
        ss.checked_add(is).ok_or(AudioStorageError::Overflow)?,
    ))
}
impl SessionAssembly {
    pub(crate) fn reserve_current_sampler_midi_rows(
        &mut self,
        project: &Project,
    ) -> Result<(), AudioStorageError> {
        if !self.sampler_midi.is_empty() || self.unused_midi.capacity() != 0 {
            return Err(AudioStorageError::Capacity);
        }
        let count = project
            .session_cells()
            .iter()
            .filter(|c| matches!(c.content(), SessionCellContent::Midi(_)))
            .count();
        reserve(&mut self.sampler_midi, count, &mut self.growth)?;
        reserve(&mut self.unused_midi, count, &mut self.growth)?;
        Ok(())
    }
    pub(crate) fn prepare_current_sampler_midi(
        &mut self,
        project: &Project,
        metadata: &CurrentSamplerMetadata,
        contexts: &OwnedShared<crate::shared_analytic::SharedAnalyticContextTable>,
    ) -> Result<(), AudioStorageError> {
        if self.phase != AssemblyPhase::Reserved
            || !self.unused_midi.is_empty()
            || !self.sampler_midi.is_empty()
            || self.sampler_midi_scope != Some(metadata.id)
            || self.sampler_context_address != Some(Arc::as_ptr(contexts.arc()) as usize)
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        let max =
            crate::PreparedTempoMap::validate_canonical(project.tempo_map(), project.sample_rate())
                .map_err(|_| AudioStorageError::UnsupportedGraph)?;
        let timing =
            crate::prepared_tempo::CanonicalMidiTiming::for_validated(project.tempo_map(), max);
        for cell in project.session_cells() {
            let SessionCellContent::Midi(clip) = cell.content() else {
                continue;
            };
            if self.unused_midi.len() == self.unused_midi.capacity()
                || self.sampler_midi.len() == self.sampler_midi.capacity()
            {
                return Err(AudioStorageError::Capacity);
            }
            let config = metadata
                .configurations()
                .iter()
                .find(|c| c.track == cell.track())
                .ok_or(AudioStorageError::SourceMismatch)?;
            let factory = self
                .current_sampler_values(None)?
                .iter()
                .find(|f| f.track == cell.track())
                .ok_or(AudioStorageError::SourceMismatch)?;
            let factory_address = factory.factory.as_ref()
                as *const dyn PreparedMutableInstrumentRuntime
                as *const () as usize;
            let limits = crate::midi_compile::MutableInstrumentLimits::from_factory(
                factory.factory.as_ref(),
            )
            .map_err(|_| AudioStorageError::UnsupportedGraph)?;
            let device = project
                .track(cell.track())
                .and_then(|t| t.instrument())
                .filter(|d| {
                    d.id() == config.device && !d.bypassed() && config.matches_state(d.state())
                })
                .ok_or(AudioStorageError::SourceMismatch)?;
            let prepared =
                hex_devices::sampler::catalog::PreparedSamplerInstrument::from_runtime_on_control(
                    config.prepared.clone(),
                );
            for note in clip
                .source()
                .notes()
                .iter()
                .filter(|note| clip.active() && note.active())
            {
                prepared
                    .validate_event_note(note.pitch(), note.velocity())
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
            }
            let source = PreparedMidiSourceTemplate::prepare_source_window_admitted(
                cell.track(),
                clip.id(),
                clip.source(),
                clip.playback_offset(),
                clip.active(),
                project.sample_rate(),
                u64::from(max),
            )
            .map_err(|_| AudioStorageError::UnsupportedGraph)?;
            let (source, source_declaration) =
                OwnedShared::new_owned(source).into_paired_parts_on_control();
            let (instrument, events) =
                PreparedMidiInstrumentTemplate::prepare_current_sampler_events(
                    &source, limits, contexts,
                )?;
            let (instrument, instrument_declaration) =
                OwnedShared::new_owned(instrument).into_paired_parts_on_control();
            let work =
                work_for(&source, &instrument).map_err(|_| AudioStorageError::UnsupportedGraph)?;
            let volume =
                crate::midi_clip_volume::PreparedMidiClipVolume::prepare_session(clip, &timing)
                    .map_err(|_| AudioStorageError::UnsupportedGraph)?;
            let volume_identity = volume.as_ref().map(|p| p.preparation_identity());
            let row = MidiTarget {
                clip: clip.id(),
                track: cell.track(),
                scene: cell.scene(),
                device: device.id(),
                scope: metadata.id,
                tuning: contexts.tuning_identity(),
                source_identity: clip.source_identity(),
                volume_identity,
                active: clip.active(),
                playback_offset: clip.playback_offset(),
                limits,
                work,
                factory_address,
                source_address: Arc::as_ptr(&source) as usize,
                instrument_address: Arc::as_ptr(&instrument) as usize,
                source_declaration,
                instrument_declaration,
                events,
                handles: [
                    crate::audio_only_inputs::handle()?,
                    crate::audio_only_inputs::handle()?,
                ],
            };
            let value = PreparedSessionMidiClip {
                id: clip.id(),
                track: cell.track(),
                scene: cell.scene(),
                name: clone_string(clip.name()).map_err(|_| AudioStorageError::Allocation)?,
                source_identity: clip.source_identity(),
                active: clip.active(),
                playback_offset: clip.playback_offset(),
                source,
                volume,
                instrument,
                instrument_id: Some(device.id()),
                instrument_state: None,
                tuning_identity: contexts.tuning_identity(),
                instrument_enabled: clip.active(),
                mutable_limits: Some(limits),
                work,
            };
            // Both Vec destinations were reserved. No fallible work between moves.
            self.unused_midi.push(value);
            self.sampler_midi.push(row);
        }
        // Stable IDs are unique. Each sidecar retains its exact born Arc
        // addresses; sorting never manufactures a declaration for another owner.
        self.unused_midi.sort_unstable_by_key(|c| c.id);
        self.sampler_midi.sort_unstable_by_key(|r| r.clip);
        self.current_sampler_midi_bytes(None)?;
        Ok(())
    }
    fn current_sampler_midi_values<'a>(
        &'a self,
        external: Option<&'a PreparedSessionInventory>,
    ) -> Result<&'a Vec<PreparedSessionMidiClip>, AudioStorageError> {
        let values = self
            .inventory
            .as_ref()
            .or(external)
            .map(|i| &i.midi_clips)
            .unwrap_or(&self.unused_midi);
        if (self.inventory.is_some() || external.is_some()) && self.unused_midi.capacity() != 0
            || values.len() != self.sampler_midi.len()
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        self.current_sampler_values(external)?;
        for (clip, row) in values.iter().zip(&self.sampler_midi) {
            match (clip.volume.as_ref(), row.volume_identity.as_ref()) {
                (None, None) => {}
                (Some(program), Some(identity)) if identity.matches(program) => {}
                _ => return Err(AudioStorageError::SourceMismatch),
            }
            if Some(row.scope) != self.sampler_midi_scope
                || !self.current_sampler_values(external)?.iter().any(|f| {
                    f.track == row.track
                        && f.factory.as_ref() as *const dyn PreparedMutableInstrumentRuntime
                            as *const () as usize
                            == row.factory_address
                })
                || clip.id != row.clip
                || clip.track != row.track
                || clip.scene != row.scene
                || clip.source_identity != row.source_identity
                || clip.instrument_id != Some(row.device)
                || clip.instrument_state.is_some()
                || clip.tuning_identity != row.tuning
                || clip.active != row.active
                || clip.playback_offset != row.playback_offset
                || clip.instrument_enabled != row.active
                || clip.work != row.work
                || clip.mutable_limits != Some(row.limits)
                || Some(clip.instrument.current_sampler_context_address())
                    != self.sampler_context_address
                || clip.source.track_id() != row.track
                || clip.source.clip_id() != row.clip
                || Arc::as_ptr(&clip.source) as usize != row.source_address
                || Arc::as_ptr(&clip.instrument) as usize != row.instrument_address
                || !clip.instrument.is_event_source()
            {
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        Ok(values)
    }
    pub(super) fn current_sampler_midi_facts(
        &self,
        external: Option<&PreparedSessionInventory>,
    ) -> Result<(SessionWorkEnvelope, usize, usize), AudioStorageError> {
        let values = self.current_sampler_midi_values(external)?;
        let mut work = SessionWorkEnvelope::default();
        let mut refs = 0usize;
        let mut bank_bytes = 0usize;
        for track in self.tracks.iter().chain(
            self.inventory
                .as_ref()
                .or(external)
                .into_iter()
                .flat_map(|i| i.tracks.iter()),
        ) {
            let per_track = values
                .iter()
                .filter(|c| c.track == track.id)
                .fold(SessionWorkEnvelope::default(), |n, c| {
                    n.component_max(c.work)
                });
            if self.phase == AssemblyPhase::Installed && track.work != per_track {
                return Err(AudioStorageError::SourceMismatch);
            }
            work = work
                .checked_sum(per_track)
                .map_err(|_| AudioStorageError::Overflow)?;
        }
        for clip in values {
            refs = refs
                .checked_add(
                    clip.instrument
                        .index_reference_count()
                        .map_err(|_| AudioStorageError::UnsupportedGraph)?,
                )
                .ok_or(AudioStorageError::Overflow)?;
            bank_bytes = bank_bytes
                .checked_add(mem::size_of::<
                    Arc<crate::midi_note_bank::PreparedMidiNoteBank>,
                >())
                .ok_or(AudioStorageError::Overflow)?;
        }
        crate::midi_note_bank::PreparedMidiNoteBank::validate_project_budget(
            bank_bytes,
            refs,
            crate::midi_note_bank::MAX_SESSION_NOTE_BANK_REFERENCES,
        )
        .map_err(|_| AudioStorageError::ReservationExceeded)?;
        Ok((work, bank_bytes, refs))
    }
    pub(super) fn current_sampler_midi_bytes(
        &self,
        external: Option<&PreparedSessionInventory>,
    ) -> Result<usize, AudioStorageError> {
        let values = self.current_sampler_midi_values(external)?;
        let mut total = bytes(values)?
            .checked_add(bytes(&self.sampler_midi)?)
            .ok_or(AudioStorageError::Overflow)?;
        for (clip, row) in values.iter().zip(&self.sampler_midi) {
            for part in [
                row.source_declaration.parts(&clip.source),
                row.instrument_declaration.parts(&clip.instrument),
            ] {
                let (p, o) = part.ok_or(AudioStorageError::UnknownStorage)?;
                total = total
                    .checked_add(p)
                    .and_then(|n| n.checked_add(o))
                    .ok_or(AudioStorageError::Overflow)?;
            }
            total = total
                .checked_add(identity_metadata_bytes(&clip.source_identity)?)
                .and_then(|n| n.checked_add(identity_metadata_bytes(&row.source_identity).ok()?))
                .and_then(|n| {
                    n.checked_add(
                        clip.volume
                            .as_ref()
                            .map_or(0, |p| p.retained_control_metadata_bytes()),
                    )
                })
                .and_then(|n| {
                    n.checked_add(
                        row.volume_identity
                            .as_ref()
                            .map_or(0, |p| p.retained_control_metadata_bytes()),
                    )
                })
                .and_then(|n| n.checked_add(clip.name.capacity()))
                .and_then(|n| n.checked_add(clip.source.current_sampler_bytes().ok()?))
                .and_then(|n| {
                    n.checked_add(clip.instrument.current_sampler_bytes(&row.events).ok()?)
                })
                .ok_or(AudioStorageError::Overflow)?;
        }
        Ok(total)
    }
    pub(super) fn visit_current_sampler_midi<'a>(
        &'a self,
        ledger: &mut crate::AudioRootStorageLedger<'a>,
        external: Option<&'a PreparedSessionInventory>,
    ) -> Result<(), AudioStorageError> {
        use crate::audio_storage::Owner;
        let values = self.current_sampler_midi_values(external)?;
        ledger.vec(&self.sampler_midi)?;
        for (clip, row) in values.iter().zip(&self.sampler_midi) {
            ledger.string(&clip.name)?;
            // Borrow the token field as the allowance owner, not the canonical
            // Arc allocation: two retained handles are intentionally charged twice.
            for identity in [&clip.source_identity, &row.source_identity] {
                ledger.bytes(
                    Owner::exclusive(identity),
                    crate::audio_storage::Location(
                        "current-midi-source-control-metadata-allowance",
                        0,
                    ),
                    identity_metadata_bytes(identity)?,
                )?;
            }
            if let Some(program) = &clip.volume {
                ledger.bytes(
                    Owner::exclusive(program),
                    crate::audio_storage::Location(
                        "current-midi-volume-control-metadata-allowance",
                        0,
                    ),
                    program.retained_control_metadata_bytes(),
                )?;
            }
            if let Some(identity) = &row.volume_identity {
                ledger.bytes(
                    Owner::exclusive(identity),
                    crate::audio_storage::Location(
                        "current-midi-volume-identity-control-metadata-allowance",
                        0,
                    ),
                    identity.retained_control_metadata_bytes(),
                )?;
            }
            ledger.shared(
                &clip.source,
                row.source_declaration.parts(&clip.source),
                Owner::exclusive(&clip.source),
                row.handles[0],
                None,
            )?;
            clip.source.visit_current_sampler(ledger)?;
            ledger.shared(
                &clip.instrument,
                row.instrument_declaration.parts(&clip.instrument),
                Owner::exclusive(&clip.instrument),
                row.handles[1],
                None,
            )?;
            clip.instrument.visit_current_sampler(&row.events, ledger)?;
        }
        Ok(())
    }
    pub(super) fn dispose_current_sampler_midi(&mut self) -> Result<bool, AudioStorageError> {
        if self.unused_midi.is_empty() {
            return Ok(false);
        }
        self.current_sampler_midi_bytes(None)?;
        drop(self.unused_midi.pop());
        self.sampler_midi.pop();
        Ok(true)
    }
}
