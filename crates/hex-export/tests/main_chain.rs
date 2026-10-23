//! Nonlinear Main order, shared live/export PCM and pre-Main stem boundaries.
use hex_engine::{
    Audio, OutputCapture, OutputCaptureId, PreparedRevision, Renderer, SessionPlayback,
    SessionPlaybackCommand, SineTable,
};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{Command, Created, Editor, Project};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hex-main-export-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Fixture {
    editor: Editor,
    media: BTreeMap<hex_project::AssetId, Audio>,
    track: hex_project::TrackId,
    bus: hex_project::ReturnId,
}
fn fixture(compensated: bool) -> Fixture {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "memory/main-order.wav".into(),
            sample_rate: 48_000,
            frames: 320,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "source".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::PlaceClip {
            track,
            asset,
            start: 0,
            duration: 320,
            source_offset: 0,
            source_length: 320,
            looping: false,
        })
        .unwrap();
    let Created::ReturnTrack(bus) = editor
        .execute(Command::AddReturnTrack {
            name: "send".into(),
        })
        .unwrap()
    else {
        panic!("return")
    };
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track: bus,
            gain: 1.0,
        })
        .unwrap();
    if compensated {
        let Created::ReturnTrack(destination) = editor
            .execute(Command::AddReturnTrack {
                name: "feed".into(),
            })
            .unwrap()
        else {
            panic!("return")
        };
        editor
            .execute(Command::SetReturnFeed {
                source: bus,
                destination,
                gain: 1.0,
            })
            .unwrap();
    }
    let state = hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.saturator"))
        .unwrap();
    let state = hex_engine::patch_native_device_parameter(
        &state,
        ParameterId::from_static(4),
        ParameterValue::Choice(1),
    )
    .unwrap();
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap();
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    Fixture {
        editor,
        media: BTreeMap::from([(asset, Audio::new(48_000, vec![[0.6, -0.6]; 320]).unwrap())]),
        track,
        bus,
    }
}
fn prepared(f: &Fixture) -> Arc<PreparedRevision> {
    Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &f.media,
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    )
}
fn assert_pcm(path: &Path, expected: [f32; 2]) {
    let bytes = fs::read(path).unwrap();
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(bytes.len(), 44 + 320 * 4);
    for sample in bytes[44..].chunks_exact(4) {
        for channel in 0..2 {
            let actual = i16::from_le_bytes([sample[channel * 2], sample[channel * 2 + 1]]);
            let expected = (expected[channel] * 32767.0).round() as i16;
            assert_eq!(actual, expected, "actual serialized PCM");
        }
    }
}
#[test]
fn nonlinear_main_runs_once_after_track_return_sum_before_master_and_observer() {
    for compensated in [false, true] {
        let f = fixture(compensated);
        let prepared = prepared(&f);
        let mut renderer = Renderer::new(prepared.arrangement());
        renderer.play();
        let mut actual = [[9.0; 2]; 320];
        let mut observed = actual;
        renderer
            .render_with_main_observer(
                &mut actual,
                Some(&mut |frames| observed.copy_from_slice(frames)),
            )
            .unwrap();
        assert_eq!(
            actual,
            [[0.5, -0.5]; 320],
            "nonlinear post-sum/Main then master, compensated={compensated}"
        );
        assert_eq!(observed, actual);
        let mut session = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
        session.apply(SessionPlaybackCommand::Play).unwrap();
        session
            .render_with_post_node_and_main_observers(
                &mut actual,
                None,
                Some(&mut |frames| observed.copy_from_slice(frames)),
            )
            .unwrap();
        assert_eq!(actual, [[0.5, -0.5]; 320]);
        assert_eq!(observed, actual);

        let mut capture_runtime = SessionPlayback::new_linear_arrangement_export(prepared).unwrap();
        let plan = capture_runtime
            .output_capture_plan(
                &[
                    OutputCaptureId::Track(f.track),
                    OutputCaptureId::Return(f.bus),
                ],
                320,
            )
            .unwrap();
        capture_runtime.apply(SessionPlaybackCommand::Play).unwrap();
        let mut track = [[0.0; 2]; 320];
        let mut bus = track;
        let mut slices = [track.as_mut_slice(), bus.as_mut_slice()];
        let mut capture = OutputCapture::new(&plan, &mut slices);
        capture_runtime
            .render_with_capture(&mut actual, &mut capture)
            .unwrap();
        assert_eq!(capture.valid_frames(), 320);
        assert_eq!(capture.output(0).unwrap(), [[0.6, -0.6]; 320]);
        assert_eq!(capture.output(1).unwrap(), [[0.6, -0.6]; 320]);
        assert_eq!(actual, [[0.5, -0.5]; 320]);
    }
}
#[test]
fn actual_linear_prepared_wav_and_stems_preserve_main_boundary() {
    for compensated in [false, true] {
        let f = fixture(compensated);
        let prepared = prepared(&f);
        let directory = Directory::new();
        let linear = directory.0.join("linear.wav");
        let shared = directory.0.join("prepared.wav");
        hex_export::render_new(&linear, prepared.arrangement(), 0, 320).unwrap();
        hex_export::render_prepared_new(&shared, Arc::clone(&prepared), 0, 320).unwrap();
        assert_pcm(&linear, [0.5, -0.5]);
        assert_pcm(&shared, [0.5, -0.5]);
        assert_eq!(fs::read(&linear).unwrap(), fs::read(&shared).unwrap());
        let stems = directory.0.join("stems");
        hex_export::render_stems_new(
            &stems,
            prepared,
            0,
            320,
            &[
                hex_export::Stem {
                    id: OutputCaptureId::Track(f.track),
                    name: "source".into(),
                },
                hex_export::Stem {
                    id: OutputCaptureId::Return(f.bus),
                    name: "send".into(),
                },
            ],
            &hex_export::ExportControl::default(),
        )
        .unwrap();
        assert_pcm(
            &stems.join(format!("track-{}-source.wav", f.track.value())),
            [0.6, -0.6],
        );
        assert_pcm(
            &stems.join(format!("return-{}-send.wav", f.bus.value())),
            [0.6, -0.6],
        );
    }
}
#[test]
fn main_bypass_and_remove_restore_raw_sum_with_exact_undo_redo() {
    let mut f = fixture(false);
    let main = f.editor.project().main_devices()[0].id();
    let original = f.editor.project().snapshot();
    for command in [
        Command::SetDeviceBypass {
            device: main,
            bypassed: true,
        },
        Command::RemoveDevice(main),
    ] {
        f.editor.execute(command).unwrap();
        let changed = f.editor.project().snapshot();
        let revision = prepared(&f);
        let mut session = SessionPlayback::new(revision).unwrap();
        session.apply(SessionPlaybackCommand::Play).unwrap();
        let mut frames = [[0.0; 2]; 64];
        session.render(&mut frames).unwrap();
        assert_eq!(frames, [[0.6, -0.6]; 64]);
        assert!(f.editor.undo());
        assert_eq!(f.editor.project().snapshot(), original);
        assert!(f.editor.redo());
        assert_eq!(f.editor.project().snapshot(), changed);
        assert!(f.editor.undo());
    }
}

