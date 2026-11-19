//! Existing assets reproduce their saved conversion law on every control path
//! (reopen, relink, verified-file binding); new imports record the import law.
use super::resident_audio::decode_relinks;
use crate::{PreparedImport, Session, VerifiedAudioFile};
use hex_media::{Limits, ResampleLimits, ResampleVersion, decode_wav_file, resample_with_version};
use hex_project::{
    AssetId, Command, Created, Editor, Project, SINC_CONVERSION_V1, SINC_CONVERSION_V2,
};
use std::{collections::BTreeMap, fs, path::PathBuf, sync::atomic::AtomicU64};

const FRAMES: u32 = 441;
const PLAYBACK: u64 = 480;

struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "hex-conversion-selection-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    /// Mono 16-bit 44.1 kHz with broadband content, so the laws differ.
    fn wav(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        let mut bytes = Vec::from(*b"RIFF");
        bytes.extend_from_slice(&(36 + FRAMES * 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&44_100u32.to_le_bytes());
        bytes.extend_from_slice(&(44_100u32 * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(FRAMES * 2).to_le_bytes());
        for frame in 0..FRAMES {
            let value = ((frame * 7_919) % 2_000) as i16 - 1_000;
            bytes.extend_from_slice(&(value * 20).to_le_bytes());
        }
        fs::write(&path, bytes).unwrap();
        path
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const LIMITS: ResampleLimits = ResampleLimits {
    max_source_frames: FRAMES as usize,
    max_output_frames: PLAYBACK as usize,
};

fn expected(path: &PathBuf, version: ResampleVersion) -> Vec<[f32; 2]> {
    let decoded = decode_wav_file(path, Limits::new(65_536, FRAMES as usize)).unwrap();
    resample_with_version(decoded.audio(), 48_000, LIMITS, version)
        .unwrap()
        .frames()
        .to_vec()
}

/// One v1 and one v2 asset of the same file, reopened from saved metadata.
fn reopened(path: &PathBuf) -> (Session, [AssetId; 2]) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let mut assets = Vec::new();
    for version in [SINC_CONVERSION_V1, SINC_CONVERSION_V2] {
        let Created::Imported { asset, .. } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: "converted".into(),
                path: path.clone(),
                source_sample_rate: 44_100,
                source_frames: u64::from(FRAMES),
                playback_frames: PLAYBACK,
                conversion_version: version,
            })
            .unwrap()
        else {
            panic!("import")
        };
        assets.push(asset);
    }
    let decoded = assets
        .iter()
        .map(|asset| {
            let pcm = decode_wav_file(path, Limits::new(65_536, FRAMES as usize)).unwrap();
            (*asset, pcm)
        })
        .collect();
    let budget = 2 * (u64::from(FRAMES) + PLAYBACK);
    let session = Session::from_loaded(editor.project().clone(), 8, budget, decoded).unwrap();
    (session, [assets[0], assets[1]])
}

#[test]
fn reopened_v1_and_v2_assets_render_their_saved_law_bit_for_bit() {
    let files = Files::new();
    let path = files.wav("source.wav");
    let (session, [v1, v2]) = reopened(&path);
    let law_v1 = expected(&path, ResampleVersion::V1);
    let law_v2 = expected(&path, ResampleVersion::V2);
    assert_ne!(law_v1, law_v2);
    // The unchanged public v1 entry point is the bit-for-bit reference.
    let decoded = decode_wav_file(&path, Limits::new(65_536, FRAMES as usize)).unwrap();
    let legacy = hex_media::resample(decoded.audio(), 48_000, LIMITS).unwrap();
    assert_eq!(law_v1, legacy.frames());
    assert_eq!(session.audio(v1).unwrap().frames(), law_v1);
    assert_eq!(session.audio(v2).unwrap().frames(), law_v2);
}

#[test]
fn relinks_reproduce_each_assets_saved_law() {
    let files = Files::new();
    let path = files.wav("source.wav");
    let moved = files.wav("moved.wav");
    let (session, [v1, v2]) = reopened(&path);
    let relinks = BTreeMap::from([(v1, moved.clone()), (v2, moved.clone())]);
    let prepared = decode_relinks(&session, relinks, 2 * PLAYBACK).unwrap();
    let by_asset: BTreeMap<_, _> = prepared.into_iter().collect();
    for (asset, version, law) in [
        (v1, SINC_CONVERSION_V1, ResampleVersion::V1),
        (v2, SINC_CONVERSION_V2, ResampleVersion::V2),
    ] {
        let member = &by_asset[&asset];
        assert_eq!(member.conversion_version, version);
        assert_eq!(member.audio.frames(), expected(&moved, law));
    }
}

#[test]
fn prepared_conversion_refuses_unknown_and_rate_inconsistent_versions() {
    let files = Files::new();
    let path = files.wav("source.wav");
    let decoded = || decode_wav_file(&path, Limits::new(65_536, FRAMES as usize)).unwrap();
    for version in [hex_project::IDENTITY_CONVERSION, 3, u32::MAX] {
        assert!(PreparedImport::for_conversion(decoded(), 48_000, LIMITS, version).is_err());
    }
    // Identity media admits only identity conversion.
    let identity = ResampleLimits {
        max_source_frames: FRAMES as usize,
        max_output_frames: FRAMES as usize,
    };
    assert!(
        PreparedImport::for_conversion(decoded(), 44_100, identity, SINC_CONVERSION_V2).is_err()
    );
    assert!(PreparedImport::for_conversion(decoded(), 44_100, identity, 0).is_ok());
}

#[test]
fn verified_files_bind_the_saved_law_and_new_imports_record_the_import_law() {
    let files = Files::new();
    let path = files.wav("source.wav");
    let (session, [v1, v2]) = reopened(&path);
    let limits = hex_media::VerifiedWavReaderLimits::new(
        hex_media::WavReaderLimits::new(1 << 20, 10_000, 32, 4_096),
        1 << 20,
    );
    for asset in [v1, v2] {
        let metadata = session.project().asset(asset).unwrap();
        let mut file = VerifiedAudioFile::inspect_for_conversion(
            path.clone(),
            48_000,
            metadata.conversion_version(),
            limits,
            || false,
        )
        .unwrap();
        assert!(file.matches_asset(metadata, 48_000));
        file.prepare_waveform_on_control(|| false).unwrap();
    }
    let imported = VerifiedAudioFile::inspect(path.clone(), 48_000, limits, || false).unwrap();
    assert_eq!(
        imported.conversion_version(),
        hex_project::import_conversion_version(44_100, 48_000)
    );
    assert!(VerifiedAudioFile::inspect_for_conversion(path, 48_000, 0, limits, || false).is_err());
}
