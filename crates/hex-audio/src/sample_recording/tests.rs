use super::*;
use crate::{device::finish_host_chunk, prepared};
use hex_engine::{Audio, PlaybackCommand, PreparedRevision, SineTable, playback::Playback};
use hex_project::{Command, Created, Editor, Project};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, Instant},
};

fn two_tracks() -> (Playback, [OutputCaptureId; 2]) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let mut tracks = Vec::new();
    let mut media = BTreeMap::new();
    for (name, value) in [("left", [0.25, 0.0]), ("right", [0.0, -0.5])] {
        let Created::Imported { track, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: name.into(),
                path: format!("{name}.wav").into(),
                sample_rate: 48_000,
                frames: 512,
            })
            .unwrap()
        else {
            panic!("expected imported track");
        };
        editor.execute(Command::Gain { track, gain: 1.0 }).unwrap();
        tracks.push(Track(track));
        media.insert(asset, Audio::new(48_000, vec![value; 512]).unwrap());
    }
    let prepared =
        PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
            .unwrap();
    (
        Playback::try_from_prepared_with_session(Arc::new(prepared)).unwrap(),
        tracks.try_into().unwrap(),
    )
}

fn deliver(callback: &mut prepared::Callback, channels: usize) -> bool {
    let mut pcm = [[0.0; 2]; 64];
    let mut device = [0.0; 128];
    let mut render = || {
        callback.begin_block();
        let rendered = callback.render(&mut pcm);
        assert!(rendered.is_ok(), "{rendered:?}");
        finish_host_chunk(
            callback,
            &mut device,
            &mut pcm,
            channels,
            &rendered,
            0,
            Instant::now(),
        )
    };
    #[cfg(feature = "allocation-probe")]
    {
        let mut result = false;
        let measured = allocation_counter::measure(|| result = render());
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        result
    }
    #[cfg(not(feature = "allocation-probe"))]
    render()
}

struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn confirmed_pcm_writes_each_armed_source_as_a_separate_take() {
    let path = Directory(std::env::temp_dir().join(
        format!("hex-sample-takes-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()),
    ));
    let (graph, outputs) = two_tracks();
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    control
        .publish_with_policy_and_context_generation(
            graph,
            prepared::PublicationPolicy::InstallFreshRuntime,
            7,
        )
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let reader = control.sample_recording.start(7, &outputs, false).unwrap();
    let state = reader.state.clone();
    let worker_path = path.0.clone();
    let worker = std::thread::spawn(move || write_takes(reader, &worker_path).unwrap());
    let deadline = Instant::now() + Duration::from_millis(500);
    while state.load(Acquire) == PENDING {
        assert!(Instant::now() < deadline, "take writer did not prepare");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(deliver(&mut callback, 2));
    assert!(deliver(&mut callback, 2));
    assert!(!deliver(&mut callback, 0));
    let batch = worker.join().unwrap();
    assert_eq!(batch.end, End::OutputFailed);
    assert!(batch.write_error.is_none());
    assert_eq!(batch.takes.len(), 2);
    for (take, value) in batch.takes.iter().zip([[0.25, 0.0], [0.0, -0.5]]) {
        assert_eq!(take.frames, 128);
        let wav =
            hex_media::decode_wav_file(&take.path, hex_media::Limits::new(8192, 512)).unwrap();
        assert_eq!(wav.sample_rate(), 48_000);
        assert_eq!(wav.audio().frames(), &[value; 128]);
    }
}

#[test]
fn overflow_preserves_a_full_prefix_and_stop_stays_available_before_first_chunk() {
    let (mut control, mut callback) = channel(48_000);
    let mut reader = control.start(1, &[], true).unwrap();
    assert!(reader.ready());
    for _ in 0..PACKETS {
        callback.begin(1);
        let (layout, stage) = callback.observation().unwrap();
        stage.observe(layout, ProjectSignal::Master, 0, &[[0.5; 2]]);
        callback.finish(true);
    }
    callback.begin(1);
    let mut confirmed = 0;
    assert_eq!(
        reader.drain(|frames, _, pcm| {
            confirmed += frames;
            assert_eq!(Reader::source_frames(pcm, frames, 0), Some(&[[0.5; 2]][..]));
        }),
        Some(End::Backpressure)
    );
    assert_eq!(confirmed, PACKETS);
    let mut next = control.start(1, &[], true).unwrap();
    control.stop();
    assert!(!next.ready());
    callback.begin(1);
    assert_eq!(
        next.drain(|_, _, _| panic!("stopped take contains PCM")),
        Some(End::Stopped)
    );
}

#[test]
fn source_replacement_ends_the_take_before_replacement_audio() {
    let (graph, outputs) = two_tracks();
    let (mut control, mut callback) = prepared::channel(48_000).unwrap();
    control
        .publish_with_policy_and_context_generation(
            graph,
            prepared::PublicationPolicy::InstallFreshRuntime,
            7,
        )
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut reader = control.sample_recording.start(7, &outputs, true).unwrap();
    assert!(reader.ready());
    assert!(deliver(&mut callback, 2));
    let (replacement, _) = two_tracks();
    control
        .publish_with_policy_and_context_generation(
            replacement,
            prepared::PublicationPolicy::InstallFreshRuntime,
            8,
        )
        .unwrap();
    assert!(deliver(&mut callback, 2));
    let mut confirmed = 0;
    assert_eq!(
        reader.drain(|frames, _, _| confirmed += frames),
        Some(End::SourceChanged)
    );
    assert_eq!(confirmed, 64);
}
