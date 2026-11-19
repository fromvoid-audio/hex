//! Verified metadata projection without resident PCM or native source admission.
use super::*;
use hex_project::{Command, Created, Editor, WarpMode};
use std::sync::atomic::AtomicBool;

pub(crate) struct Fixture {
    pub editor: Editor,
    pub file: VerifiedAudioFile,
    pub clip: hex_project::ClipId,
    pub asset: AssetId,
}
impl Fixture {
    pub fn new(mode: WarpMode, converted: bool) -> Self {
        Self::with_frames(mode, converted, 8192)
    }
    pub fn with_frames(mode: WarpMode, converted: bool, frames: u32) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hex-granular-projection-{}-{}.wav",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let rate: u32 = if converted { 44_100 } else { 48_000 };
        let mut bytes = Vec::from(*b"RIFF");
        bytes.extend_from_slice(&(36 + frames * 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(frames * 2).to_le_bytes());
        if frames <= 8192 {
            for _ in 0..frames {
                bytes.extend_from_slice(&8192i16.to_le_bytes());
            }
            std::fs::write(&path, bytes).unwrap();
        } else {
            // A genuine zero PCM16 file; sparse backing avoids constructing a
            // full-file PCM fixture in memory. Inspection still hashes its bytes.
            use std::io::Write;
            let mut output = std::fs::File::create(&path).unwrap();
            output.write_all(&bytes).unwrap();
            output.set_len(44 + u64::from(frames) * 2).unwrap();
        }
        let file = VerifiedAudioFile::inspect(
            path.clone(),
            48_000,
            VerifiedWavReaderLimits::new(
                hex_media::WavReaderLimits::new(
                    44 + u64::from(frames) * 2,
                    u64::from(frames),
                    32,
                    4096,
                ),
                1 << 20,
            ),
            || false,
        )
        .unwrap();
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Imported { clip, asset, .. } = editor
            .execute(Command::ImportPreparedAudioTrack {
                name: "granular".into(),
                path,
                source_sample_rate: rate,
                source_frames: frames as u64,
                playback_frames: file.playback_frames(),
                conversion_version: file.conversion_version(),
            })
            .unwrap()
        else {
            panic!("fixture import")
        };
        editor
            .execute(Command::SetClipFades {
                clip,
                fade_in: 0,
                fade_out: 0,
            })
            .unwrap();
        let warp = hex_project::default_audio_warp(
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
        editor
            .execute(Command::TrimClipEdge {
                clip,
                edge: hex_project::TrimEdge::Left,
                frame: 1000,
            })
            .unwrap();
        Self {
            editor,
            file,
            clip,
            asset,
        }
    }
    pub fn bank(&self) -> MediaBank {
        MediaBank {
            epoch: 11,
            device_only: Default::default(),
            resident: BTreeMap::new(),
            verified: BTreeMap::from([(self.asset, self.file.clone())]),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.file.path());
    }
}

#[test]
fn granular_semantic_projection_preserves_exact_converted_trimmed_root() {
    for mode in [
        WarpMode::Tones { grain_ms: 32 },
        WarpMode::Texture {
            grain_ms: 32,
            fluctuation: 35,
            seed: 42,
        },
    ] {
        for converted in [false, true] {
            let fixture = Fixture::new(mode, converted);
            let project = fixture.editor.project();
            let root = granular_source_root(
                project,
                project.clip(fixture.clip).unwrap(),
                &fixture.file,
                11,
            )
            .unwrap()
            .unwrap();
            let prepared = fixture
                .bank()
                .prepare(project, Arc::new(hex_engine::SineTable::new()), None)
                .unwrap();
            let metadata: Vec<_> = prepared.streamed_sources().collect();
            assert_eq!(metadata, vec![root.metadata()]);
            assert!(prepared.arrangement().contains_streamed_sources());
            let raw = fixture.file.source_metadata(fixture.asset, 11).unwrap();
            let expected = hex_engine::audio_transform::streamed_granular_root(
                &raw,
                project.clip(fixture.clip).unwrap().audio_edit().unwrap(),
                project.tempo_map(),
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(root.metadata(), expected.metadata());
            assert!(fixture.bank().resident.is_empty());
        }
    }
}

#[test]
fn granular_projection_refuses_unsupported_shape_and_mismatched_file_facts() {
    let mut fixture = Fixture::new(WarpMode::Tones { grain_ms: 32 }, false);
    let project = fixture.editor.project();
    assert!(
        granular_source_root(
            project,
            project.clip(fixture.clip).unwrap(),
            &fixture.file,
            0
        )
        .is_err()
    );
    Arc::get_mut(&mut fixture.file.facts).unwrap().source_frames += 1;
    assert!(
        granular_source_root(
            fixture.editor.project(),
            fixture.editor.project().clip(fixture.clip).unwrap(),
            &fixture.file,
            11
        )
        .is_err()
    );
    assert!(
        fixture
            .bank()
            .prepare(
                fixture.editor.project(),
                Arc::new(hex_engine::SineTable::new()),
                None
            )
            .is_err()
    );
    Arc::get_mut(&mut fixture.file.facts).unwrap().source_frames -= 1;
    fixture
        .editor
        .execute(Command::SetClipFades {
            clip: fixture.clip,
            fade_in: 8,
            fade_out: 8,
        })
        .unwrap();
    assert!(
        fixture
            .bank()
            .prepare(
                fixture.editor.project(),
                Arc::new(hex_engine::SineTable::new()),
                None
            )
            .is_err()
    );
}
