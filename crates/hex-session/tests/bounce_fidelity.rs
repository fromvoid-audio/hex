//! Real Session bounce PCM, file recovery and history through public contracts.
use hex_project::{BeatTime, BounceTarget, ClipId, Command, Created, TrackId};
use hex_session::{PreparedImport, Session};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
static NEXT_JOB: AtomicU64 = AtomicU64::new(0);
struct Folder(PathBuf);
impl Folder {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "hex-arrangement-bounce-{}-{}",
            std::process::id(),
            NEXT_JOB.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> (Session, TrackId, ClipId) {
    fixture_at_rate(48_000)
}
fn fixture_at_rate(rate: u32) -> (Session, TrackId, ClipId) {
    let mut session = Session::new(rate, 32, 2_000_000).unwrap();
    let Created::Track(track) = session
        .command(Command::InsertTrackWithDevice {
            name: "source".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!()
    };
    let Created::Clip(clip) = session
        .command(Command::PlaceMidiClip {
            track,
            start: BeatTime::ONE,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!()
    };
    session
        .command(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 110,
        })
        .unwrap();
    (session, track, clip)
}
fn pcm(revision: Arc<hex_engine::PreparedRevision>, frames: usize) -> Vec<hex_engine::Frame> {
    let mut runtime = hex_engine::SessionPlayback::new(revision).unwrap();
    runtime
        .apply(hex_engine::SessionPlaybackCommand::Play)
        .unwrap();
    let mut samples = vec![[0.0; 2]; frames];
    for chunk in samples.chunks_mut(137) {
        runtime.render(chunk).unwrap();
    }
    samples
}

fn write_fixture(path: &Path, rate: u32, frames: &[hex_engine::Frame]) {
    use std::io::Write;
    let bytes = (frames.len() * 8) as u32;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap();
    for part in [
        b"RIFF".as_slice(),
        &(36 + bytes).to_le_bytes(),
        b"WAVEfmt ",
        &16_u32.to_le_bytes(),
        &3_u16.to_le_bytes(),
        &2_u16.to_le_bytes(),
        &rate.to_le_bytes(),
        &(rate * 8).to_le_bytes(),
        &8_u16.to_le_bytes(),
        &32_u16.to_le_bytes(),
        b"data",
        &bytes.to_le_bytes(),
    ] {
        f.write_all(part).unwrap();
    }
    for frame in frames {
        for sample in frame {
            f.write_all(&sample.to_le_bytes()).unwrap();
        }
    }
}
#[test]
fn selected_clip_bounce_preserves_release_pcm_and_history() {
    selected_bounce_case(false, false);
}

#[test]
fn whole_track_in_place_bounce_preserves_release_pcm_and_history() {
    selected_bounce_case(true, true);
}

fn selected_bounce_case(inplace: bool, whole_track: bool) {
    let folder = Folder::new();
    let (mut session, track, clip) = fixture();
    session.command(Command::Gain { track, gain: 0.7 }).unwrap();
    let effect = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
    )
    .unwrap();
    let effect = hex_engine::patch_native_device_parameter_for_class(
        hex_engine::NativeDeviceClass::Effect,
        &effect,
        hex_lattice_contract::ParameterId::from_static(11),
        hex_lattice_contract::ParameterValue::Bool(true),
    )
    .unwrap();
    session
        .command(Command::InsertDevice {
            track,
            before: None,
            state: effect,
        })
        .unwrap();
    let instrument = session
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap();
    session
        .command(Command::ReplaceDeviceState {
            device: instrument.id(),
            state: instrument
                .state()
                .clone()
                .with_input_gain_db(-6.0)
                .unwrap()
                .with_output_gain_db(24.0)
                .unwrap(),
        })
        .unwrap();
    let total_frames = if whole_track { 180_000 } else { 80_000 };
    if whole_track {
        let Created::Clip(second) = session
            .command(Command::PlaceMidiClip {
                track,
                start: BeatTime::from_raw(3 << 32),
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("second midi clip")
        };
        session
            .command(Command::AddMidiNote {
                clip: second,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 67,
                channel: 0,
                velocity: 91,
            })
            .unwrap();
        let path = folder.0.join("original.wav");
        write_fixture(&path, 48_000, &[[0.2, -0.1]; 64]);
        let prepared = PreparedImport::new(
            hex_media::decode_wav_file(&path, hex_media::Limits::new(10_000, 1000)).unwrap(),
        )
        .unwrap();
        session
            .import_prepared_audio_clip_revision(prepared, track, 144_000, None)
            .unwrap();
    }
    let original = session.project().clone();
    let revision = session.compile_prepared_revision(None).unwrap();
    let expected = pcm(revision.clone(), total_frames);
    let (_, bounced) = session
        .bounce_arrangement(
            if whole_track {
                BounceTarget::Track(track)
            } else {
                BounceTarget::Clips(vec![clip])
            },
            inplace,
            &folder.0,
            &hex_export::ExportControl::default(),
            Some(&revision),
        )
        .unwrap();
    let destination = if inplace {
        track
    } else {
        session.project().tracks()[1].id()
    };
    let audio = &session.project().track(destination).unwrap().clips()[0];
    let span = original
        .plan_arrangement_bounce(&BounceTarget::Clips(vec![clip]), false)
        .ok()
        .map(|plan| (plan[0].start, plan[0].end));
    let samples = session.audio(audio.asset()).unwrap().frames();
    if inplace {
        assert!(
            audio.duration() > 24_000,
            "clip release must extend past original end"
        );
    } else {
        // The view keeps the original span; the release stays in the file.
        let (start, end) = span.unwrap();
        assert_eq!((audio.start(), audio.duration()), (start, end - start));
        assert!(
            samples.len() as u64 > end - start,
            "rendered file retains the release beyond the view"
        );
    }
    let rendered_frames = samples.len() as u64;
    assert!(
        samples.iter().flatten().any(|s| s.abs() > 1.0),
        "float PCM retains headroom above full scale"
    );
    assert!(
        samples[24_000..].iter().flatten().any(|s| s.abs() > 1e-6),
        "tail contains actual audio"
    );
    if whole_track {
        assert_eq!(audio.start(), 24_000);
        assert!(
            audio.start() + audio.duration() >= 144_064,
            "all material and gaps are covered"
        );
        assert!(
            session
                .project()
                .track(track)
                .unwrap()
                .midi_clips()
                .is_empty()
        );
        assert!(session.project().track(track).unwrap().devices().is_empty());
        assert_eq!(
            fs::read(folder.0.join("original.wav")).unwrap().len(),
            44 + 64 * 8
        );
    }
    if inplace {
        assert!(
            session
                .project()
                .track(track)
                .unwrap()
                .instrument()
                .is_none()
        );
        let actual = pcm(bounced, total_frames);
        let retained_end = usize::try_from(audio.start() + audio.duration()).unwrap();
        assert!(
            retained_end < expected.len(),
            "fixture exercises trimmed residuals"
        );
        let max = actual[..retained_end]
            .iter()
            .flatten()
            .zip(expected[..retained_end].iter().flatten())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        assert!(max < 1e-5, "baked device host gains apply once: {max}");
        assert!(
            actual[retained_end..]
                .iter()
                .flatten()
                .all(|sample| *sample == 0.0)
        );
        // The original track's 0.7 mixer gain remains live. Only residuals
        // at/below the bounce's -80 dBFS pre-mixer threshold may disappear.
        assert!(
            expected[retained_end..]
                .iter()
                .flatten()
                .all(|sample| sample.abs() <= 0.7 * 0.0001)
        );
    } else {
        assert!(session.project().midi_clip(clip).is_some());
    }
    let saved = folder.0.join("project.hex");
    hex_storage::save_new(&saved, session.project()).unwrap();
    let loaded = hex_storage::load(&saved).unwrap();
    assert_eq!(loaded.snapshot(), session.project().snapshot());
    let decoded = loaded
        .assets()
        .map(|a| {
            (
                a.id(),
                hex_media::decode_wav_file(
                    a.path(),
                    hex_media::Limits::new(64 * 1024 * 1024 + 44, 8_000_000),
                )
                .unwrap(),
            )
        })
        .collect();
    let reopened = Session::from_loaded(loaded, 32, 2_000_000, decoded).unwrap();
    assert_eq!(
        pcm(
            reopened.compile_prepared_revision(None).unwrap(),
            total_frames
        ),
        pcm(
            session.compile_prepared_revision(None).unwrap(),
            total_frames
        )
    );
    let after = session.project().clone();
    assert!(session.undo());
    assert_eq!(
        session.project().snapshot().tracks,
        original.snapshot().tracks
    );
    assert!(session.redo());
    assert_eq!(session.project().snapshot().tracks, after.snapshot().tracks);
    if !inplace {
        // With the source muted, only extending the bounced clip's right edge
        // makes the rendered release audible past the original end.
        let (_, end) = span.unwrap();
        let tail = |session: &Session| {
            let rendered = pcm(
                session.compile_prepared_revision(None).unwrap(),
                total_frames,
            );
            rendered[usize::try_from(end).unwrap()..]
                .iter()
                .flatten()
                .fold(0.0_f32, |peak, s| peak.max(s.abs()))
        };
        session
            .command(Command::SetClipActive {
                clip,
                active: false,
            })
            .unwrap();
        assert!(tail(&session) < 1e-6, "view end hides the tail");
        let bounced = session.project().track(destination).unwrap().clips()[0].clone();
        session
            .command(Command::TrimClipEdge {
                clip: bounced.id(),
                edge: hex_project::TrimEdge::Right,
                frame: bounced.start() + rendered_frames,
            })
            .unwrap();
        assert!(tail(&session) > 1e-3, "extended edge plays the tail");
    }
}
