//! Real canonical granular banks shared by owning Root and public facade tests.
use hex_engine::{
    StreamSourceKey, StreamedSourceMetadata, audio_transform::streamed_granular_root,
};
use hex_media::{
    ResampleKernel, ResamplePlan, ResampleVersion, VerifiedWavReader, VerifiedWavReaderLimits,
    WavReaderLimits,
};
use hex_project::{Command, Created, Editor, Project, WarpMode, default_audio_warp};
use hex_stream::{
    GranularBankAllowance, GranularSourceBuffers, GranularSourcePreparation, GranularWorkerBank,
    VerifiedGranularSource,
};
use std::{
    io::Write,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

pub(crate) struct Wav(PathBuf);
impl Drop for Wav {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
pub(crate) fn bank(
    mode: WarpMode,
    generation: u64,
) -> (Wav, GranularWorkerBank, StreamedSourceMetadata) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let wav = Wav(std::env::temp_dir().join(format!(
        "hex-granular-root-{}-{stamp}-{}.wav",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36u32 + 8192 * 4).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&192_000u32.to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(8192u32 * 4).to_le_bytes());
    for index in 0..8192 {
        for sample in [
            ((index % 97) as i16 - 48) * 250,
            ((index % 71) as i16 - 35) * 700,
        ] {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&wav.0)
        .unwrap();
    file.write_all(&bytes).unwrap();
    drop(file);
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { clip, asset, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "granular".into(),
            path: wav.0.clone(),
            sample_rate: 48_000,
            frames: 8192,
        })
        .unwrap()
    else {
        panic!("import fixture");
    };
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipMusical {
            clip,
            enabled: true,
        })
        .unwrap();
    let warp = default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        mode,
    )
    .unwrap();
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(warp),
        })
        .unwrap();
    let reader = VerifiedWavReader::open(
        &wav.0,
        VerifiedWavReaderLimits::new(WavReaderLimits::new(8192 * 4 + 44, 8192, 128, 8192), 4096),
    )
    .unwrap();
    let raw = StreamedSourceMetadata::new(
        StreamSourceKey::new(generation, asset),
        48_000,
        48_000,
        8192,
        8192,
        hex_project::IDENTITY_CONVERSION,
        *reader.fingerprint(),
    )
    .unwrap();
    let root = streamed_granular_root(
        &raw,
        editor.project().clip(clip).unwrap().audio_edit().unwrap(),
        editor.project().tempo_map(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let plan = ResamplePlan::with_kernel(
        ResampleKernel::with_version(48_000, 48_000, ResampleVersion::V1).unwrap(),
        8192,
        8192,
        16384,
        4096,
    )
    .unwrap();
    let source = VerifiedGranularSource::prepare(
        GranularSourcePreparation {
            raw,
            root,
            reader,
            plan,
            buffers: GranularSourceBuffers {
                source: vec![[0.0; 2]; 8192].into_boxed_slice(),
                cache: vec![[0.0; 2]; 8192].into_boxed_slice(),
                grain: vec![[0.0; 2]; 10000].into_boxed_slice(),
            },
            scratch_budget: 1_000_000,
            checkpoint_entries: 2,
            checkpoint_budget: 1_000_000,
        },
        || false,
    )
    .unwrap();
    let metadata = source.metadata().clone();
    let bank = GranularWorkerBank::try_new_on_control(
        vec![source],
        GranularBankAllowance {
            max_sources: 1,
            max_data_bytes: 8 * 1024 * 1024,
        },
    )
    .unwrap();
    (wav, bank, metadata)
}