#[test]
fn controlled_range_export_reconstructs_stateful_main_history_from_zero() {
    for compensated in [false, true] {
        let mut f = fixture(compensated);
        let old = f.editor.project().main_devices()[0].id();
        f.editor.execute(Command::RemoveDevice(old)).unwrap();
        let state = hex_engine::patch_native_device_parameter(
            &hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.utility"))
                .unwrap(),
            ParameterId::from_static(11),
            ParameterValue::Bool(true),
        )
        .unwrap();
        f.editor
            .execute(Command::InsertMainDevice {
                before: None,
                state,
            })
            .unwrap();
        let prepared = prepared(&f);
        assert!(prepared.arrangement_seek_requires_preroll());
        let mut oracle =
            SessionPlayback::new_linear_arrangement_export(Arc::clone(&prepared)).unwrap();
        oracle.apply(SessionPlaybackCommand::Play).unwrap();
        let mut full = [[0.0; 2]; 192];
        oracle.render(&mut full).unwrap();
        assert_ne!(
            full[128], full[0],
            "cold history cannot stand in for replay"
        );
        let directory = Directory::new();
        let path = directory.0.join("stateful-range.wav");
        let control = hex_export::ExportControl::default();
        hex_export::render_prepared_controlled(
            &path,
            prepared,
            128,
            64,
            hex_export::ExportSignal::Main,
            &control,
        )
        .unwrap();
        let bytes = fs::read(path).unwrap();
        assert_eq!(bytes.len(), 44 + 64 * 4);
        for (sample, expected) in bytes[44..].chunks_exact(4).zip(&full[128..192]) {
            for channel in 0..2 {
                let actual = i16::from_le_bytes([sample[channel * 2], sample[channel * 2 + 1]]);
                assert_eq!(
                    actual,
                    (expected[channel].clamp(-1.0, 1.0) * 32767.0).round() as i16
                );
            }
        }
        assert_eq!(control.progress().processed_frames, 192);
        assert_eq!(control.progress().total_frames, 192);
    }
}
