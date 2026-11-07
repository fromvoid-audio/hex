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
const MAIN_SATURATOR_LATENCY: usize = 11;
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
/// Independent Main oracle. The raw pre-Main sum is rendered with the Main
/// Saturator removed, then fed once
/// through the same oversampled Saturator state on a lone track, before the
/// same master gain. Running Main per source instead (2 x sat(0.6)) differs.
fn main_oracle(compensated: bool, frames: usize) -> Vec<[f32; 2]> {
    let mut raw = fixture(compensated);
    let main = raw.editor.project().main_devices()[0].id();
    let state = raw.editor.project().main_devices()[0].state().clone();
    raw.editor.execute(Command::RemoveDevice(main)).unwrap();
    let mut session = SessionPlayback::new(prepared(&raw)).unwrap();
    session.apply(SessionPlaybackCommand::Play).unwrap();
    let mut sum = vec![[0.0; 2]; 320];
    session.render(&mut sum).unwrap();
    // Master gain 0.5 is a power of two, so undoing it is exact.
    let sum: Vec<_> = sum.iter().map(|frame| frame.map(|v| v * 2.0)).collect();
    assert!(sum.iter().all(|frame| frame[0] >= 1.2 && frame[1] <= -1.2));

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 32).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "memory/main-oracle.wav".into(),
            sample_rate: 48_000,
            frames: 320,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "oracle".into(),
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
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state,
        })
        .unwrap();
    editor.execute(Command::MasterGain { gain: 0.5 }).unwrap();
    let oracle = Fixture {
        editor,
        media: BTreeMap::from([(asset, Audio::new(48_000, sum).unwrap())]),
        track,
        bus: raw.bus,
    };
    let mut session = SessionPlayback::new(prepared(&oracle)).unwrap();
    session.apply(SessionPlaybackCommand::Play).unwrap();
    let mut expected = vec![[0.0; 2]; frames];
    session.render(&mut expected).unwrap();
    // The oversampled Saturator delays and band-limits the clipped step, is
    // silent before its latency and settles on exact clipping.
    assert_eq!(expected[0], [0.0; 2]);
    assert_eq!(expected[319], [0.5, -0.5]);
    expected
}
/// PCM16 export applies ±1 LSB TPDF dither, so each sample lands within one
/// LSB of plain rounding.
fn assert_pcm16_near(actual: i16, expected: f32, context: &str) {
    let rounded = (expected.clamp(-1.0, 1.0) * 32_767.0).round() as i32;
    assert!(
        (i32::from(actual) - rounded).abs() <= 1,
        "{context}: {actual} not within one lsb of {rounded}"
    );
}
fn assert_pcm(path: &Path, expected: &[[f32; 2]]) {
    let bytes = fs::read(path).unwrap();
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(bytes.len(), 44 + 320 * 4);
    assert_eq!(expected.len(), 320);
    for (sample, expected) in bytes[44..].chunks_exact(4).zip(expected) {
        for channel in 0..2 {
            let actual = i16::from_le_bytes([sample[channel * 2], sample[channel * 2 + 1]]);
            assert_pcm16_near(actual, expected[channel], "actual serialized PCM");
        }
    }
}
#[test]
fn nonlinear_main_runs_once_after_track_return_sum_before_master_and_observer() {
    for compensated in [false, true] {
        let f = fixture(compensated);
        let prepared = prepared(&f);
        let expected = main_oracle(compensated, 320);
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
            actual.as_slice(),
            expected.as_slice(),
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
        assert_eq!(actual.as_slice(), expected.as_slice());
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
        assert_eq!(actual.as_slice(), expected.as_slice());
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
        // Exports align the file with the timeline: the Main Saturator's
        // declared 11-frame latency is rendered and dropped, so the file's
        // first frame is the oracle's frame 11 and its tail includes the
        // band-limited release after the clip ends.
        let expected = main_oracle(compensated, 320 + MAIN_SATURATOR_LATENCY)
            .split_off(MAIN_SATURATOR_LATENCY);
        assert_pcm(&linear, &expected);
        assert_pcm(&shared, &expected);
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
            &[[0.6, -0.6]; 320],
        );
        assert_pcm(
            &stems.join(format!("return-{}-send.wav", f.bus.value())),
            &[[0.6, -0.6]; 320],
        );
    }
}
#[test]
fn main_bypass_and_remove_restore_raw_sum_with_exact_undo_redo() {
    let mut f = fixture(false);
    let main = f.editor.project().main_devices()[0].id();
    let original = f.editor.project().snapshot();
    for (command, latency) in [
        (
            Command::SetDeviceBypass {
                device: main,
                bypassed: true,
            },
            MAIN_SATURATOR_LATENCY,
        ),
        (Command::RemoveDevice(main), 0),
    ] {
        f.editor.execute(command).unwrap();
        let changed = f.editor.project().snapshot();
        let revision = prepared(&f);
        assert_eq!(
            usize::try_from(revision.arrangement().main_latency_frames()).unwrap(),
            latency
        );
        let mut session = SessionPlayback::new(revision).unwrap();
        session.apply(SessionPlaybackCommand::Play).unwrap();
        let mut frames = [[0.0; 2]; 64];
        session.render(&mut frames).unwrap();
        assert!(frames[..latency].iter().all(|frame| *frame == [0.0; 2]));
        assert!(frames[latency..].iter().all(|frame| *frame == [0.6, -0.6]));
        assert!(f.editor.try_undo().unwrap());
        assert_eq!(f.editor.project().snapshot(), original);
        assert!(f.editor.try_redo().unwrap());
        assert_eq!(f.editor.project().snapshot(), changed);
        assert!(f.editor.try_undo().unwrap());
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
                assert_pcm16_near(actual, expected[channel], "selected range PCM");
            }
        }
        assert_eq!(control.progress().processed_frames, 192);
        assert_eq!(control.progress().total_frames, 192);
    }
}
