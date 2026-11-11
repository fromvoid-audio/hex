//! Actual WAV export of current registered generators; no audio backend or MTS.
use hex_engine::{NativeDeviceClass, PreparedRevision, SineTable};
use hex_lattice_contract::{DeviceTypeId, NativeDeviceState, ParameterId, ParameterValue};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use hex_tuning::{Audition, MAX_DEGREES, ProjectTuning, Scale};
use std::{collections::BTreeMap, fs, path::PathBuf, sync::Arc};

const RATE: u32 = 48_000;
const FRAMES: usize = 8192;
struct Scratch(PathBuf);
impl Scratch {
    fn new(kind: &str) -> Self {
        // Each generator occurs once in this executable. create_dir refuses an
        // existing path; cleanup only owns the successfully created directory.
        let path = std::env::temp_dir().join(format!(
            "hex-native-tuning-export-{}-{kind}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn patch(state: NativeDeviceState, id: u32, value: ParameterValue) -> NativeDeviceState {
    hex_engine::patch_native_device_parameter_for_class(
        NativeDeviceClass::Instrument,
        &state,
        ParameterId::from_static(id),
        value,
    )
    .unwrap()
}
fn tone(kind: DeviceTypeId, vector: bool) -> NativeDeviceState {
    let mut state =
        hex_engine::default_native_device_state_for_class(NativeDeviceClass::Instrument, kind)
            .unwrap();
    if kind == DeviceTypeId::from_static("hex.instrument.sine") {
        return state;
    }
    // Stable registered parameter IDs: oscillator level, directed FM, carrier.
    // This is an independently configured single carrier, not factory-default FM.
    for op in 1..=4 {
        let level = if op == 1 { 1.0 } else { 0.0 };
        state = patch(state, (op - 1) * 3 + 2, ParameterValue::Float(level));
        state = patch(state, 28 + op, ParameterValue::Float(level));
        for target in 1..=4 {
            if target != op {
                state = patch(
                    state,
                    12 + (op - 1) * 4 + target,
                    ParameterValue::Float(0.0),
                );
            }
        }
    }
    if vector {
        assert_eq!(
            hex_engine::native_device_parameter_for_class(
                NativeDeviceClass::Instrument,
                &state,
                ParameterId::from_static(608)
            )
            .unwrap(),
            ParameterValue::Choice(1),
            "exercise the current Vector renderer"
        );
    }
    let envelope: &[(u32, f32)] = if vector {
        // Initial/peak/sustain/attack/decay. Keep current mode1 and its quality.
        &[(33, 1.0), (34, 1.0), (35, 1.0), (36, 0.0), (37, 0.001)]
    } else {
        &[(33, 0.0), (34, 0.0), (35, 1.0)]
    };
    for &(id, value) in envelope {
        state = patch(state, id, ParameterValue::Float(value));
    }
    state
}
fn revision(kind: DeviceTypeId, vector: bool) -> Arc<PreparedRevision> {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "export oracle".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    if editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .is_some()
    {
        editor.execute(Command::RemoveInstrument { track }).unwrap();
    }
    editor
        .execute(Command::InsertInstrument {
            track,
            state: tone(kind, vector),
        })
        .unwrap();
    let mut cents = [0.0; MAX_DEGREES];
    cents[..5].copy_from_slice(&[0.0, 177.0, 393.0, 786.0, 1270.0]);
    let scale = Scale::new(5, cents, 64, 337.45, 2, 1901.955).unwrap();
    editor
        .execute(Command::SetTuning(
            ProjectTuning::new(true, scale, Audition::default()).unwrap(),
        ))
        .unwrap();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 69,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    )
}
fn observed_hz(pcm: &[i16]) -> f64 {
    let mut crossings = Vec::new();
    for (index, pair) in pcm.windows(2).enumerate().skip(960) {
        if pair[0] <= 0 && pair[1] > 0 {
            let left = f64::from(pair[0]);
            crossings.push(index as f64 - left / (f64::from(pair[1]) - left));
        }
    }
    assert!(crossings.len() >= 8);
    f64::from(RATE) * (crossings.len() - 1) as f64 / (crossings.last().unwrap() - crossings[0])
}
#[test]
fn native_tuning_current_generators_export_non_octave_pitch_and_exact_rendered_pcm() {
    for name in ["sine", "fm4", "operator"] {
        let kind = DeviceTypeId::new(&format!("hex.instrument.{name}")).unwrap();
        let revision = revision(kind, name == "operator");
        let scratch = Scratch::new(name);
        let path = scratch.0.join("tuned.wav");
        let report =
            hex_export::render_prepared_new(&path, Arc::clone(&revision), 0, FRAMES as u64)
                .unwrap();
        assert_eq!(report.frames, FRAMES as u64);
        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(bytes.len(), 44 + FRAMES * 4);
        let mut playback = hex_engine::playback::Playback::from_prepared(Arc::clone(&revision));
        playback.apply(hex_engine::playback::PlaybackCommand::Play);
        let mut direct = vec![[0.0; 2]; FRAMES + 2048];
        for block in direct.chunks_mut(137) {
            playback.render(block).unwrap();
        }
        let mut pcm = Vec::with_capacity(FRAMES);
        for (frame, (samples, expected)) in bytes[44..].chunks_exact(4).zip(&direct).enumerate() {
            let left = i16::from_le_bytes([samples[0], samples[1]]);
            let right = i16::from_le_bytes([samples[2], samples[3]]);
            // Channels carry independent dither, so centred mono differs by
            // at most two LSB between channels.
            assert!(
                (i32::from(left) - i32::from(right)).abs() <= 2,
                "{name} stereo frame{frame}"
            );
            for (actual, value) in [left, right].into_iter().zip(expected) {
                assert_pcm16_near(actual, *value, &format!("{name} rendered frame{frame}"));
            }
            pcm.push(left);
        }
        // Key69-root64+rootdegree2=7: degree2 plus one tritave,
        // minus the root's own degree2 offset. No effective-table getter oracle.
        let expected_hz = 337.45 * 2.0_f64.powf(f64::from(1901.955_f32) / 1200.0);
        assert!((observed_hz(&pcm) - expected_hz).abs() < 0.01, "{name}");
        // Real float bounce at a nonzero range uses the control-side preroll,
        // including current Vector history, rather than raw callback Seek.
        let bounce_path = scratch.0.join("tuned-bounce.wav");
        hex_export::render_bounce_new(&bounce_path, revision, 2048, FRAMES as u64, 1.0).unwrap();
        let bounce = fs::read(bounce_path).unwrap();
        assert_eq!(u16::from_le_bytes([bounce[20], bounce[21]]), 3);
        assert_eq!(
            bounce.len(),
            hex_export::FLOAT32_WAV_HEADER_BYTES as usize + FRAMES * 8
        );
        for (frame, (bytes, expected)) in bounce[hex_export::FLOAT32_WAV_HEADER_BYTES as usize..]
            .chunks_exact(8)
            .zip(&direct[2048..])
            .enumerate()
        {
            for (channel, expected) in bytes.chunks_exact(4).zip(expected) {
                let actual = f32::from_le_bytes(channel.try_into().unwrap());
                assert_eq!(
                    actual, *expected,
                    "{name} nonzero-range bounce frame{frame}"
                );
            }
        }
        // A second create-new export must preserve the original successful WAV.
        assert!(
            hex_export::render_prepared_new(
                &path,
                self::revision(kind, name == "operator"),
                0,
                137
            )
            .is_err()
        );
        assert_eq!(fs::read(path).unwrap(), bytes);
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
