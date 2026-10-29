//! Conservative caller reservation, never a constructor/storage authority.
//! Row envelopes cover the selected compiler's fixed representations; the real
//! rooted constructor and borrowed ledger still validate actual capacities.
use super::*;

#[derive(Clone, Copy, Debug)]
pub struct AudioCandidateReservationQuote {
    pub source_capacity: usize,
    /// Complete fresh transformed Arrangement PCM, separate from input assets.
    /// An upper bound until the constructor ledger observes actual Vec capacity.
    pub prepared_pcm_frames: u64,
    /// Ephemeral closed audio/MIDI/effect compiler overlap; excluded from retained DATA.
    pub compiler_workspace_bytes: usize,
    pub data_rows: usize,
    pub handle_rows: usize,
    pub member_rows: usize,
    pub transfer_rows: usize,
    /// Separate immutable/preparation metadata allowance, excluding input PCM.
    pub metadata_data_bytes: usize,
}
impl AudioCandidateReservationQuote {
    pub fn for_project(
        project: &hex_project::Project,
        sources: usize,
    ) -> Result<Self, AudioStorageError> {
        crate::audio_preparation::validate_constructor_project(project)?;
        Self::for_project_counts(project, sources, false)
    }
    /// Caller sizing for the explicitly selected closed foreign-effect class.
    /// A registry or this quote never admits a native source or callback.
    pub fn for_project_with_effect_registry(
        project: &hex_project::Project,
        sources: usize,
        effects: &hex_lattice::NativeDeviceRegistry<'_>,
    ) -> Result<Self, AudioStorageError> {
        crate::audio_preparation::validate_constructor_project_with_effects(
            project,
            Some(effects),
        )?;
        Self::for_project_counts(project, sources, true)
    }
    fn for_project_counts(
        project: &hex_project::Project,
        sources: usize,
        effect_payloads: bool,
    ) -> Result<Self, AudioStorageError> {
        if sources > hex_project::MAX_ASSETS {
            return Err(AudioStorageError::Capacity);
        }
        let mut counts = Counts::default();
        counts.add(
            Kind::PreparedSource,
            crate::audio_preparation::transformed::transformed_count(project),
        )?;
        counts.add(Kind::Fixed, 1)?;
        counts.add(Kind::Source, sources)?;
        counts.add(Kind::Scene, project.scenes().len())?;
        counts.add(Kind::Cell, project.session_cells().len())?;
        counts.add(Kind::Return, project.return_tracks().len())?;
        counts.add(Kind::Point, project.tempo_map().points().len())?;
        counts.add(Kind::Point, project.meter_map().points().len())?;
        for track in project.tracks() {
            counts.add(Kind::Track, 1)?;
            counts.add(Kind::Clip, track.clips().len())?;
            counts.add(Kind::Clip, track.midi_clips().len())?;
            if track.instrument().is_some() {
                counts.add(Kind::Sampler, 1)?;
            }
            for clip in track.midi_clips() {
                counts.add(Kind::MidiNote, clip.notes().len())?;
            }
            counts.add(Kind::Device, track.devices().len())?;
            counts.add(Kind::Point, track.gain_envelope().len())?;
        }
        let reverb_count = project
            .tracks()
            .iter()
            .flat_map(|t| t.devices())
            .chain(project.return_tracks().iter().flat_map(|r| r.devices()))
            .chain(project.main_devices())
            .filter(|d| d.state().type_id() == hex_devices::reverb::REVERB_TYPE_ID)
            .count();
        counts.add(Kind::ReverbRings, reverb_count)?;
        counts.add(Kind::Device, project.main_devices().len())?;
        for bus in project.return_tracks() {
            counts.add(Kind::Device, bus.devices().len())?;
        }
        for cell in project.session_cells() {
            if let hex_project::SessionCellContent::Midi(clip) = cell.content() {
                counts.session_clips = counts
                    .session_clips
                    .checked_add(1)
                    .ok_or(AudioStorageError::Overflow)?;
                counts.session_notes = counts
                    .session_notes
                    .checked_add(clip.source().notes().len())
                    .ok_or(AudioStorageError::Overflow)?;
            }
        }
        let mut quote = Self::from_counts(
            counts,
            project.tracks().len(),
            project.return_tracks().len(),
            sources,
        )?;
        quote.prepared_pcm_frames =
            crate::audio_preparation::transformed::prepared_pcm_frames(project)?;
        let pcm_bytes = usize::try_from(quote.prepared_pcm_frames)
            .ok()
            .and_then(|frames| frames.checked_mul(std::mem::size_of::<crate::Frame>()))
            .ok_or(AudioStorageError::Overflow)?;
        let (retained, workspace) = crate::audio_preparation::transformed_audio_quote(project)?;
        quote.metadata_data_bytes = quote
            .metadata_data_bytes
            .checked_add(
                retained
                    .checked_sub(pcm_bytes)
                    .ok_or(AudioStorageError::Overflow)?,
            )
            .ok_or(AudioStorageError::Overflow)?;
        quote.compiler_workspace_bytes = quote
            .compiler_workspace_bytes
            .checked_add(workspace)
            .ok_or(AudioStorageError::Overflow)?;
        if effect_payloads || project.has_group_tracks() {
            quote.metadata_data_bytes = quote
                .metadata_data_bytes
                .checked_add(crate::compensation::MAX_COMPENSATION_BYTES)
                .ok_or(AudioStorageError::Overflow)?;
            quote.compiler_workspace_bytes = quote
                .compiler_workspace_bytes
                .checked_add(crate::compensation::MAX_COMPENSATION_BYTES)
                .ok_or(AudioStorageError::Overflow)?;
        }
        if effect_payloads {
            let mut payloads = 0usize;
            let mut largest = 0;
            for device in project
                .tracks()
                .iter()
                .flat_map(|t| t.devices())
                .chain(project.return_tracks().iter().flat_map(|r| r.devices()))
                .chain(project.main_devices())
            {
                payloads = payloads
                    .checked_add(device.state().payload().len())
                    .ok_or(AudioStorageError::Overflow)?;
                largest = largest.max(device.state().payload().len());
            }
            quote.metadata_data_bytes = quote
                .metadata_data_bytes
                .checked_add(payloads)
                .ok_or(AudioStorageError::Overflow)?;
            quote.compiler_workspace_bytes = quote
                .compiler_workspace_bytes
                .checked_add(largest)
                .ok_or(AudioStorageError::Overflow)?;
        }
        // Cardinalities cannot describe spare capacity retained by canonical
        // sources. Quote the actual two owning identity tokens per Console clip.
        let canonical_bytes =
            crate::session_inventory::audio_assembly::current_midi::canonical_source_metadata_bytes(
                project,
            )?;
        quote.metadata_data_bytes = quote
            .metadata_data_bytes
            .checked_add(canonical_bytes)
            .ok_or(AudioStorageError::Overflow)?;
        // Actual authored curve backings and the two sealed Arrangement row
        // families are separate from note/cardinality allowances. No media or
        // prepared owner is allocated to obtain this conservative control quote.
        for track in project.tracks() {
            quote.metadata_data_bytes = quote
                .metadata_data_bytes
                .checked_add(
                    crate::midi_clip_volume::arrangement::PreparedArrangementVolumes::closed_quote(
                        track,
                    )?,
                )
                .ok_or(AudioStorageError::Overflow)?;
        }
        Ok(quote)
    }
    /// Ceiling from existing canonical cardinality limits, used only to bound
    /// a persistent host's finite reservation. Per-request scratch can use the
    /// much smaller current-project quote. No runtime or bank is constructed.
    /// This cardinality ceiling has zero actual prepared PCM: its caller's
    /// separately bounded physical input allowance covers all native PCM.
    /// Per-project callers must add `prepared_pcm_frames` to physical admission.
    /// Canonical source spare capacity is checked by the actual constructor;
    /// cardinalities alone cannot certify that every project fits this ceiling.
    pub fn for_supported_limits(sources: usize) -> Result<Self, AudioStorageError> {
        if sources > hex_project::MAX_ASSETS {
            return Err(AudioStorageError::Capacity);
        }
        let mut counts = Counts::default();
        for (kind, count) in [
            (Kind::Fixed, 1),
            (Kind::Source, sources),
            (Kind::PreparedSource, hex_project::MAX_CLIPS),
            (Kind::Track, hex_project::MAX_TRACKS),
            (Kind::Return, hex_project::MAX_RETURN_TRACKS),
            (Kind::Clip, hex_project::MAX_CLIPS),
            (Kind::Device, hex_project::MAX_DEVICES),
            (Kind::ReverbRings, hex_lattice::MAX_STATEFUL_PROCESSORS),
            (Kind::Sampler, hex_project::MAX_TRACKS),
            (Kind::MidiNote, hex_project::MAX_NOTES),
            (Kind::Cell, hex_project::MAX_SESSION_CELLS),
            (Kind::Scene, hex_project::MAX_SCENES),
            (Kind::Point, hex_project::MAX_GAIN_POINTS),
            (Kind::Point, hex_project::MAX_TEMPO_POINTS),
            (Kind::Point, hex_project::MAX_METER_POINTS),
        ] {
            counts.add(kind, count)?;
        }
        counts.session_clips = hex_project::MAX_SESSION_CELLS;
        counts.session_notes = hex_project::MAX_NOTES;
        let mut quote = Self::from_counts(
            counts,
            hex_project::MAX_TRACKS,
            hex_project::MAX_RETURN_TRACKS,
            sources,
        )?;
        quote.compiler_workspace_bytes = quote
            .compiler_workspace_bytes
            .checked_add(crate::audio_preparation::transformed::workspace_ceiling()?)
            .and_then(|bytes| bytes.checked_add(crate::compensation::MAX_COMPENSATION_BYTES))
            .ok_or(AudioStorageError::Overflow)?;
        quote.metadata_data_bytes = quote
            .metadata_data_bytes
            .checked_add(crate::compensation::MAX_COMPENSATION_BYTES)
            .ok_or(AudioStorageError::Overflow)?;
        Ok(quote)
    }
    /// Resident constructor cardinalities use the canonical asset ceiling,
    /// independently of any streaming worker's configured source capacity.
    pub fn for_supported_resident_limits() -> Result<Self, AudioStorageError> {
        Self::for_supported_limits(hex_project::MAX_ASSETS)
    }
    fn from_counts(
        counts: Counts,
        tracks: usize,
        returns: usize,
        sources: usize,
    ) -> Result<Self, AudioStorageError> {
        let groups = crate::session_playback::candidate_group_reservation(tracks, returns)?;
        let (session_bytes, session_workspace) =
            crate::session_inventory::audio_assembly::current_midi::cardinality_quote(
                counts.session_notes,
                counts.session_clips,
            )?;
        Ok(Self {
            source_capacity: sources,
            prepared_pcm_frames: 0,
            compiler_workspace_bytes: crate::audio_preparation::sampler_workspace_bound(
                counts.notes,
                counts.clips,
            )?
            .checked_add(session_workspace)
            .ok_or(AudioStorageError::Overflow)?,
            data_rows: counts.rows,
            handle_rows: counts.handles,
            member_rows: groups.checked_mul(2).ok_or(AudioStorageError::Overflow)?,
            transfer_rows: groups,
            metadata_data_bytes: counts
                .bytes
                .checked_add(session_bytes)
                .ok_or(AudioStorageError::Overflow)?,
        })
    }
    /// Add a separately bounded input bank and the existing mutable-runtime
    /// allowance. This is a caller ceiling, not a statement of complete costs.
    pub fn storage_reservation(
        self,
        input_data_bytes: usize,
    ) -> Result<crate::AudioStorageReservation, AudioStorageError> {
        let prepared_pcm_bytes = usize::try_from(self.prepared_pcm_frames)
            .ok()
            .and_then(|frames| frames.checked_mul(std::mem::size_of::<crate::Frame>()))
            .ok_or(AudioStorageError::Overflow)?;
        let retained = self
            .metadata_data_bytes
            .checked_add(prepared_pcm_bytes)
            .ok_or(AudioStorageError::Overflow)?
            .checked_add(input_data_bytes)
            .and_then(|n| n.checked_add(crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES))
            .ok_or(AudioStorageError::Overflow)?;
        // A second input collection may coexist during fresh consuming copies.
        let peak = retained
            .checked_add(input_data_bytes)
            .and_then(|n| n.checked_add(self.compiler_workspace_bytes))
            .ok_or(AudioStorageError::Overflow)?;
        Ok(crate::AudioStorageReservation {
            allocation_items: self.data_rows,
            handle_obligations: self.handle_rows,
            max_prepared_data_bytes: retained,
            max_construction_data_bytes: peak,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        })
    }
}
// These deliberately loose row envelopes reserve the actual compiler families,
// not user DSP or arbitrary providers. Zero-capacity visitors consume rows too.
// Fixed: custody/root/tempo/shared/runtime buffers. Track: staging/compiled/mix/
// gain/meter/chain. Device: original payload, fresh config, definition, meter and
// concrete runtime twins. Clip/Cell: pair + metadata + source handles. Changes
// still face the actual ledger; exceeding a quote is an explicit refusal.
#[derive(Clone, Copy)]
enum Kind {
    Fixed,
    Source,
    PreparedSource,
    Scene,
    Cell,
    Return,
    Point,
    Track,
    Clip,
    Device,
    ReverbRings,
    Sampler,
    MidiNote,
}
#[derive(Default)]
struct Counts {
    notes: usize,
    clips: usize,
    session_clips: usize,
    session_notes: usize,
    rows: usize,
    handles: usize,
    bytes: usize,
}
impl Counts {
    fn add(&mut self, kind: Kind, count: usize) -> Result<(), AudioStorageError> {
        if matches!(kind, Kind::MidiNote) {
            self.notes = self
                .notes
                .checked_add(count)
                .ok_or(AudioStorageError::Overflow)?;
        }
        if matches!(kind, Kind::Clip) {
            self.clips = self
                .clips
                .checked_add(count)
                .ok_or(AudioStorageError::Overflow)?;
        }
        let (rows, handles) = match kind {
            // Includes the fixed MIDI span Box under the Session owner; its
            // former inline payload remains under the same runtime ceiling.
            Kind::Fixed => (257usize, 128usize),
            Kind::Source => (8, 8),
            Kind::PreparedSource => (4, 2),
            Kind::Scene => (8, 4),
            Kind::Cell | Kind::Clip => (16, 16),
            Kind::Return => (32, 16),
            Kind::Point => (4, 2),
            Kind::Track => (64, 32),
            Kind::Device => (32, 32),
            Kind::ReverbRings => (2 * hex_devices::reverb::RUNTIME_HEAP_ALLOCATIONS, 0),
            Kind::Sampler => (32, 8),
            Kind::MidiNote => (0, 0),
        };
        let rows = rows.checked_mul(count).ok_or(AudioStorageError::Overflow)?;
        let handles = handles
            .checked_mul(count)
            .ok_or(AudioStorageError::Overflow)?;
        self.rows = self
            .rows
            .checked_add(rows)
            .ok_or(AudioStorageError::Overflow)?;
        self.handles = self
            .handles
            .checked_add(handles)
            .ok_or(AudioStorageError::Overflow)?;
        // Bounded strings/configuration payloads are reserved separately from
        // the 32 MiB mutable scope. A row's allowance is conservative metadata,
        // never a guessed Arc header, OS allocation or RSS measurement.
        let stride = hex_project::MAX_NAME_BYTES
            .max(std::mem::size_of::<crate::Track>())
            .max(std::mem::size_of::<crate::PreparedSessionCell>())
            .max(std::mem::size_of::<crate::StreamedSourceMetadata>());
        if matches!(kind, Kind::MidiNote) {
            let stride = std::mem::size_of::<crate::midi_compile::CompiledNoteSpan>()
                .checked_add(std::mem::size_of::<crate::midi_compile::SpanNode>())
                .ok_or(AudioStorageError::Overflow)?;
            self.bytes = self
                .bytes
                .checked_add(
                    count
                        .checked_mul(stride)
                        .ok_or(AudioStorageError::Overflow)?,
                )
                .ok_or(AudioStorageError::Overflow)?;
        }
        if matches!(kind, Kind::Sampler) {
            self.bytes = self
                .bytes
                .checked_add(
                    count
                        .checked_mul(std::mem::size_of::<
                            hex_devices::sampler::PreparedSamplerRuntime,
                        >())
                        .and_then(|n| n.checked_mul(2))
                        .ok_or(AudioStorageError::Overflow)?,
                )
                .ok_or(AudioStorageError::Overflow)?;
        }
        self.bytes = self
            .bytes
            .checked_add(
                rows.checked_mul(stride)
                    .ok_or(AudioStorageError::Overflow)?,
            )
            .ok_or(AudioStorageError::Overflow)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice_contract::NativeDeviceMetadata;
    #[test]
    fn conservative_quote_covers_actual_empty_and_hundred_track_utility_constructors() {
        if !crate::audio_storage_constructor_layout_supported() {
            return;
        }
        for count in [0, 100] {
            let mut editor = hex_project::Editor::new(
                if count == 0 {
                    hex_project::Project::new(48000).unwrap()
                } else {
                    crate::audio_preparation::candidate_test_project()
                },
                8,
            )
            .unwrap();
            if count > 0 {
                let asset = editor.project().assets().next().unwrap().id();
                for index in 1..count {
                    let hex_project::Created::Track(track) = editor
                        .execute(hex_project::Command::InsertTrackWithDevice {
                            name: format!("audio {index}"),
                            before_track: None,
                            slot: hex_project::TrackDeviceSlot::Instrument,
                            state: hex_project::default_instrument_state()
                                .expect("fixture instrument state"),
                        })
                        .unwrap()
                    else {
                        panic!("track");
                    };
                    editor
                        .execute(hex_project::Command::RemoveInstrument { track })
                        .unwrap();
                    editor
                        .execute(hex_project::Command::PlaceClip {
                            track,
                            asset,
                            start: 0,
                            duration: 8,
                            source_offset: 0,
                            source_length: 8,
                            looping: false,
                        })
                        .unwrap();
                }
            }
            if count > 0 {
                let hex_project::Created::Scene(scene) = editor
                    .execute(hex_project::Command::AddScene {
                        name: "audio scene".into(),
                    })
                    .unwrap()
                else {
                    panic!("scene");
                };
                let asset = editor.project().assets().next().unwrap().id();
                let tracks = editor
                    .project()
                    .tracks()
                    .iter()
                    .map(|t| t.id())
                    .collect::<Vec<_>>();
                for track in tracks {
                    editor
                        .execute(hex_project::Command::CreateSessionAudioClip {
                            track,
                            scene,
                            name: "audio cell".into(),
                            asset,
                            source_start: 0,
                            source_end: 8,
                        })
                        .unwrap();
                }
            }
            let mut snapshot = editor.project().snapshot();
            for track in &mut snapshot.tracks {
                track.devices.push(hex_project::DeviceSnapshot {
                    automation: Vec::new(),
                    comparison: None,
                    id: snapshot.next_id,
                    bypassed: false,
                    state: hex_devices::utility::UTILITY_DEFINITION
                        .default_state()
                        .unwrap(),
                });
                snapshot.next_id += 1;
            }
            let project = hex_project::Project::from_snapshot(snapshot).unwrap();
            let quote =
                AudioCandidateReservationQuote::for_project(&project, usize::from(count > 0))
                    .unwrap();
            let storage = quote.storage_reservation(1024).unwrap();
            let mut preparation =
                AudioCandidatePreparation::try_new_on_control(usize::from(count > 0), storage)
                    .unwrap();
            if count > 0 {
                let asset = project.assets().next().unwrap().id();
                preparation
                    .copy_resident_source_on_control(
                        asset,
                        &crate::Audio::new(48000, vec![[0.25; 2]; 8]).unwrap(),
                    )
                    .unwrap();
            }
            preparation
                .prepare_on_control(
                    &project,
                    11,
                    crate::AudioSourceContext {
                        project_generation: 2,
                        editor_revision: 3,
                    },
                )
                .unwrap();
            let actual = preparation
                .owner_on_control()
                .unwrap()
                .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
                    data_rows: quote.data_rows,
                    handle_rows: quote.handle_rows,
                    root_rows: 1,
                    max_retained_bytes: storage.max_prepared_data_bytes,
                    max_construction_peak_bytes: storage.max_construction_data_bytes,
                })
                .unwrap();
            assert!(actual.member_rows <= quote.member_rows);
            assert!(actual.transfer_rows <= quote.transfer_rows);
            assert!(actual.data.data_allocations <= quote.data_rows);
            assert!(actual.shared_resource_handles <= quote.handle_rows);
            assert!(actual.data.retained_bytes <= storage.max_prepared_data_bytes);
            assert!(actual.data.require_complete().is_err());
            preparation.cleanup_on_control().unwrap();
        }
    }
    #[test]
    fn quote_refuses_unsupported_class_and_checked_arithmetic_overflow() {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48000).unwrap(), 8).unwrap();
        editor
            .execute(hex_project::Command::InsertTrackWithDevice {
                name: "instrument".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            })
            .unwrap();
        assert!(matches!(
            AudioCandidateReservationQuote::for_project(editor.project(), 0),
            Err(AudioStorageError::UnsupportedGraph)
        ));
        assert!(matches!(
            AudioCandidateReservationQuote::for_project(
                &hex_project::Project::new(48000).unwrap(),
                usize::MAX
            ),
            Err(AudioStorageError::Capacity)
        ));
        let quote = AudioCandidateReservationQuote::for_project(
            &hex_project::Project::new(48000).unwrap(),
            0,
        )
        .unwrap();
        assert!(matches!(
            quote.storage_reservation(usize::MAX),
            Err(AudioStorageError::Overflow)
        ));
        assert!(matches!(
            Counts::default().add(Kind::Track, usize::MAX),
            Err(AudioStorageError::Overflow)
        ));
    }
}
