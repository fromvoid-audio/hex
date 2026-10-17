//! Saved metadata, media resolution, compilation and WAV publication together.
use hex_project::{Command, Created, Editor, Project};
use std::{
    fs,
    path::PathBuf,
    process::Command as ProcessCommand,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new(frames: u32) -> Self {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("hex-cli-fades-{}-{id}", std::process::id()));
        fs::create_dir(&path).expect("create original fixture directory");
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + frames * 2).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&48_000_u32.to_le_bytes());
        wav.extend_from_slice(&96_000_u32.to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(frames * 2).to_le_bytes());
        for _ in 0..frames {
            wav.extend_from_slice(&8_192_i16.to_le_bytes());
        }
        fs::write(path.join("Original-MiXeD.WAV"), wav).expect("write own quarter-scale PCM");
        Self(path)
    }

    fn editor(&self, frames: u64) -> (Editor, hex_project::ClipId) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        let Created::Imported { clip, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: "original".into(),
                path: "Original-MiXeD.WAV".into(),
                sample_rate: 48_000,
                frames,
            })
            .unwrap()
        else {
            panic!("import creates clip");
        };
        (editor, clip)
    }

    fn render(&self, project: &Project, stem: &str) -> Vec<u8> {
        let metadata = format!("{stem}.hex");
        let output = format!("{stem}.wav");
        hex_storage::save_new(self.0.join(&metadata), project).unwrap();
        assert_eq!(hex_storage::load(self.0.join(&metadata)).unwrap(), *project);
        let result = ProcessCommand::new(env!("CARGO_BIN_EXE_hex-cli"))
            .current_dir(&self.0)
            .args(["render-project", &metadata, &output])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        fs::read(self.0.join(output)).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn assert_stereo_pcm(wav: &[u8], expected: &[i16]) {
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(&wav[36..40], b"data");
    assert_eq!(wav.len(), 44 + expected.len() * 4);
    for (frame, value) in wav[44..].chunks_exact(4).zip(expected) {
        assert_eq!(i16::from_le_bytes([frame[0], frame[1]]), *value);
        assert_eq!(i16::from_le_bytes([frame[2], frame[3]]), *value);
    }
}

#[test]
fn saved_fade_endpoints_and_recursive_splits_export_identical_pcm() {
    for shaped in [false, true] {
        let fixture = Fixture::new(13);
        let (mut editor, clip) = fixture.editor(13);
        if shaped {
            editor
                .execute(Command::SetClipFadeEnvelope {
                    clip,
                    fade: hex_project::ClipFade::new(13, 0, 5, 5)
                        .unwrap()
                        .with_shape(1, 1, 500, -500)
                        .unwrap(),
                })
                .unwrap();
        } else {
            editor
                .execute(Command::SetClipFades {
                    clip,
                    fade_in: 5,
                    fade_out: 5,
                })
                .unwrap();
        }
        let original = fixture.render(editor.project(), "faded");
        let expected: &[i16] = if shaped {
            &[
                0, 0, 820, 2050, 4099, 8192, 8192, 8192, 7372, 6142, 4093, 0, 0,
            ]
        } else {
            &[
                0, 2048, 4096, 6144, 8192, 8192, 8192, 8192, 8192, 6144, 4096, 2048, 0,
            ]
        };
        assert_stereo_pcm(&original, expected);
        let Created::Clip(right) = editor
            .execute(Command::SplitClip { clip, frame: 2 })
            .unwrap()
        else {
            panic!("split creates right clip");
        };
        editor
            .execute(Command::SplitClip { clip, frame: 1 })
            .unwrap();
        let Created::Clip(tail) = editor
            .execute(Command::SplitClip {
                clip: right,
                frame: 10,
            })
            .unwrap()
        else {
            panic!("split creates tail");
        };
        editor
            .execute(Command::SplitClip {
                clip: tail,
                frame: 11,
            })
            .unwrap();
        assert_eq!(fixture.render(editor.project(), "split"), original);
        assert!(editor.undo());
        assert_eq!(fixture.render(editor.project(), "undo"), original);
        assert!(editor.redo());
        assert_eq!(fixture.render(editor.project(), "redo"), original);
    }
}

#[test]
fn overlapping_saved_ramps_multiply_and_reset_restores_neutral_pcm() {
    let fixture = Fixture::new(5);
    let (mut editor, clip) = fixture.editor(5);
    let neutral = fixture.render(editor.project(), "neutral");
    assert_stereo_pcm(&neutral, &[8192; 5]);
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 5,
            fade_out: 5,
        })
        .unwrap();
    assert_stereo_pcm(
        &fixture.render(editor.project(), "overlap"),
        &[0, 1536, 2048, 1536, 0],
    );
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    assert_eq!(fixture.render(editor.project(), "reset"), neutral);
}
