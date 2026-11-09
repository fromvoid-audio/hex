use hex_engine::{NativeDeviceClass, default_native_device_state_for_class};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{
    f64::consts::TAU,
    fs,
    sync::atomic::{AtomicU64, Ordering},
};

const RATE: u32 = 48_000;
const PHASE_SCALE: f64 = 18_446_744_073_709_551_616.0;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[test]
fn built_in_fm4_offline_export_matches_independent_carrier_pcm() {
    let mut editor = Editor::new(Project::new(RATE).expect("project"), 8).expect("editor");
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "fm4 export".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .expect("midi track")
    else {
        panic!("midi track result")
    };
    let instrument = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    editor
        .execute(Command::ReplaceDeviceState {
            device: instrument,
            state: default_native_device_state_for_class(
                NativeDeviceClass::Instrument,
                hex_engine::native_device_catalog_for_class(NativeDeviceClass::Instrument)
                    .expect("registered instrument catalog")
                    .into_iter()
                    .find(|entry| entry.name == "fm4")
                    .expect("FM4 is registered")
                    .type_id,
            )
            .expect("registered FM4 default"),
        })
        .expect("select FM4");
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).unwrap(),
        })
        .expect("MIDI clip")
    else {
        panic!("MIDI clip result")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(2).unwrap(),
            pitch: 69,
            channel: 0,
            velocity: 127,
        })
        .expect("MIDI note");
    let project = hex_engine::compile::project(editor.project(), &Default::default())
        .expect("compile built-in FM4 arrangement");
    let unique = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "hex-fm4-export-{}-{unique}.wav",
        std::process::id()
    ));
    let report = hex_export::render_new(&path, &project, 0, 256).expect("export FM4 PCM");
    assert_eq!(report.frames, 256);
    let bytes = fs::read(&path).expect("read exported WAV");
    let _ = fs::remove_file(&path);
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[36..40], b"data");
    for (age, frame) in bytes[44..].chunks_exact(4).enumerate() {
        let left = i16::from_le_bytes([frame[0], frame[1]]);
        let right = i16::from_le_bytes([frame[2], frame[3]]);
        let age = age as u64;
        let step = (440.0 / f64::from(RATE) * PHASE_SCALE).round() as u64;
        let turns = step.wrapping_mul(age) as f64 / PHASE_SCALE;
        let envelope = if age < 240 {
            age as f64 / 240.0
        } else if age < 5_040 {
            1.0 - (age - 240) as f64 / 4_800.0 * 0.3
        } else {
            0.7
        };
        let expected = (((TAU * turns).sin() as f32) as f64 * envelope) as f32;
        let expected_pcm = (expected.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        // Oracle rounding error plus ±1 LSB TPDF dither per channel.
        assert!(
            (i32::from(left) - i32::from(expected_pcm)).abs() <= 2,
            "age {age}: exported {left} != independent PCM {expected_pcm}"
        );
        assert!(
            (i32::from(left) - i32::from(right)).abs() <= 2,
            "default FM4 carrier exports centered mono"
        );
    }
}
