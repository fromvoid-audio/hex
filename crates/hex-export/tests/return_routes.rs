use hex_engine::{Audio, PreparedRevision, SineTable};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{Command, Created, Editor, Project};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

const RATE: u32 = 48_000;
static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("hex-export-return-{}-{id}", std::process::id()));
        fs::create_dir(&path).expect("create isolated export directory");
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn prepared_export_renders_effectful_return_before_master_once() {
    let samples = [[0.25, -0.5]];
    let mut editor = Editor::new(Project::new(RATE).expect("project"), 16).expect("editor");
    let asset = match editor
        .execute(Command::AddAsset {
            path: "memory/return-export.wav".into(),
            sample_rate: RATE,
            frames: 1,
        })
        .expect("asset")
    {
        Created::Asset(id) => id,
        _ => panic!("expected audio asset"),
    };
    let track = match editor
        .execute(Command::AddTrack {
            name: "source".into(),
        })
        .expect("track")
    {
        Created::Track(id) => id,
        _ => panic!("expected audio track"),
    };
    editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 1,
            source_offset: 0,
            source_length: 1,
            looping: false,
        })
        .expect("clip");
    editor
        .execute(Command::Gain { track, gain: 0.5 })
        .expect("track gain");
    let return_track = match editor
        .execute(Command::AddReturnTrack {
            name: "utility".into(),
        })
        .expect("return")
    {
        Created::ReturnTrack(id) => id,
        _ => panic!("expected return track"),
    };
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track,
            gain: 0.5,
        })
        .expect("send");
    editor
        .execute(Command::MasterGain { gain: 0.5 })
        .expect("master gain");
    editor
        .execute(Command::InsertReturnDevice {
            return_track,
            before: None,
            state: hex_engine::patch_native_device_parameter(
                &hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.utility"))
                    .expect("Utility defaults"),
                ParameterId::from_static(6),
                ParameterValue::Bool(true),
            )
            .expect("Utility state"),
        })
        .expect("return effect");

    let media = BTreeMap::from([(asset, Audio::new(RATE, samples.to_vec()).expect("audio"))]);
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
            .expect("prepared return revision"),
    );
    let directory = TestDir::new();
    let path = directory.0.join("return.wav");
    let report = hex_export::render_prepared_new(&path, Arc::clone(&revision), 0, 1)
        .expect("prepared Return export");
    assert_eq!(report.frames, 1);
    let bytes = fs::read(path).expect("read export");
    let left = i16::from_le_bytes([bytes[44], bytes[45]]);
    let right = i16::from_le_bytes([bytes[46], bytes[47]]);
    let expected = [0.09375_f32, -0.0625_f32];
    for (actual, expected) in [left, right].into_iter().zip(expected) {
        let pcm = (expected * 32_767.0).round() as i16;
        assert!((i32::from(actual) - i32::from(pcm)).abs() <= 1);
    }

    // Capture actual post-fader/Return outputs from the same graph, excluding
    // master gain. These independent PCM expectations distinguish send input,
    // Return output, dry track and Main instead of mirroring capture buffers.
    for (name, id, expected) in [
        (
            "track",
            hex_engine::OutputCaptureId::Track(track),
            [0.125_f32, -0.25],
        ),
        (
            "return",
            hex_engine::OutputCaptureId::Return(return_track),
            [0.0625_f32, 0.125],
        ),
    ] {
        let control = hex_export::ExportControl::default();
        let target = directory.0.join(format!("stem-{name}.wav"));
        hex_export::render_prepared_controlled(
            &target,
            Arc::clone(&revision),
            0,
            1,
            hex_export::ExportSignal::CapturedMix(vec![id]),
            &control,
        )
        .expect("captured output export");
        let data = fs::read(target).unwrap();
        for (channel, expected) in expected.into_iter().enumerate() {
            let offset = 44 + channel * 2;
            assert_eq!(
                i16::from_le_bytes([data[offset], data[offset + 1]]),
                (expected * 32767.0).round() as i16
            );
        }
        assert_eq!(
            control.progress(),
            hex_export::ExportProgress {
                processed_frames: 1,
                total_frames: 1
            }
        );
    }
    let batch = directory.0.join("stems");
    let stems = vec![
        hex_export::Stem {
            id: hex_engine::OutputCaptureId::Return(return_track),
            name: "same/name".into(),
        },
        hex_export::Stem {
            id: hex_engine::OutputCaptureId::Track(track),
            name: "same/name".into(),
        },
    ];
    hex_export::render_stems_new(
        &batch,
        Arc::clone(&revision),
        0,
        1,
        &stems,
        &hex_export::ExportControl::default(),
    )
    .unwrap();
    let files: Vec<_> = fs::read_dir(&batch)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(files.len(), 2);
    for (kind, id, expected) in [
        ("track", track.value(), [0.125_f32, -0.25]),
        ("return", return_track.value(), [0.0625_f32, 0.125]),
    ] {
        let data = fs::read(batch.join(format!("{kind}-{id}-same_name.wav"))).unwrap();
        assert_eq!(data.len(), 48);
        for (channel, expected) in expected.into_iter().enumerate() {
            let offset = 44 + channel * 2;
            assert_eq!(
                i16::from_le_bytes([data[offset], data[offset + 1]]),
                (expected * 32767.0).round() as i16
            );
        }
    }
    // Refuse the whole existing batch; never merge/delete files in it.
    fs::write(batch.join("unrelated.txt"), b"preserve me").unwrap();
    assert!(
        hex_export::render_stems_new(
            &batch,
            Arc::clone(&revision),
            0,
            1,
            &stems,
            &hex_export::ExportControl::default()
        )
        .is_err()
    );
    assert_eq!(
        fs::read(batch.join("unrelated.txt")).unwrap(),
        b"preserve me"
    );
    let duplicate = directory.0.join("duplicates");
    let repeated = vec![stems[0].clone(), stems[0].clone()];
    assert!(
        hex_export::render_stems_new(
            &duplicate,
            Arc::clone(&revision),
            0,
            1,
            &repeated,
            &hex_export::ExportControl::default()
        )
        .is_err()
    );
    assert!(!duplicate.exists());
    assert!(!fs::read_dir(&directory.0).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".hex-stems-")
    }));
    let cancelled = hex_export::ExportControl::default();
    cancelled.cancel();
    let target = directory.0.join("cancelled.wav");
    assert!(matches!(
        hex_export::render_prepared_controlled(
            &target,
            revision,
            0,
            1,
            hex_export::ExportSignal::Main,
            &cancelled
        ),
        Err(hex_export::ExportError::Cancelled)
    ));
    assert!(!target.exists());
}
