//! Verified real-file catalog and immutable metadata; no synthetic lease.
use hex_engine::{StreamSourceKey, StreamedSourceMetadata};
use std::{io::Write, path::PathBuf};
pub(in crate::prepared) struct Wav(PathBuf, u64, u32);
impl Wav {
    pub(super) fn new() -> Self {
        Self::with_frames(8)
    }
    pub(in crate::prepared) fn with_frames(frames: u32) -> Self {
        Self::with_samples(frames, |_| 8192)
    }
    pub(super) fn with_samples(frames: u32, sample: impl Fn(u32) -> i16) -> Self {
        Self::with_rate_and_samples(frames, 48_000, sample)
    }
    pub(super) fn with_rate_and_samples(
        frames: u32,
        rate: u32,
        sample: impl Fn(u32) -> i16,
    ) -> Self {
        let data_bytes = frames.checked_mul(4).unwrap();
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
        bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&rate.checked_mul(4).unwrap().to_le_bytes());
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_bytes.to_le_bytes());
        for frame in 0..frames {
            for _ in 0..2 {
                bytes.extend_from_slice(&sample(frame).to_le_bytes());
            }
        }
        file.write_all(&bytes).unwrap();
        file.sync_all().unwrap();
        Self(path, u64::from(frames), rate)
    }
    pub(in crate::prepared) fn definition(
        &self,
        asset: hex_project::AssetId,
        context: u64,
    ) -> hex_stream::SourceDefinition {
        let limits = hex_media::VerifiedWavReaderLimits::new(
            hex_media::WavReaderLimits::new(1024 * 1024, self.1, 32, 32768),
            1024 * 1024,
        );
        let verified = hex_media::VerifiedWavReader::open(&self.0, limits).unwrap();
        let metadata = StreamedSourceMetadata::new(
            StreamSourceKey::new(context, asset),
            self.2,
            48000,
            self.1,
            hex_project::converted_frame_count(self.1, self.2, 48_000).unwrap(),
            if self.2 == 48_000 {
                hex_project::IDENTITY_CONVERSION
            } else {
                hex_project::SINC_CONVERSION_V1
            },
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
    pair_with_pool(vec![definition], 4)
}
pub(super) fn pair_with_pool(
    definitions: Vec<hex_stream::SourceDefinition>,
    buffers: usize,
) -> crate::CandidateDomain {
    let mut runtime = hex_stream::StreamRuntime::new(
        hex_stream::StreamConfig::with_pool_pages(buffers),
        definitions,
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
    owner_with_context(project, metadata, None)
}
pub(super) fn owner_with_context(
    project: &hex_project::Project,
    metadata: StreamedSourceMetadata,
    context: Option<hex_engine::AudioSourceContext>,
) -> hex_engine::AudioCandidateOwner {
    owner_sources(project, vec![metadata], context)
}
pub(super) fn project_with_return_feed(project: hex_project::Project) -> hex_project::Project {
    use hex_project::{Command, Created, Editor, RoutingOwner, TrackDeviceSlot};
    let mut editor = Editor::new(project, 16).unwrap();
    let source = editor.project().tracks()[0].id();
    let Created::Track(receiver) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "receiver".into(),
            before_track: None,
            slot: TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("receiver")
    };
    editor
        .execute(Command::RemoveInstrument { track: receiver })
        .unwrap();
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack {
            name: "return".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    editor
        .execute(Command::SetReturnSend {
            track: source,
            return_track: bus,
            gain: 0.5,
        })
        .unwrap();
    editor
        .execute(Command::InsertReturnDevice {
            return_track: bus,
            before: None,
            state: hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
                "hex.limiter",
            ))
            .unwrap(),
        })
        .unwrap();
    editor
        .execute(Command::SetAudioFeed {
            source: RoutingOwner::Return(bus),
            destination: receiver,
            gain: 0.5,
        })
        .unwrap();
    editor.project().clone()
}
pub(super) fn owner_sources(
    project: &hex_project::Project,
    sources: Vec<StreamedSourceMetadata>,
    context: Option<hex_engine::AudioSourceContext>,
) -> hex_engine::AudioCandidateOwner {
    owner_sources_impl(project, sources, context, false)
}
pub(super) fn owner_with_effects(
    project: &hex_project::Project,
    metadata: StreamedSourceMetadata,
    context: Option<hex_engine::AudioSourceContext>,
) -> hex_engine::AudioCandidateOwner {
    owner_sources_impl(project, vec![metadata], context, true)
}
fn owner_sources_impl(
    project: &hex_project::Project,
    sources: Vec<StreamedSourceMetadata>,
    context: Option<hex_engine::AudioSourceContext>,
    with_effects: bool,
) -> hex_engine::AudioCandidateOwner {
    let sources = sources
        .into_iter()
        .map(|metadata| {
            (
                metadata.key().asset(),
                hex_engine::OwnedAudioSource::try_streamed(metadata)
                    .ok()
                    .unwrap(),
            )
        })
        .collect();
    let bank = hex_engine::OwnedAudioInputs::try_new(
        sources,
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
    let mut input =
        hex_engine::AudioArrangementInput::new(project, 11, bank).with_audio_prepared_revision();
    let registry = hex_devices::native_registry().unwrap();
    if with_effects {
        input = input.with_effect_registry(&registry);
    }
    if let Some(context) = context {
        input = input.with_source_context(context);
    }
    let token = custody.begin_input_on_control(input).ok().unwrap();
    hex_engine::AudioCandidateOwner::try_new_on_control(custody, token)
        .ok()
        .unwrap()
}
