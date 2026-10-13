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
    assert_return_export(false);
}
#[test]
fn prepared_export_includes_authored_additive_return_feed_before_master_once() {
    assert_return_export(true);
}
fn assert_return_export(authored_feed: bool) {
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

    if authored_feed {
        let Created::ReturnTrack(destination) = editor
            .execute(Command::AddReturnTrack {
                name: "destination".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        editor
            .execute(Command::SetReturnFeed {
                source: return_track,
                destination,
                gain: 0.5,
            })
            .unwrap();
    }
    let media = BTreeMap::from([(asset, Audio::new(RATE, samples.to_vec()).expect("audio"))]);
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
            .expect("prepared return revision"),
    );
    let directory = TestDir::new();
    let path = directory.0.join("return.wav");
    let report =
        hex_export::render_prepared_new(&path, revision, 0, 1).expect("prepared Return export");
    assert_eq!(report.frames, 1);
    let bytes = fs::read(path).expect("read export");
    let left = i16::from_le_bytes([bytes[44], bytes[45]]);
    let right = i16::from_le_bytes([bytes[46], bytes[47]]);
    let expected = if authored_feed {
        [0.109375_f32, -0.03125_f32]
    } else {
        [0.09375_f32, -0.0625_f32]
    };
    for (actual, expected) in [left, right].into_iter().zip(expected) {
        let pcm = (expected * 32_767.0).round() as i16;
        assert!((i32::from(actual) - i32::from(pcm)).abs() <= 1);
    }
}
