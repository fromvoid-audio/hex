//! Neutral source facts projected from the current canonical musical references.
//! This record is not a lease: every reuse checks the actual retained Root cell.
use hex_session::{AudioSourceBinding, VerifiedAudioFile, service::CanonicalAudioSourceSnapshot};

struct Member {
    file: VerifiedAudioFile,
    metadata: hex_engine::StreamedSourceMetadata,
}
pub(super) struct DomainSources {
    project_generation: u64,
    catalog_generation: u64,
    members: Vec<Member>,
}
impl DomainSources {
    pub(super) fn capture(
        snapshot: &CanonicalAudioSourceSnapshot,
        catalog_generation: u64,
    ) -> Result<Self, String> {
        if catalog_generation == 0 {
            return Err("source catalog generation exhausted".into());
        }
        let count = snapshot
            .required_sources()
            .filter(|(_, s)| matches!(s, AudioSourceBinding::VerifiedFile(_)))
            .count();
        if count > hex_stream::StreamConfig::initial().max_sources {
            return Err("source projection exceeds its reservation".into());
        }
        let mut members = Vec::new();
        members
            .try_reserve_exact(count)
            .map_err(|_| "could not reserve source projection")?;
        for (asset, source) in snapshot.required_sources() {
            if let AudioSourceBinding::VerifiedFile(file) = source {
                members.push(Member {
                    file: file.clone(),
                    metadata: file
                        .source_metadata(asset, catalog_generation)
                        .map_err(|e| e.to_string())?,
                });
            }
        }
        for (asset, file, cents, law) in snapshot.required_plain_pitch_sources() {
            let metadata = file
                .source_metadata(asset, catalog_generation)
                .and_then(|raw| raw.with_plain_pitch(cents, law))
                .map_err(|e| e.to_string())?;
            Self::add_derived_member(&mut members, file, metadata)?;
        }
        for requirement in snapshot.required_repitch_metadata(catalog_generation) {
            let (file, metadata) = requirement.map_err(|error| error.to_string())?;
            Self::add_derived_member(&mut members, file, metadata)?;
        }
        Ok(Self {
            project_generation: snapshot.context().project_generation,
            catalog_generation,
            members,
        })
    }
    fn add_derived_member(
        members: &mut Vec<Member>,
        file: &VerifiedAudioFile,
        metadata: hex_engine::StreamedSourceMetadata,
    ) -> Result<(), String> {
        if let Some(existing) = members
            .iter()
            .find(|member| member.metadata.key() == metadata.key())
        {
            return if existing.file == *file && existing.metadata == metadata {
                Ok(())
            } else {
                Err("conflicting derived source facts".into())
            };
        }
        if members.len() >= hex_stream::StreamConfig::initial().max_sources {
            return Err("source projection exceeds its reservation".into());
        }
        members
            .try_reserve(1)
            .map_err(|_| "could not reserve source projection")?;
        members.push(Member {
            file: file.clone(),
            metadata,
        });
        Ok(())
    }
    /// Precommit ordinary projection uses the same source definitions as the
    /// verified constructor; these facts are not a reader or a page loan.
    pub(super) fn capture_files(
        input: &hex_session::service::ResidentFileInput<'_>,
        catalog_generation: u64,
    ) -> Result<Self, String> {
        if !input.repitches.is_empty() {
            return Err("ordinary repitch source catalog unavailable".into());
        }
        if catalog_generation == 0
            || input.files.len() > hex_stream::StreamConfig::initial().max_sources
        {
            return Err("ordinary source projection exceeds its reservation".into());
        }
        let mut members = Vec::new();
        members
            .try_reserve_exact(input.files.len())
            .map_err(|_| "could not reserve source projection")?;
        for (asset, file) in input.files {
            members.push(Member {
                file: (*file).clone(),
                metadata: file
                    .source_metadata(*asset, catalog_generation)
                    .map_err(|e| e.to_string())?,
            });
        }
        for pitch in input.pitches {
            let (file, metadata) = Self::pitch_for(input, pitch, catalog_generation)?;
            Self::add_derived_member(&mut members, file, metadata)?;
        }
        Ok(Self {
            project_generation: input.context.project_generation,
            catalog_generation,
            members,
        })
    }
    pub(super) fn matches_files(
        &self,
        input: &hex_session::service::ResidentFileInput<'_>,
    ) -> bool {
        input.repitches.is_empty()
            && self.project_generation == input.context.project_generation
            && input.files.iter().all(|(id, file)| {
                self.members.iter().any(|member| {
                    member.metadata.key()
                        == hex_engine::StreamSourceKey::new(self.catalog_generation, *id)
                        && &member.file == *file
                })
            })
            && input.pitches.iter().all(|pitch| {
                Self::pitch_for(input, pitch, self.catalog_generation)
                    .is_ok_and(|(file, expected)| self.contains_member(file, &expected))
            })
    }
    fn pitch_for<'a>(
        input: &hex_session::service::ResidentFileInput<'a>,
        pitch: &hex_session::service::ResidentFilePitch,
        generation: u64,
    ) -> Result<(&'a VerifiedAudioFile, hex_engine::StreamedSourceMetadata), String> {
        let file = input
            .files
            .iter()
            .find(|(asset, _)| *asset == pitch.asset)
            .map(|(_, file)| *file)
            .ok_or("ordinary pitch file missing")?;
        let metadata = file
            .source_metadata(pitch.asset, generation)
            .and_then(|raw| raw.with_plain_pitch(pitch.cents, pitch.law))
            .map_err(|error| error.to_string())?;
        Ok((file, metadata))
    }
    pub(super) fn file_metadata(
        &self,
        input: &hex_session::service::ResidentFileInput<'_>,
    ) -> Result<hex_session::service::ResidentFileMetadata, String> {
        if !self.matches_files(input) {
            return Err("ordinary source projection differs".into());
        }
        let sources = input
            .files
            .iter()
            .map(|(id, _)| {
                let key = hex_engine::StreamSourceKey::new(self.catalog_generation, *id);
                self.metadata_for(key)
                    .cloned()
                    .map(|metadata| (*id, std::sync::Arc::new(metadata)))
                    .ok_or_else(|| "ordinary source metadata missing".to_owned())
            })
            .collect::<Result<_, _>>()?;
        let pitched = input
            .pitches
            .iter()
            .map(|pitch| {
                let (_, expected) = Self::pitch_for(input, pitch, self.catalog_generation)?;
                self.metadata_for(expected.key())
                    .filter(|actual| *actual == &expected)
                    .cloned()
                    .map(|metadata| (expected.key(), std::sync::Arc::new(metadata)))
                    .ok_or_else(|| "ordinary pitch metadata missing".to_owned())
            })
            .collect::<Result<_, _>>()?;
        hex_session::service::ResidentFileMetadata::new_with_pitched(
            self.catalog_generation,
            sources,
            pitched,
        )
    }
    pub(super) fn covers_files(
        &self,
        input: &hex_session::service::ResidentFileInput<'_>,
        token: hex_audio::AudioCandidateDomainToken,
        control: &hex_audio::prepared::PublicationControl,
    ) -> bool {
        control.audio_candidate_domain_is_sealed(token)
            && self.matches_files(input)
            && input.files.iter().all(|(id, _)| {
                self.metadata_for(hex_engine::StreamSourceKey::new(
                    self.catalog_generation,
                    *id,
                ))
                .is_some_and(|metadata| {
                    control.audio_candidate_domain_covers_source(token, metadata)
                })
            })
            && input.pitches.iter().all(|pitch| {
                Self::pitch_for(input, pitch, self.catalog_generation).is_ok_and(|(_, expected)| {
                    self.metadata_for(expected.key()).is_some_and(|actual| {
                        actual == &expected
                            && control.audio_candidate_domain_covers_source(token, actual)
                    })
                })
            })
    }
    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
    pub(super) fn catalog_generation(&self) -> u64 {
        self.catalog_generation
    }
    /// Definitions reopen independently on the source worker. The complete
    /// captured fingerprint/format remains expected; no silent byte refresh.
    /// Each carries the session's already-verified page manifest, so a rebuild
    /// reopens an unchanged file from its header pages instead of re-hashing
    /// it. A changed length/mtime falls back to a full re-hash, which must
    /// still match the captured fingerprint.
    pub(super) fn definitions(&self) -> Result<Vec<hex_stream::SourceDefinition>, String> {
        let mut definitions = Vec::new();
        definitions
            .try_reserve_exact(self.members.len())
            .map_err(|_| "could not reserve source definitions")?;
        for member in &self.members {
            if member.metadata.key().repitch_identity().is_some() {
                return Err("repitch source requires its canonical snapshot".into());
            }
            definitions.push(
                hex_stream::SourceDefinition::new(
                    member.file.path().to_path_buf(),
                    member.metadata.clone(),
                    member.file.reader_limits(),
                )
                .with_verified_manifest(member.file.verified_manifest().clone()),
            );
        }
        Ok(definitions)
    }
    /// Reconstruct maps from the captured snapshot only while creating worker
    /// definitions. DomainSources retains no extra map allocation.
    pub(super) fn definitions_for_snapshot(
        &self,
        snapshot: &CanonicalAudioSourceSnapshot,
    ) -> Result<Vec<hex_stream::SourceDefinition>, String> {
        if !self.matches_snapshot(snapshot) {
            return Err("source projection differs from snapshot".into());
        }
        let mut definitions = Vec::new();
        definitions
            .try_reserve_exact(self.members.len())
            .map_err(|_| "could not reserve source definitions")?;
        let mut map_bytes = 0usize;
        for member in &self.members {
            if member.metadata.key().repitch_identity().is_none() {
                definitions.push(
                    hex_stream::SourceDefinition::new(
                        member.file.path().to_path_buf(),
                        member.metadata.clone(),
                        member.file.reader_limits(),
                    )
                    .with_verified_manifest(member.file.verified_manifest().clone()),
                );
                continue;
            }
            let mut found = None;
            for (asset, file, edit, tempo) in snapshot.required_repitch_sources() {
                if asset != member.metadata.key().asset() || file != &member.file {
                    continue;
                }
                let raw = file
                    .source_metadata(asset, self.catalog_generation)
                    .map_err(|error| error.to_string())?;
                let (map, law) = hex_engine::audio_transform::streamed_repitch_map(
                    edit,
                    tempo,
                    raw.playback_frames(),
                    &std::sync::atomic::AtomicBool::new(false),
                )
                .map_err(|error| error.to_string())?;
                if raw
                    .with_repitch(&map, law)
                    .is_ok_and(|expected| expected == member.metadata)
                {
                    map_bytes = map_bytes
                        .checked_add(map.backing_bytes().ok_or("repitch map size overflow")?)
                        .ok_or("repitch map size overflow")?;
                    if map_bytes > hex_stream::StreamConfig::initial().max_kernel_table_bytes {
                        return Err("repitch map reservation exceeded".into());
                    }
                    found = Some(
                        hex_stream::SourceDefinition::new(
                            file.path().to_path_buf(),
                            raw,
                            file.reader_limits(),
                        )
                        .with_repitch(map, law)
                        .map_err(|error| error.to_string())?
                        .with_verified_manifest(file.verified_manifest().clone()),
                    );
                    break;
                }
            }
            definitions.push(found.ok_or("repitch source definition missing")?);
        }
        Ok(definitions)
    }
    fn matches_snapshot(&self, snapshot: &CanonicalAudioSourceSnapshot) -> bool {
        self.project_generation == snapshot.context().project_generation
            && snapshot
                .required_sources()
                .all(|(asset, source)| match source {
                    AudioSourceBinding::Resident(_) => true,
                    AudioSourceBinding::VerifiedFile(file) => self.members.iter().any(|member| {
                        member.metadata.key()
                            == hex_engine::StreamSourceKey::new(self.catalog_generation, asset)
                            && &member.file == file
                    }),
                })
            && snapshot
                .required_plain_pitch_sources()
                .all(|(asset, file, cents, law)| {
                    file.source_metadata(asset, self.catalog_generation)
                        .and_then(|raw| raw.with_plain_pitch(cents, law))
                        .is_ok_and(|expected| self.contains_member(file, &expected))
                })
            && snapshot
                .required_repitch_metadata(self.catalog_generation)
                .all(|requirement| {
                    requirement.is_ok_and(|(file, expected)| self.contains_member(file, &expected))
                })
    }
    fn contains_member(
        &self,
        file: &VerifiedAudioFile,
        metadata: &hex_engine::StreamedSourceMetadata,
    ) -> bool {
        self.members
            .iter()
            .any(|member| &member.file == file && &member.metadata == metadata)
    }
    pub(super) fn covers(
        &self,
        snapshot: &CanonicalAudioSourceSnapshot,
        token: hex_audio::AudioCandidateDomainToken,
        control: &hex_audio::prepared::PublicationControl,
    ) -> bool {
        control.audio_candidate_domain_is_sealed(token)
            && self.matches_snapshot(snapshot)
            && snapshot
                .required_sources()
                .all(|(asset, source)| match source {
                    AudioSourceBinding::Resident(_) => true,
                    AudioSourceBinding::VerifiedFile(_) => self
                        .metadata_for(hex_engine::StreamSourceKey::new(
                            self.catalog_generation,
                            asset,
                        ))
                        .is_some_and(|metadata| {
                            control.audio_candidate_domain_covers_source(token, metadata)
                        }),
                })
            && snapshot
                .required_plain_pitch_sources()
                .all(|(asset, file, cents, law)| {
                    file.source_metadata(asset, self.catalog_generation)
                        .and_then(|raw| raw.with_plain_pitch(cents, law))
                        .is_ok_and(|expected| {
                            self.metadata_for(expected.key()).is_some_and(|metadata| {
                                metadata == &expected
                                    && control.audio_candidate_domain_covers_source(token, metadata)
                            })
                        })
                })
            && snapshot
                .required_repitch_metadata(self.catalog_generation)
                .all(|requirement| {
                    requirement.is_ok_and(|(_, expected)| {
                        self.metadata_for(expected.key()).is_some_and(|metadata| {
                            metadata == &expected
                                && control.audio_candidate_domain_covers_source(token, metadata)
                        })
                    })
                })
    }
    pub(super) fn metadata_for(
        &self,
        key: hex_engine::StreamSourceKey,
    ) -> Option<&hex_engine::StreamedSourceMetadata> {
        self.members
            .iter()
            .find(|member| member.metadata.key() == key)
            .map(|member| &member.metadata)
    }

    /// Add only derived sources required by this snapshot from the associated domain.
    pub(super) fn add_required_derived(
        &self,
        snapshot: &CanonicalAudioSourceSnapshot,
        candidate: &mut hex_engine::AudioCandidatePreparation,
    ) -> Result<(), hex_engine::AudioStorageError> {
        use hex_engine::AudioStorageError;
        let mut seen = std::collections::BTreeSet::new();
        for (asset, file, cents, law) in snapshot.required_plain_pitch_sources() {
            let expected = file
                .source_metadata(asset, self.catalog_generation)
                .and_then(|raw| raw.with_plain_pitch(cents, law))
                .map_err(|_| AudioStorageError::SourceMismatch)?;
            if !seen.insert(expected.key()) {
                continue;
            }
            let metadata = self
                .metadata_for(expected.key())
                .filter(|actual| *actual == &expected)
                .ok_or(AudioStorageError::SourceMismatch)?;
            candidate.add_pitched_streamed_source_on_control(metadata.clone())?;
        }
        for requirement in snapshot.required_repitch_metadata(self.catalog_generation) {
            let (_, expected) = requirement.map_err(|_| AudioStorageError::SourceMismatch)?;
            if !seen.insert(expected.key()) {
                continue;
            }
            let metadata = self
                .metadata_for(expected.key())
                .filter(|actual| *actual == &expected)
                .ok_or(AudioStorageError::SourceMismatch)?;
            candidate.add_repitch_streamed_source_on_control(metadata.clone())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_session::{
        DocumentAudioMode, SourcePlanReservation,
        service::{Event, Request, Service},
    };
    use std::{path::PathBuf, time::Duration};
    struct Files(PathBuf);
    impl Files {
        fn new() -> Self {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("hex-desktop-source-{}-{stamp}", std::process::id()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn wav(&self, name: &str, sample: i16) -> PathBuf {
            let path = self.0.join(name);
            let mut bytes = Vec::from(*b"RIFF");
            bytes.extend_from_slice(&68u32.to_le_bytes());
            bytes.extend_from_slice(b"WAVEfmt ");
            bytes.extend_from_slice(&16u32.to_le_bytes());
            bytes.extend_from_slice(&1u16.to_le_bytes());
            bytes.extend_from_slice(&1u16.to_le_bytes());
            bytes.extend_from_slice(&24000u32.to_le_bytes());
            bytes.extend_from_slice(&48000u32.to_le_bytes());
            bytes.extend_from_slice(&2u16.to_le_bytes());
            bytes.extend_from_slice(&16u16.to_le_bytes());
            bytes.extend_from_slice(b"data");
            bytes.extend_from_slice(&32u32.to_le_bytes());
            for _ in 0..16 {
                bytes.extend_from_slice(&sample.to_le_bytes());
            }
            std::fs::write(&path, bytes).unwrap();
            path
        }
    }
    impl Drop for Files {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn next(service: &Service) -> Event {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            match service.try_event() {
                Ok(event) => return event,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(error) => panic!("service closed: {error}"),
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn run(service: &Service, request: Request) -> Event {
        service.submit(request).unwrap();
        let event = next(service);
        assert!(event.error.is_none(), "{:?}", event.error);
        event
    }
    fn mode() -> Request {
        Request::NewWithAudioMode {
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(SourcePlanReservation {
                reader: hex_media::VerifiedWavReaderLimits::new(
                    hex_media::WavReaderLimits::new(64 * 1024 * 1024, 8_000_000, 32, 4096),
                    1024 * 1024,
                ),
                max_retained_sources: 2,
                max_path_bytes: 1024 * 1024,
                max_manifest_bytes: 4 * 1024 * 1024,
            }),
        }
    }
    #[test]
    fn verified_repitch_catalog_captures_derived_source() {
        let files = Files::new();
        let service = Service::new(48_000, 16, 0).unwrap();
        let _ = next(&service);
        run(&service, mode());
        let imported = run(&service, Request::Import(files.wav("repitch.wav", 1000)));
        let clip = imported.state.project.tracks()[0].clips()[0].id();
        let neutral = run(
            &service,
            Request::Edit(hex_project::Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            }),
        );
        let warp = hex_project::default_audio_warp(
            neutral.state.project.clip(clip).unwrap(),
            neutral.state.project.tempo_map(),
            hex_project::WarpMode::RePitch,
        )
        .unwrap();
        let _edited = run(
            &service,
            Request::Edit(hex_project::Command::SetAudioClipWarp {
                clip,
                settings: Some(warp),
            }),
        );
        let duplicated = run(
            &service,
            Request::Edit(hex_project::Command::DuplicateClip(clip)),
        );
        let snapshot = duplicated.state.canonical_audio_source.as_ref().unwrap();
        assert_eq!(snapshot.required_repitch_sources().count(), 2);
        let domain = DomainSources::capture(snapshot, 11).unwrap();
        assert_eq!(domain.members.len(), 2);
        assert!(domain.matches_snapshot(snapshot));
        assert!(domain.definitions().is_err());
        let key = domain
            .members
            .iter()
            .find(|member| member.metadata.key().repitch_identity().is_some())
            .unwrap()
            .metadata
            .key();
        let definitions = domain.definitions_for_snapshot(snapshot).unwrap();
        assert_eq!(definitions.len(), 2);
        assert!(definitions.iter().any(|definition| definition.key == key));
        let quote = snapshot.candidate_reservation_quote().unwrap();
        assert_eq!(quote.source_capacity, 2);
        assert_eq!(quote.prepared_pcm_frames, 0);
        let mut incomplete = DomainSources::capture(snapshot, 11).unwrap();
        incomplete
            .members
            .retain(|member| member.metadata.key().is_raw());
        assert!(!incomplete.matches_snapshot(snapshot));
        assert!(incomplete.definitions_for_snapshot(snapshot).is_err());
        let mut refused = hex_engine::AudioCandidatePreparation::try_new_on_control(
            2,
            quote.storage_reservation(0).unwrap(),
        )
        .unwrap();
        for (asset, _) in snapshot.required_sources() {
            refused
                .add_streamed_source_on_control(
                    incomplete
                        .metadata_for(hex_engine::StreamSourceKey::new(11, asset))
                        .unwrap()
                        .clone(),
                )
                .unwrap();
        }
        assert_eq!(
            incomplete.add_required_derived(snapshot, &mut refused),
            Err(hex_engine::AudioStorageError::SourceMismatch)
        );
        refused.cleanup_on_control().unwrap();
        let mut candidate = hex_engine::AudioCandidatePreparation::try_new_on_control(
            2,
            quote.storage_reservation(0).unwrap(),
        )
        .unwrap();
        for (asset, _) in snapshot.required_sources() {
            candidate
                .add_streamed_source_on_control(
                    domain
                        .metadata_for(hex_engine::StreamSourceKey::new(11, asset))
                        .unwrap()
                        .clone(),
                )
                .unwrap();
        }
        domain
            .add_required_derived(snapshot, &mut candidate)
            .unwrap();
        snapshot
            .prepare_candidate_on_control(11, &mut candidate)
            .unwrap();
        assert_eq!(candidate.resident_pcm_capacity_frames().unwrap(), 0);
        candidate.cleanup_on_control().unwrap();
        use hex_engine::MediaPages;
        let mut runtime = hex_stream::StreamRuntime::new(
            hex_stream::StreamConfig::with_pool_pages(3),
            definitions,
        )
        .unwrap();
        runtime.wait_until_ready(Duration::from_secs(3)).unwrap();
        runtime.request_page(key, 0);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            runtime.pump_ready(8);
            if let Some(page) = runtime.page(key, 0) {
                assert!(page.frames.iter().flatten().any(|sample| *sample != 0.0));
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "repitch page not ready"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(runtime.shutdown().unwrap().accounting_complete);
        let mut changed_warp = hex_project::default_audio_warp(
            duplicated.state.project.clip(clip).unwrap(),
            duplicated.state.project.tempo_map(),
            hex_project::WarpMode::RePitch,
        )
        .unwrap();
        let beat = changed_warp.markers.last().unwrap().beat.raw() / 2;
        changed_warp.markers.insert(
            1,
            hex_project::WarpMarker {
                source_frame: 8,
                beat: hex_project::BeatTime::from_raw(beat),
            },
        );
        let changed = run(
            &service,
            Request::Edit(hex_project::Command::SetAudioClipWarp {
                clip,
                settings: Some(changed_warp),
            }),
        );
        let changed_snapshot = changed.state.canonical_audio_source.as_ref().unwrap();
        assert!(!domain.matches_snapshot(changed_snapshot));
        assert!(domain.definitions_for_snapshot(changed_snapshot).is_err());
    }

    #[test]
    fn verified_candidate_collects_only_matching_distinct_pitch_sources() {
        if !hex_engine::audio_storage_constructor_layout_supported() {
            return;
        }
        let files = Files::new();
        let service = Service::new(48000, 16, 0).unwrap();
        let _ = next(&service);
        run(&service, mode());
        let imported = run(&service, Request::Import(files.wav("pitch.wav", 1000)));
        let clip = imported.state.project.tracks()[0].clips()[0].id();
        run(
            &service,
            Request::Edit(hex_project::Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            }),
        );
        let duplicate = run(
            &service,
            Request::Edit(hex_project::Command::DuplicateClip(clip)),
        );
        let other = duplicate.state.project.tracks()[0]
            .clips()
            .iter()
            .find(|other| other.id() != clip)
            .unwrap()
            .id();
        run(
            &service,
            Request::Edit(hex_project::Command::SetAudioClipPitch {
                clip: other,
                cents: 700,
            }),
        );
        let edited = run(
            &service,
            Request::Edit(hex_project::Command::SetAudioClipPitch { clip, cents: 700 }),
        );
        let snapshot = edited.state.canonical_audio_source.as_ref().unwrap();
        assert_eq!(snapshot.required_plain_pitch_sources().count(), 2);
        let domain = DomainSources::capture(snapshot, 11).unwrap();
        let quote = snapshot.candidate_reservation_quote().unwrap();
        assert_eq!(quote.source_capacity, 2);
        let mut incomplete = DomainSources::capture(snapshot, 11).unwrap();
        incomplete
            .members
            .retain(|member| member.metadata.key().is_raw());
        let mut refused = hex_engine::AudioCandidatePreparation::try_new_on_control(
            quote.source_capacity,
            quote.storage_reservation(0).unwrap(),
        )
        .unwrap();
        for (asset, _) in snapshot.required_sources() {
            refused
                .add_streamed_source_on_control(
                    incomplete
                        .metadata_for(hex_engine::StreamSourceKey::new(11, asset))
                        .unwrap()
                        .clone(),
                )
                .unwrap();
        }
        assert_eq!(
            incomplete.add_required_derived(snapshot, &mut refused),
            Err(hex_engine::AudioStorageError::SourceMismatch)
        );
        refused.cleanup_on_control().unwrap();
        let mut candidate = hex_engine::AudioCandidatePreparation::try_new_on_control(
            quote.source_capacity,
            quote.storage_reservation(0).unwrap(),
        )
        .unwrap();
        for (asset, _) in snapshot.required_sources() {
            candidate
                .add_streamed_source_on_control(
                    domain
                        .metadata_for(hex_engine::StreamSourceKey::new(11, asset))
                        .unwrap()
                        .clone(),
                )
                .unwrap();
        }
        domain
            .add_required_derived(snapshot, &mut candidate)
            .unwrap();
        snapshot
            .prepare_candidate_on_control(11, &mut candidate)
            .unwrap();
        candidate.cleanup_on_control().unwrap();
    }
    #[test]
    fn actual_worker_projection_reuses_undo_subset_but_never_relabels_new_generation_or_changed_bytes()
     {
        let files = Files::new();
        let a = files.wav("a.wav", 1000);
        let b = files.wav("b.wav", 2000);
        let service = Service::new(48000, 16, 0).unwrap();
        let _ = next(&service);
        let empty = run(&service, mode());
        assert!(
            DomainSources::capture(empty.state.canonical_audio_source.as_ref().unwrap(), 11)
                .unwrap()
                .is_empty()
        );
        let first = run(&service, Request::Import(a.clone()));
        let snapshot = first.state.canonical_audio_source.as_ref().unwrap();
        let first_plan = DomainSources::capture(snapshot, 11).unwrap();
        assert_eq!(first_plan.catalog_generation(), 11);
        assert!(first_plan.matches_snapshot(snapshot));
        let definitions = first_plan.definitions().unwrap();
        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].path, a);
        assert_eq!(definitions[0].metadata.source_rate(), 24000);
        assert_eq!(definitions[0].metadata.project_rate(), 48000);
        assert_eq!(definitions[0].metadata.source_frames(), 16);
        assert_eq!(definitions[0].metadata.playback_frames(), 32);
        assert!(first_plan.metadata_for(definitions[0].key).is_some());
        // The same captured file can supply multiple distinct pitches; repeated
        // clips share one definition and another pitch is not falsely covered.
        let mut pitched_plan = DomainSources::capture(snapshot, 11).unwrap();
        let file = pitched_plan.members[0].file.clone();
        let raw = pitched_plan.members[0].metadata.clone();
        let up = raw
            .with_plain_pitch(700, hex_project::WarpResampler::V2)
            .unwrap();
        let down = raw
            .with_plain_pitch(-300, hex_project::WarpResampler::V2)
            .unwrap();
        DomainSources::add_derived_member(&mut pitched_plan.members, &file, up.clone()).unwrap();
        DomainSources::add_derived_member(&mut pitched_plan.members, &file, up.clone()).unwrap();
        let other =
            VerifiedAudioFile::inspect(b.clone(), 48_000, file.reader_limits(), || false).unwrap();
        assert!(
            DomainSources::add_derived_member(&mut pitched_plan.members, &other, up.clone())
                .is_err()
        );
        assert!(!pitched_plan.contains_member(&file, &down));
        DomainSources::add_derived_member(&mut pitched_plan.members, &file, down.clone()).unwrap();
        assert_eq!(pitched_plan.definitions().unwrap().len(), 3);
        assert!(pitched_plan.contains_member(&file, &down));
        assert_eq!(pitched_plan.metadata_for(up.key()), Some(&up));
        assert_eq!(pitched_plan.metadata_for(down.key()), Some(&down));
        let mut full = DomainSources::capture(snapshot, 11).unwrap();
        let limit = hex_stream::StreamConfig::initial().max_sources;
        for cents in 1..limit {
            let metadata = raw
                .with_plain_pitch(cents as i16, hex_project::WarpResampler::V2)
                .unwrap();
            DomainSources::add_derived_member(&mut full.members, &file, metadata).unwrap();
        }
        let overflow = raw
            .with_plain_pitch(limit as i16, hex_project::WarpResampler::V2)
            .unwrap();
        assert!(DomainSources::add_derived_member(&mut full.members, &file, overflow).is_err());
        assert_eq!(full.members.len(), limit);
        assert!(
            first_plan
                .metadata_for(hex_engine::StreamSourceKey::new(
                    12,
                    definitions[0].key.asset()
                ))
                .is_none()
        );
        let second = run(&service, Request::Import(b));
        let second_snapshot = second.state.canonical_audio_source.as_ref().unwrap();
        assert!(!first_plan.matches_snapshot(second_snapshot));
        let superset = DomainSources::capture(second_snapshot, 12).unwrap();
        let undone = run(&service, Request::Undo);
        let undone_snapshot = undone.state.canonical_audio_source.as_ref().unwrap();
        assert!(first_plan.matches_snapshot(undone_snapshot));
        assert!(superset.matches_snapshot(undone_snapshot));
        assert_eq!(undone_snapshot.required_sources().count(), 1);
        let _ = run(&service, mode());
        let reopened = run(&service, Request::Import(a));
        assert!(
            !first_plan.matches_snapshot(reopened.state.canonical_audio_source.as_ref().unwrap())
        );
        files.wav("a.wav", 3000);
        assert!(first_plan.matches_snapshot(snapshot)); // immutable old expected bytes
        let _ = run(&service, mode());
        let changed = run(&service, Request::Import(files.0.join("a.wav")));
        let fresh =
            DomainSources::capture(changed.state.canonical_audio_source.as_ref().unwrap(), 13)
                .unwrap();
        assert_ne!(
            fresh.members[0].file.fingerprint(),
            first_plan.members[0].file.fingerprint()
        );
        // These are neutral matching observations, not a fabricated actual pair
        // witness. Production covers() additionally checks the retained Root.
    }
}
