use hex_engine::{
    NativeDeviceClass, PreparedRevision, SineTable, default_native_device_state_for_class,
    patch_native_device_parameter_for_class,
};
use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{
    collections::BTreeMap,
    f64::consts::TAU,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

const RATE: u32 = 48_000;
const TYPE: DeviceTypeId = DeviceTypeId::from_static("hex.instrument.operator");
const RELEASE_FRAMES: u64 = 240;
static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("hex-export-operator-{}-{id}", std::process::id()));
        fs::create_dir(&path).expect("create isolated export directory");
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn operator_state() -> hex_lattice_contract::NativeDeviceState {
    let mut state = default_native_device_state_for_class(NativeDeviceClass::Instrument, TYPE)
        .expect("registered Operator defaults");
    for (id, value) in [
        (29, 1.0),
        (33, 1.0),
        (34, 1.0),
        (35, 1.0),
        (36, 0.0),
        (37, 0.001),
        (38, 0.005),
    ] {
        state = patch_native_device_parameter_for_class(
            NativeDeviceClass::Instrument,
            &state,
            ParameterId::from_static(id),
            ParameterValue::Float(value),
        )
        .expect("patch valid Operator envelope");
    }
    state
}

fn expected(age: u64) -> f32 {
    let envelope = if age < 6_000 {
        1.0
    } else {
        let release_age = age - 6_000;
        if release_age >= RELEASE_FRAMES {
            0.0
        } else {
            0.000_01_f64.powf(release_age as f64 / RELEASE_FRAMES as f64)
        }
    };
    ((TAU * 440.0 * age as f64 / f64::from(RATE)).sin() * envelope) as f32
}

fn operator_revision(state: hex_lattice_contract::NativeDeviceState) -> Arc<PreparedRevision> {
    operator_revision_with_tuning(state, None)
}

fn operator_revision_with_tuning(
    state: hex_lattice_contract::NativeDeviceState,
    hz: Option<f64>,
) -> Arc<PreparedRevision> {
    let mut editor = Editor::new(Project::new(RATE).expect("project"), 16).expect("editor");
    let track = match editor
        .execute(Command::InsertTrackWithDevice {
            name: "operator export".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .expect("MIDI track")
    {
        Created::Track(track) => track,
        _ => panic!("expected track"),
    };
    let instrument = editor
        .project()
        .track(track)
        .and_then(|track| track.instrument())
        .expect("default instrument")
        .id();
    editor
        .execute(Command::ReplaceDeviceState {
            device: instrument,
            state,
        })
        .expect("register Operator state");
    let clip = match editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(8).expect("clip length"),
        })
        .expect("MIDI clip")
    {
        Created::Clip(clip) => clip,
        _ => panic!("expected MIDI clip"),
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_ratio(1, 4).expect("note duration"),
            pitch: 69,
            channel: 0,
            velocity: 127,
        })
        .expect("MIDI note");

    if let Some(hz) = hz {
        let tuning = editor.project().tuning();
        let mut scale = tuning.scale();
        scale.set_reference(69, hz, 0).expect("tuned reference");
        editor
            .execute(Command::SetTuning(
                hex_tuning::ProjectTuning::new(true, scale, tuning.audition())
                    .expect("valid tuning"),
            ))
            .expect("project tuning");
    }
    Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .expect("prepared registered Operator"),
    )
}

