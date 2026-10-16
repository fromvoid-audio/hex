//! Verified real-file catalog and immutable metadata; no synthetic lease.
use hex_engine::{StreamSourceKey, StreamedSourceMetadata};
use std::{io::Write, path::PathBuf};
pub(super) struct Wav(PathBuf);
impl Wav {
    pub(super) fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "hex-native-claims-{}-{stamp}.wav",
            std::process::id()
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&68u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&48000u32.to_le_bytes());
        bytes.extend_from_slice(&192000u32.to_le_bytes());
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&32u32.to_le_bytes());
        for _ in 0..16 {
            bytes.extend_from_slice(&8192i16.to_le_bytes());
        }
        file.write_all(&bytes).unwrap();
        file.sync_all().unwrap();
        Self(path)
    }
    pub(super) fn definition(
        &self,
        asset: hex_project::AssetId,
        context: u64,
    ) -> hex_stream::SourceDefinition {
        let limits = hex_media::VerifiedWavReaderLimits::new(
            hex_media::WavReaderLimits::new(1024 * 1024, 8, 32, 32768),
            1024 * 1024,
        );
        let verified = hex_media::VerifiedWavReader::open(&self.0, limits).unwrap();
        let metadata = StreamedSourceMetadata::new(
            StreamSourceKey::new(context, asset),
            48000,
            48000,
            8,
            8,
            hex_project::IDENTITY_CONVERSION,
            *verified.fingerprint(),
        )
        .unwrap();
        hex_stream::SourceDefinition::new(self.0.clone(), metadata, limits)
    }
}
impl Drop for Wav {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
pub(super) fn pair(definition: hex_stream::SourceDefinition) -> crate::CandidateDomain {
    let mut runtime = hex_stream::StreamRuntime::new(
        hex_stream::StreamConfig::with_pool_pages(4),
        vec![definition],
    )
    .unwrap();
    runtime
        .wait_until_ready(std::time::Duration::from_secs(2))
        .unwrap();
    runtime.split().ok().unwrap()
}
pub(super) fn owner(
    project: &hex_project::Project,
    metadata: StreamedSourceMetadata,
) -> hex_engine::AudioCandidateOwner {
    let asset = project.assets().next().unwrap().id();
    let bank = hex_engine::OwnedAudioInputs::try_new(
        vec![(
            asset,
            hex_engine::OwnedAudioSource::try_streamed(metadata)
                .ok()
                .unwrap(),
        )],
        hex_engine::AudioStorageReservation {
            allocation_items: 64,
            handle_obligations: 256,
            max_prepared_data_bytes: 96 * 1024 * 1024,
            max_construction_data_bytes: 96 * 1024 * 1024,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        },
    )
    .ok()
    .unwrap();
    let mut custody = hex_engine::AudioPreparationCustody::try_new_on_control().unwrap();
    let token = custody
        .begin_input_on_control(
            hex_engine::AudioArrangementInput::new(project, 11, bank)
                .with_audio_prepared_revision(),
        )
        .ok()
        .unwrap();
    hex_engine::AudioCandidateOwner::try_new_on_control(custody, token)
        .ok()
        .unwrap()
}
