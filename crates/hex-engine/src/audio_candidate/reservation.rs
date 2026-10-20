//! Conservative caller reservation, never a constructor/storage authority.
//! Row envelopes cover the selected compiler's fixed representations; the real
//! rooted constructor and borrowed ledger still validate actual capacities.
use super::*;

#[derive(Clone, Copy, Debug)]
pub struct AudioCandidateReservationQuote {
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
        if sources > hex_project::MAX_ASSETS {
            return Err(AudioStorageError::Capacity);
        }
        let mut counts = Counts::default();
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
            counts.add(Kind::Device, track.devices().len())?;
            counts.add(Kind::Point, track.gain_envelope().len())?;
        }
        Self::from_counts(
            counts,
            project.tracks().len(),
            project.return_tracks().len(),
        )
    }
    /// Ceiling from existing canonical cardinality limits, used only to bound
    /// a persistent host's finite reservation. Per-request scratch can use the
    /// much smaller current-project quote. No runtime or bank is constructed.
    pub fn for_supported_limits(sources: usize) -> Result<Self, AudioStorageError> {
        if sources > hex_project::MAX_ASSETS {
            return Err(AudioStorageError::Capacity);
        }
        let mut counts = Counts::default();
        for (kind, count) in [
            (Kind::Fixed, 1),
            (Kind::Source, sources),
            (Kind::Track, hex_project::MAX_TRACKS),
            (Kind::Return, hex_project::MAX_RETURN_TRACKS),
            (Kind::Clip, hex_project::MAX_CLIPS),
            (Kind::Device, hex_project::MAX_DEVICES),
            (Kind::Cell, hex_project::MAX_SESSION_CELLS),
            (Kind::Scene, hex_project::MAX_SCENES),
            (Kind::Point, hex_project::MAX_GAIN_POINTS),
            (Kind::Point, hex_project::MAX_TEMPO_POINTS),
            (Kind::Point, hex_project::MAX_METER_POINTS),
        ] {
            counts.add(kind, count)?;
        }
        Self::from_counts(
            counts,
            hex_project::MAX_TRACKS,
            hex_project::MAX_RETURN_TRACKS,
        )
    }
    fn from_counts(
        counts: Counts,
        tracks: usize,
        returns: usize,
    ) -> Result<Self, AudioStorageError> {
        let groups = crate::session_playback::candidate_group_reservation(tracks, returns)?;
        Ok(Self {
            data_rows: counts.rows,
            handle_rows: counts.handles,
            member_rows: groups.checked_mul(2).ok_or(AudioStorageError::Overflow)?,
            transfer_rows: groups,
            metadata_data_bytes: counts.bytes,
        })
    }
    /// Add a separately bounded input bank and the existing mutable-runtime
    /// allowance. This is a caller ceiling, not a statement of complete costs.
    pub fn storage_reservation(
        self,
        input_data_bytes: usize,
    ) -> Result<crate::AudioStorageReservation, AudioStorageError> {
        let retained = self
            .metadata_data_bytes
            .checked_add(input_data_bytes)
            .and_then(|n| n.checked_add(crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES))
            .ok_or(AudioStorageError::Overflow)?;
        // A second input collection may coexist during fresh consuming copies.
        let peak = retained
            .checked_add(input_data_bytes)
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
enum Kind {
    Fixed,
    Source,
    Scene,
    Cell,
    Return,
    Point,
    Track,
    Clip,
    Device,
}
#[derive(Default)]
struct Counts {
    rows: usize,
    handles: usize,
    bytes: usize,
}
impl Counts {
    fn add(&mut self, kind: Kind, count: usize) -> Result<(), AudioStorageError> {
        let (rows, handles) = match kind {
            Kind::Fixed => (256usize, 128usize),
            Kind::Source => (8, 8),
            Kind::Scene => (8, 4),
            Kind::Cell | Kind::Clip => (16, 16),
            Kind::Return => (32, 16),
            Kind::Point => (4, 2),
            Kind::Track => (64, 32),
            Kind::Device => (32, 32),
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
                        .execute(hex_project::Command::AddTrack {
                            name: format!("audio {index}"),
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
                before: None,
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