#[test]
fn project_tuning_operator_export_uses_custom_pitch_across_blocks_and_release() {
    let state = patch_native_device_parameter_for_class(
        NativeDeviceClass::Instrument,
        &operator_state(),
        ParameterId::from_static(608),
        ParameterValue::Choice(0),
    )
    .expect("legacy processing");
    let revision = operator_revision_with_tuning(state, Some(333.0));
    let directory = TestDir::new();
    let path = directory.0.join("tuned-operator.wav");
    hex_export::render_prepared_new(&path, revision, 0, 6500).expect("tuned export");
    let bytes = fs::read(path).expect("exported PCM");
    for (age, samples) in bytes[44..].chunks_exact(4).enumerate() {
        let envelope = if age < 6000 {
            1.0
        } else if age < 6240 {
            0.000_01_f64.powf((age - 6000) as f64 / 240.0)
        } else {
            0.0
        };
        let expected = (((TAU * 333.0 * age as f64 / f64::from(RATE)).sin() * envelope) as f32
            * 32767.0)
            .round() as i16;
        for channel in samples.chunks_exact(2) {
            let actual = i16::from_le_bytes([channel[0], channel[1]]);
            assert!(
                (i32::from(actual) - i32::from(expected)).abs() <= 1,
                "tuned export frame {age}: {actual} vs {expected}"
            );
        }
    }
}

#[test]
fn legacy_operator_export_matches_independent_pcm16_oracle_across_blocks_and_release() {
    // This independent sine oracle is the historical output-rate law. The
    // remaining tests cover current operator-mode export/playback agreement.
    let state = patch_native_device_parameter_for_class(
        NativeDeviceClass::Instrument,
        &operator_state(),
        ParameterId::from_static(608),
        ParameterValue::Choice(0),
    )
    .expect("legacy processing");
    let revision = operator_revision(state);
    let directory = TestDir::new();
    let path = directory.0.join("operator.wav");
    let report = hex_export::render_prepared_new(&path, revision, 0, 6_500)
        .expect("prepared Operator export");
    assert_eq!(report.frames, 6_500);

    let bytes = fs::read(path).expect("read exported WAV");
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    assert_eq!(&bytes[36..40], b"data");
    for (frame_index, samples) in bytes[44..].chunks_exact(4).enumerate() {
        let left = i16::from_le_bytes([samples[0], samples[1]]);
        let right = i16::from_le_bytes([samples[2], samples[3]]);
        let expected_sample = (expected(frame_index as u64) * 32_767.0).round() as i16;
        // Independent per-channel dither: centred mono within two LSB.
        assert!(
            (i32::from(left) - i32::from(right)).abs() <= 2,
            "centered mono at frame {frame_index}"
        );
        assert!(
            (i32::from(left) - i32::from(expected_sample)).abs() <= 2,
            "PCM16 mismatch at frame {frame_index}: got {left}, expected {expected_sample}"
        );
    }
}

#[test]
fn mixed_waveform_operator_export_matches_partitioned_prepared_playback() {
    let mut state = operator_state();
    for (parameter, choice) in [(57, 1), (58, 2), (59, 3)] {
        state = patch_native_device_parameter_for_class(
            NativeDeviceClass::Instrument,
            &state,
            ParameterId::from_static(parameter),
            ParameterValue::Choice(choice),
        )
        .expect("periodic waveform choice");
    }
    let revision = operator_revision(state);
    let mut playback =
        hex_engine::playback::Playback::try_from_prepared_with_session(Arc::clone(&revision))
            .expect("prepared mixed-wave playback");
    playback.apply(hex_engine::PlaybackCommand::Play);
    let mut expected = vec![[0.0; 2]; 6_500];
    for block in expected.chunks_mut(97) {
        playback
            .render(block)
            .expect("partitioned mixed-wave render");
    }
    let directory = TestDir::new();
    let path = directory.0.join("mixed-operator.wav");
    let report = hex_export::render_prepared_new(&path, revision, 0, 6_500)
        .expect("mixed-wave prepared export");
    assert_eq!(report.frames, 6_500);
    let bytes = fs::read(path).expect("exported mixed-wave PCM");
    assert_eq!(bytes.len(), 44 + 6_500 * 4);
    for (samples, frame) in bytes[44..].chunks_exact(4).zip(expected) {
        for (channel, sample) in frame.into_iter().enumerate() {
            let offset = channel * 2;
            let actual = i16::from_le_bytes([samples[offset], samples[offset + 1]]);
            assert_pcm16_near(actual, sample, "export and playback disagree");
        }
    }
}

#[test]
fn lfo_free_sync_and_feedback_export_match_registered_partitioned_playback() {
    for sync in [false, true] {
        for selected in [false, true] {
            modulation_export(Some(sync), false, selected);
        }
    }
}

