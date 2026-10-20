//! A saved instrument project must never export an apparently successful silent mix.
use hex_project::{BeatTime, Command, Created, Editor, Tempo};
use std::{fs, path::PathBuf, process::Command as ProcessCommand};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("hex-cli-midi-project-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        ProcessCommand::new(env!("CARGO_BIN_EXE_hex-cli"))
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn saved_mixed_project_preserves_midi_and_renders_audible_mix() {
    let fixture = Fixture::new();
    for args in [
        vec!["demo", "Original-MiXeD.wav"],
        vec!["import-project", "Original-MiXeD.wav", "audio.hex"],
    ] {
        let output = fixture.run(&args);
        assert!(output.status.success(), "{:?}", output.stderr);
    }
    let original = hex_storage::load(fixture.0.join("audio.hex")).unwrap();
    let audio_clips = original.tracks()[0].clips().to_vec();
    let mut editor = Editor::new(original, 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "Original-MiXeD instrument".into(),
            before: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().expect("fixture instrument state"),
        })
        .unwrap()
    else {
        panic!("expected instrument track");
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ONE,
            length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    else {
        panic!("expected midi clip");
    };
    let Created::Note(note) = editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(2).unwrap(),
            pitch: 60,
            channel: 15,
            velocity: 100,
        })
        .unwrap()
    else {
        panic!("expected note");
    };
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::from_quarters(2).unwrap(),
            tempo: Tempo::new(90_000_000).unwrap(),
        })
        .unwrap();
    let expected = editor.project().clone();
    hex_storage::save_new(fixture.0.join("mixed.hex"), &expected).unwrap();
    let reopened = hex_storage::load(fixture.0.join("mixed.hex")).unwrap();
    assert_eq!(reopened, expected);
    assert_eq!(reopened.tracks()[0].clips(), audio_clips);
    assert_eq!(reopened.track(track).unwrap().clips(), &[]);
    assert_eq!(reopened.track(track).unwrap().midi_clips().len(), 1);
    assert_eq!(
        reopened.track(track).unwrap().name(),
        "Original-MiXeD instrument"
    );
    assert_eq!(reopened.midi_clip(clip).unwrap().notes()[0].id(), note);
    let events = hex_engine::compile::midi_events(&reopened).unwrap();
    assert_eq!(events.len(), 2);
    // Beat 1 at 120 bpm is 24,000; beat 3 spans two quarters at
    // 120 bpm (48,000) and one at 90 bpm (32,000).
    assert_eq!(events[0].frame, 24_000);
    assert_eq!(events[1].frame, 80_000);
    assert_eq!(events[0].note, note);
    assert_eq!(events[1].note, note);
    assert_eq!(events[0].pitch, 60);
    assert_eq!(events[0].channel, 15);
    assert_eq!(events[0].velocity, 100);

    for (project, wav) in [("audio.hex", "audio.wav"), ("mixed.hex", "mixed.wav")] {
        let output = fixture.run(&["render-project", project, wav]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let limits = hex_media::Limits::new(64 * 1024 * 1024, 8_000_000);
    let baseline = hex_media::decode_wav_file(fixture.0.join("audio.wav"), limits).unwrap();
    let mixed = hex_media::decode_wav_file(fixture.0.join("mixed.wav"), limits).unwrap();
    let base = baseline.audio().frames();
    let frames = mixed.audio().frames();
    assert_eq!(&frames[..24_000], &base[..24_000]);
    assert!(
        frames[24_001..80_000]
            .iter()
            .zip(&base[24_001..80_000])
            .any(|(actual, audio)| (actual[0] - audio[0]).abs() > 0.01)
    );
    assert_eq!(&frames[82_400..base.len()], &base[82_400..]);

    // A MIDI-only saved project must also export, including its release tail.
    editor
        .execute(Command::RemoveTrack(expected.tracks()[0].id()))
        .unwrap();
    hex_storage::save_new(fixture.0.join("midi.hex"), editor.project()).unwrap();
    let output = fixture.run(&["render-project", "midi.hex", "midi.wav"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let midi = hex_media::decode_wav_file(fixture.0.join("midi.wav"), limits).unwrap();
    let midi_frames = midi.audio().frames();
    assert!(midi_frames[..24_000].iter().all(|frame| *frame == [0.0; 2]));
    assert!(
        midi_frames[24_001..80_000]
            .iter()
            .any(|frame| frame[0].abs() > 0.01)
    );
    assert!(
        midi_frames[80_000..82_400]
            .iter()
            .any(|frame| frame[0].abs() > 0.0001)
    );
    assert!(midi_frames[82_400..].iter().all(|frame| *frame == [0.0; 2]));
    // Compare content, not just successful export or two shared-render hashes:
    // early notes must survive interval-tree medians starting in the future.
    editor.execute(Command::RemoveMidiClip(clip)).unwrap();
    let Created::Clip(separated) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(8).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip expected")
    };
    for (index, pitch) in [60, 64, 67, 72].into_iter().enumerate() {
        editor
            .execute(Command::AddMidiNote {
                clip: separated,
                start: BeatTime::from_quarters(index as u64 * 2).unwrap(),
                duration: BeatTime::ONE,
                pitch,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    hex_storage::save_new(fixture.0.join("four.hex"), editor.project()).unwrap();
    let output = fixture.run(&["render-project", "four.hex", "four.wav"]);
    assert!(output.status.success());
    let four = hex_media::decode_wav_file(fixture.0.join("four.wav"), limits).unwrap();
    let edges = hex_engine::compile::midi_events(editor.project()).unwrap();
    for on in edges
        .iter()
        .filter(|event| event.kind == hex_engine::midi_compile::MidiEventKind::NoteOn)
    {
        let start = on.frame as usize + 300;
        assert!(
            four.audio().frames()[start..start + 256]
                .iter()
                .any(|frame| frame[0].abs() > 0.01),
            "missing note at {}",
            on.frame
        );
    }
}
