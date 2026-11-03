//! Saved loop trims/splits must retain the same source samples at surviving times.
use hex_project::{Command, Created, Editor, Project, TrimEdge};
use std::{fs, path::PathBuf, process::Command as ProcessCommand};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("hex-cli-loop-trims-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        let samples = [0_i16, 8192, 16384, -8192, 0];
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&46_u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&48_000_u32.to_le_bytes());
        wav.extend_from_slice(&96_000_u32.to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&10_u32.to_le_bytes());
        for sample in samples {
            wav.extend_from_slice(&sample.to_le_bytes());
        }
        fs::write(path.join("source.wav"), wav).unwrap();
        Self(path)
    }

    fn render(&self, project: &Project, stem: &str) -> Vec<i16> {
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
        let wav = fs::read(self.0.join(output)).unwrap();
        assert_eq!(&wav[36..40], b"data");
        wav[44..]
            .chunks_exact(4)
            .map(|frame| {
                let left = i16::from_le_bytes([frame[0], frame[1]]);
                let right = i16::from_le_bytes([frame[2], frame[3]]);
                // Independent per-channel ±1 LSB TPDF dither.
                assert!((i32::from(left) - i32::from(right)).abs() <= 2);
                left
            })
            .collect()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn saved_loop_edge_trims_and_recursive_splits_preserve_absolute_pcm() {
    let fixture = Fixture::new();
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "loop".into(),
        })
        .unwrap()
    else {
        panic!("track created");
    };
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "source.wav".into(),
            sample_rate: 48_000,
            frames: 5,
        })
        .unwrap()
    else {
        panic!("asset created");
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 10,
            duration: 12,
            source_offset: 1,
            source_length: 3,
            looping: true,
        })
        .unwrap()
    else {
        panic!("clip created");
    };
    let original = fixture.render(editor.project(), "original");
    assert_eq!(&original[..10], &[0; 10]);
    assert_near(
        &original[10..],
        &[
            8192, 16384, -8192, 8192, 16384, -8192, 8192, 16384, -8192, 8192, 16384, -8192,
        ],
    );
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Left,
            frame: 12,
        })
        .unwrap();
    assert_eq!(editor.project().clip(clip).unwrap().source_phase(), 2);
    let trimmed = fixture.render(editor.project(), "trimmed");
    assert_eq!(&trimmed[..12], &[0; 12]);
    assert_eq!(&trimmed[12..], &original[12..]);
    assert!(editor.undo());
    assert_eq!(fixture.render(editor.project(), "undo"), original);
    assert!(editor.redo());
    assert_eq!(fixture.render(editor.project(), "redo"), trimmed);
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Left,
            frame: 11,
        })
        .unwrap();
    assert_eq!(editor.project().clip(clip).unwrap().source_phase(), 1);
    editor
        .execute(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Right,
            frame: 25,
        })
        .unwrap();
    assert_eq!(editor.project().clip(clip).unwrap().source_length(), 3);
    let extended = fixture.render(editor.project(), "extended");
    assert_eq!(&extended[11..22], &original[11..22]);
    assert_near(&extended[22..], &[8192, 16384, -8192]);
    let Created::Clip(right) = editor
        .execute(Command::SplitClip { clip, frame: 17 })
        .unwrap()
    else {
        panic!("right child created");
    };
    assert_eq!(editor.project().clip(right).unwrap().source_phase(), 1);
    editor
        .execute(Command::SplitClip {
            clip: right,
            frame: 19,
        })
        .unwrap();
    assert_eq!(fixture.render(editor.project(), "split"), extended);
}

/// PCM16 export applies ±1 LSB TPDF dither: within one LSB of `expected`.
/// Renders of identical timeline audio stay byte-identical, so equality
/// comparisons between renders remain exact.
fn assert_near(actual: &[i16], expected: &[i16]) {
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        assert!(
            (i32::from(*actual) - i32::from(*expected)).abs() <= 1,
            "{actual} not within one lsb of {expected}"
        );
    }
}