#[test]
fn feedback_operator_full_prepared_export_matches_registered_playback() {
    modulation_export(None, false, true);
}

#[test]
fn filter_modes_export_match_registered_playback_across_blocks_and_release() {
    for sync in [None, Some(false), Some(true)] {
        modulation_export(sync, true, true);
    }
}

fn modulation_export(lfo: Option<bool>, filter: bool, selected: bool) {
    let state = patch_native_device_parameter_for_class(
        NativeDeviceClass::Instrument,
        &operator_state(),
        ParameterId::from_static(78),
        ParameterValue::Float(0.4),
    )
    .expect("enable first oscillator feedback");
    let mut state = state;
    if let Some(sync) = lfo {
        for (id, value) in [
            (82, ParameterValue::Bool(true)),
            (83, ParameterValue::Choice(4)),
            (84, ParameterValue::Bool(sync)),
            (85, ParameterValue::Float(100.0)),
            (87, ParameterValue::Float(100.0)),
            (89, ParameterValue::Bool(selected)),
        ] {
            state = patch_native_device_parameter_for_class(
                NativeDeviceClass::Instrument,
                &state,
                ParameterId::from_static(id),
                value,
            )
            .unwrap();
        }
    }
    if filter {
        for (id, value) in [
            (20001, ParameterValue::Bool(true)),
            (20002, ParameterValue::Choice(4)),
            (20005, ParameterValue::Float(777.0)),
            (20006, ParameterValue::Float(80.0)),
            (20007, ParameterValue::Float(35.0)),
        ] {
            state = patch_native_device_parameter_for_class(
                NativeDeviceClass::Instrument,
                &state,
                ParameterId::from_static(id),
                value,
            )
            .unwrap();
        }
    }
    let revision = operator_revision(state);
    assert!(revision.has_arrangement_mutable_instruments());
    assert!(revision.arrangement_seek_requires_preroll());

    let mut playback =
        hex_engine::playback::Playback::try_from_prepared_with_session(Arc::clone(&revision))
            .expect("mutable Operator playback");
    playback.apply(hex_engine::PlaybackCommand::Play);
    let mut expected = vec![[0.0; 2]; 6_500];
    for block in expected.chunks_mut(113) {
        playback
            .render(block)
            .expect("registered feedback playback");
    }

    let directory = TestDir::new();
    let path = directory.0.join("operator-feedback.wav");
    let report = hex_export::render_prepared_new(&path, Arc::clone(&revision), 0, 6_500)
        .expect("linear prepared feedback export");
    assert_eq!(report.frames, 6_500);
    let bytes = fs::read(path).expect("read exported feedback wav");
    // Seek into both a held feedback note and its release. The selected files
    // must exactly match the full uninterrupted export's corresponding PCM.
    for start in [127, 5_999, 6_113] {
        let selected = directory.0.join(format!("feedback-range-{start}.wav"));
        let control = hex_export::ExportControl::default();
        hex_export::render_prepared_controlled(
            &selected,
            Arc::clone(&revision),
            start,
            300,
            hex_export::ExportSignal::Main,
            &control,
        )
        .expect("stateful selected range");
        let data = fs::read(selected).unwrap();
        let offset = 44 + start as usize * 4;
        assert_eq!(&data[44..], &bytes[offset..offset + 300 * 4]);
        assert_eq!(
            control.progress(),
            hex_export::ExportProgress {
                processed_frames: start + 300,
                total_frames: start + 300
            }
        );
    }
    for (frame_index, (samples, expected)) in bytes[44..].chunks_exact(4).zip(expected).enumerate()
    {
        let left = i16::from_le_bytes([samples[0], samples[1]]);
        let right = i16::from_le_bytes([samples[2], samples[3]]);
        let context = format!("feedback export mismatch at {frame_index}");
        assert_pcm16_near(left, expected[0], &context);
        assert_pcm16_near(right, expected[0], &context);
    }
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
